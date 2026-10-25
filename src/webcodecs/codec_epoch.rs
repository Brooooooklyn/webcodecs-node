//! Cancellation shared by a codec, its worker thread and the callbacks the
//! worker queues for the JS thread.
//!
//! close() and reset() advance the epoch on the JS thread before they wait for
//! the codec lock. A worker serving an older generation stops at its next
//! check, and every output or error callback tagged with an older generation
//! is dropped when it reaches the JS thread. This matches Chrome, where no
//! output or error from earlier work runs after close() or reset() returns.
//! Dequeue events carry no generation: one that is already scheduled still
//! runs, as in Chrome (see `CodecEventState::clear_queue`).
//!
//! An unsupported config that configure() reports is the exception: Chrome
//! queues that error as a task, which reset() does not cancel. Such an error
//! still reaches JS after reset() and closes the codec then; only close()
//! drops it.
//!
//! Outputs produced before an error reach JS before the error callback, and
//! none reaches JS after it, as in Chrome. Outputs, including those buffered
//! by a pending flush, and the error share one ordered queue and one
//! ThreadsafeFunction. Every callback therefore has a microtask checkpoint,
//! and an error cannot overtake outputs when Node moves on after 999 calls.

use std::collections::VecDeque;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use crossbeam::channel::Sender;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi::{check_status, sys};

use super::error::{DOMExceptionName, error_as_dom_exception};
use super::event_target::{CodecEventState, ErrorDelivery, report_exception};
use super::output_delivery::OutputDelivery;

/// Worker → JS: one output or error waits in the epoch's queue. The payload
/// carries the generation it was queued under.
pub(crate) type GatedOutput = ThreadsafeFunction<u32, UnknownReturnValue, u32, Status, false, true>;

/// Where a codec error came from, which decides what drops it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ErrorOrigin {
  /// Queued work, or a configure() step Chrome runs while the codec starts:
  /// close() and reset() drop it.
  Work,
  /// configure() found the config unsupported: only close() drops it.
  Configure,
}

impl ErrorOrigin {
  /// The origin of an error configure() reports on the JS thread. Chrome
  /// checks whether the config is supported when configure() runs and queues
  /// a NotSupportedError task; other errors come from starting the codec,
  /// which reset() cancels.
  pub(crate) fn of_configure_error(message: &str) -> Self {
    match DOMExceptionName::from_error_message(message) {
      DOMExceptionName::NotSupportedError => ErrorOrigin::Configure,
      _ => ErrorOrigin::Work,
    }
  }
}

/// JS thread: closes the codec for a configure() error that outlived a
/// reset(). Gets the error's name.
type CloseOnError = Box<dyn Fn(&Env, DOMExceptionName) + Send + Sync>;

/// An output on its way to JS, or one a codec buffered for a pending flush.
/// Turned into the output callback's arguments on the JS thread.
pub(crate) type BufferedOutput =
  Box<dyn FnOnce(sys::napi_env) -> Result<Vec<sys::napi_value>> + Send>;

/// Buffer `args` for the output callback.
pub(crate) fn buffered_output<Args: JsValuesTupleIntoVec + Send + 'static>(
  args: Args,
) -> BufferedOutput {
  Box::new(move |env| args.into_vec(env))
}

/// A flush response held until the error queued before it reaches JS.
pub(crate) struct DeferredFlush {
  sender: Sender<Result<()>>,
  result: Result<()>,
}

impl DeferredFlush {
  /// Never block a worker or the JS thread. reset()/close() may already have
  /// filled the response channel with their AbortError.
  pub(crate) fn send(self) {
    let _ = self.sender.try_send(self.result);
  }
}

struct PendingErrorState {
  delivery: Option<ErrorDelivery>,
  flushes: Vec<DeferredFlush>,
  completed: bool,
}

struct PendingError {
  generation: u32,
  origin: ErrorOrigin,
  reason: String,
  state: Mutex<PendingErrorState>,
}

impl PendingError {
  fn matches(&self, generation: u32) -> bool {
    self.origin == ErrorOrigin::Configure || self.generation == generation
  }

  fn defer(&self, flush: DeferredFlush) -> std::result::Result<(), DeferredFlush> {
    let Ok(mut state) = self.state.lock() else {
      return Err(flush);
    };
    if state.completed {
      return Err(flush);
    }
    state.flushes.push(flush);
    Ok(())
  }

