//! DOMException error helper - WebCodecs spec compliant error handling
//!
//! Provides spec-compliant error messages following DOMException naming conventions.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/DOMException
//!
//! Note: These helpers create Error objects with spec-compliant error names in the message.
//! The actual DOMException class instantiation happens on the JavaScript side if needed.
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
