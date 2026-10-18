//! DOMException error helper - WebCodecs spec compliant error handling
//!
//! Provides spec-compliant error handling following W3C DOMException conventions.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/DOMException
//!
//! ## Native DOMException Support
//!
//! This module provides helpers to throw actual JavaScript DOMException objects
//! that pass `instanceof DOMException` checks per W3C WebCodecs spec.
//!
//! Use the `throw_*_error()` helpers with an `Env` reference to throw native DOMException:
//! - `throw_invalid_state_error()` - for closed objects or wrong state
//! - `throw_not_supported_error()` - for unsupported codecs/configs
//! - `throw_encoding_error()` - for encoding/decoding failures
//! - `throw_data_error()` - for invalid data format
//! - `throw_abort_error()` - for aborted operations
//! - `throw_constraint_error()` - for constraint violations
//!
//! ## Native TypeError Support
//!
//! For W3C WebCodecs spec compliance, certain errors must be native JavaScript TypeErrors.
//! Use the `throw_type_error()` helper with an `Env` reference to throw actual TypeErrors,
//! or use `js_type_error()` to create a native TypeError that can be returned as `Result<T>`.

use napi::bindgen_prelude::*;

/// DOMException error names per WebCodecs spec
#[derive(Debug, Clone, Copy)]
pub enum DOMExceptionName {
  /// Encoding or decoding operation failed
  EncodingError,
  /// The input resource cannot be consumed by the encoder.
  OperationError,
  /// Unsupported codec or configuration
  NotSupportedError,
  /// Wrong state (e.g., operating on closed object)
  InvalidStateError,
  /// Invalid data format
  DataError,
  /// Operation was aborted
  AbortError,
  /// Generic type error
  TypeError,
  /// Constraint not satisfied
  ConstraintError,
}

impl DOMExceptionName {
  pub fn as_str(&self) -> &'static str {
    match self {
      DOMExceptionName::EncodingError => "EncodingError",
      DOMExceptionName::OperationError => "OperationError",
      DOMExceptionName::NotSupportedError => "NotSupportedError",
      DOMExceptionName::InvalidStateError => "InvalidStateError",
      DOMExceptionName::DataError => "DataError",
      DOMExceptionName::AbortError => "AbortError",
      DOMExceptionName::TypeError => "TypeError",
      DOMExceptionName::ConstraintError => "ConstraintError",
    }
  }

  /// The name a codec error callback reports for `message`: the
  /// `"NameError: detail"` prefix when present, else EncodingError (the same
  /// rule as `error_as_dom_exception`).
  pub fn from_error_message(message: &str) -> Self {
    let prefix = message
      .split_once(':')
      .map_or(message, |(prefix, _)| prefix);
    match prefix.trim() {
      "OperationError" => DOMExceptionName::OperationError,
      "NotSupportedError" => DOMExceptionName::NotSupportedError,
      "InvalidStateError" => DOMExceptionName::InvalidStateError,
      "DataError" => DOMExceptionName::DataError,
      "AbortError" => DOMExceptionName::AbortError,
      "ConstraintError" => DOMExceptionName::ConstraintError,
      _ => DOMExceptionName::EncodingError,
    }
  }
}

/// Create a spec-compliant error with DOMException-style naming
///
/// # Arguments
/// * `name` - DOMException name (e.g., EncodingError, NotSupportedError)
/// * `message` - Error message
///
/// # Example
/// ```ignore
/// return Err(dom_exception(DOMExceptionName::NotSupportedError, "Codec not supported"));
/// ```
pub fn dom_exception(name: DOMExceptionName, message: &str) -> Error {
  Error::new(
    Status::GenericFailure,
    format!("{}: {}", name.as_str(), message),
  )
}

