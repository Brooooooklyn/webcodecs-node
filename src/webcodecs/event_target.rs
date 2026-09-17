//! Shared EventTarget dispatch for WebCodecs codec classes.
//!
//! Each codec keeps a registry of JS listener callbacks plus a weak reference
//! to the codec's own JS object (captured via `this`). Worker threads fire
//! through a single ThreadsafeFunction whose JS-side closure dispatches one
//! Event to all listeners, so every listener observes the same event identity
//! and `event.target`/`event.currentTarget`/`this` resolve to the codec
//! object, matching DOM EventTarget semantics.

use napi::Env;
use napi::PropertyAttributes;
use napi::bindgen_prelude::*;
use napi::check_status;
use napi::sys;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use std::collections::HashMap;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use super::error::new_event;

/// Dispatcher commands, sent as three u32 args (a pointer cannot cross the JS
/// boundary as a single number without f64 truncation).
/// - (CMD_DISPATCH, hi, lo): dispatch a dequeue event, taking ownership of the
///   `Arc<RwLock<CodecEventState>>` leaked at that pointer — a queued dispatch
///   keeps the state alive until it runs.
/// - (CMD_DELETE_REF, hi, lo): delete the raw napi_ref at that pointer —
///   off-thread drops of WeakCodecRef route handle release through here.
const CMD_DELETE_REF: u32 = 1;
const CMD_DISPATCH: u32 = 2;

/// A registered event listener. The FunctionRef both calls the JS callback and
/// identifies the listener for removeEventListener (strict equality).
struct EventListenerEntry {
  id: u64,
  once: bool,
  /// DOM capture flag — part of listener identity for add/remove matching.
  capture: bool,
  callback: Arc<FunctionRef<Unknown<'static>, UnknownReturnValue>>,
}

/// Weak reference to the codec's JS object.
///
/// A strong napi_ref would create a reference cycle (codec object → Rust state
/// → napi_ref → codec object) and pin the codec against GC forever, so this
/// uses a zero-refcount (weak) reference: once the codec is collected,
/// `get()` returns None and dispatches degrade to a null target.
struct WeakCodecRef {
  raw_ref: sys::napi_ref,
  env: sys::napi_env,
  /// Thread this ref was created on (the JS thread). Used to decide whether
  /// drop can delete the ref directly.
  js_thread: std::thread::ThreadId,
  /// Dispatcher TSF used to release this ref on the JS thread when the final
  /// drop lands on a worker thread. Optional because the dispatcher is created
  /// lazily after the ref itself.
  gc: Option<Arc<DispatcherTsf>>,
}

// The napi_ref is only ever dereferenced or deleted on the JS thread, so
// sharing the handle across threads is sound.
unsafe impl Send for WeakCodecRef {}
unsafe impl Sync for WeakCodecRef {}

