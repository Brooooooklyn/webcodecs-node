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

use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeCallContext, ThreadsafeFunction, UnknownReturnValue};
use napi::{check_status, sys};

use super::error::{DOMExceptionName, error_as_dom_exception};
use super::event_target::CodecEventState;

/// Worker → JS output callback. The first argument is the codec generation.
pub(crate) type GatedOutput<Args> =
  ThreadsafeFunction<Args, UnknownReturnValue, Args, Status, false, true>;

/// Worker → JS error callback, carrying the codec generation, where the
/// error came from, and the error. The error is converted to a native
/// DOMException on the JS thread.
pub(crate) type GatedError = ThreadsafeFunction<
  (u32, ErrorOrigin, Error),
  UnknownReturnValue,
  FnArgs<(u32, Option<String>, Unknown<'static>)>,
  Status,
  false,
  true,
>;

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

#[derive(Default)]
struct EpochState {
  generation: AtomicU32,
  /// Set on the JS thread once the codec's error callback has run.
  error_delivered: AtomicBool,
  /// close() ran.
  closed: AtomicBool,
  /// Set once by the codec's constructor.
  close_on_error: OnceLock<CloseOnError>,
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
}

/// Which callback a gated function forwards to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CallbackKind {
  Output,
  Error,
}

/// Wrap a user callback in a native function whose first argument is the
/// generation the call was queued under. Calls from an older generation are
/// dropped; the others call `callback` with the remaining arguments and an
/// undefined `this`. For the error callback, the second argument is the
/// error's name when configure() found it, or null (see `admit_error`). A
/// worker ThreadsafeFunction targets the returned function.
///
/// An exception thrown by the callback stays pending, so it surfaces exactly
/// as it did when the ThreadsafeFunction called the callback directly.
fn gated_callback<'env, Args: JsValuesTupleIntoVec>(
  env: &'env Env,
  callback: sys::napi_value,
  epoch: CodecEpoch,
  kind: CallbackKind,
) -> Result<Function<'env, Args, UnknownReturnValue>> {
  let callback = unsafe {
    Function::<Unknown<'static>, UnknownReturnValue>::from_napi_value(env.raw(), callback)?
  }
  .create_ref()?;
  let gated = env.create_function_from_closure::<Args, (), _>("codecCallback", move |ctx| {
    let generation = ctx.get::<u32>(0)?;
    let first_arg = match kind {
      CallbackKind::Output => {
        if !epoch.is_current(generation) {
          return Ok(());
        }
        1
      }
      CallbackKind::Error => {
        let configure_error = ctx.get::<Option<String>>(1)?;
        if !epoch.admit_error(ctx.env, generation, configure_error.as_deref()) {
          return Ok(());
        }
        2
      }
    };
    let mut args = Vec::with_capacity(ctx.length().saturating_sub(first_arg));
    for index in first_arg..ctx.length() {
      args.push(ctx.get::<Unknown>(index)?.raw());
    }
    let function = callback.borrow_back(ctx.env)?;
    let mut this = ptr::null_mut();
    check_status!(unsafe { sys::napi_get_undefined(ctx.env.raw(), &mut this) })?;
    let mut result = ptr::null_mut();
    let status = unsafe {
      sys::napi_call_function(
        ctx.env.raw(),
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
  })?;
  unsafe { Function::from_napi_value(env.raw(), gated.raw()) }
}

/// Weak ThreadsafeFunction that calls the output callback `callback` with
/// `Args` minus its leading generation, unless the generation is stale.
pub(crate) fn gated_output<Args: 'static + JsValuesTupleIntoVec>(
  env: &Env,
  callback: sys::napi_value,
  epoch: &CodecEpoch,
) -> Result<GatedOutput<Args>> {
  gated_callback::<Args>(env, callback, epoch.clone(), CallbackKind::Output)?
    .build_threadsafe_function()
    .callee_handled::<false>()
    .weak::<true>()
    .build()
}

/// Weak ThreadsafeFunction that hands the error callback `callback` a native
/// DOMException (WebCodecsErrorCallback receives DOMException per spec),
/// unless close() or reset() dropped the error.
pub(crate) fn gated_error(
  env: &Env,
  callback: sys::napi_value,
  epoch: &CodecEpoch,
) -> Result<GatedError> {
  gated_callback::<FnArgs<(u32, Option<String>, Unknown<'static>)>>(
    env,
    callback,
    epoch.clone(),
    CallbackKind::Error,
  )?
  .build_threadsafe_function::<(u32, ErrorOrigin, Error)>()
  .callee_handled::<false>()
  .weak::<true>()
  .build_callback(|ctx: ThreadsafeCallContext<(u32, ErrorOrigin, Error)>| {
    let (generation, origin, error) = ctx.value;
    let configure_error = (origin == ErrorOrigin::Configure).then(|| {
      DOMExceptionName::from_error_message(&error.reason)
        .as_str()
        .to_owned()
    });
    Ok(FnArgs::from((
      generation,
      configure_error,
      error_as_dom_exception(&ctx.env, error)?,
    )))
  })
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
}