/// Throw a native JavaScript DOMException
///
/// Creates and throws an actual DOMException object that passes `instanceof DOMException` checks.
/// This is the core function used by all specialized throw_*_error helpers.
///
/// # Arguments
/// * `env` - NAPI environment reference
/// * `name` - DOMException name (e.g., InvalidStateError, NotSupportedError)
/// * `message` - Error message
pub fn throw_dom_exception<T>(env: &Env, name: DOMExceptionName, message: &str) -> Result<T> {
  let global = env.get_global()?;
  let dom_exception_constructor =
    global.get_named_property_unchecked::<Function<FnArgs<(&str, &str)>>>("DOMException")?;
  let error = dom_exception_constructor.new_instance((message, name.as_str()).into())?;
  env.throw(error)?;
  Err(Error::new(
    Status::GenericFailure,
    format!("{}: {}", name.as_str(), message),
  ))
}

/// Create a native JavaScript DOMException as a napi Error without throwing it.
///
/// Async completion callbacks must return the error to the promise machinery;
/// throwing inside such a callback leaves a pending exception instead of
/// rejecting the promise on some Node.js versions.
pub fn native_dom_exception_error(
  env: &Env,
  name: DOMExceptionName,
  message: &str,
) -> Result<Error> {
  let global = env.get_global()?;
  let dom_exception_constructor =
    global.get_named_property_unchecked::<Function<FnArgs<(&str, &str)>>>("DOMException")?;
  let error = dom_exception_constructor.new_instance((message, name.as_str()).into())?;
  Ok(Error::from(error))
}

/// Create a native JavaScript RangeError as a napi Error without throwing it.
pub fn native_range_error(env: &Env, message: &str) -> Result<Error> {
  let error =
    napi::JsRangeError::from(Error::new(Status::GenericFailure, message)).into_unknown(*env);
  Ok(Error::from(error))
}

/// Take the pending JS exception, if there is one, so that a promise can reject with it.
pub fn take_pending_exception(env: &Env) -> Result<Option<Unknown<'_>>> {
  let mut pending = false;
  napi::check_status!(unsafe { napi::sys::napi_is_exception_pending(env.raw(), &mut pending) })?;
  if !pending {
    return Ok(None);
  }
  let mut exception = std::ptr::null_mut();
  napi::check_status!(unsafe {
    napi::sys::napi_get_and_clear_last_exception(env.raw(), &mut exception)
  })?;
  Ok(Some(unsafe {
    Unknown::from_raw_unchecked(env.raw(), exception)
  }))
}

/// For a synchronous method: throw the native JS error that `error`'s "Name: message" prefix
/// names (a TypeError for "TypeError", a DOMException for a DOMException name), so JS gets a real
/// `TypeError` / `DOMException` instead of a plain `Error` with the name in its message.
/// An error with a pending exception or without such a prefix is returned unchanged.
pub fn throw_as_native_error(env: &Env, error: Error) -> Error {
  let mut pending = false;
  unsafe { napi::sys::napi_is_exception_pending(env.raw(), &mut pending) };
  if pending || error.status == Status::PendingException {
    return error;
  }
  let Some((name, message)) = error.reason.split_once(':') else {
    return error;
  };
  let (name, message) = (name.trim(), message.trim_start());
  let thrown = if name == "TypeError" {
    throw_type_error_with_length(env, message)
  } else if DOM_EXCEPTION_NAMES.contains(&name) {
    env
      .get_global()
      .and_then(|global| {
        global.get_named_property_unchecked::<Function<FnArgs<(&str, &str)>>>("DOMException")
      })
      .and_then(|constructor| constructor.new_instance((message, name).into()))
      .and_then(|exception| env.throw(exception))
  } else {
    return error;
  };
  match thrown {
    Ok(()) => Error::new(Status::PendingException, error.reason),
    Err(_) => error,
  }
}