impl WeakCodecRef {
  fn new(env: &Env, obj: Object) -> Result<Self> {
    let mut raw_ref = ptr::null_mut();
    check_status!(unsafe { sys::napi_create_reference(env.raw(), obj.raw(), 0, &mut raw_ref) })?;
    Ok(Self {
      raw_ref,
      env: env.raw(),
      js_thread: std::thread::current().id(),
      gc: None,
    })
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

impl Drop for WeakCodecRef {
  fn drop(&mut self) {
    if self.raw_ref.is_null() {
      return;
    }
    if std::thread::current().id() == self.js_thread {
      // Dropped on the JS thread: delete the ref directly.
      unsafe {
        sys::napi_delete_reference(self.env, self.raw_ref);
      }
    } else if let Some(gc) = self.gc.take() {
      // Dropped on a worker thread: napi_delete_reference must run on the
      // owning JS thread, so route the handle through the dispatcher. If the
      // TSFN is already closing (env teardown), the handle is reclaimed by
      // Node anyway — leaking is safe in that path.
      let addr = self.raw_ref as usize as u64;
      gc.call(
        FnArgs::from((
          CMD_DELETE_REF,
          ((addr >> 32) & 0xffff_ffff) as u32,
          addr as u32,
        )),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }
    // No dispatcher available on a worker-thread drop: the codec was dropped
    // without ever registering a listener — the raw handle slot leaks, but it
    // does not keep the JS object alive.
  }
}

// The payload must be FnArgs — a plain tuple goes through the ToNapiValue
// blanket impl and arrives as a single Array argument, not three args.
type DispatcherTsf =
  ThreadsafeFunction<FnArgs<(u32, u32, u32)>, (), FnArgs<(u32, u32, u32)>, Status, false, true>;

/// Per-codec event state shared between the JS thread and the worker thread.
#[derive(Default)]
pub struct CodecEventState {
  listeners: HashMap<String, Vec<Arc<EventListenerEntry>>>,
  next_listener_id: u64,
  /// Registered once-listeners not yet fired. While >0 the dispatcher TSFN is
  /// ref'd (strong) so Node cannot exit before they run — matching the
  /// historical strong-TSF semantics for once listeners.
  once_pending: usize,
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
  /// Ensure the worker→JS dispatcher exists (JS thread only).
  ///
  /// The codec's JS object (`this`) is captured lazily here rather than in a
  /// separate step: a WeakCodecRef without a dispatcher would have no route to
  /// delete its napi_ref when the final drop lands on a worker thread.
  pub fn ensure_dispatcher(&mut self, env: &Env, this: Object) -> Result<()> {
    if self.dispatcher.is_some() {
      return Ok(());
    }
    if self.codec_obj.is_none() {
      self.codec_obj = Some(WeakCodecRef::new(env, this)?);
    }
    // The closure captures no state: each dispatch payload carries ownership
    // of a strong Arc<RwLock<CodecEventState>> (leaked via Arc::into_raw in
    // fire()), so a queued dispatch keeps the state alive until it runs —
    // without the dispatcher retaining the state permanently.
    let dispatch_fn = env.create_function_from_closure(
      "codecDispatch",
      |ctx: FunctionCallContext| -> Result<()> {
        let cmd = ctx.get::<u32>(0).unwrap_or(0);
        let ptr =
          ((ctx.get::<u32>(1).unwrap_or(0) as u64) << 32) | ctx.get::<u32>(2).unwrap_or(0) as u64;
        match cmd {
          CMD_DISPATCH => {
            let shared = unsafe { Arc::from_raw(ptr as usize as *const RwLock<CodecEventState>) };
            dispatch(ctx.env, &shared, "dequeue");
          }
          CMD_DELETE_REF => {
            // Delete a napi_ref handle on the JS thread (see WeakCodecRef::drop)
            unsafe {
              sys::napi_delete_reference(ctx.env.raw(), ptr as usize as sys::napi_ref);
            }
          }
          _ => {}
        }
        Ok(())
      },
    )?;
    let tsf: DispatcherTsf = dispatch_fn
      .build_threadsafe_function()
      .callee_handled::<false>()
      .weak::<true>()
      .build()?;
    let dispatcher = Arc::new(tsf);
    // Let the codec-obj ref route its deletion through this dispatcher when it
    // drops on a worker thread.
    if let Some(ref mut codec) = self.codec_obj {
      codec.gc = Some(dispatcher.clone());
    }
    self.dispatcher = Some(dispatcher);
    Ok(())
  }

  /// Toggle the dispatcher TSFN's ref on 0↔1 transitions of once_pending.
  /// JS thread only.
  fn sync_once_ref(&mut self, env: &Env) {
    if let Some(ref dispatcher) = self.dispatcher {
      unsafe {
        if self.once_pending > 0 {
          sys::napi_ref_threadsafe_function(env.raw(), dispatcher.handle.get_raw());
        } else {
          sys::napi_unref_threadsafe_function(env.raw(), dispatcher.handle.get_raw());
        }
      }
    }
  }

  /// Register an event listener (DOM addEventListener).
  /// Re-registering the identical callback with the same capture flag for the
  /// same type is a no-op per DOM spec.
  pub fn add_listener(
    &mut self,
    env: &Env,
    event_type: &str,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    once: bool,
    capture: bool,
  ) -> Result<()> {
    let listeners = self.listeners.entry(event_type.to_string()).or_default();
    for entry in listeners.iter() {
      let same = entry.capture == capture
        && match (entry.callback.borrow_back(env), callback.borrow_back(env)) {
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
      capture,
      callback: Arc::new(callback),
    }));
    if once {
      self.once_pending += 1;
      self.sync_once_ref(env);
    }
    Ok(())
  }

  /// Remove the first registered listener whose callback strict-equals the
  /// one passed with a matching capture flag (DOM removeEventListener).
  pub fn remove_listener(
    &mut self,
    env: &Env,
    event_type: &str,
    callback: &FunctionRef<Unknown<'static>, UnknownReturnValue>,
    capture: bool,
  ) {
    let mut removed_once = false;
    if let Some(listeners) = self.listeners.get_mut(event_type) {
      if let Some(pos) = listeners.iter().position(|entry| {
        entry.capture == capture
          && match (entry.callback.borrow_back(env), callback.borrow_back(env)) {
            (Ok(registered), Ok(passed)) => env.strict_equals(registered, passed).unwrap_or(false),
            _ => false,
          }
      }) {
        removed_once = listeners[pos].once;
        listeners.remove(pos);
      }
      if listeners.is_empty() {
        self.listeners.remove(event_type);
      }
    }
    if removed_once {
      self.once_pending = self.once_pending.saturating_sub(1);
      self.sync_once_ref(env);
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
  /// A strong Arc is leaked into the payload and reclaimed by the JS callback,
  /// so a queued dispatch keeps the state (and its listeners) alive even if
  /// the codec wrapper and worker are gone by the time it runs.
  pub fn fire(shared: &Arc<RwLock<CodecEventState>>) {
    let dispatcher = shared.read().ok().and_then(|s| s.dispatcher.clone());
    if let Some(dispatcher) = dispatcher {
      let ptr = Arc::into_raw(Arc::clone(shared)) as usize as u64;
      let status = dispatcher.call(
        FnArgs::from((CMD_DISPATCH, (ptr >> 32) as u32, ptr as u32)),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
      if status != Status::Ok {
        // TSFN already closing — reclaim the leaked Arc.
        unsafe {
          drop(Arc::from_raw(
            ptr as usize as *const RwLock<CodecEventState>,
          ));
        }
      }
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
/// before their turn are skipped; `once` listeners are removed before
/// invocation so a re-entrant dispatchEvent cannot observe them again.
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

  // DOM: honor stopImmediatePropagation. The event never passes through a real
  // EventTarget, so shadow the method with a wrapper that flips a flag this
  // loop can observe, then forwards to the native implementation so the
  // event's internal flag stays correct if it is re-dispatched elsewhere.
  let stop_immediate = Arc::new(AtomicBool::new(false));
  {
    // The prototype method is an intrinsic that outlives this dispatch, so the
    // raw handle can be captured into the 'static closure safely.
    let orig_sip = env
      .get_global()
      .and_then(|g| g.get_named_property::<Object>("Event"))
      .and_then(|e| e.get_named_property::<Object>("prototype"))
      .and_then(|p| p.get_named_property::<Unknown>("stopImmediatePropagation"))
      .map(|f| f.raw() as usize)
      .unwrap_or(0);
    let flag = stop_immediate.clone();
    if let Ok(wrapper) = env.create_function_from_closure::<(), (), _>(
      "stopImmediatePropagation",
      move |ctx: FunctionCallContext| -> Result<()> {
        flag.store(true, Ordering::SeqCst);
        if orig_sip != 0
          && let Ok(this) = ctx.this::<Unknown>()
          && let Ok(orig) = unsafe {
            Function::<(), Unknown>::from_napi_value(ctx.env.raw(), orig_sip as sys::napi_value)
          }
        {
          let _ = orig.apply(this, ());
        }
        Ok(())
      },
    ) && let Ok(sip_prop) = Property::new()
      .with_utf8_name("stopImmediatePropagation")
      .and_then(|p| p.with_napi_value(env, wrapper))
    {
      let _ = event_obj.define_properties(&[sip_prop.with_property_attributes(
        PropertyAttributes::Writable | PropertyAttributes::Configurable,
      )]);
    }
  }

  // DOM: emulate the dispatch-phase state a real EventTarget would set.
  // eventPhase reads AT_TARGET while listeners run (reset to NONE below);
  // composedPath returns [codec] during dispatch — the wrapper is deleted in
  // cleanup so post-dispatch calls hit the native prototype method (which
  // returns [] for a non-dispatching event) and the captured codec handle is
  // never dereferenced outside this callback's handle scope.
  if let Ok(phase_prop) = Property::new()
    .with_utf8_name("eventPhase")
    .and_then(|p| p.with_napi_value(env, 2u32))
  {
    let _ = event_obj.define_properties(&[phase_prop]);
  }
  if let Some(ref codec) = codec_obj {
    let codec_raw = codec.raw() as usize;
    if let Ok(wrapper) = env.create_function_from_closure::<(), Unknown, _>(
      "composedPath",
      move |ctx: FunctionCallContext| -> Result<Unknown> {
        let mut path = ctx.env.create_array(1)?;
        let codec =
          unsafe { Object::from_napi_value(ctx.env.raw(), codec_raw as sys::napi_value) }?;
        path.set_element(0, codec)?;
        unsafe { Unknown::from_napi_value(ctx.env.raw(), path.raw()) }
      },
    ) && let Ok(cp_prop) = Property::new()
      .with_utf8_name("composedPath")
      .and_then(|p| p.with_napi_value(env, wrapper))
    {
      let _ = event_obj.define_properties(&[cp_prop.with_property_attributes(
        PropertyAttributes::Writable | PropertyAttributes::Configurable,
      )]);
    }
  }

  let mut thrown: Vec<sys::napi_value> = Vec::new();
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
    // DOM spec: remove "once" listeners before invoking so a re-entrant
    // dispatchEvent inside the listener cannot observe it again.
    if item.once
      && let Ok(mut state) = shared.write()
      && let Some(listeners) = state.listeners.get_mut(event_type)
    {
      listeners.retain(|e| e.id != item.id);
      if listeners.is_empty() {
        state.listeners.remove(event_type);
      }
      state.once_pending = state.once_pending.saturating_sub(1);
      state.sync_once_ref(env);
    }
    if let Ok(func) = item.callback.borrow_back(env) {
      // Relax the event's phantom lifetime to match the stored callback's
      // args type; the event is only used within this synchronous call.
      let arg: Unknown<'static> = unsafe { std::mem::transmute(event) };
      let res = match &codec_obj {
        Some(codec) => func.apply(*codec, arg).map(|_| ()),
        None => func.call(arg).map(|_| ()),
      };
      if res.is_err() {
        // A throwing listener leaves a pending exception on the env that
        // would poison every subsequent napi call. DOM reports listener
        // exceptions and continues dispatch, so clear it now and report each
        // one after dispatch completes.
        let mut pending = false;
        unsafe { sys::napi_is_exception_pending(env.raw(), &mut pending) };
        if pending {
          let mut exc = ptr::null_mut();
          let status = unsafe { sys::napi_get_and_clear_last_exception(env.raw(), &mut exc) };
          if status == sys::Status::napi_ok {
            thrown.push(exc);
          }
        }
      }
    }
    if stop_immediate.load(Ordering::SeqCst) {
      break;
    }
  }

  // DOM: currentTarget is null and eventPhase is NONE once dispatch completes
  if codec_obj.is_some()
    && let Ok(ct_prop) = Property::new()
      .with_utf8_name("currentTarget")
      .and_then(|p| p.with_napi_value(env, Null))
  {
    let _ = event_obj.define_properties(&[ct_prop]);
  }
  if let Ok(phase_prop) = Property::new()
    .with_utf8_name("eventPhase")
    .and_then(|p| p.with_napi_value(env, 0u32))
  {
    let _ = event_obj.define_properties(&[phase_prop]);
  }

  // Remove the dispatch-time method wrappers so listeners retaining the event
  // fall back to the prototype methods — the wrappers' captured raw napi_values
  // are only valid within this dispatcher's handle scope.
  let _ = event_obj.delete_named_property("stopImmediatePropagation");
  let _ = event_obj.delete_named_property("composedPath");

  // DOM: every listener exception is reported (not propagated from
  // dispatchEvent). Schedule a rethrow on the microtask queue so each
  // surfaces as an uncaught error while the caller still sees a normal return.
  for exc in thrown {
    report_exception(env, exc);
  }
}

/// Report one listener exception as an uncaught error via the microtask queue.
/// Falls back to throwing inline if scheduling fails — better than swallowing.
fn report_exception(env: &Env, exc: sys::napi_value) {
  let mut exc_ref = ptr::null_mut();
  let env_raw = env.raw();
  let reported =
    unsafe { sys::napi_create_reference(env_raw, exc, 1, &mut exc_ref) == sys::Status::napi_ok }
      && env
        .create_function_from_closure::<(), (), _>("reportListenerException", move |_| {
          unsafe {
            let mut v = ptr::null_mut();
            sys::napi_get_reference_value(env_raw, exc_ref, &mut v);
            sys::napi_delete_reference(env_raw, exc_ref);
            if !v.is_null() {
              sys::napi_throw(env_raw, v);
            }
          }
          Ok(())
        })
        .and_then(|reporter| {
          let global = env.get_global()?;
          let qm =
            global.get_named_property::<Function<Function<(), ()>, Unknown>>("queueMicrotask")?;
          qm.call(reporter).map(|_| ())
        })
        .is_ok();
  if !reported {
    unsafe { sys::napi_throw(env_raw, exc) };
    if !exc_ref.is_null() {
      unsafe { sys::napi_delete_reference(env_raw, exc_ref) };
    }
  }
}