  fn complete(&self) -> (Option<ErrorDelivery>, Vec<DeferredFlush>) {
    let Ok(mut state) = self.state.lock() else {
      return (None, Vec::new());
    };
    if state.completed {
      return (None, Vec::new());
    }
    state.completed = true;
    (state.delivery.take(), std::mem::take(&mut state.flushes))
  }
}

enum EpochItem {
  Output(BufferedOutput),
  /// Flush-buffered output keeps the existing exception backstop behavior.
  FlushOutput(BufferedOutput),
  Error(Arc<PendingError>),
}

#[derive(Default)]
struct EpochState {
  generation: AtomicU32,
  /// Set on the JS thread once the codec's error callback has run.
  error_delivered: AtomicBool,
  /// close() ran.
  closed: AtomicBool,
  /// Set once by the codec's constructor.
  close_on_error: OnceLock<CloseOnError>,
  /// Outputs and errors on their way to JS, oldest first, each tagged with
  /// the generation it was queued under.
  outputs: Mutex<VecDeque<(u32, EpochItem)>>,
  /// Errors still able to accept a flush response. This includes the error
  /// currently executing, after its queue entry has been removed.
  errors: Mutex<Vec<Arc<PendingError>>>,
}

/// Generation counter shared by one codec, its workers and its callbacks.
#[derive(Clone, Default)]
pub(crate) struct CodecEpoch(Arc<EpochState>);

impl CodecEpoch {
  pub(crate) fn current(&self) -> u32 {
    self.0.generation.load(Ordering::SeqCst)
  }

  pub(crate) fn is_current(&self, generation: u32) -> bool {
    self.current() == generation
  }

  /// JS thread: drop all work, outputs and errors queued so far (a scheduled
  /// dequeue event still runs). Returns the new generation.
  pub(crate) fn advance(&self) -> u32 {
    self
      .0
      .generation
      .fetch_add(1, Ordering::SeqCst)
      .wrapping_add(1)
  }

  /// Token for a worker that serves the current generation.
  pub(crate) fn token(&self) -> CancelToken {
    CancelToken {
      epoch: self.clone(),
      generation: self.current(),
    }
  }

  fn send_item(
    &self,
    callback: &GatedOutput,
    generation: u32,
    item: EpochItem,
    output_delivery: &Arc<OutputDelivery>,
  ) {
    let Ok(mut outputs) = self.0.outputs.lock() else {
      return;
    };
    outputs.push_back((generation, item));
    drop(outputs);
    let on_delivered = output_delivery.track();
    callback.call_with_return_value(
      generation,
      ThreadsafeFunctionCallMode::NonBlocking,
      on_delivered,
    );
  }

  /// Worker: queue an output and wake the shared callback for it.
  pub(crate) fn send_output(
    &self,
    callback: &GatedOutput,
    generation: u32,
    output: BufferedOutput,
    output_delivery: &Arc<OutputDelivery>,
  ) {
    self.send_item(
      callback,
      generation,
      EpochItem::Output(output),
      output_delivery,
    );
  }

  /// Move an output buffered for a pending flush into the same ordered queue
  /// when an error is reported, ahead of that error.
  pub(crate) fn send_flush_output(
    &self,
    callback: &GatedOutput,
    generation: u32,
    output: BufferedOutput,
    output_delivery: &Arc<OutputDelivery>,
  ) {
    self.send_item(
      callback,
      generation,
      EpochItem::FlushOutput(output),
      output_delivery,
    );
  }

  /// JS thread: recover a direct flush drain's unconsumed suffix ahead of
  /// items queued while its callbacks ran, without changing their order.
  pub(crate) fn recover_flush_outputs(
    &self,
    callback: &GatedOutput,
    generation: u32,
    recovered: Vec<BufferedOutput>,
    output_delivery: &Arc<OutputDelivery>,
  ) {
    if recovered.is_empty() {
      return;
    }
    let count = recovered.len();
    let Ok(mut outputs) = self.0.outputs.lock() else {
      return;
    };
    let insertion = outputs
      .iter()
      .position(|(queued, _)| *queued == generation)
      .unwrap_or(outputs.len());
    let mut tail = outputs.split_off(insertion);
    for output in recovered {
      outputs.push_back((generation, EpochItem::FlushOutput(output)));
    }
    outputs.append(&mut tail);
    drop(outputs);

    for _ in 0..count {
      let on_delivered = output_delivery.track();
      callback.call_with_return_value(
        generation,
        ThreadsafeFunctionCallMode::NonBlocking,
        on_delivered,
      );
    }
  }