/// Throw a native JS TypeError. Unlike `Env::throw_type_error`, which passes the message as a C
/// string and fails on a NUL character, this passes it with its length, so a message that quotes
/// a user string with a NUL (an invalid enum value) still becomes a TypeError.
fn throw_type_error_with_length(env: &Env, message: &str) -> Result<()> {
  let mut js_message = std::ptr::null_mut();
  napi::check_status!(unsafe {
    napi::sys::napi_create_string_utf8(
      env.raw(),
      message.as_ptr().cast(),
      message.len() as isize,
      &mut js_message,
    )
  })?;
  let mut type_error = std::ptr::null_mut();
  napi::check_status!(unsafe {
    napi::sys::napi_create_type_error(env.raw(), std::ptr::null_mut(), js_message, &mut type_error)
  })?;
  napi::check_status!(unsafe { napi::sys::napi_throw(env.raw(), type_error) })
}

/// Standard DOMException names that may prefix codec error messages.
const DOM_EXCEPTION_NAMES: &[&str] = &[
  "AbortError",
  "ConstraintError",
  "DataCloneError",
  "DataError",
  "EncodingError",
  "HierarchyRequestError",
  "InUseAttributeError",
  "InvalidAccessError",
  "InvalidCharacterError",
  "InvalidModificationError",
  "InvalidNodeTypeError",
  "InvalidStateError",
  "NamespaceError",
  "NetworkError",
  "NoModificationAllowedError",
  "NotAllowedError",
  "NotFoundError",
  "NotReadableError",
  "NotSupportedError",
  "OperationError",
  "OptOutError",
  "QuotaExceededError",
  "ReadError",
  "SecurityError",
  "SyntaxError",
  "TimeoutError",
  "TransactionInactiveError",
  "TypeMismatchError",
  "UnknownError",
  "URLMismatchError",
  "VersionError",
  "WrongDocumentError",
];

/// Convert a plain napi Error into a native DOMException for codec error
/// callbacks (WebCodecsErrorCallback receives DOMException, not Error).
///
/// Many report_error() call sites embed the DOMException name as a
/// `"NameError: message"` prefix; recover it when present, otherwise
/// default to EncodingError (the spec's generic codec-processing error).
pub fn error_as_dom_exception(env: &Env, error: Error) -> Result<Unknown<'static>> {
  let message = error.reason;
  let (name, detail) = match message.split_once(':') {
    Some((prefix, rest)) if DOM_EXCEPTION_NAMES.contains(&prefix.trim()) => {
      (prefix.trim().to_string(), rest.trim_start().to_string())
    }
    // A bare exception name with no message
    _ if DOM_EXCEPTION_NAMES.contains(&message.trim()) => {
      (message.trim().to_string(), String::new())
    }
    _ => ("EncodingError".to_string(), message),
  };

  let global = env.get_global()?;
  let dom_exception_constructor =
    global.get_named_property_unchecked::<Function<FnArgs<(String, String)>>>("DOMException")?;
  dom_exception_constructor.new_instance((detail, name).into())
}

/// `error_as_dom_exception` for a promise rejection, where failing to build the value must not
/// turn into a synchronous throw. Page code can replace the global `DOMException` (with undefined,
/// a throwing getter, a non-constructor). Then this returns the exception left pending, or, when
/// there is none, a plain Error with the same name and message.
//
// Chrome builds the DOMException from the engine's own constructor, so it always rejects with a
// real DOMException. Doing that here means caching the constructor for every caller of
// `error_as_dom_exception`.
pub fn dom_exception_for_rejection(env: &Env, error: Error) -> Result<Unknown<'_>> {
  let reason = error.reason.clone();
  if let Ok(exception) = error_as_dom_exception(env, error) {
    return Ok(exception);
  }
  if let Some(exception) = take_pending_exception(env)? {
    return Ok(exception);
  }
  // The same name and message `error_as_dom_exception` takes from the reason.
  let (name, message) = match reason.split_once(':') {
    Some((prefix, rest)) if DOM_EXCEPTION_NAMES.contains(&prefix.trim()) => {
      (prefix.trim(), rest.trim_start())
    }
    _ if DOM_EXCEPTION_NAMES.contains(&reason.trim()) => (reason.trim(), ""),
    _ => ("EncodingError", reason.as_str()),
  };
  error_with_name(env, name, message)
}

