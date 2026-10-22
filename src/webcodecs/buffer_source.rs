//! Reading JavaScript `BufferSource` values (ArrayBuffer, SharedArrayBuffer,
//! TypedArray, DataView) without trusting them or keeping them.
//!
//! Three rules keep native code memory safe here:
//!
//! - A view's bytes are located with engine calls (`napi_get_typedarray_info`,
//!   `napi_get_dataview_info`, `napi_get_arraybuffer_info`), never with the
//!   `buffer` / `byteOffset` / `byteLength` properties: JS can override those
//!   (own properties, getters, proxies), and reading them runs JS.
//! - A pointer into a non-shared JS buffer is only used until JS runs again.
//!   A later getter, callback, or return to the event loop can detach, shrink
//!   or rewrite the buffer, so inputs are copied into native memory right away
//!   and outputs are written before control goes back to JS.
//! - Shared memory is copied with the captured intrinsic TypedArray.set on
//!   fresh Uint8Array views. The engine handles mixed-width worker accesses;
//!   Rust never dereferences a shared-memory pointer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr;

use napi::bindgen_prelude::*;
use napi::{check_status, sys};

/// What kind of `BufferSource` a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BufferSourceKind {
  /// A `Uint8Array` (this includes Node.js `Buffer`).
  Uint8Array,
  /// Any other TypedArray.
  OtherTypedArray,
  DataView,
  /// An `ArrayBuffer` or `SharedArrayBuffer` passed directly.
  ArrayBuffer,
}

/// The bytes of a `BufferSource` as the engine reports them.
///
/// The pointer is for non-shared buffers and is only valid until JS runs again.
/// Shared buffers use `buffer` and `byte_offset` for engine-mediated copies.
pub(crate) struct RawBufferSource {
  env: sys::napi_env,
  data: *mut u8,
  len: usize,
  byte_offset: usize,
  /// The underlying ArrayBuffer (or SharedArrayBuffer).
  buffer: sys::napi_value,
  kind: BufferSourceKind,
  shared: bool,
}

impl RawBufferSource {
  pub(crate) fn kind(&self) -> BufferSourceKind {
    self.kind
  }

  pub(crate) fn len(&self) -> usize {
    self.len
  }

  /// Copies the bytes into native memory.
  pub(crate) fn to_vec(&self) -> Result<Vec<u8>> {
    if self.len == 0 || self.data.is_null() {
      return Ok(Vec::new());
    }
    if self.shared {
      return self.shared_to_vec();
    }
    let mut out = Vec::with_capacity(self.len);
    // SAFETY: the engine reported `len` readable bytes at `data`, and no JS
    // has run since. A non-shared buffer cannot be accessed by a worker.
    unsafe {
      ptr::copy_nonoverlapping(self.data, out.as_mut_ptr(), self.len);
      out.set_len(self.len);
    }
    Ok(out)
  }

  fn shared_to_vec(&self) -> Result<Vec<u8>> {
    let source = create_uint8_view(self.env, self.buffer, self.byte_offset, self.len)?;
    let mut native_buffer = ptr::null_mut();
    check_status!(unsafe {
      sys::napi_create_arraybuffer(self.env, self.len, ptr::null_mut(), &mut native_buffer)
    })?;
    let target = create_uint8_view(self.env, native_buffer, 0, self.len)?;
    typed_array_set(self.env, target, source)?;

    // Locate the ordinary destination only after the engine copy. No worker
    // can access it, and no pointer is live across the intrinsic call.
    let mut data = ptr::null_mut();
    let mut len = 0usize;
    check_status!(unsafe {
      sys::napi_get_arraybuffer_info(self.env, native_buffer, &mut data, &mut len)
    })?;
    let mut out = Vec::with_capacity(len);
    // SAFETY: this is the ordinary ArrayBuffer created above, `len` is its
    // engine-reported size, and no JS runs before the native copy finishes.
    unsafe {
      ptr::copy_nonoverlapping(data.cast::<u8>(), out.as_mut_ptr(), len);
      out.set_len(len);
    }
    Ok(out)
  }