  /// Queue an error behind every output already produced. Its dequeue guard
  /// starts now and finishes only when the error tail runs or is dropped.
  pub(crate) fn send_error(
    &self,
    callback: &GatedOutput,
    event_state: &Arc<RwLock<CodecEventState>>,
    generation: u32,
    origin: ErrorOrigin,
    error: Error,
  ) {
    let pending = Arc::new(PendingError {
      generation,
      origin,
      reason: error.reason,
      state: Mutex::new(PendingErrorState {
        delivery: Some(CodecEventState::track_error(event_state)),
        flushes: Vec::new(),
        completed: false,
      }),
    });
    let Ok(mut errors) = self.0.errors.lock() else {
      return;
    };
    errors.push(pending.clone());
    drop(errors);
    let Ok(mut outputs) = self.0.outputs.lock() else {
      if let Ok(mut errors) = self.0.errors.lock() {
        errors.retain(|error| !Arc::ptr_eq(error, &pending));
      }
      return;
    };
    outputs.push_back((generation, EpochItem::Error(pending)));
    drop(outputs);
    callback.call(generation, ThreadsafeFunctionCallMode::NonBlocking);
  }

  /// Hold a flush response behind a matching queued error. An unsupported
  /// configure() error also accepts work queued after a reset, since it closes
  /// that new configuration when it arrives.
  pub(crate) fn defer_flush(
    &self,
    generation: u32,
    sender: Sender<Result<()>>,
    result: Result<()>,
  ) -> std::result::Result<(), DeferredFlush> {
    let mut flush = DeferredFlush { sender, result };
    let candidates = match self.0.errors.lock() {
      Ok(errors) => errors.clone(),
      Err(_) => return Err(flush),
    };
    for pending in candidates {
      if !pending.matches(generation) {
        continue;
      }
      match pending.defer(flush) {
        Ok(()) => return Ok(()),
        Err(returned) => flush = returned,
      }
    }
    Err(flush)
  }

  /// JS thread: the oldest item queued for this wakeup's generation. A stale
  /// wakeup cannot consume an item queued after reset()/reconfigure().
  fn next_item(&self, generation: u32) -> Option<EpochItem> {
    let mut outputs = self.0.outputs.lock().ok()?;
    let index = outputs
      .iter()
      .position(|(queued, _)| *queued == generation)?;
    outputs.remove(index).map(|(_, item)| item)
  }

  /// Whether the error callback has run. Until then a codec that a worker
  /// error closed still looks open to JS, as in Chrome, where the codec only
  /// becomes closed when its error task runs.
  pub(crate) fn error_delivered(&self) -> bool {
    self.0.error_delivered.load(Ordering::SeqCst)
  }

  /// JS thread: close() ran, so a configure() error still on its way is dropped.
  pub(crate) fn mark_closed(&self) {
    self.0.closed.store(true, Ordering::SeqCst);
  }