/// `dom_exception_for_rejection` as the `Err` a promise callback returns (the codec flush
/// resolvers): the promise rejects with that value as it is.
//
// `Error::from_unknown_without_coercion` keeps the value itself for the rejection and reads its
// `message` and `cause` only as data properties, following at most eight `cause` links.
// `Error::from` calls the value's `toString` and converts its `cause` the same way with no limit,
// so a value whose `cause` is itself (thrown by a replaced global DOMException, or returned by a
// replaced constructor) overflowed the stack and aborted the process.
pub fn dom_exception_rejection_error(env: &Env, error: Error) -> Result<Error> {
  Ok(Error::from_unknown_without_coercion(
    dom_exception_for_rejection(env, error)?,
  ))
}

/// A plain JS Error with an own `name`. Runs no page code: the Error comes from the engine and
/// `name` is defined, not assigned.
fn error_with_name<'env>(env: &'env Env, name: &str, message: &str) -> Result<Unknown<'env>> {
  let string = |text: &str| -> Result<napi::sys::napi_value> {
    let mut value = std::ptr::null_mut();
    napi::check_status!(unsafe {
      napi::sys::napi_create_string_utf8(
        env.raw(),
        text.as_ptr().cast(),
        text.len() as isize,
        &mut value,
      )
    })?;
    Ok(value)
  };
  let mut error = std::ptr::null_mut();
  napi::check_status!(unsafe {
    napi::sys::napi_create_error(
      env.raw(),
      std::ptr::null_mut(),
      string(message)?,
      &mut error,
    )
  })?;
  let name_property = napi::sys::napi_property_descriptor {
    utf8name: c"name".as_ptr(),
    name: std::ptr::null_mut(),
    method: None,
    getter: None,
    setter: None,
    value: string(name)?,
    attributes: napi::sys::PropertyAttributes::writable
      | napi::sys::PropertyAttributes::configurable,
    data: std::ptr::null_mut(),
  };
  napi::check_status!(unsafe {
    napi::sys::napi_define_properties(env.raw(), error, 1, &name_property)
  })?;
  Ok(unsafe { Unknown::from_raw_unchecked(env.raw(), error) })
}

/// Create a native JavaScript Event for EventTarget dispatch (dequeue, etc.).
///
/// Listeners registered via addEventListener/ondequeue must receive an Event
/// object whose type matches the event name, per DOM/WebCodecs spec.
pub fn new_event(env: &Env, event_type: &str) -> Result<Unknown<'static>> {
  let global = env.get_global()?;
  let event_constructor =
    global.get_named_property_unchecked::<Function<FnArgs<(String,)>>>("Event")?;
  event_constructor.new_instance((event_type.to_string(),).into())
}

// ============================================================================
// Native DOMException Throwing Helpers
// ============================================================================
// These helpers throw actual native JavaScript DOMException objects
// that pass `instanceof DOMException` checks per W3C WebCodecs spec.

/// Throw a native InvalidStateError DOMException
///
/// Use when operating on a closed object or when in wrong state.
///
/// # Example
/// ```ignore
/// if inner.closed {
///     return throw_invalid_state_error(&env, "VideoFrame is closed");
/// }
/// ```
pub fn throw_invalid_state_error<T>(env: &Env, message: &str) -> Result<T> {
  throw_dom_exception(env, DOMExceptionName::InvalidStateError, message)
}

/// Throw a native NotSupportedError DOMException
///
/// Use when a codec, configuration, or feature is not supported.
pub fn throw_not_supported_error<T>(env: &Env, message: &str) -> Result<T> {
  throw_dom_exception(env, DOMExceptionName::NotSupportedError, message)
}

/// Throw a native EncodingError DOMException
///
/// Use when an encoding or decoding operation fails.
pub fn throw_encoding_error<T>(env: &Env, message: &str) -> Result<T> {
  throw_dom_exception(env, DOMExceptionName::EncodingError, message)
}

