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

use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeCallContext, ThreadsafeFunction, UnknownReturnValue};
use napi::{check_status, sys};

use super::error::error_as_dom_exception;

/// Worker → JS output callback. The first argument is the codec generation.
pub(crate) type GatedOutput<Args> =
  ThreadsafeFunction<Args, UnknownReturnValue, Args, Status, false, true>;

/// Worker → JS error callback, carrying the codec generation and the error.
/// The error is converted to a native DOMException on the JS thread.
pub(crate) type GatedError = ThreadsafeFunction<
  (u32, Error),
  UnknownReturnValue,
  FnArgs<(u32, Unknown<'static>)>,
  Status,
  false,
  true,
>;

#[derive(Default)]
struct EpochState {
  generation: AtomicU32,
  /// Set on the JS thread once the codec's error callback has run.
  error_delivered: AtomicBool,
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
/// undefined `this`. A worker ThreadsafeFunction targets the returned function.
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
    if !epoch.is_current(generation) {
      return Ok(());
    }
    if kind == CallbackKind::Error {
      epoch.0.error_delivered.store(true, Ordering::SeqCst);
    }
    let mut args = Vec::with_capacity(ctx.length().saturating_sub(1));
    for index in 1..ctx.length() {
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
/// unless the generation is stale.
pub(crate) fn gated_error(
  env: &Env,
  callback: sys::napi_value,
  epoch: &CodecEpoch,
) -> Result<GatedError> {
  gated_callback::<FnArgs<(u32, Unknown<'static>)>>(
    env,
    callback,
    epoch.clone(),
    CallbackKind::Error,
  )?
  .build_threadsafe_function::<(u32, Error)>()
  .callee_handled::<false>()
  .weak::<true>()
  .build_callback(|ctx: ThreadsafeCallContext<(u32, Error)>| {
    let (generation, error) = ctx.value;
    Ok(FnArgs::from((
      generation,
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
  fn generation_wraps() {
    let epoch = CodecEpoch::default();
    epoch.0.generation.store(u32::MAX, Ordering::SeqCst);
    let token = epoch.token();
    assert_eq!(epoch.advance(), 0);
    assert!(token.is_cancelled());
  }
}
