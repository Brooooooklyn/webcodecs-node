//! Shared EventTarget dispatch for WebCodecs codec classes.
//!
//! Each codec keeps a registry of JS listener callbacks plus a weak reference
//! to the codec's own JS object (captured via `this`). Worker threads fire
//! through a single ThreadsafeFunction whose JS-side closure dispatches one
//! Event to all listeners, so every listener observes the same event identity
//! and `event.target`/`event.currentTarget`/`this` resolve to the codec
//! object, matching DOM EventTarget semantics.

use napi::Env;
use napi::bindgen_prelude::*;
use napi::check_status;
use napi::sys;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use std::collections::HashMap;
use std::ptr;
use std::sync::{Arc, RwLock};

use super::error::new_event;

/// A registered event listener. The FunctionRef both calls the JS callback and
/// identifies the listener for removeEventListener (strict equality).
struct EventListenerEntry {
  id: u64,
  once: bool,
  callback: Arc<FunctionRef<Unknown<'static>, UnknownReturnValue>>,
}

/// Weak reference to the codec's JS object.
///
/// A strong napi_ref would create a reference cycle (codec object → Rust state
/// → napi_ref → codec object) and pin the codec against GC forever, so this
/// uses a zero-refcount (weak) reference: once the codec is collected,
/// `get()` returns None and dispatches degrade to a null target.
///
/// The raw napi_ref handle itself is intentionally never deleted: deleting a
/// reference must run on the owning JS thread, while this state can be dropped
/// by the worker thread holding the last Arc. The leaked handle is a small GC
/// slot only — it does not keep the JS object alive.
struct WeakCodecRef {
  raw_ref: sys::napi_ref,
}

// The napi_ref is only ever dereferenced on the JS thread (inside `dispatch`),
// so sharing the handle across threads is sound.
unsafe impl Send for WeakCodecRef {}
unsafe impl Sync for WeakCodecRef {}

impl WeakCodecRef {
  fn new(env: &Env, obj: Object) -> Result<Self> {
    let mut raw_ref = ptr::null_mut();
    check_status!(unsafe { sys::napi_create_reference(env.raw(), obj.raw(), 0, &mut raw_ref) })?;
    Ok(Self { raw_ref })
  }

  /// Get the codec object on the JS thread; None once the codec is GC'd.
  fn get<'env>(&self, env: &'env Env) -> Option<Object<'env>> {
    let mut value = ptr::null_mut();
    let status = unsafe { sys::napi_get_reference_value(env.raw(), self.raw_ref, &mut value) };
    if status != sys::Status::napi_ok || value.is_null() {
      return None;
    }
    unsafe { Object::from_napi_value(env.raw(), value).ok() }
  }
}

type DispatcherTsf = ThreadsafeFunction<(), (), (), Status, false, true>;

/// Per-codec event state shared between the JS thread and the worker thread.
#[derive(Default)]
pub struct CodecEventState {
  listeners: HashMap<String, Vec<Arc<EventListenerEntry>>>,
  next_listener_id: u64,
  /// Weak ref to the codec's JS object (`this`), captured lazily on the first
  /// event-related call so dispatched events expose it as target/currentTarget.
  codec_obj: Option<WeakCodecRef>,
  /// The ondequeue event handler property value.
  ondequeue: Option<Arc<FunctionRef<Unknown<'static>, UnknownReturnValue>>>,
  /// Single dispatcher invoked from the worker thread; runs the whole
  /// dispatch on the JS thread in one call.
  dispatcher: Option<Arc<DispatcherTsf>>,
}

impl CodecEventState {
  /// Capture the codec's JS object on first use (JS thread only).
  pub fn set_codec_obj(&mut self, env: &Env, this: Object) -> Result<()> {
    if self.codec_obj.is_none() {
      self.codec_obj = Some(WeakCodecRef::new(env, this)?);
    }
    Ok(())
  }

  /// Ensure the worker→JS dispatcher exists (JS thread only).
  pub fn ensure_dispatcher(
    &mut self,
    env: &Env,
    shared: &Arc<RwLock<CodecEventState>>,
  ) -> Result<()> {
    if self.dispatcher.is_some() {
      return Ok(());
    }
    let shared = shared.clone();
    let dispatch_fn = env.create_function_from_closure(
      "codecDispatch",
      move |ctx: FunctionCallContext| -> Result<()> {
        dispatch(ctx.env, &shared, "dequeue");
        Ok(())
      },
    )?;
    let tsf: DispatcherTsf = dispatch_fn
      .build_threadsafe_function()
      .callee_handled::<false>()
      .weak::<true>()
      .build()?;
    self.dispatcher = Some(Arc::new(tsf));
    Ok(())
  }

  /// Register an event listener (DOM addEventListener).
  /// Re-registering the identical callback for the same type is a no-op per
  /// DOM spec.
  pub fn add_listener(
    &mut self,
    env: &Env,
    event_type: &str,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    once: bool,
  ) -> Result<()> {
    let listeners = self.listeners.entry(event_type.to_string()).or_default();
    for entry in listeners.iter() {
      let same = match (entry.callback.borrow_back(env), callback.borrow_back(env)) {
        (Ok(registered), Ok(passed)) => env.strict_equals(registered, passed).unwrap_or(false),
        _ => false,
      };
      if same {
        return Ok(());
      }
    }
    let id = self.next_listener_id;
    self.next_listener_id += 1;
    listeners.push(Arc::new(EventListenerEntry {
      id,
      once,
      callback: Arc::new(callback),
    }));
    Ok(())
  }