/// Throw a native DataError DOMException
///
/// Use when input data is malformed or invalid.
pub fn throw_data_error<T>(env: &Env, message: &str) -> Result<T> {
  throw_dom_exception(env, DOMExceptionName::DataError, message)
}

/// Throw a native AbortError DOMException
///
/// Use when an operation was aborted.
pub fn throw_abort_error<T>(env: &Env, message: &str) -> Result<T> {
  throw_dom_exception(env, DOMExceptionName::AbortError, message)
}

/// Throw a native ConstraintError DOMException
///
/// Use when a constraint (like buffer size) is not satisfied.
pub fn throw_constraint_error<T>(env: &Env, message: &str) -> Result<T> {
  throw_dom_exception(env, DOMExceptionName::ConstraintError, message)
}

/// Helper to create NotSupportedError for unsupported codecs/configs
///
/// Use when a codec, configuration, or feature is not supported.
pub fn not_supported_error(message: &str) -> Error {
  dom_exception(DOMExceptionName::NotSupportedError, message)
}

/// Helper to create InvalidStateError for closed objects or wrong state
///
/// Use when operating on a closed object or when in wrong state.
pub fn invalid_state_error(message: &str) -> Error {
  dom_exception(DOMExceptionName::InvalidStateError, message)
}

/// Helper to create EncodingError for encoding/decoding failures
///
/// Use when an encoding or decoding operation fails.
pub fn encoding_error(message: &str) -> Error {
  dom_exception(DOMExceptionName::EncodingError, message)
}

/// Helper to create DataError for invalid data format
///
/// Use when input data is malformed or invalid.
pub fn data_error(message: &str) -> Error {
  dom_exception(DOMExceptionName::DataError, message)
}

/// Helper to create AbortError for aborted operations
///
/// Use when an operation was aborted.
pub fn abort_error(message: &str) -> Error {
  dom_exception(DOMExceptionName::AbortError, message)
}

/// Helper to create TypeError for type-related errors
///
/// Use for invalid argument types or constraint violations.
pub fn type_error(message: &str) -> Error {
  dom_exception(DOMExceptionName::TypeError, message)
}

/// Helper to create ConstraintError for constraint violations
///
/// Use when a constraint (like buffer size) is not satisfied.
pub fn constraint_error(message: &str) -> Error {
  dom_exception(DOMExceptionName::ConstraintError, message)
}

/// Convert an Error with DOMException-style message to native DOMException and throw it
///
/// Parses error messages like "EncodingError: Decode failed" and throws the corresponding
/// native DOMException. Returns PendingException status to propagate to Promise rejection.
///
/// Use this in async completion callbacks where worker thread errors need to be
/// converted to native DOMException for Promise rejection.
pub fn throw_error_as_dom_exception<T>(env: &Env, error: &Error) -> Result<T> {
  let message = error.reason.as_str();

  // Parse the error message prefix to determine DOMException type
  if let Some(rest) = message.strip_prefix("EncodingError:") {
    return throw_encoding_error(env, rest.trim());
  }
  if let Some(rest) = message.strip_prefix("InvalidStateError:") {
    return throw_invalid_state_error(env, rest.trim());
  }
  if let Some(rest) = message.strip_prefix("NotSupportedError:") {
    return throw_not_supported_error(env, rest.trim());
  }
  if let Some(rest) = message.strip_prefix("DataError:") {
    return throw_data_error(env, rest.trim());
  }
  if let Some(rest) = message.strip_prefix("AbortError:") {
    return throw_abort_error(env, rest.trim());
  }
  if let Some(rest) = message.strip_prefix("ConstraintError:") {
    return throw_constraint_error(env, rest.trim());
  }

  // Not a DOMException-style message, propagate original error
  Err(Error::new(error.status, &error.reason))
}

