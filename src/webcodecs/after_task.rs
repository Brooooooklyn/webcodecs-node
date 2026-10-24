//! Work on the JS thread once the current task and its microtasks are over, like a setImmediate()
//! callback, but out of reach of fake timers that replace the `setImmediate` global of the main
//! context (`@sinonjs/fake-timers`, Vitest's vi.useFakeTimers(), node:test's mock.timers): they
//! replace the global, not the event loop. Jest's fake timers live in the vm context of its tests,
//! whose globals the addon does not read. Chrome's own scheduling does not use page globals either.
//!
//! In Node the work runs in a libuv check handle of the env's loop: the loop phase in which Node
//! runs the setImmediate queue. libuv runs a loop's check handles from the one started last
//! (uv_check_start puts a handle at the head of the list, loop-watcher.c), and Node starts its own
//! once, when the env starts. So this work runs before every setImmediate callback of that loop
//! turn, also those queued before it. An empty idle handle runs along, as Node's does for
//! setImmediate: while it is active the loop does not wait for I/O before the check phase.
//!
//! The work must not call JS or Node-API: it runs in a raw libuv callback, with no HandleScope and
//! no microtask checkpoint, and it can run while Node tears the env down and JS is no longer
//! allowed (the loop runs on until the cleanup hook below is done).
//!
//! The libuv and Node-API functions are looked up in the process at run time with `libloading`,
//! much as napi-sys looks up Node-API. On Windows only the exe is searched (napi-sys also tries a
//! node DLL): where Node lives in a DLL, nothing is found. Where they are missing (Deno has no
//! uv_check_init), where the runtime is not Node (Bun exports these libuv names, but as stubs that
//! abort with "unsupported uv function"), and on WASI, a Node-API threadsafe function queues the
//! work on the JS thread instead. Only where that is unavailable too does `after_task` return
//! false and leave scheduling to the caller.
//!
//! Teardown: an async env cleanup hook closes both handles and frees them once libuv is done with
//! them. Node runs the loop until the hook says it is done (napi_remove_async_cleanup_hook): a
//! worker's loop must have no handle left when it closes. A fallback threadsafe function's
//! finalizer frees queued work without running it once JS is unavailable.

use napi::Env;

/// Work for `after_task`
pub(crate) type Work = Box<dyn FnOnce()>;

/// Run `work` on the JS thread of `env` once the current task and its microtasks are over (see the
/// module docs). Returns false, and drops `work`, where the host has no such scheduler: the caller
/// then schedules the work itself. `work` may be dropped at env teardown without running.
pub(crate) fn after_task(env: &Env, work: Work) -> bool {
  #[cfg(not(target_family = "wasm"))]
  let work = match uv::after_task(env, work) {
    Ok(()) => return true,
    Err(work) => work,
  };
  tsfn::after_task(env, work)
}

#[cfg(not(target_family = "wasm"))]
mod uv {
  use std::alloc::{Layout, alloc_zeroed, dealloc};
  use std::cell::{Cell, RefCell};
  use std::collections::HashMap;
  use std::ffi::{c_int, c_void};
  use std::panic::{AssertUnwindSafe, catch_unwind};
  use std::ptr;
  use std::sync::OnceLock;

  use napi::bindgen_prelude::*;
  use napi::sys;

  use super::Work;

  /// `uv_handle_type` (uv.h, UV_HANDLE_TYPE_MAP)
  const UV_CHECK: c_int = 2;
  const UV_IDLE: c_int = 6;

  type HandleCb = unsafe extern "C" fn(*mut c_void);
  type CleanupHook = unsafe extern "C" fn(*mut c_void, *mut c_void);

  /// The host's libuv and Node-API functions, looked up once per process
  struct Host {
    handle_size: unsafe extern "C" fn(c_int) -> usize,
    check_init: unsafe extern "C" fn(*mut sys::uv_loop_s, *mut c_void) -> c_int,
    check_start: unsafe extern "C" fn(*mut c_void, HandleCb) -> c_int,
    check_stop: unsafe extern "C" fn(*mut c_void) -> c_int,
    idle_init: unsafe extern "C" fn(*mut sys::uv_loop_s, *mut c_void) -> c_int,
    idle_start: unsafe extern "C" fn(*mut c_void, HandleCb) -> c_int,
    idle_stop: unsafe extern "C" fn(*mut c_void) -> c_int,
    close: unsafe extern "C" fn(*mut c_void, Option<HandleCb>),
    add_cleanup_hook: unsafe extern "C" fn(
      sys::napi_env,
      Option<CleanupHook>,
      *mut c_void,
      *mut *mut c_void,
    ) -> sys::napi_status,
    remove_cleanup_hook: unsafe extern "C" fn(*mut c_void) -> sys::napi_status,
    /// Keeps the functions loaded
    _library: libloading::Library,
  }