  /// Install how the codec closes itself for a configure() error that
  /// reaches JS after reset(). The codec's constructor calls this once. The
  /// codec state is held weakly: the epoch must not keep it alive.
  pub(crate) fn set_close_on_error<T: Send + 'static>(
    &self,
    inner: &Arc<Mutex<T>>,
    event_state: &Arc<RwLock<CodecEventState>>,
    close: fn(&mut T, &Env, &Arc<RwLock<CodecEventState>>, DOMExceptionName),
  ) {
    let inner = Arc::downgrade(inner);
    let event_state = Arc::downgrade(event_state);
    let close: CloseOnError = Box::new(move |env, name| {
      if let (Some(inner), Some(event_state)) = (inner.upgrade(), event_state.upgrade())
        && let Ok(mut guard) = inner.lock()
      {
        close(&mut guard, env, &event_state, name);
      }
    });
    let _ = self.0.close_on_error.set(close);
  }

  /// JS thread: whether `admit_error` drops an error queued under
  /// `generation`, without its side effects. Once true it stays true: the
  /// generation only advances, and `closed` and `error_delivered` are never
  /// cleared.
  fn drops_error(&self, generation: u32, origin: ErrorOrigin) -> bool {
    match origin {
      ErrorOrigin::Work => !self.is_current(generation),
      ErrorOrigin::Configure => self.0.closed.load(Ordering::SeqCst) || self.error_delivered(),
    }
  }

  /// JS thread: whether an error queued under `generation` reaches the error
  /// callback. `configure_error` is the error's name when configure() found
  /// it (ErrorOrigin::Configure). Marks the error delivered when it does.
  fn admit_error(&self, env: &Env, generation: u32, configure_error: Option<&str>) -> bool {
    match configure_error {
      // An error from work belongs to the generation it was queued under.
      None => {
        if !self.is_current(generation) {
          return false;
        }
      }
      // Chrome closes the codec when this queued error task runs, unless the
      // codec is closed by then.
      Some(name) => {
        if self.0.closed.load(Ordering::SeqCst) || self.error_delivered() {
          return false;
        }
        if !self.is_current(generation) {
          // reset() ran since configure(): close the codec now and drop the
          // work queued since.
          self.advance();
          if let Some(close) = self.0.close_on_error.get() {
            close(env, DOMExceptionName::from_error_message(name));
          }
        }
      }
    }
    self.0.error_delivered.store(true, Ordering::SeqCst);
    true
  }

  fn deliver_error(&self, env: &Env, pending: &PendingError, callback: &OutputRef) -> Result<()> {
    let configure_error = (pending.origin == ErrorOrigin::Configure).then(|| {
      DOMExceptionName::from_error_message(&pending.reason)
        .as_str()
        .to_owned()
    });
    // Do not run page code to build a dropped error, and re-check admission
    // after a replaced DOMException constructor runs.
    if self.drops_error(pending.generation, pending.origin) {
      return Ok(());
    }
    let error = error_as_dom_exception(env, Error::new(Status::GenericFailure, &pending.reason))?;
    if !self.admit_error(env, pending.generation, configure_error.as_deref()) {
      return Ok(());
    }
    call_js(env, callback, &[error.raw()])
  }

  fn finish_error(&self, env: &Env, pending: &Arc<PendingError>, callback: &OutputRef) {
    if let Err(thrown) = self.deliver_error(env, pending, callback) {
      report_callback_exception(env, thrown);
    }
    let (delivery, flushes) = pending.complete();
    if let Ok(mut errors) = self.0.errors.lock() {
      errors.retain(|error| !Arc::ptr_eq(error, pending));
    }
    if let Some(delivery) = delivery {
      delivery.delivered();
    }
    for flush in flushes {
      flush.send();
    }
  }
}

/// A worker's view of the epoch: cancelled once close() or reset() advanced it.
#[derive(Clone)]
pub(crate) struct CancelToken {
  epoch: CodecEpoch,
  generation: u32,
}

impl CancelToken {
  pub(crate) fn is_cancelled(&self) -> bool {
    !self.epoch.is_current(self.generation)
  }

  pub(crate) fn defer_flush(
    &self,
    sender: Sender<Result<()>>,
    result: Result<()>,
  ) -> std::result::Result<(), DeferredFlush> {
    self.epoch.defer_flush(self.generation, sender, result)
  }
}

type OutputRef = FunctionRef<Unknown<'static>, UnknownReturnValue>;

fn call_js(env: &Env, callback: &OutputRef, args: &[sys::napi_value]) -> Result<()> {
  let function = callback.borrow_back(env)?;
  let mut this = ptr::null_mut();
  check_status!(unsafe { sys::napi_get_undefined(env.raw(), &mut this) })?;
  let mut result = ptr::null_mut();
  let status = unsafe {
    sys::napi_call_function(
      env.raw(),
      this,
      function.raw(),
      args.len(),
      args.as_ptr(),
      &mut result,
    )
  };
  if status == sys::Status::napi_pending_exception {
    return Err(Error::new(Status::PendingException, "codec callback threw"));
  }
  check_status!(status)
}

/// Keep flush-buffered output exceptions on the existing reporting/backstop
/// path without letting them suppress later outputs or the codec error.
fn call_flush_output(env: &Env, callback: &OutputRef, buffered: BufferedOutput) -> Result<()> {
  if let Ok(args) = buffered(env.raw()) {
    let _ = call_js(env, callback, &args);
  }
  let mut pending = false;
  unsafe { sys::napi_is_exception_pending(env.raw(), &mut pending) };
  if pending {
    let mut exception = ptr::null_mut();
    if unsafe { sys::napi_get_and_clear_last_exception(env.raw(), &mut exception) }
      == sys::Status::napi_ok
    {
      report_exception(env, exception);
    }
  }
  Ok(())
}

/// Report the exact pending JS exception, if there is one. Converting the
/// Rust error first would make N-API warn about the pending exception and can
/// skip the page's uncaught-exception handler.
fn report_callback_exception(env: &Env, thrown: Error) {
  let mut value = ptr::null_mut();
  let mut pending = false;
  unsafe { sys::napi_is_exception_pending(env.raw(), &mut pending) };
  if pending {
    unsafe { sys::napi_get_and_clear_last_exception(env.raw(), &mut value) };
  }
  if value.is_null()
    && let Ok(converted) = unsafe { Error::to_napi_value(env.raw(), thrown) }
  {
    value = converted;
  }
  if !value.is_null() {
    unsafe { sys::napi_fatal_exception(env.raw(), value) };
  }
}