// ============================================================================
// Native JavaScript Error Type Helpers
// ============================================================================
// These helpers create actual native JavaScript error types (TypeError, etc.)
// rather than generic Error objects with error names in the message.
// Required for W3C WebCodecs spec compliance where tests check instanceof.

/// Create a native JavaScript TypeError
///
/// This creates an actual TypeError instance that passes `instanceof TypeError` checks.
/// Use this for WebCodecs spec compliance where the spec requires TypeError.
///
/// # Example
/// ```ignore
/// return Err(js_type_error("codec is required"));
/// ```
pub fn js_type_error(message: &str) -> Error {
  Error::new(Status::InvalidArg, message)
}

/// Throw a native JavaScript TypeError and return Ok(())
///
/// This directly throws a TypeError on the JavaScript side.
/// Use when you need to throw and return early from a function.
///
/// # Example
/// ```ignore
/// if config.codec.is_empty() {
///     return throw_type_error(&env, "codec is required");
/// }
/// ```
pub fn throw_type_error<T>(env: &Env, message: &str) -> Result<T>
where
  T: Default,
{
  env.throw_type_error(message, None)?;
  Ok(T::default())
}

/// Throw a native JavaScript TypeError (unit return)
///
/// Convenience wrapper for functions returning `Result<()>`.
pub fn throw_type_error_unit(env: &Env, message: &str) -> Result<()> {
  env.throw_type_error(message, None)?;
  Ok(())
}

/// Throw a native JavaScript RangeError
///
/// Use for out-of-range values per WebCodecs spec.
pub fn throw_range_error_unit(env: &Env, message: &str) -> Result<()> {
  env.throw_range_error(message, None)?;
  Ok(())
}

/// Create a TypeError that will be thrown when returned as Err
///
/// Note: Due to NAPI-RS limitations, this creates an Error with InvalidArg status,
/// not a native TypeError. For perfect W3C spec compliance, use a JS wrapper.
pub fn config_type_error(message: &str) -> Error {
  // Using InvalidArg status for config validation errors
  Error::new(Status::InvalidArg, message)
}

// ============================================================================
// WebIDL Type Conversion Helpers
// ============================================================================
// These helpers implement WebIDL type conversion algorithms for W3C spec compliance.

/// Convert f64 to i64 per WebIDL `[EnforceRange] long long` algorithm
///
/// Per WebIDL spec:
/// 1. If value is NaN, throw TypeError
/// 2. If value is +∞ or -∞, throw TypeError
/// 3. Truncate to integer (like Math.trunc())
/// 4. If outside i64 range, throw TypeError
/// 5. Return the value
///
/// # Arguments
/// * `env` - NAPI environment for throwing errors
/// * `value` - The f64 value to convert
/// * `field_name` - Name of the field for error messages
///
/// # Returns
/// * `Ok(i64)` - The converted value
/// * `Err` - TypeError if conversion fails
pub fn enforce_range_long_long(env: &Env, value: f64, field_name: &str) -> Result<i64> {
  // WebIDL step 1-2: Check for NaN and infinity
  if value.is_nan() {
    env.throw_type_error(&format!("{} cannot be NaN", field_name), None)?;
    return Err(Error::new(
      Status::InvalidArg,
      format!("{} cannot be NaN", field_name),
    ));
  }
  if value.is_infinite() {
    env.throw_type_error(&format!("{} cannot be Infinity", field_name), None)?;
    return Err(Error::new(
      Status::InvalidArg,
      format!("{} cannot be Infinity", field_name),
    ));
  }

  // WebIDL step 3: Truncate to integer
  let truncated = value.trunc();

  // WebIDL step 4: Check if in range of long long (i64)
  // i64::MIN = -9223372036854775808 is exactly representable in f64, but
  // i64::MAX = 9223372036854775807 is not — it rounds up to 2^63, so the
  // upper bound must be exclusive to reject 2^63 itself.
  const I64_MIN_F64: f64 = i64::MIN as f64;
  const I64_MAX_F64: f64 = i64::MAX as f64; // evaluates to 2^63

  if truncated < I64_MIN_F64 || truncated >= I64_MAX_F64 {
    env.throw_type_error(
      &format!("{} is out of range for long long", field_name),
      None,
    )?;
    return Err(Error::new(
      Status::InvalidArg,
      format!("{} is out of range for long long", field_name),
    ));
  }

  // Safe to convert - value is within i64 range
  Ok(truncated as i64)
}