  fn host() -> Option<&'static Host> {
    static HOST: OnceLock<Option<Host>> = OnceLock::new();
    HOST
      .get_or_init(|| {
        #[cfg(unix)]
        let library: libloading::Library = libloading::os::unix::Library::this().into();
        #[cfg(windows)]
        let library: libloading::Library = libloading::os::windows::Library::this().ok()?.into();
        if bun(&library) {
          return None;
        }
        // SAFETY: each name is a libuv or Node-API function with the signature given here
        // (uv.h, node_api.h); `_library` keeps them loaded for as long as the process runs.
        unsafe {
          Some(Host {
            handle_size: *library.get(b"uv_handle_size\0").ok()?,
            check_init: *library.get(b"uv_check_init\0").ok()?,
            check_start: *library.get(b"uv_check_start\0").ok()?,
            check_stop: *library.get(b"uv_check_stop\0").ok()?,
            idle_init: *library.get(b"uv_idle_init\0").ok()?,
            idle_start: *library.get(b"uv_idle_start\0").ok()?,
            idle_stop: *library.get(b"uv_idle_stop\0").ok()?,
            close: *library.get(b"uv_close\0").ok()?,
            add_cleanup_hook: *library.get(b"napi_add_async_cleanup_hook\0").ok()?,
            remove_cleanup_hook: *library.get(b"napi_remove_async_cleanup_hook\0").ok()?,
            _library: library,
          })
        }
      })
      .as_ref()
  }

  /// Whether the process is Bun, whose libuv functions must never be called (see the module docs),
  /// from a symbol that page code cannot edit: Bun's debug helper `dumpBtjsTrace`, which Node 18 to
  /// 26, Electron 44 and macOS's JavaScriptCore do not export. Bun exports it on macOS and
  /// Windows, not on Linux (the macOS arm64 builds 1.3.11 to 1.4.3 and 1.4.3 for macOS x64 and
  /// Windows x64 export it; 1.4.3 for Linux gnu and musl, x64 and arm64, and 1.3.11 for Linux
  /// arm64 do not): there `in_node` tells Bun apart. (Bun's JavaScriptCore symbols tell nothing:
  /// an Electron process has macOS's JavaScriptCore loaded.)
  fn bun(library: &libloading::Library) -> bool {
    // SAFETY: only looked up, never called or read
    unsafe { library.get::<*const c_void>(b"dumpBtjsTrace\0") }.is_ok()
  }

  /// Whether the runtime is Node (Electron too), from what page code can see: `process.versions`
  /// has `node` but not `bun` or `deno`, and there is no `Bun` global or `process.isBun`. Page code
  /// can edit `process.versions` and `process.isBun`, but not Bun's `Bun` global: Bun makes it
  /// neither writable nor configurable (seen in 1.3.11 to 1.4.3). So on Linux, where Bun has no
  /// symbol to look up (`bun`), the `Bun` global tells it apart. On any doubt the answer is no,
  /// and the caller uses setImmediate. Known once one env has a `process` to read.
  fn in_node(env: &Env) -> bool {
    static NODE: OnceLock<bool> = OnceLock::new();
    if let Some(node) = NODE.get() {
      return *node;
    }
    let node = (|| -> Result<bool> {
      let global = env.get_global()?;
      let process: Object = global.get_named_property("process")?;
      let versions: Object = process.get_named_property("versions")?;
      Ok(
        versions.get_named_property::<Unknown>("node")?.get_type()? == ValueType::String
          && !versions.has_named_property("bun")?
          && !versions.has_named_property("deno")?
          && global.get_named_property::<Unknown>("Bun")?.get_type()? == ValueType::Undefined
          && !process
            .get_named_property::<Unknown>("isBun")?
            .coerce_to_bool()?,
      )
    })();
    match node {
      Ok(node) => *NODE.get_or_init(|| node),
      // No `process` here (a getter may have thrown): not known
      Err(_) => {
        let mut exception = ptr::null_mut();
        unsafe { sys::napi_get_and_clear_last_exception(env.raw(), &mut exception) };
        false
      }
    }
  }

  /// The loop check of one env. Only its JS thread uses it.
  struct Checks {
    host: &'static Host,
    env: usize,
    /// uv_check_t and uv_idle_t, `data` (their first field) pointing here
    check: *mut c_void,
    idle: *mut c_void,
    check_layout: Layout,
    idle_layout: Layout,
    /// The work for the next check
    work: RefCell<Vec<Work>>,
    /// At teardown: the handles not closed yet, and the cleanup hook to remove after them
    open: Cell<u8>,
    hook: Cell<*mut c_void>,
  }

  thread_local! {
    /// The loop checks of the envs on this thread (the main thread or a worker, possibly with the
    /// module loaded more than once), by env pointer. An env's cleanup hook removes its entry
    /// before the env goes away, so a reused pointer never finds a stale one.
    static CHECKS: RefCell<HashMap<usize, *mut Checks>> = RefCell::new(HashMap::new());
  }

  pub(super) fn after_task(env: &Env, work: Work) -> std::result::Result<(), Work> {
    let key = env.raw() as usize;
    let Ok(found) = CHECKS.try_with(|checks| checks.borrow().get(&key).copied()) else {
      return Err(work);
    };
    let checks = match found {
      Some(checks) => checks,
      None => {
        let Some(checks) = Checks::new(env) else {
          return Err(work);
        };
        CHECKS.with(|all| all.borrow_mut().insert(key, checks));
        checks
      }
    };
    // SAFETY: entries live until the env's teardown hook, which removes them first
    let checks = unsafe { &*checks };
    let first = {
      let mut work_list = checks.work.borrow_mut();
      work_list.push(work);
      work_list.len() == 1
    };
    if first {
      // SAFETY: open handles of this env's loop, on its JS thread
      unsafe {
        (checks.host.check_start)(checks.check, on_check);
        (checks.host.idle_start)(checks.idle, on_idle);
      }
    }
    Ok(())
  }

  impl Checks {
    /// The loop check of `env`, with its teardown hook, or None where there is none
    fn new(env: &Env) -> Option<*mut Checks> {
      let host = host()?;
      if !in_node(env) {
        return None;
      }
      let uv_loop = env
        .get_uv_event_loop()
        .ok()
        .filter(|uv_loop| !uv_loop.is_null())?;
      // SAFETY: plain libuv call; it gives (size_t)-1 for a bad type, which Layout rejects
      let (check_size, idle_size) =
        unsafe { ((host.handle_size)(UV_CHECK), (host.handle_size)(UV_IDLE)) };
      let check_layout = Layout::from_size_align(check_size, 16).ok()?;
      let idle_layout = Layout::from_size_align(idle_size, 16).ok()?;
      if check_size < size_of::<*mut c_void>() || idle_size < size_of::<*mut c_void>() {
        return None;
      }
      // SAFETY: non-zero sizes
      let check = unsafe { alloc_zeroed(check_layout) }.cast::<c_void>();
      let idle = unsafe { alloc_zeroed(idle_layout) }.cast::<c_void>();
      let free = |check: *mut c_void, idle: *mut c_void| {
        // SAFETY: memory from above that libuv does not know
        unsafe {
          if !check.is_null() {
            dealloc(check.cast(), check_layout);
          }
          if !idle.is_null() {
            dealloc(idle.cast(), idle_layout);
          }
        }
      };
      if check.is_null() || idle.is_null() {
        free(check, idle);
        return None;
      }
      // SAFETY: zeroed memory of the size libuv asks for, on the loop's thread
      if unsafe { (host.check_init)(uv_loop, check) } != 0 {
        free(check, idle);
        return None;
      }
      let checks = Box::into_raw(Box::new(Checks {
        host,
        env: env.raw() as usize,
        check,
        idle,
        check_layout,
        idle_layout,
        work: RefCell::new(Vec::new()),
        open: Cell::new(1),
        hook: Cell::new(ptr::null_mut()),
      }));
      // SAFETY: `data` is the first field of every handle (uv.h UV_HANDLE_FIELDS), which libuv
      // leaves to its user
      unsafe { *check.cast::<*mut Checks>() = checks };
      // SAFETY: as for the check handle
      if unsafe { (host.idle_init)(uv_loop, idle) } != 0 {
        // Only the check handle is open: `on_close` frees it and `checks`
        free(ptr::null_mut(), idle);
        unsafe { (host.close)(check, Some(on_close)) };
        return None;
      }
      // SAFETY: as above
      unsafe { *idle.cast::<*mut Checks>() = checks };
      // SAFETY: `checks` lives until both handles are closed
      unsafe { (*checks).open.set(2) };
      let mut hook = ptr::null_mut();
      let status =
        unsafe { (host.add_cleanup_hook)(env.raw(), Some(teardown), checks.cast(), &mut hook) };
      if status != sys::Status::napi_ok {
        // No teardown would close them: close them now, while the loop runs (`on_close` frees all)
        unsafe {
          (host.close)(check, Some(on_close));
          (host.close)(idle, Some(on_close));
        }
        return None;
      }
      Some(checks)
    }
  }

  /// The check phase: run the work queued since the last one
  unsafe extern "C" fn on_check(handle: *mut c_void) {
    // SAFETY: `data` was set to the env's live `Checks`
    let checks = unsafe { &**handle.cast::<*mut Checks>() };
    // SAFETY: open handles of this loop
    unsafe {
      (checks.host.check_stop)(checks.check);
      (checks.host.idle_stop)(checks.idle);
    }
    let work = std::mem::take(&mut *checks.work.borrow_mut());
    for work in work {
      // A panic must not unwind into libuv
      let _ = catch_unwind(AssertUnwindSafe(work));
    }
  }

  /// Keeps the poll phase from waiting while a check is due (see the module docs)
  unsafe extern "C" fn on_idle(_handle: *mut c_void) {}

  /// The env's async cleanup hook: forget the env, drop its work and close both handles
  unsafe extern "C" fn teardown(hook: *mut c_void, arg: *mut c_void) {
    // SAFETY: `arg` is the env's live `Checks`
    let checks = unsafe { &*arg.cast::<Checks>() };
    let _ = CHECKS.try_with(|all| all.borrow_mut().remove(&checks.env));
    checks.hook.set(hook);
    drop(std::mem::take(&mut *checks.work.borrow_mut()));
    // SAFETY: open handles; libuv calls `on_close` for each in a later loop turn
    unsafe {
      (checks.host.close)(checks.check, Some(on_close));
      (checks.host.close)(checks.idle, Some(on_close));
    }
  }

  /// A handle is closed: free it, and once both are, the rest, and tell Node the hook is done
  unsafe extern "C" fn on_close(handle: *mut c_void) {
    // SAFETY: `data` was set to the env's `Checks`, which lives until its last handle closes
    let checks = unsafe { *handle.cast::<*mut Checks>() };
    let state = unsafe { &*checks };
    let layout = if handle == state.check {
      state.check_layout
    } else {
      state.idle_layout
    };
    // SAFETY: libuv no longer uses a handle once its close callback runs
    unsafe { dealloc(handle.cast(), layout) };
    state.open.set(state.open.get() - 1);
    if state.open.get() == 0 {
      let host = state.host;
      let hook = state.hook.get();
      // SAFETY: both handles are gone; nothing else points here
      drop(unsafe { Box::from_raw(checks) });
      if !hook.is_null() {
        unsafe { (host.remove_cleanup_hook)(hook) };
      }
    }
  }
}