  /// Copies native bytes into the destination without writing past them.
  pub(crate) fn write_from_slice(&self, source: &[u8]) -> Result<()> {
    assert!(source.len() <= self.len);
    if source.is_empty() {
      return Ok(());
    }
    if self.shared {
      let mut native_buffer = ptr::null_mut();
      let mut data = ptr::null_mut();
      check_status!(unsafe {
        sys::napi_create_arraybuffer(self.env, source.len(), &mut data, &mut native_buffer)
      })?;
      // SAFETY: the new ordinary ArrayBuffer has source.len() bytes, and no
      // JS runs between obtaining its pointer and initializing them.
      unsafe { ptr::copy_nonoverlapping(source.as_ptr(), data.cast::<u8>(), source.len()) };
      let native_source = create_uint8_view(self.env, native_buffer, 0, source.len())?;
      let target = create_uint8_view(self.env, self.buffer, self.byte_offset, source.len())?;
      return typed_array_set(self.env, target, native_source);
    }
    // SAFETY: the destination is valid until JS runs again, and all callers
    // provide native source storage that does not overlap the destination.
    unsafe { ptr::copy_nonoverlapping(source.as_ptr(), self.data, source.len()) };
    Ok(())
  }

  /// Writes through a native temporary for shared memory, then uses the
  /// engine to copy it back. Ordinary buffers keep their direct slice path.
  pub(crate) fn write_with(
    &self,
    len: usize,
    write: impl FnOnce(&mut [u8]) -> Result<()>,
  ) -> Result<()> {
    assert!(len <= self.len);
    if self.shared {
      let mut native = vec![0; len];
      let result = write(&mut native);
      self.write_from_slice(&native)?;
      return result;
    }
    if len == 0 || self.data.is_null() {
      return write(&mut []);
    }
    // SAFETY: this buffer is not shared, and callers do not run JS until the
    // closure returns. The slice covers only the established copy boundary.
    let destination = unsafe { std::slice::from_raw_parts_mut(self.data, len) };
    write(destination)
  }
}

fn create_uint8_view(
  env: sys::napi_env,
  buffer: sys::napi_value,
  byte_offset: usize,
  len: usize,
) -> Result<sys::napi_value> {
  let mut view = ptr::null_mut();
  check_status!(unsafe {
    sys::napi_create_typedarray(
      env,
      sys::TypedarrayType::uint8_array,
      len,
      buffer,
      byte_offset,
      &mut view,
    )
  })?;
  Ok(view)
}

thread_local! {
  // The `TypedArray.set` refs of the envs on this thread, by env pointer. Each
  // env's one cleanup hook removes its entry before the env goes away.
  static TYPED_ARRAY_SET: RefCell<HashMap<usize, sys::napi_ref>> =
    RefCell::new(HashMap::new());
}

/// Captures Uint8Array.prototype.set before page code can use the module.
/// Replacing the prototype property later cannot change the stored reference.
pub(crate) fn capture_typed_array_set(env: &Env) -> Result<()> {
  let raw_env = env.raw();
  let key = raw_env as usize;
  if TYPED_ARRAY_SET.with(|intrinsics| intrinsics.borrow().contains_key(&key)) {
    return Ok(());
  }
  let mut global = ptr::null_mut();
  check_status!(unsafe { sys::napi_get_global(raw_env, &mut global) })?;
  let mut constructor = ptr::null_mut();
  check_status!(unsafe {
    sys::napi_get_named_property(raw_env, global, c"Uint8Array".as_ptr(), &mut constructor)
  })?;
  let mut prototype = ptr::null_mut();
  check_status!(unsafe {
    sys::napi_get_named_property(raw_env, constructor, c"prototype".as_ptr(), &mut prototype)
  })?;
  let mut function = ptr::null_mut();
  check_status!(unsafe {
    sys::napi_get_named_property(raw_env, prototype, c"set".as_ptr(), &mut function)
  })?;
  let mut value_type = 0;
  check_status!(unsafe { sys::napi_typeof(raw_env, function, &mut value_type) })?;
  if value_type != sys::ValueType::napi_function {
    return Err(Error::new(
      Status::GenericFailure,
      "Uint8Array.prototype.set is not a function",
    ));
  }
  let mut reference = ptr::null_mut();
  check_status!(unsafe { sys::napi_create_reference(raw_env, function, 1, &mut reference) })?;
  unsafe extern "C" fn forget(env: *mut std::ffi::c_void) {
    let _ = TYPED_ARRAY_SET.try_with(|intrinsics| {
      intrinsics.borrow_mut().remove(&(env as usize));
    });
  }
  if let Err(error) =
    check_status!(unsafe { sys::napi_add_env_cleanup_hook(raw_env, Some(forget), raw_env.cast()) })
  {
    if !reference.is_null() {
      unsafe { sys::napi_delete_reference(raw_env, reference) };
    }
    return Err(error);
  }
  TYPED_ARRAY_SET.with(|intrinsics| intrinsics.borrow_mut().insert(key, reference));
  Ok(())
}

