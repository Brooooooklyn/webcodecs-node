//! Reading JavaScript `BufferSource` values (ArrayBuffer, SharedArrayBuffer,
//! TypedArray, DataView) without trusting them or keeping them.
//!
//! Two rules keep native code memory safe here:
//!
//! - A view's bytes are located with engine calls (`napi_get_typedarray_info`,
//!   `napi_get_dataview_info`, `napi_get_arraybuffer_info`), never with the
//!   `buffer` / `byteOffset` / `byteLength` properties: JS can override those
//!   (own properties, getters, proxies), and reading them runs JS.
//! - A pointer into a JS buffer is only used until JS runs again. A later
//!   getter, a callback, or returning to the event loop can detach, shrink or
//!   rewrite the buffer, so inputs are copied into native memory right away
//!   and outputs are written before control goes back to JS.

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
/// The pointer is only valid until JS runs again; copy or write through it
/// before calling anything that can run JS.
pub(crate) struct RawBufferSource {
  data: *mut u8,
  len: usize,
  /// The underlying ArrayBuffer (or SharedArrayBuffer).
  buffer: sys::napi_value,
  kind: BufferSourceKind,
}

impl RawBufferSource {
  pub(crate) fn kind(&self) -> BufferSourceKind {
    self.kind
  }

  pub(crate) fn len(&self) -> usize {
    self.len
  }

  /// Copies the bytes into native memory.
  pub(crate) fn to_vec(&self) -> Vec<u8> {
    if self.len == 0 || self.data.is_null() {
      return Vec::new();
    }
    let mut out = Vec::with_capacity(self.len);
    // SAFETY: the engine reported `len` readable bytes at `data`, and no JS has
    // run since. A raw copy (rather than a `&[u8]`) keeps this sound even when
    // another thread writes a SharedArrayBuffer concurrently.
    unsafe {
      ptr::copy_nonoverlapping(self.data, out.as_mut_ptr(), self.len);
      out.set_len(self.len);
    }
    out
  }

  /// The bytes as a writable slice.
  ///
  /// # Safety
  ///
  /// The slice must not be used after JS runs again (no JS calls, callbacks,
  /// getters, or returning to JS while it is alive).
  pub(crate) unsafe fn as_mut_slice<'a>(&self) -> &'a mut [u8] {
    if self.len == 0 || self.data.is_null() {
      return &mut [];
    }
    unsafe { std::slice::from_raw_parts_mut(self.data, self.len) }
  }
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
      data: data.cast(),
      len,
      buffer,
      kind: if kind == sys::TypedarrayType::uint8_array {
        BufferSourceKind::Uint8Array
      } else {
        BufferSourceKind::OtherTypedArray
      },
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
      data: data.cast(),
      len,
      buffer,
      kind: BufferSourceKind::DataView,
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
    data: data.cast(),
    len,
    buffer: value,
    kind: BufferSourceKind::ArrayBuffer,
  }))
}

/// Whether `value` is a detached ArrayBuffer. False for anything else,
/// including a SharedArrayBuffer (which cannot be detached).
pub(crate) fn is_detached_array_buffer(env: sys::napi_env, value: sys::napi_value) -> Result<bool> {
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
    Some(raw) => Ok(raw.to_vec()),
    None => Err(throw_type_error(env, "data must be a BufferSource")),
  }
}

/// An optional BufferSource dictionary member, such as a decoder config's
/// `description`.
pub(crate) enum BufferSourceMember {
  /// `undefined` or `null`.
  Absent,
  /// A value that is not a BufferSource.
  NotBufferSource,
  /// A BufferSource. Its bytes were copied when the member was read.
  Copied {
    bytes: Vec<u8>,
    /// The underlying ArrayBuffer, to check later whether a getter that ran
    /// after this member was read detached it. Valid for the current native
    /// call only.
    buffer: sys::napi_value,
  },
}

/// Reads an optional BufferSource member (one property read, so a getter runs
/// once) and copies its bytes into native memory right away.
pub(crate) fn read_buffer_source_member(
  env: sys::napi_env,
  obj: &Object,
  name: &str,
) -> Result<BufferSourceMember> {
  let value: Unknown = obj.get_named_property(name)?;
  if matches!(value.get_type()?, ValueType::Undefined | ValueType::Null) {
    return Ok(BufferSourceMember::Absent);
  }
  Ok(match unsafe { inspect(env, value.raw())? } {
    Some(raw) => BufferSourceMember::Copied {
      bytes: raw.to_vec(),
      buffer: raw.buffer,
    },
    None => BufferSourceMember::NotBufferSource,
  })
}

/// Locates the destination of a `copyTo()` call with engine calls.
///
/// Throws a TypeError when it is not a BufferSource. The returned pointer is
/// only valid until JS runs again, so write the bytes before returning.
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
    match unsafe { inspect(env, value)? } {
      Some(raw) if raw.kind() == BufferSourceKind::Uint8Array => Ok(Self(raw.to_vec())),
      _ => Err(Error::new(Status::InvalidArg, "Expected a Uint8Array")),
    }
  }
}

impl ToNapiValue for Uint8ArrayCopy {
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