/// The portable host queue used where the libuv check is unavailable. One threadsafe function per
/// batch preserves call order and stays referenced until the batch is drained. The batch owns its
/// non-Send closures; only a notification crosses the host queue.
mod tsfn {
  use std::cell::RefCell;
  use std::collections::HashMap;
  use std::ffi::c_void;
  use std::panic::{AssertUnwindSafe, catch_unwind};
  use std::ptr;

  use napi::bindgen_prelude::*;
  use napi::sys;

  use super::Work;

  /// One env's pending batch. Only its JS thread accesses this state. The threadsafe function's
  /// finalizer frees it after the notification is drained.
  struct Tasks {
    env: usize,
    work: RefCell<Vec<Work>>,
  }

  thread_local! {
    static TASKS: RefCell<HashMap<usize, *mut Tasks>> = RefCell::new(HashMap::new());
  }

  pub(super) fn after_task(env: &Env, work: Work) -> bool {
    let key = env.raw() as usize;
    let Ok(found) = TASKS.try_with(|tasks| tasks.borrow().get(&key).copied()) else {
      return false;
    };
    if let Some(tasks) = found {
      // SAFETY: the map holds this batch until its callback, on this same JS thread
      unsafe { &*tasks }.work.borrow_mut().push(work);
      return true;
    }
    let Some((tasks, function)) = Tasks::new(env, work) else {
      clear_exception(env.raw());
      return false;
    };
    TASKS.with(|all| all.borrow_mut().insert(key, tasks));
    let status = unsafe {
      sys::napi_call_threadsafe_function(
        function,
        ptr::null_mut(),
        sys::ThreadsafeFunctionCallMode::nonblocking,
      )
    };
    if status == sys::Status::napi_closing {
      // The finalizer owns and drops the batch now; the function must not be touched again.
      return false;
    }
    if status != sys::Status::napi_ok {
      forget(tasks);
      unsafe {
        sys::napi_release_threadsafe_function(function, sys::ThreadsafeFunctionReleaseMode::abort)
      };
      clear_exception(env.raw());
      return false;
    }
    // Release the only acquisition. The referenced function keeps the loop alive until `on_call`
    // has drained the batch and the host has finalized it. This must be its last use here.
    unsafe {
      sys::napi_release_threadsafe_function(function, sys::ThreadsafeFunctionReleaseMode::release)
    };
    true
  }