fn typed_array_set(
  env: sys::napi_env,
  target: sys::napi_value,
  source: sys::napi_value,
) -> Result<()> {
  let function = TYPED_ARRAY_SET
    .with(|intrinsics| intrinsics.borrow().get(&(env as usize)).copied())
    .ok_or_else(|| {
      Error::new(
        Status::GenericFailure,
        "TypedArray.set intrinsic is unavailable",
      )
    })?;
  let mut function_value = ptr::null_mut();
  check_status!(unsafe { sys::napi_get_reference_value(env, function, &mut function_value) })?;
  let mut result = ptr::null_mut();
  // Both arguments are fresh Uint8Arrays. The intrinsic's typed-array source
  // path uses internal slots and does not read user properties or run user code.
  check_status!(unsafe {
    sys::napi_call_function(env, target, function_value, 1, &source, &mut result)
  })?;
  Ok(())
}

/// Bytes per element for a `napi_typedarray_type` (values fixed by the Node-API ABI).
///
/// `None` for a type this table does not know, including
/// [`TYPED_ARRAY_TYPE_NOT_REPORTED`].
fn typed_array_element_size(kind: sys::napi_typedarray_type) -> Option<usize> {
  Some(match kind {
    // Int8Array, Uint8Array, Uint8ClampedArray
    0..=2 => 1,
    // Int16Array, Uint16Array
    3 | 4 => 2,
    // Int32Array, Uint32Array, Float32Array
    5..=7 => 4,
    // Float64Array, BigInt64Array, BigUint64Array
    8..=10 => 8,
    // Float16Array, on engines that report it
    11 => 2,
    _ => return None,
  })
}

/// Starting value for the type out-parameter of `napi_get_typedarray_info`.
///
/// Runtimes older than the Node-API type of a TypedArray leave the
/// out-parameter untouched: Node.js before 24.13 for a `Float16Array` (and
/// Node.js 22 with `--js-float16array`). A starting value that is a real type
/// would then give the wrong element size, and so the wrong byte length.
const TYPED_ARRAY_TYPE_NOT_REPORTED: sys::napi_typedarray_type = -1;

const UNSUPPORTED_TYPED_ARRAY: &str =
  "Unsupported TypedArray type on this runtime; pass a Uint8Array or DataView over the same bytes";

/// Locates the bytes of `value` with engine calls.
///
/// Returns `Ok(None)` when `value` is not an ArrayBuffer, SharedArrayBuffer,
/// TypedArray or DataView. Never runs JS.
///
/// Throws a TypeError for a TypedArray whose type the runtime does not report
/// (see [`TYPED_ARRAY_TYPE_NOT_REPORTED`]): its byte length is unknown.
///
/// # Safety
///
/// `env` and `value` must be valid handles for the current native call.
pub(crate) unsafe fn inspect(
  env: sys::napi_env,
  value: sys::napi_value,
) -> Result<Option<RawBufferSource>> {
  let mut is = false;
  check_status!(unsafe { sys::napi_is_typedarray(env, value, &mut is) })?;
  if is {
    let mut kind = TYPED_ARRAY_TYPE_NOT_REPORTED;
    let mut length = 0usize;
    let mut data = ptr::null_mut();
    let mut buffer = ptr::null_mut();
    let mut byte_offset = 0usize;
    // `data` already points at the first element (it includes byteOffset).
    check_status!(unsafe {
      sys::napi_get_typedarray_info(
        env,
        value,
        &mut kind,
        &mut length,
        &mut data,
        &mut buffer,
        &mut byte_offset,
      )
    })?;
    let Some(element_size) = typed_array_element_size(kind) else {
      return Err(throw_type_error(env, UNSUPPORTED_TYPED_ARRAY));
    };
    let len = length
      .checked_mul(element_size)
      .ok_or_else(|| Error::new(Status::InvalidArg, "TypedArray is too large"))?;
    return Ok(Some(RawBufferSource {
      env,
      data: data.cast(),
      len,
      byte_offset,
      buffer,
      kind: if kind == sys::TypedarrayType::uint8_array {
        BufferSourceKind::Uint8Array
      } else {
        BufferSourceKind::OtherTypedArray
      },
      shared: is_shared_buffer(env, buffer)?,
    }));
  }

  check_status!(unsafe { sys::napi_is_dataview(env, value, &mut is) })?;
  if is {
    let mut len = 0usize;
    let mut data = ptr::null_mut();
    let mut buffer = ptr::null_mut();
    let mut byte_offset = 0usize;
    // Like the TypedArray case, `data` already includes byteOffset.
    check_status!(unsafe {
      sys::napi_get_dataview_info(
        env,
        value,
        &mut len,
        &mut data,
        &mut buffer,
        &mut byte_offset,
      )
    })?;
    return Ok(Some(RawBufferSource {
      env,
      data: data.cast(),
      len,
      byte_offset,
      buffer,
      kind: BufferSourceKind::DataView,
      shared: is_shared_buffer(env, buffer)?,
    }));
  }

  let mut value_type = 0;
  check_status!(unsafe { sys::napi_typeof(env, value, &mut value_type) })?;
  if value_type != sys::ValueType::napi_object {
    return Ok(None);
  }
  // An ArrayBuffer, or a SharedArrayBuffer on runtimes whose
  // napi_get_arraybuffer_info accepts one. Anything else fails the call
  // without running JS.
  let mut data = ptr::null_mut();
  let mut len = 0usize;
  let status = unsafe { sys::napi_get_arraybuffer_info(env, value, &mut data, &mut len) };
  if status != sys::Status::napi_ok {
    return Ok(None);
  }
  Ok(Some(RawBufferSource {
    env,
    data: data.cast(),
    len,
    byte_offset: 0,
    buffer: value,
    kind: BufferSourceKind::ArrayBuffer,
    shared: is_shared_buffer(env, value)?,
  }))
}

