use napi::bindgen_prelude::ToNapiValue;
use napi::{Env, Result};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::flush_tracker::FlushAbort;

/// Counts outputs handed to a threadsafe output callback and the callbacks
/// that have run, so flush() can wait until earlier outputs reached JS before
/// it delivers its own outputs and resolves.
#[derive(Default)]
pub(crate) struct OutputDelivery {
  counts: Mutex<(u64, u64)>,
  changed: Condvar,
}

impl OutputDelivery {
  /// Returns the completion callback for one threadsafe output call. The
  /// output counts as delivered once the callback runs or is dropped unrun.
  pub(crate) fn track<R: 'static>(
    self: &Arc<Self>,
  ) -> impl FnOnce(Result<R>, Env) -> Result<()> + 'static {
    if let Ok(mut counts) = self.counts.lock() {
      counts.0 += 1;
    }
    let token = DeliveryToken(self.clone());
    move |result, env| {
      drop(token);
      // Requesting a completion callback makes napi catch what the JS output
      // callback throws. Rethrow the original value as a fatal exception so it
      // reaches uncaughtException unchanged, as it does without one.
      if let Err(error) = result {
        let value = unsafe { ToNapiValue::to_napi_value(env.raw(), error)? };
        unsafe { napi::sys::napi_fatal_exception(env.raw(), value) };
      }
      Ok(())
    }
  }

  fn delivered(&self) {
    if let Ok(mut counts) = self.counts.lock() {
      counts.1 += 1;
      self.changed.notify_all();
    }
  }

  pub(crate) fn wait_until_delivered(&self, abort_flag: &FlushAbort) {
    let Ok(mut counts) = self.counts.lock() else {
      return;
    };
    let target = counts.0;
    // A callback leaked by napi, when napi_call_threadsafe_function rejects the
    // call or the env is shutting down, is never marked delivered, so the abort
    // flag is polled to let reset() and close() release the wait.
    while counts.1 < target && !abort_flag.is_aborted() {
      counts = match self.changed.wait_timeout(counts, Duration::from_millis(10)) {
        Ok((counts, _)) => counts,
        Err(_) => return,
      };
    }
  }
}

struct DeliveryToken(Arc<OutputDelivery>);

impl Drop for DeliveryToken {
  fn drop(&mut self) {
    self.0.delivered();
  }
}