  impl Tasks {
    fn new(env: &Env, work: Work) -> Option<(*mut Tasks, sys::napi_threadsafe_function)> {
      // The custom call_js callback does not call this function; it is only the queue's target.
      let callback = env
        .create_function_from_closure::<(), (), _>("afterTask", |_| Ok(()))
        .ok()?;
      let name = env.create_string("webcodecs_after_task").ok()?;
      let tasks = Box::into_raw(Box::new(Tasks {
        env: env.raw() as usize,
        work: RefCell::new(vec![work]),
      }));
      let mut function = ptr::null_mut();
      // SAFETY: all values belong to this live env. On success the host calls `finalize` once for
      // `tasks`; before success this function reclaims it below.
      let status = unsafe {
        sys::napi_create_threadsafe_function(
          env.raw(),
          callback.raw(),
          ptr::null_mut(),
          name.raw(),
          0,
          1,
          tasks.cast(),
          Some(finalize),
          tasks.cast(),
          Some(on_call),
          &mut function,
        )
      };
      if status != sys::Status::napi_ok {
        // SAFETY: creation failed, so the host did not take ownership of `tasks`
        drop(unsafe { Box::from_raw(tasks) });
        return None;
      }
      Some((tasks, function))
    }
  }

  unsafe extern "C" fn on_call(
    env: sys::napi_env,
    js_callback: sys::napi_value,
    context: *mut c_void,
    _data: *mut c_void,
  ) {
    // Aborting can finalize `context` before the host drains notifications with no JS callback.
    // The finalizer owns any closures still in the queue, so there is nothing to do in that case.
    if env.is_null() || js_callback.is_null() {
      return;
    }
    // SAFETY: with JS still available, the context remains live until this notification is drained
    let tasks = unsafe { &*context.cast::<Tasks>() };
    loop {
      let work = std::mem::take(&mut *tasks.work.borrow_mut());
      if work.is_empty() {
        break;
      }
      for item in work {
        // A panic must not unwind into the host. Work queued by an item joins this batch because
        // its map entry remains until the loop below has drained it.
        let _ = catch_unwind(AssertUnwindSafe(item));
      }
    }
    forget(context.cast());
  }

