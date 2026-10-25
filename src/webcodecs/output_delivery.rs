use napi::bindgen_prelude::{FromNapiValue, JsValue, ToNapiValue, Unknown, spawn_blocking};
use napi::{Env, Error, Result, Status};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::Duration;

use super::flush_tracker::FlushAbort;

pub(crate) enum DeliveryCompletion {
  Retry,
  Complete,
}

/// Counts outputs handed to a threadsafe output callback and the callbacks
/// that have run, so flush() can wait until earlier outputs reached JS before
/// it delivers its own outputs and resolves.
#[derive(Default)]
struct EnvShutdown {
  cancelled: AtomicBool,
  changed: Condvar,
  counts: Mutex<HashMap<usize, (u64, u64)>>,
}

impl EnvShutdown {
  fn cancel(&self) {
    self.cancelled.store(true, Ordering::SeqCst);
    self.changed.notify_all();
  }
}

static NEXT_DELIVERY_ID: AtomicUsize = AtomicUsize::new(1);
static SHUTDOWN_STATES: OnceLock<Mutex<HashMap<usize, Weak<EnvShutdown>>>> = OnceLock::new();

fn shutdown_states() -> &'static Mutex<HashMap<usize, Weak<EnvShutdown>>> {
  SHUTDOWN_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) struct OutputDelivery {
  id: usize,
  shutdown: Arc<EnvShutdown>,
}

impl OutputDelivery {
  fn from_shared(shutdown: Arc<EnvShutdown>) -> Arc<Self> {
    let id = NEXT_DELIVERY_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut counts) = shutdown.counts.lock() {
      counts.insert(id, (0, 0));
    }
    Arc::new(Self { id, shutdown })
  }

  #[cfg(test)]
  fn default() -> Self {
    let shutdown = Arc::new(EnvShutdown::default());
    let id = NEXT_DELIVERY_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut counts) = shutdown.counts.lock() {
      counts.insert(id, (0, 0));
    }
    Self { id, shutdown }
  }

  pub(crate) fn new(env: &Env) -> Result<Arc<Self>> {
    let key = env.raw() as usize;
    let mut states = shutdown_states()
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    let shutdown = match states.get(&key).and_then(Weak::upgrade) {
      Some(shutdown) => shutdown,
      None => {
        let shutdown = Arc::new(EnvShutdown::default());
        let cleanup = shutdown.clone();
        // One hook per env owns the shared state. Codec trackers hold only a
        // clone of that Arc and can be dropped independently after close/GC.
        env.add_env_cleanup_hook(cleanup, move |shutdown| {
          shutdown.cancel();
          if let Ok(mut states) = shutdown_states().lock() {
            states.remove(&key);
          }
        })?;
        states.insert(key, Arc::downgrade(&shutdown));
        shutdown
      }
    };
    drop(states);
    Ok(Self::from_shared(shutdown))
  }

  #[cfg(test)]
  fn cancel(&self) {
    self.shutdown.cancel();
  }

  /// Returns the completion callback for one threadsafe output call. The
  /// output counts as delivered once the callback runs or is dropped unrun.
  pub(crate) fn track<R: 'static>(
    self: &Arc<Self>,
  ) -> impl FnOnce(Result<R>, Env) -> Result<()> + 'static {
    if let Ok(mut all) = self.shutdown.counts.lock()
      && let Some(counts) = all.get_mut(&self.id)
    {
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
    if let Ok(mut all) = self.shutdown.counts.lock()
      && let Some(counts) = all.get_mut(&self.id)
    {
      counts.1 += 1;
      self.shutdown.changed.notify_all();
    }
  }

  pub(crate) fn is_delivered(&self) -> bool {
    match self.shutdown.counts.lock() {
      Ok(all) => all.get(&self.id).is_none_or(|counts| counts.1 >= counts.0),
      Err(_) => true,
    }
  }

  /// Run `complete` on the JS thread after all tracked outputs have run.
  /// Error handling can add outputs after a blocking wait returns, so every
  /// wakeup rechecks delivery before completing flush. A completion that finds
  /// new outputs while taking its codec lock returns Retry without calling JS.
  pub(crate) fn after_delivered<'env, F>(
    self: &Arc<Self>,
    env: &'env Env,
    abort: Arc<FlushAbort>,
    complete: F,
  ) -> Result<Unknown<'env>>
  where
    F: for<'a> FnMut(&'a Env) -> Result<DeliveryCompletion> + 'static,
  {
    let completion: SharedCompletion = Rc::new(RefCell::new(Some(Box::new(complete))));
    self.after_delivered_inner(env, abort, completion)
  }

  fn after_delivered_inner<'env>(
    self: &Arc<Self>,
    env: &'env Env,
    abort: Arc<FlushAbort>,
    completion: SharedCompletion,
  ) -> Result<Unknown<'env>> {
    if self.shutdown.cancelled.load(Ordering::SeqCst) {
      return Err(Error::new(
        Status::Closing,
        "Codec environment is shutting down",
      ));
    }
    if abort.is_aborted() || self.is_delivered() {
      // Take the closure out so no RefCell borrow crosses a user output callback.
      let complete = completion.borrow_mut().take();
      if let Some(mut complete) = complete {
        match complete(env)? {
          DeliveryCompletion::Retry => {
            *completion.borrow_mut() = Some(complete);
          }
          DeliveryCompletion::Complete => return Self::undefined(env),
        }
      } else {
        return Self::undefined(env);
      }
    }

    let delivery = self.clone();
    let wait_delivery = delivery.clone();
    let wait_abort = abort.clone();
    env
      .spawn_future_with_callback(
        async move {
          spawn_blocking(move || wait_delivery.wait_until_delivered(&wait_abort))
            .await
            .map_err(|join_error| {
              Error::new(
                Status::GenericFailure,
                format!("Flush failed: {}", join_error),
              )
            })?;
          Ok((delivery, abort))
        },
        move |env, (delivery, abort)| delivery.after_delivered_inner(env, abort, completion),
      )
      .map(|promise| promise.to_unknown())
  }

  fn undefined<'env>(env: &'env Env) -> Result<Unknown<'env>> {
    let value = unsafe { ToNapiValue::to_napi_value(env.raw(), ())? };
    unsafe { Unknown::from_napi_value(env.raw(), value) }
  }

  pub(crate) fn wait_until_delivered(&self, abort_flag: &FlushAbort) {
    let Ok(mut all) = self.shutdown.counts.lock() else {
      return;
    };
    // A callback leaked by napi, when napi_call_threadsafe_function rejects the
    // call or the env is shutting down, is never marked delivered. Teardown and
    // reset()/close() must both release the wait without running JS.
    loop {
      let Some(&(submitted, delivered)) = all.get(&self.id) else {
        return;
      };
      if delivered >= submitted
        || abort_flag.is_aborted()
        || self.shutdown.cancelled.load(Ordering::SeqCst)
      {
        return;
      }
      all = match self
        .shutdown
        .changed
        .wait_timeout(all, Duration::from_millis(10))
      {
        Ok((all, _)) => all,
        Err(_) => return,
      };
    }
  }
}