/// Node-API distinguishes SharedArrayBuffer from ArrayBuffer without running JS.
fn is_shared_buffer(env: sys::napi_env, buffer: sys::napi_value) -> Result<bool> {
  let mut is_array = false;
  check_status!(unsafe { sys::napi_is_arraybuffer(env, buffer, &mut is_array) })?;
  Ok(!is_array)
}

/// Whether `value` is a detached ArrayBuffer. False for anything else,
/// including a SharedArrayBuffer (which cannot be detached).
fn is_detached_array_buffer(env: sys::napi_env, value: sys::napi_value) -> Result<bool> {
  let mut detached = false;
  check_status!(unsafe { sys::napi_is_detached_arraybuffer(env, value, &mut detached) })?;
  Ok(detached)
}

/// Message for a value that is not an ArrayBuffer, SharedArrayBuffer, TypedArray or DataView.
pub(crate) const NOT_A_BUFFER_SOURCE: &str =
  "must be an ArrayBuffer, SharedArrayBuffer, TypedArray or DataView";

fn throw_type_error(env: sys::napi_env, message: &str) -> Error {
  let _ = Env::from_raw(env).throw_type_error(message, None);
  Error::new(Status::InvalidArg, message)
}

/// Reads the required `data` member of an init dictionary (one property read,
/// so a getter runs once) and copies its bytes into native memory.
///
/// Throws a TypeError when `data` is missing, `null`, or not a BufferSource.
pub(crate) fn copy_required_data(env: sys::napi_env, obj: &Object) -> Result<Vec<u8>> {
  let data: Unknown = obj.get_named_property("data")?;
  if matches!(data.get_type()?, ValueType::Undefined | ValueType::Null) {
    return Err(throw_type_error(env, "data is required"));
  }
  match unsafe { inspect(env, data.raw())? } {
    Some(raw) => raw.to_vec(),
    None => Err(throw_type_error(env, "data must be a BufferSource")),
  }
}

/// An optional BufferSource dictionary member, such as a decoder config's
/// `description`, after [`copy_member`].
pub(crate) enum BufferSourceMember {
  /// `undefined`.
  Absent,
  /// A value that is not a BufferSource. This includes `null`: the member is
  /// not nullable, so Chrome throws a TypeError for it.
  NotBufferSource,
  /// A BufferSource whose ArrayBuffer is detached (W3C: the config is not
  /// valid).
  Detached,
  /// A copy of the BufferSource's bytes.
  Copied(Vec<u8>),
}

/// Copies an optional BufferSource member, read earlier with a single
/// property read so that a getter runs once.
///
/// Call it once every other member of the dictionary was read. Chrome reads
/// all members first and copies the bytes when it uses the config, so a later
/// getter that rewrites, resizes or detaches the buffer changes what it gets.
/// Never runs user JS, so no pointer into an ordinary buffer outlives this call.
pub(crate) fn copy_member(env: sys::napi_env, value: &Unknown) -> Result<BufferSourceMember> {
  if value.get_type()? == ValueType::Undefined {
    return Ok(BufferSourceMember::Absent);
  }
  let Some(raw) = (unsafe { inspect(env, value.raw())? }) else {
    return Ok(BufferSourceMember::NotBufferSource);
  };
  if is_detached_array_buffer(env, raw.buffer)? {
    return Ok(BufferSourceMember::Detached);
  }
  Ok(BufferSourceMember::Copied(raw.to_vec()?))
}