  /// Remove the first registered listener whose callback strict-equals the
  /// one passed (DOM removeEventListener).
  pub fn remove_listener(
    &mut self,
    env: &Env,
    event_type: &str,
    callback: &FunctionRef<Unknown<'static>, UnknownReturnValue>,
  ) {
    if let Some(listeners) = self.listeners.get_mut(event_type) {
      if let Some(pos) = listeners.iter().position(|entry| {
        match (entry.callback.borrow_back(env), callback.borrow_back(env)) {
          (Ok(registered), Ok(passed)) => env.strict_equals(registered, passed).unwrap_or(false),
          _ => false,
        }
      }) {
        listeners.remove(pos);
      }
      if listeners.is_empty() {
        self.listeners.remove(event_type);
      }
    }
  }

  /// Replace the ondequeue event handler.
  pub fn set_ondequeue(
    &mut self,
    handler: Option<FunctionRef<Unknown<'static>, UnknownReturnValue>>,
  ) {
    self.ondequeue = handler.map(Arc::new);
  }

  /// The current ondequeue handler, for the property getter.
  pub fn ondequeue(&self) -> Option<&Arc<FunctionRef<Unknown<'static>, UnknownReturnValue>>> {
    self.ondequeue.as_ref()
  }

  /// Fire the dequeue event from the worker thread (async, non-blocking).
  pub fn fire(&self) {
    if let Some(ref dispatcher) = self.dispatcher {
      dispatcher.call((), ThreadsafeFunctionCallMode::NonBlocking);
    }
  }
}

/// Snapshot item: listener id plus a shared handle to the JS callback.
struct DispatchItem {
  id: u64,
  once: bool,
  callback: Arc<FunctionRef<Unknown<'static>, UnknownReturnValue>>,
  /// True for the ondequeue handler pseudo-entry
  is_ondequeue: bool,
}

/// Dispatch one Event to ondequeue (for "dequeue" events) and all registered
/// listeners for `event_type`. Runs on the JS thread.
///
/// A single Event is created per dispatch so every listener observes the same
/// identity; `target`/`currentTarget` are set to the codec object and `this`
/// inside each listener is the codec (DOM dispatch semantics). Listeners
/// added during dispatch do not run in this dispatch; listeners removed
/// before their turn are skipped; `once` listeners are removed after firing.
pub fn dispatch(env: &Env, shared: &Arc<RwLock<CodecEventState>>, event_type: &str) {
  let (codec_obj, snapshot) = {
    let Ok(state) = shared.read() else {
      return;
    };
    let mut snapshot: Vec<DispatchItem> = Vec::new();
    // The ondequeue handler only applies to "dequeue" events
    if event_type == "dequeue"
      && let Some(ref handler) = state.ondequeue
    {
      snapshot.push(DispatchItem {
        id: u64::MAX,
        once: false,
        callback: handler.clone(),
        is_ondequeue: true,
      });
    }
    if let Some(listeners) = state.listeners.get(event_type) {
      for entry in listeners {
        snapshot.push(DispatchItem {
          id: entry.id,
          once: entry.once,
          callback: entry.callback.clone(),
          is_ondequeue: false,
        });
      }
    }
    (state.codec_obj.as_ref().and_then(|r| r.get(env)), snapshot)
  };

  if snapshot.is_empty() {
    return;
  }

  // One Event per dispatch; expose the codec as target/currentTarget via own
  // data properties (the Event prototype only defines getters, so plain
  // assignment would fail).
  let Ok(event) = new_event(env, event_type) else {
    return;
  };
  let Ok(mut event_obj) = (unsafe { event.cast::<Object>() }) else {
    return;
  };
  if let Some(ref codec) = codec_obj {
    if let Ok(target_prop) = Property::new()
      .with_utf8_name("target")
      .and_then(|p| p.with_napi_value(env, codec))
    {
      let _ = event_obj.define_properties(&[target_prop]);
    }
    if let Ok(ct_prop) = Property::new()
      .with_utf8_name("currentTarget")
      .and_then(|p| p.with_napi_value(env, codec))
    {
      let _ = event_obj.define_properties(&[ct_prop]);
    }
  }

  let mut once_ids = Vec::new();
  for item in &snapshot {
    // DOM: skip listeners removed between snapshot and invocation
    let still_registered = {
      let Ok(state) = shared.read() else {
        break;
      };
      if item.is_ondequeue {
        state
          .ondequeue
          .as_ref()
          .map(|h| Arc::ptr_eq(h, &item.callback))
          .unwrap_or(false)
      } else {
        state
          .listeners
          .get(event_type)
          .map(|l| l.iter().any(|e| e.id == item.id))
          .unwrap_or(false)
      }
    };
    if !still_registered {
      continue;
    }
    if let Ok(func) = item.callback.borrow_back(env) {
      // Relax the event's phantom lifetime to match the stored callback's
      // args type; the event is only used within this synchronous call.
      let arg: Unknown<'static> = unsafe { std::mem::transmute(event) };
      let _ = match &codec_obj {
        Some(codec) => func.apply(*codec, arg).map(|_| ()),
        None => func.call(arg).map(|_| ()),
      };
    }
    if item.once {
      once_ids.push(item.id);
    }
  }

  // DOM: currentTarget is null once dispatch completes
  if codec_obj.is_some()
    && let Ok(ct_prop) = Property::new()
      .with_utf8_name("currentTarget")
      .and_then(|p| p.with_napi_value(env, Null))
  {
    let _ = event_obj.define_properties(&[ct_prop]);
  }

  // Remove once listeners that fired
  if !once_ids.is_empty()
    && let Ok(mut state) = shared.write()
    && let Some(listeners) = state.listeners.get_mut(event_type)
  {
    listeners.retain(|e| !once_ids.contains(&e.id));
    if listeners.is_empty() {
      state.listeners.remove(event_type);
    }
  }
}