impl Drop for OutputDelivery {
  fn drop(&mut self) {
    if let Ok(mut all) = self.shutdown.counts.lock() {
      all.remove(&self.id);
    }
  }
}

struct DeliveryToken(Arc<OutputDelivery>);

impl Drop for DeliveryToken {
  fn drop(&mut self) {
    self.0.delivered();
  }
}

type SharedCompletion =
  Rc<RefCell<Option<Box<dyn for<'a> FnMut(&'a Env) -> Result<DeliveryCompletion> + 'static>>>>;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn shutdown_releases_a_pending_delivery_wait() {
    let delivery = Arc::new(OutputDelivery::default());
    let _pending = delivery.track::<()>();
    let waiter = {
      let delivery = delivery.clone();
      std::thread::spawn(move || delivery.wait_until_delivered(&FlushAbort::default()))
    };

    delivery.cancel();

    waiter.join().unwrap();
    assert!(delivery.shutdown.cancelled.load(Ordering::SeqCst));

    let shutdown = delivery.shutdown.clone();
    let id = delivery.id;
    drop(_pending);
    drop(delivery);
    assert!(!shutdown.counts.lock().unwrap().contains_key(&id));
  }

  #[test]
  fn shared_shutdown_releases_all_waiters_and_removes_every_id() {
    let shutdown = Arc::new(EnvShutdown::default());
    let first = OutputDelivery::from_shared(shutdown.clone());
    let second = OutputDelivery::from_shared(shutdown.clone());
    let first_pending = first.track::<()>();
    let second_pending = second.track::<()>();
    let waiters = [first.clone(), second.clone()].map(|delivery| {
      std::thread::spawn(move || delivery.wait_until_delivered(&FlushAbort::default()))
    });

    shutdown.cancel();

    for waiter in waiters {
      waiter.join().unwrap();
    }
    drop(first_pending);
    drop(second_pending);
    let first_id = first.id;
    let second_id = second.id;
    drop(first);
    drop(second);
    let counts = shutdown.counts.lock().unwrap();
    assert!(!counts.contains_key(&first_id));
    assert!(!counts.contains_key(&second_id));
  }
}