  fn forget(tasks: *mut Tasks) {
    // An old env's finalizer must not remove a new env at the same address.
    let _ = TASKS.try_with(|all| {
      let mut all = all.borrow_mut();
      // SAFETY: `tasks` is live in both callers
      let env = unsafe { (*tasks).env };
      if all.get(&env).is_some_and(|&current| current == tasks) {
        all.remove(&env);
      }
    });
  }

  unsafe extern "C" fn finalize(_env: sys::napi_env, data: *mut c_void, _hint: *mut c_void) {
    let tasks = data.cast::<Tasks>();
    forget(tasks);
    // SAFETY: Node-API calls this once after the queue and all acquisitions are gone. Dropping the
    // box also drops any work left by a failed call or env shutdown.
    drop(unsafe { Box::from_raw(tasks) });
  }

  fn clear_exception(env: sys::napi_env) {
    let mut exception = ptr::null_mut();
    unsafe { sys::napi_get_and_clear_last_exception(env, &mut exception) };
  }

  #[cfg(test)]
  mod tests {
    use super::on_call;
    use std::ptr;

    #[test]
    fn shutdown_callbacks_return_before_touching_queue_state() {
      let mut marker = 0;
      let nonnull = &mut marker as *mut i32;
      // Each null shutdown value is checked before the other one or the context is dereferenced.
      unsafe {
        on_call(
          ptr::null_mut(),
          nonnull.cast(),
          ptr::null_mut(),
          ptr::null_mut(),
        );
        on_call(
          nonnull.cast(),
          ptr::null_mut(),
          ptr::null_mut(),
          ptr::null_mut(),
        );
      }
    }
  }
}
