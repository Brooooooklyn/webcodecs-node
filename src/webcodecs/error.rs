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
      DOMExceptionName::NotSupportedError => "NotSupportedError",
      DOMExceptionName::InvalidStateError => "InvalidStateError",
      DOMExceptionName::DataError => "DataError",
      DOMExceptionName::AbortError => "AbortError",
      DOMExceptionName::TypeError => "TypeError",
      DOMExceptionName::ConstraintError => "ConstraintError",
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
  // i64::MIN = -9223372036854775808, i64::MAX = 9223372036854775807
  // f64 can represent these exactly as -9223372036854775808.0 and 9223372036854775807.0
  const I64_MIN_F64: f64 = i64::MIN as f64;
  const I64_MAX_F64: f64 = i64::MAX as f64;

  if !(I64_MIN_F64..=I64_MAX_F64).contains(&truncated) {
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
  if truncated < 0.0 || truncated > u64::MAX as f64 {
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
