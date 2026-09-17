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
  /// The ondequeue event handler property value, plus its registration-order
  /// id — an event handler participates in listener ordering, so assigning it
  /// after addEventListener must run it after earlier registrations.
  ondequeue: Option<Arc<FunctionRef<Unknown<'static>, UnknownReturnValue>>>,
  ondequeue_id: u64,
  /// Enqueued work items whose dequeue dispatch has not been delivered (or
  /// discarded) yet. While >0 the codec_obj ref is held strong so a queued
  /// dispatch always finds a live event target — the codec cannot be GC'd
  /// between the worker's fire() and delivery on the JS thread. Returns to
  /// weak when the count drains to zero.
  outstanding: usize,
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
      // A listener registered while work is already in flight must find the
      // codec pinned, matching what note_enqueue does for later enqueues.
      if self.outstanding > 0 {
        self.set_codec_strong(env, true);
      }
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
            if let Ok(mut state) = shared.write() {
              state.note_delivery(ctx.env);
            }
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

  /// JS thread: a work item was enqueued, so a dequeue dispatch will follow.
  /// Retains the codec object strongly until that dispatch is delivered so
  /// listeners observe a live target/currentTarget/this.
  pub fn note_enqueue(&mut self, env: &Env) {
    if self.outstanding == 0 {
      self.set_codec_strong(env, true);
    }
    self.outstanding += 1;
  }

  /// JS thread: queued work was discarded without dequeue dispatches
  /// (reset/close/reconfigure). `cleared` is the number of dropped items;
  /// items already fired but undelivered stay outstanding until delivered.
  pub fn note_queue_cleared(&mut self, env: &Env, cleared: usize) {
    self.outstanding = self.outstanding.saturating_sub(cleared);
    if self.outstanding == 0 {
      self.set_codec_strong(env, false);
    }
  }

  /// Any thread: a work item completed without producing a dispatch (no
  /// dispatcher existed, or the TSFN was already closing). Only adjusts the
  /// counter — with no dispatcher there is no codec ref, and a closing TSFN
  /// means the env is tearing down, so no unref is required here.
  fn note_undelivered(&mut self) {
    self.outstanding = self.outstanding.saturating_sub(1);
  }

  /// JS thread: a queued dequeue dispatch finished delivery — its work item
  /// is no longer outstanding.
  fn note_delivery(&mut self, env: &Env) {
    self.outstanding = self.outstanding.saturating_sub(1);
    if self.outstanding == 0 {
      self.set_codec_strong(env, false);
    }
  }

  /// Bump/lower the codec_obj ref between strong and weak. JS thread only.
  fn set_codec_strong(&mut self, env: &Env, strong: bool) {
    if let Some(ref codec) = self.codec_obj {
      let mut count = 0u32;
      unsafe {
        if strong {
          sys::napi_reference_ref(env.raw(), codec.raw_ref, &mut count);
        } else {
          sys::napi_reference_unref(env.raw(), codec.raw_ref, &mut count);
        }
      }
    }
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
    // Only once-listeners for 'dequeue' — the codec's sole automatic event —
    // keep the process alive. A once-listener for a type the codec never
    // emits on its own would pin the process forever.
    if once && event_type == "dequeue" {
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
    if removed_once && event_type == "dequeue" {
      self.once_pending = self.once_pending.saturating_sub(1);
      self.sync_once_ref(env);
    }
  }

  /// Replace the ondequeue event handler. Each non-null assignment counts as
  /// a fresh registration for ordering purposes (DOM event-handler semantics).
  pub fn set_ondequeue(
    &mut self,
    handler: Option<FunctionRef<Unknown<'static>, UnknownReturnValue>>,
  ) {
    // Re-assigning while a handler is set replaces the callback in the same
    // slot (DOM event-handler semantics); only a null -> non-null transition
    // creates a new registration.
    if handler.is_some() && self.ondequeue.is_none() {
      self.ondequeue_id = self.next_listener_id;
      self.next_listener_id += 1;
    }
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
    let Some(dispatcher) = dispatcher else {
      // No dispatcher: the item completed without a dispatch. Drain it so a
      // listener registered later doesn't see a stale outstanding count —
      // with no dispatcher there is no codec ref to unpin anyway.
      if let Ok(mut state) = shared.write() {
        state.note_undelivered();
      }
      return;
    };
    let ptr = Arc::into_raw(Arc::clone(shared)) as usize as u64;
    let status = dispatcher.call(
      FnArgs::from((CMD_DISPATCH, (ptr >> 32) as u32, ptr as u32)),
      ThreadsafeFunctionCallMode::NonBlocking,
    );
    if status != Status::Ok {
      // TSFN already closing — reclaim the leaked Arc and drain the item;
      // the env is tearing down so no unref is needed on this thread.
      unsafe {
        drop(Arc::from_raw(
          ptr as usize as *const RwLock<CodecEventState>,
        ));
      }
      if let Ok(mut state) = shared.write() {
        state.note_undelivered();
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
    // The ondequeue handler only applies to "dequeue" events; it participates
    // in registration order with ordinary listeners.
    if event_type == "dequeue"
      && let Some(ref handler) = state.ondequeue
    {
      snapshot.push(DispatchItem {
        id: state.ondequeue_id,
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
    snapshot.sort_by_key(|i| i.id);
    (state.codec_obj.as_ref().and_then(|r| r.get(env)), snapshot)
  };

  if snapshot.is_empty() {
    return;
  }

  // One Event per dispatch. The event is never dispatched through a real
  // EventTarget, so target/currentTarget/eventPhase are exposed as own
  // getters that report codec state while dispatching and forward to the
  // native prototype getters otherwise — transparent if a listener retains
  // the event and re-dispatches it through a real EventTarget.
  let Ok(event) = new_event(env, event_type) else {
    return;
  };
  let Ok(mut event_obj) = (unsafe { event.cast::<Object>() }) else {
    return;
  };
  let in_dispatch = Arc::new(AtomicBool::new(true));
  // A strong ref keeps the codec alive for as long as the event's `target`
  // can be observed (DOM keeps event.target reachable after dispatch). The
  // finalizer is attached to the target getter FUNCTION below, not the event:
  // an accessor extracted via getOwnPropertyDescriptor can outlive the event,
  // so the handle's lifetime must follow the closure that dereferences it.
  let codec_ref = codec_obj.as_ref().and_then(|codec| {
    let mut r = ptr::null_mut();
    let ok = unsafe {
      sys::napi_create_reference(env.raw(), codec.raw(), 1, &mut r) == sys::Status::napi_ok
    };
    if ok { Some(r as usize) } else { None }
  });

  // The getters are real functions installed via Object.defineProperty —
  // with_getter_closure ties the Rust closure's lifetime to the EVENT (its
  // finalizer lives on the target object), so an accessor extracted via
  // getOwnPropertyDescriptor would call freed memory after the event is
  // collected. create_function_from_closure anchors the closure on the
  // function itself, which keeps extracted accessors safe.
  let mut target_getter: Option<sys::napi_value> = None;
  for name in ["target", "currentTarget"] {
    let flag = in_dispatch.clone();
    let Ok(getter_fn) = env.create_function_from_closure::<(), Unknown, _>(
      name,
      move |ctx: FunctionCallContext| -> Result<Unknown> {
        if flag.load(Ordering::SeqCst)
          && let Some(r) = codec_ref
          && let Ok(v) = ref_value(ctx.env, r)
        {
          return Ok(as_static(v));
        }
        let this = ctx.this::<Object>()?;
        let native = forward_event_getter(ctx.env, this, name).ok();
        if let Some(v) = native
          && !is_nullish(ctx.env.raw(), v.raw())
        {
          return Ok(as_static(v));
        }
        // Native slot empty (never natively dispatched or dispatch finished):
        // target still resolves to the codec (event.target persists after
        // dispatch); currentTarget correctly stays null.
        if name == "target"
          && let Some(r) = codec_ref
          && let Ok(v) = ref_value(ctx.env, r)
        {
          return Ok(as_static(v));
        }
        match native {
          Some(v) => Ok(as_static(v)),
          None => {
            let raw = unsafe { Null::to_napi_value(ctx.env.raw(), Null) }?;
            Ok(as_static(unsafe {
              Unknown::from_napi_value(ctx.env.raw(), raw)
            }?))
          }
        }
      },
    ) else {
      continue;
    };
    if name == "target" {
      target_getter = Some(getter_fn.raw());
    }
    let _ = define_getter(env, event_obj.raw(), name, getter_fn.raw());
  }

  // Anchor codec_ref's lifetime to the target getter — the only closure that
  // dereferences it outside dispatch (currentTarget/eventPhase only touch it
  // while in_dispatch, when the event is necessarily alive). If anchoring
  // fails, drop the own props so no closure captures a dangling handle.
  let mut ref_anchored = codec_ref.is_none();
  if let (Some(r), Some(getter_fn)) = (codec_ref, target_getter) {
    ref_anchored = unsafe {
      sys::napi_add_finalizer(
        env.raw(),
        getter_fn,
        r as *mut std::ffi::c_void,
        Some(release_codec_ref),
        ptr::null_mut(),
        ptr::null_mut(),
      ) == sys::Status::napi_ok
    };
  }
  if !ref_anchored {
    let _ = event_obj.delete_named_property("target");
    let _ = event_obj.delete_named_property("currentTarget");
    if let Some(r) = codec_ref {
      unsafe { sys::napi_delete_reference(env.raw(), r as sys::napi_ref) };
    }
  }

  {
    let flag = in_dispatch.clone();
    if let Ok(phase_fn) = env.create_function_from_closure::<(), u32, _>(
      "eventPhase",
      move |ctx: FunctionCallContext| -> Result<u32> {
        if flag.load(Ordering::SeqCst) {
          return Ok(2); // Event.AT_TARGET
        }
        let this = ctx.this::<Object>()?;
        forward_event_getter(ctx.env, this, "eventPhase")
          .and_then(|v| unsafe { u32::from_napi_value(ctx.env.raw(), v.raw()) })
          .or(Ok(0))
      },
    ) {
      let _ = define_getter(env, event_obj.raw(), "eventPhase", phase_fn.raw());
    }
  }

  // DOM: honor stopImmediatePropagation. During our dispatch the wrapper only
  // flips a flag this loop observes — deliberately NOT forwarding to the
  // native method, which would set the event's internal stop flag and poison
  // a later native dispatchEvent. After dispatch (the wrapper may be
  // extracted and retained), calls forward to the native method so the real
  // stop flag is set — matching DOM for an event dispatched natively later.
  let stop_immediate = Arc::new(AtomicBool::new(false));
  {
    let flag = stop_immediate.clone();
    let dispatching = in_dispatch.clone();
    if let Ok(wrapper) = env.create_function_from_closure::<(), Unknown, _>(
      "stopImmediatePropagation",
      move |ctx: FunctionCallContext| -> Result<Unknown> {
        if dispatching.load(Ordering::SeqCst) {
          flag.store(true, Ordering::SeqCst);
          let mut raw = ptr::null_mut();
          check_status!(unsafe { sys::napi_get_undefined(ctx.env.raw(), &mut raw) })?;
          return unsafe { Unknown::from_napi_value(ctx.env.raw(), raw) };
        }
        let this = ctx.this::<Unknown>()?;
        call_event_proto_method(ctx.env, this.raw(), "stopImmediatePropagation")
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

  // DOM: composedPath returns [codec] during dispatch. Outside our dispatch
  // the captured raw codec handle may be stale (its handle scope has ended),
  // so the wrapper forwards to the native prototype method instead — which
  // returns [] for a non-dispatching event and the real path during a native
  // re-dispatch. This stays safe even if the wrapper is extracted and called
  // after the event property is deleted.
  if let Some(ref codec) = codec_obj {
    let codec_raw = codec.raw() as usize;
    let dispatching = in_dispatch.clone();
    if let Ok(wrapper) = env.create_function_from_closure::<(), Unknown, _>(
      "composedPath",
      move |ctx: FunctionCallContext| -> Result<Unknown> {
        if !dispatching.load(Ordering::SeqCst) {
          let this = ctx.this::<Unknown>()?;
          return call_event_proto_method(ctx.env, this.raw(), "composedPath");
        }
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
    // DOM: skip listeners removed between snapshot and invocation. For the
    // ondequeue slot, match by registration id and resolve the CURRENT
    // handler — an earlier listener may have replaced the callback in-place
    // (which keeps the slot), and the replacement must run.
    let invoke_cb = {
      let Ok(state) = shared.read() else {
        break;
      };
      if item.is_ondequeue {
        if state.ondequeue_id == item.id {
          state.ondequeue.clone()
        } else {
          None
        }
      } else {
        let live = state
          .listeners
          .get(event_type)
          .map(|l| l.iter().any(|e| e.id == item.id))
          .unwrap_or(false);
        if live {
          Some(item.callback.clone())
        } else {
          None
        }
      }
    };
    let Some(callback) = invoke_cb else {
      continue;
    };
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
      if event_type == "dequeue" {
        state.once_pending = state.once_pending.saturating_sub(1);
        state.sync_once_ref(env);
      }
    }
    if let Ok(func) = callback.borrow_back(env) {
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

  // Dispatch complete: the getters now forward to the native prototype
  // getters, so currentTarget/eventPhase read null/NONE automatically while
  // target keeps resolving to the codec.
  in_dispatch.store(false, Ordering::SeqCst);

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

/// Relax a JS value's scope lifetime to 'static. Only used for values that are
/// converted back to a raw napi_value within the same synchronous napi call.
fn as_static(v: Unknown) -> Unknown<'static> {
  unsafe { std::mem::transmute(v) }
}

/// Get a JS value through a strong napi_ref created on this env.
fn ref_value<'a>(env: &'a Env, refr: usize) -> Result<Unknown<'a>> {
  let mut v = ptr::null_mut();
  check_status!(unsafe {
    sys::napi_get_reference_value(env.raw(), refr as sys::napi_ref, &mut v)
  })?;
  unsafe { Unknown::from_napi_value(env.raw(), v) }
}

/// Finalizer releasing the per-event strong ref to the codec object.
unsafe extern "C" fn release_codec_ref(
  env: sys::napi_env,
  finalize_data: *mut std::ffi::c_void,
  _hint: *mut std::ffi::c_void,
) {
  unsafe { sys::napi_delete_reference(env, finalize_data as sys::napi_ref) };
}

fn is_nullish(env: sys::napi_env, v: sys::napi_value) -> bool {
  let mut t = sys::ValueType::napi_undefined;
  unsafe { sys::napi_typeof(env, v, &mut t) };
  matches!(
    t,
    sys::ValueType::napi_undefined | sys::ValueType::napi_null
  )
}

/// Invoke `Event.prototype`'s native getter for `name` on `this`. Values are
/// fetched fresh in the caller's handle scope, so this is safe to call at any
/// time — including from retained events long after our dispatch returned.
fn forward_event_getter<'a>(env: &'a Env, this: Object<'a>, name: &str) -> Result<Unknown<'a>> {
  let env_raw = env.raw();
  unsafe {
    let global = env.get_global()?.raw();
    let object_ctor = get_named_raw(env_raw, global, "Object")?;
    let gopd = get_named_raw(env_raw, object_ctor, "getOwnPropertyDescriptor")?;
    let event_ctor = get_named_raw(env_raw, global, "Event")?;
    let proto = get_named_raw(env_raw, event_ctor, "prototype")?;
    let name_v = env.create_string(name)?.raw();
    let mut desc = ptr::null_mut();
    check_status!(sys::napi_call_function(
      env_raw,
      object_ctor,
      gopd,
      2,
      [proto, name_v].as_ptr(),
      &mut desc,
    ))?;
    if is_nullish(env_raw, desc) {
      return Err(Error::new(Status::GenericFailure, "no such Event getter"));
    }
    let getter = get_named_raw(env_raw, desc, "get")?;
    let mut out = ptr::null_mut();
    check_status!(sys::napi_call_function(
      env_raw,
      this.raw(),
      getter,
      0,
      ptr::null(),
      &mut out,
    ))?;
    Unknown::from_napi_value(env_raw, out)
  }
}

/// Invoke the native `Event.prototype.<name>` method on `this` via raw
/// N-API — the Event constructor is a function, so typed
/// `get_named_property::<Object>` reads reject it. Safe at any time; fetches
/// everything fresh in the caller's handle scope.
fn call_event_proto_method(
  env: &Env,
  this: sys::napi_value,
  name: &str,
) -> Result<Unknown<'static>> {
  let env_raw = env.raw();
  unsafe {
    let global = env.get_global()?.raw();
    let event_ctor = get_named_raw(env_raw, global, "Event")?;
    let proto = get_named_raw(env_raw, event_ctor, "prototype")?;
    let method = get_named_raw(env_raw, proto, name)?;
    let mut out = ptr::null_mut();
    check_status!(sys::napi_call_function(
      env_raw,
      this,
      method,
      0,
      ptr::null(),
      &mut out,
    ))?;
    Ok(as_static(Unknown::from_napi_value(env_raw, out)?))
  }
}

/// Install `getter_fn` as an own getter `name` on `obj` via
/// `Object.defineProperty(obj, name, { get, configurable, enumerable })` —
/// raw N-API because the Object constructor is a function value.
fn define_getter(
  env: &Env,
  obj: sys::napi_value,
  name: &str,
  getter_fn: sys::napi_value,
) -> Result<()> {
  let env_raw = env.raw();
  unsafe {
    let global = env.get_global()?.raw();
    let object_ctor = get_named_raw(env_raw, global, "Object")?;
    let define_property = get_named_raw(env_raw, object_ctor, "defineProperty")?;
    let mut desc = ptr::null_mut();
    check_status!(sys::napi_create_object(env_raw, &mut desc))?;
    check_status!(sys::napi_set_named_property(
      env_raw,
      desc,
      c"get".as_ptr(),
      getter_fn
    ))?;
    let mut t = ptr::null_mut();
    check_status!(sys::napi_get_boolean(env_raw, true, &mut t))?;
    check_status!(sys::napi_set_named_property(
      env_raw,
      desc,
      c"configurable".as_ptr(),
      t
    ))?;
    check_status!(sys::napi_set_named_property(
      env_raw,
      desc,
      c"enumerable".as_ptr(),
      t
    ))?;
    let name_v = env.create_string(name)?.raw();
    let mut out = ptr::null_mut();
    check_status!(sys::napi_call_function(
      env_raw,
      object_ctor,
      define_property,
      3,
      [obj, name_v, desc].as_ptr(),
      &mut out,
    ))?;
    Ok(())
  }
}

fn get_named_raw(env: sys::napi_env, obj: sys::napi_value, name: &str) -> Result<sys::napi_value> {
  let cname = std::ffi::CString::new(name)?;
  let mut v = ptr::null_mut();
  check_status!(unsafe { sys::napi_get_named_property(env, obj, cname.as_ptr(), &mut v) })?;
  Ok(v)
}