/// Weak ThreadsafeFunction that delivers ordered outputs and error tails with
/// their original generation, unless close()/reset() cancelled them.
pub(crate) fn gated_output(
  env: &Env,
  callback: sys::napi_value,
  error_callback: sys::napi_value,
  epoch: &CodecEpoch,
) -> Result<GatedOutput> {
  let callback = unsafe {
    Function::<Unknown<'static>, UnknownReturnValue>::from_napi_value(env.raw(), callback)?
  }
  .create_ref()?;
  let error_callback = unsafe {
    Function::<Unknown<'static>, UnknownReturnValue>::from_napi_value(env.raw(), error_callback)?
  }
  .create_ref()?;
  let epoch = epoch.clone();
  let gated = env.create_function_from_closure::<u32, (), _>("codecCallback", move |ctx| {
    let generation = ctx.get::<u32>(0)?;
    let Some(item) = epoch.next_item(generation) else {
      return Ok(());
    };
    match item {
      EpochItem::Output(output) => {
        if !epoch.is_current(generation) || epoch.error_delivered() {
          return Ok(());
        }
        let args = output(ctx.env.raw())?;
        call_js(ctx.env, &callback, &args)
      }
      EpochItem::FlushOutput(output) => {
        if !epoch.is_current(generation) || epoch.error_delivered() {
          return Ok(());
        }
        call_flush_output(ctx.env, &callback, output)
      }
      EpochItem::Error(pending) => {
        epoch.finish_error(ctx.env, &pending, &error_callback);
        Ok(())
      }
    }
  })?;
  unsafe { Function::from_napi_value(env.raw(), gated.raw()) }?
    .build_threadsafe_function()
    .callee_handled::<false>()
    .weak::<true>()
    .build()
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn advancing_cancels_older_tokens_only() {
    let epoch = CodecEpoch::default();
    let old = epoch.token();
    assert!(!old.is_cancelled());
    epoch.advance();
    let new = epoch.token();
    assert!(old.is_cancelled());
    assert!(!new.is_cancelled());
    assert!(epoch.is_current(epoch.current()));
  }

  #[test]
  fn only_an_unsupported_config_outlives_reset() {
    assert!(
      ErrorOrigin::of_configure_error("NotSupportedError: Invalid codec: bogus")
        == ErrorOrigin::Configure
    );
    assert!(ErrorOrigin::of_configure_error("Failed to open encoder: -22") == ErrorOrigin::Work);
  }

  #[test]
  fn generation_wraps() {
    let epoch = CodecEpoch::default();
    epoch.0.generation.store(u32::MAX, Ordering::SeqCst);
    let token = epoch.token();
    assert_eq!(epoch.advance(), 0);
    assert!(token.is_cancelled());
  }

  #[test]
  fn output_wakeups_only_take_their_generation() {
    fn next(epoch: &CodecEpoch, generation: u32) -> Option<BufferedOutput> {
      match epoch.next_item(generation) {
        Some(EpochItem::Output(output)) => Some(output),
        Some(_) => panic!("expected an output"),
        None => None,
      }
    }

    let epoch = CodecEpoch::default();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let output = |value| {
      let delivered = delivered.clone();
      Box::new(move |_| {
        delivered.lock().unwrap().push(value);
        Ok(Vec::new())
      }) as BufferedOutput
    };
    {
      let mut outputs = epoch.0.outputs.lock().unwrap();
      outputs.push_back((0, EpochItem::Output(output(0))));
      outputs.push_back((1, EpochItem::Output(output(1))));
      outputs.push_back((0, EpochItem::Output(output(2))));
    }

    // The new wakeup runs before both old ones. It takes only its own output, even though two
    // older entries were queued around it, and leaves the old entries in their original order.
    next(&epoch, 1).unwrap()(ptr::null_mut()).unwrap();
    assert!(next(&epoch, 1).is_none());
    next(&epoch, 0).unwrap()(ptr::null_mut()).unwrap();
    next(&epoch, 0).unwrap()(ptr::null_mut()).unwrap();
    assert!(next(&epoch, 0).is_none());
    assert_eq!(*delivered.lock().unwrap(), [1, 0, 2]);
  }
}