/// Convert optional f64 to i64 per WebIDL `[EnforceRange] long long` algorithm
///
/// Same as `enforce_range_long_long` but for optional values.
pub fn enforce_range_long_long_optional(
  env: &Env,
  value: Option<f64>,
  field_name: &str,
) -> Result<Option<i64>> {
  match value {
    Some(v) => Ok(Some(enforce_range_long_long(env, v, field_name)?)),
    None => Ok(None),
  }
}

/// Convert f64 to u64 per WebIDL `[EnforceRange] unsigned long long` algorithm
///
/// Per WebIDL spec:
/// 1. If value is NaN, throw TypeError
/// 2. If value is +∞ or -∞, throw TypeError
/// 3. Truncate to integer (like Math.trunc())
/// 4. If negative or outside u64 range, throw TypeError
/// 5. Return the value
pub fn enforce_range_unsigned_long_long(env: &Env, value: f64, field_name: &str) -> Result<u64> {
  // WebIDL step 1-2: Check for NaN and infinity
  if value.is_nan() {
    env.throw_type_error(&format!("{} cannot be NaN", field_name), None)?;
    return Err(Error::new(
      Status::InvalidArg,
      format!("{} cannot be NaN", field_name),
    ));
  }
  if value.is_infinite() {
    env.throw_type_error(&format!("{} cannot be Infinity", field_name), None)?;
    return Err(Error::new(
      Status::InvalidArg,
      format!("{} cannot be Infinity", field_name),
    ));
  }

  // WebIDL step 3: Truncate to integer
  let truncated = value.trunc();

  // WebIDL step 4: Check if in range of unsigned long long (u64)
  // u64::MAX is not representable in f64 — `u64::MAX as f64` rounds up to
  // 2^64, so the upper bound must be exclusive to reject 2^64 itself.
  // The largest representable f64 below it (2^64 - 2048) is still in range.
  if truncated < 0.0 || truncated >= u64::MAX as f64 {
    env.throw_type_error(
      &format!("{} is out of range for unsigned long long", field_name),
      None,
    )?;
    return Err(Error::new(
      Status::InvalidArg,
      format!("{} is out of range for unsigned long long", field_name),
    ));
  }

  Ok(truncated as u64)
}

/// Convert optional f64 to u32 per WebIDL `[EnforceRange] unsigned long` algorithm
///
/// NaN, ±Infinity, and values outside [0, 2^32 - 1] after truncation throw TypeError.
/// A plain `u32` napi conversion would apply ToUint32 instead and wrap them.
pub fn enforce_range_unsigned_long_optional(
  env: &Env,
  value: Option<f64>,
  field_name: &str,
) -> Result<Option<u32>> {
  let Some(value) = value else {
    return Ok(None);
  };
  match enforce_range_unsigned_long_value(value, field_name) {
    Ok(value) => Ok(Some(value)),
    Err(message) => {
      env.throw_type_error(&message, None)?;
      Err(Error::new(Status::InvalidArg, message))
    }
  }
}

/// Convert f64 to u32 per WebIDL `[EnforceRange] unsigned long`, without throwing
///
/// Returns the TypeError message for NaN, ±Infinity, and values outside [0, 2^32 - 1] after
/// truncation, so a caller that must reject a promise instead can keep it.
pub fn enforce_range_unsigned_long_value(
  value: f64,
  field_name: &str,
) -> std::result::Result<u32, String> {
  if value.is_nan() {
    return Err(format!("{} cannot be NaN", field_name));
  }
  if value.is_infinite() {
    return Err(format!("{} cannot be Infinity", field_name));
  }
  let truncated = value.trunc();
  if (0.0..=f64::from(u32::MAX)).contains(&truncated) {
    // -0.5 truncates to -0, which is 0
    return Ok(truncated as u32);
  }
  Err(format!("{} is out of range for unsigned long", field_name))
}

