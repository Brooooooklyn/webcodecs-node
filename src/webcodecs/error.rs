//! DOMException error helper - WebCodecs spec compliant error handling
//!
//! Provides spec-compliant error messages following DOMException naming conventions.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/DOMException
//!
//! Note: These helpers create Error objects with spec-compliant error names in the message.
//! The actual DOMException class instantiation happens on the JavaScript side if needed.

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