/// Locates the destination of a `copyTo()` call with engine calls.
///
/// Throws a TypeError when it is not a BufferSource. The returned value must
/// be consumed before control returns to JS.
pub(crate) fn copy_to_destination(
  env: sys::napi_env,
  destination: &Unknown,
) -> Result<RawBufferSource> {
  match unsafe { inspect(env, destination.raw())? } {
    Some(raw) => Ok(raw),
    None => Err(throw_type_error(
      env,
      &format!("destination {}", NOT_A_BUFFER_SOURCE),
    )),
  }
}

/// Like [`inspect`], but accepts only a `Uint8Array` (this includes Node.js
/// `Buffer`) and fails with "Expected a Uint8Array" for anything else.
///
/// # Safety
///
/// `env` and `value` must be valid handles for the current native call.
pub(crate) unsafe fn inspect_uint8_array(
  env: sys::napi_env,
  value: sys::napi_value,
) -> Result<RawBufferSource> {
  match unsafe { inspect(env, value)? } {
    Some(raw) if raw.kind() == BufferSourceKind::Uint8Array => Ok(raw),
    _ => Err(Error::new(Status::InvalidArg, "Expected a Uint8Array")),
  }
}

/// A `Uint8Array` argument or field whose bytes are copied into native
/// memory while the argument is converted, before any later getter or
/// callback can run. Use it instead of `Uint8Array` for any input that native
/// code reads after JS can run again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Uint8ArrayCopy(Vec<u8>);

impl Uint8ArrayCopy {
  pub fn into_vec(self) -> Vec<u8> {
    self.0
  }
}

impl std::ops::Deref for Uint8ArrayCopy {
  type Target = [u8];

  fn deref(&self) -> &[u8] {
    &self.0
  }
}

impl TypeName for Uint8ArrayCopy {
  fn type_name() -> &'static str {
    "Uint8Array"
  }

  fn value_type() -> ValueType {
    ValueType::Object
  }
}

impl ValidateNapiValue for Uint8ArrayCopy {}

impl FromNapiValue for Uint8ArrayCopy {
  unsafe fn from_napi_value(env: sys::napi_env, value: sys::napi_value) -> Result<Self> {
    Ok(Self(unsafe { inspect_uint8_array(env, value)? }.to_vec()?))
  }
}

impl ToNapiValue for Uint8ArrayCopy {
  unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
    unsafe { Uint8Array::to_napi_value(env, Uint8Array::new(val.0)) }
  }
}

/// Like [`Uint8ArrayCopy`], but accepts any BufferSource (ArrayBuffer,
/// SharedArrayBuffer, TypedArray or DataView).
///
/// For muxer inputs that are often what a WebCodecs API returned: a decoder
/// config's `description` comes back from `isConfigSupported()` as an
/// ArrayBuffer, like in Chrome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BufferSourceCopy(Vec<u8>);

impl std::ops::Deref for BufferSourceCopy {
  type Target = [u8];

  fn deref(&self) -> &[u8] {
    &self.0
  }
}

impl TypeName for BufferSourceCopy {
  fn type_name() -> &'static str {
    "BufferSource"
  }

  fn value_type() -> ValueType {
    ValueType::Object
  }
}

impl ValidateNapiValue for BufferSourceCopy {}

impl FromNapiValue for BufferSourceCopy {
  unsafe fn from_napi_value(env: sys::napi_env, value: sys::napi_value) -> Result<Self> {
    match unsafe { inspect(env, value)? } {
      Some(raw) => Ok(Self(raw.to_vec()?)),
      None => Err(Error::new(
        Status::InvalidArg,
        "Expected an ArrayBuffer, SharedArrayBuffer, TypedArray or DataView",
      )),
    }
  }
}

impl ToNapiValue for BufferSourceCopy {
  unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> Result<sys::napi_value> {
    unsafe { Uint8Array::to_napi_value(env, Uint8Array::new(val.0)) }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn element_size_is_unknown_for_an_unreported_type() {
    // The sentinel must not be a real type, or an unreported TypedArray would
    // get that type's element size.
    assert_eq!(
      typed_array_element_size(TYPED_ARRAY_TYPE_NOT_REPORTED),
      None
    );
    assert_eq!(
      typed_array_element_size(sys::TypedarrayType::uint8_array),
      Some(1)
    );
    assert_eq!(typed_array_element_size(11), Some(2));
    assert_eq!(typed_array_element_size(12), None);
  }
}