/// Read an optional number member of a WebIDL dictionary
///
/// A missing or undefined member is None. null is 0, like WebIDL (ToNumber(null) is +0); a
/// plain `obj.get::<f64>` would fail on null. Other values convert as `T` does.
pub fn get_number_member<T: FromNapiValue + Default>(
  obj: &Object,
  name: &str,
) -> Result<Option<T>> {
  let Some(value) = obj.get::<Unknown>(name)? else {
    return Ok(None);
  };
  if value.get_type()? == ValueType::Null {
    return Ok(Some(T::default()));
  }
  // SAFETY: `T::from_napi_value` checks the value type itself.
  unsafe { value.cast::<T>() }.map(Some)
}

/// Convert f64 to u32 per WebIDL `unsigned long` (ECMAScript ToUint32)
///
/// NaN and ±Infinity give 0. Other values truncate and wrap modulo 2^32, so -90 gives
/// 4294967206 and 2^32 + 90 gives 90.
pub fn to_unsigned_long(value: f64) -> u32 {
  if !value.is_finite() {
    return 0;
  }
  // Exact: `%` on f64 is exact, and the result is an integer in [0, 2^32).
  value.trunc().rem_euclid(4_294_967_296.0) as u32
}

/// Convert optional f64 to u64 per WebIDL `[EnforceRange] unsigned long long` algorithm
pub fn enforce_range_unsigned_long_long_optional(
  env: &Env,
  value: Option<f64>,
  field_name: &str,
) -> Result<Option<u64>> {
  match value {
    Some(v) => Ok(Some(enforce_range_unsigned_long_long(env, v, field_name)?)),
    None => Ok(None),
  }
}

#[cfg(test)]
mod tests {
  use super::{enforce_range_unsigned_long_value, to_unsigned_long};

  #[test]
  fn enforce_range_unsigned_long_value_truncates_or_rejects() {
    // [input, Some(value) or None for a TypeError]
    let cases: [(f64, Option<u32>); 12] = [
      (16.0, Some(16)),
      (16.7, Some(16)),
      (-0.0, Some(0)),
      (-0.5, Some(0)),
      (4_294_967_295.0, Some(4_294_967_295)),
      (4_294_967_295.5, Some(4_294_967_295)),
      (-1.0, None),
      (-1.5, None),
      (4_294_967_296.0, None),
      (f64::NAN, None),
      (f64::INFINITY, None),
      (f64::NEG_INFINITY, None),
    ];
    for (input, expected) in cases {
      assert_eq!(
        enforce_range_unsigned_long_value(input, "x").ok(),
        expected,
        "[EnforceRange] unsigned long ({input})"
      );
    }
  }

  #[test]
  fn to_unsigned_long_matches_to_uint32() {
    // [input, ToUint32(input)]; the large values are what Chrome 155 echoes for
    // VideoDecoderConfig.rotation.
    let cases: [(f64, u32); 15] = [
      (0.0, 0),
      (-0.0, 0),
      (90.0, 90),
      (44.9, 44),
      (-0.5, 0),
      (-90.0, 4_294_967_206),
      (-315.0, 4_294_966_981),
      (4_294_967_295.0, 4_294_967_295),
      (4_294_967_296.0 + 90.0, 90),
      (-4_294_967_296.0, 0),
      (1e20, 1_661_992_960),
      (-1e20, 2_632_974_336),
      (f64::NAN, 0),
      (f64::INFINITY, 0),
      (f64::NEG_INFINITY, 0),
    ];
    for (input, expected) in cases {
      assert_eq!(to_unsigned_long(input), expected, "ToUint32({input})");
    }
  }
}
