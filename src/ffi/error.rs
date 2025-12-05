//! FFmpeg error handling
//!
//! Provides error codes, error conversion, and result types.

use std::ffi::CStr;
use std::fmt;
use std::os::raw::c_int;

// ============================================================================
// FFmpeg Error Codes
// ============================================================================

/// End of file / stream reached
pub const AVERROR_EOF: c_int = fferrtag(b'E', b'O', b'F', b' ');

/// Bitstream filter not found
pub const AVERROR_BSF_NOT_FOUND: c_int = fferrtag(0xF8, b'B', b'S', b'F');

/// Internal bug (should never happen)
pub const AVERROR_BUG: c_int = fferrtag(b'B', b'U', b'G', b'!');

/// Buffer too small
pub const AVERROR_BUFFER_TOO_SMALL: c_int = fferrtag(b'B', b'U', b'F', b'S');

/// Decoder not found
pub const AVERROR_DECODER_NOT_FOUND: c_int = fferrtag(0xF8, b'D', b'E', b'C');

/// Demuxer not found
pub const AVERROR_DEMUXER_NOT_FOUND: c_int = fferrtag(0xF8, b'D', b'E', b'M');

/// Encoder not found
pub const AVERROR_ENCODER_NOT_FOUND: c_int = fferrtag(0xF8, b'E', b'N', b'C');

/// Exit requested
pub const AVERROR_EXIT: c_int = fferrtag(b'E', b'X', b'I', b'T');

/// External error
pub const AVERROR_EXTERNAL: c_int = fferrtag(b'E', b'X', b'T', b' ');

/// Filter not found
pub const AVERROR_FILTER_NOT_FOUND: c_int = fferrtag(0xF8, b'F', b'I', b'L');

/// Invalid data found
pub const AVERROR_INVALIDDATA: c_int = fferrtag(b'I', b'N', b'D', b'A');

/// Muxer not found
pub const AVERROR_MUXER_NOT_FOUND: c_int = fferrtag(0xF8, b'M', b'U', b'X');

/// Option not found
pub const AVERROR_OPTION_NOT_FOUND: c_int = fferrtag(0xF8, b'O', b'P', b'T');

/// Not yet implemented
pub const AVERROR_PATCHWELCOME: c_int = fferrtag(b'P', b'A', b'W', b'E');

/// Protocol not found
pub const AVERROR_PROTOCOL_NOT_FOUND: c_int = fferrtag(0xF8, b'P', b'R', b'O');

/// Stream not found
pub const AVERROR_STREAM_NOT_FOUND: c_int = fferrtag(0xF8, b'S', b'T', b'R');

/// Unknown error
pub const AVERROR_UNKNOWN: c_int = fferrtag(b'U', b'N', b'K', b'N');

/// Experimental feature
pub const AVERROR_EXPERIMENTAL: c_int = -0x2bb2afa8;

/// Input changed between calls
pub const AVERROR_INPUT_CHANGED: c_int = -0x636e6701;

/// Output changed between calls
pub const AVERROR_OUTPUT_CHANGED: c_int = -0x636e6702;

// POSIX error codes (negated) - platform specific
// Note: FFmpeg negates errno values, so we need platform-specific values

/// Resource temporarily unavailable (try again)
/// Linux: EAGAIN = 11, macOS: EAGAIN = 35
#[cfg(target_os = "macos")]
pub const AVERROR_EAGAIN: c_int = -35;

#[cfg(target_os = "linux")]
pub const AVERROR_EAGAIN: c_int = -11;

#[cfg(target_os = "windows")]
pub const AVERROR_EAGAIN: c_int = -11; // WSAEWOULDBLOCK maps to 11

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub const AVERROR_EAGAIN: c_int = -11;

/// Out of memory
/// Same across platforms (12)
pub const AVERROR_ENOMEM: c_int = -12;

/// Invalid argument
/// Same across platforms (22)
pub const AVERROR_EINVAL: c_int = -22;

// ============================================================================
// Error Tag Helper
// ============================================================================

/// Create FFmpeg error tag from 4 bytes
const fn fferrtag(a: u8, b: u8, c: u8, d: u8) -> c_int {
  -((a as c_int) | ((b as c_int) << 8) | ((c as c_int) << 16) | ((d as c_int) << 24))
}

// ============================================================================
// FFmpeg Error Type
// ============================================================================

/// FFmpeg error with code and message
#[derive(Clone)]
pub struct FFmpegError {
  /// Error code (negative)
  pub code: c_int,
  /// Human-readable message
  pub message: String,
}

impl FFmpegError {
  /// Create error from FFmpeg error code
  pub fn from_code(code: c_int) -> Self {
    let mut buf = [0 as std::os::raw::c_char; 256];
    unsafe {
      super::avutil::av_strerror(code, buf.as_mut_ptr(), buf.len());
      let message = CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
      Self { code, message }
    }
  }

  /// Create error with custom message
  pub fn new(code: c_int, message: impl Into<String>) -> Self {
    Self {
      code,
      message: message.into(),
    }
  }

  /// Check if this is EAGAIN (resource temporarily unavailable)
  #[inline]
  pub fn is_eagain(&self) -> bool {
    self.code == AVERROR_EAGAIN
  }

  /// Check if this is EOF
  #[inline]
  pub fn is_eof(&self) -> bool {
    self.code == AVERROR_EOF
  }

  /// Check if this error indicates "would block" (EAGAIN or EOF)
  #[inline]
  pub fn would_block(&self) -> bool {
    self.is_eagain() || self.is_eof()
  }

  /// Check if this is an invalid argument error
  #[inline]
  pub fn is_invalid(&self) -> bool {
    self.code == AVERROR_EINVAL
  }

  /// Check if this is an out of memory error
  #[inline]
  pub fn is_oom(&self) -> bool {
    self.code == AVERROR_ENOMEM
  }
}

impl fmt::Debug for FFmpegError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("FFmpegError")
      .field("code", &self.code)
      .field("message", &self.message)
      .finish()
  }
}

impl fmt::Display for FFmpegError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "FFmpeg error {}: {}", self.code, self.message)
  }
}

impl std::error::Error for FFmpegError {}

// ============================================================================
// Result Type
// ============================================================================

/// Result type for FFmpeg operations
pub type FFmpegResult<T> = Result<T, FFmpegError>;

// ============================================================================
// Error Checking
// ============================================================================

/// Check FFmpeg return code and convert to Result
///
/// Returns Ok with the value if >= 0, Err with FFmpegError if < 0
#[inline]
pub fn check_error(ret: c_int) -> FFmpegResult<c_int> {
  if ret < 0 {
    Err(FFmpegError::from_code(ret))
  } else {
    Ok(ret)
  }
}

/// Check FFmpeg return code, ignoring EAGAIN
///
/// Returns Ok(Some(value)) if >= 0, Ok(None) if EAGAIN, Err otherwise
#[inline]
pub fn check_error_except_eagain(ret: c_int) -> FFmpegResult<Option<c_int>> {
  if ret >= 0 {
    Ok(Some(ret))
  } else if ret == AVERROR_EAGAIN {
    Ok(None)
  } else {
    Err(FFmpegError::from_code(ret))
  }
}

/// Check FFmpeg return code, ignoring EAGAIN and EOF
///
/// Returns Ok(Some(value)) if >= 0, Ok(None) if EAGAIN/EOF, Err otherwise
#[inline]
pub fn check_error_except_eagain_eof(ret: c_int) -> FFmpegResult<Option<c_int>> {
  if ret >= 0 {
    Ok(Some(ret))
  } else if ret == AVERROR_EAGAIN || ret == AVERROR_EOF {
    Ok(None)
  } else {
    Err(FFmpegError::from_code(ret))
  }
}

// ============================================================================
// Error Message Helper
// ============================================================================

/// Get error message for an FFmpeg error code
pub fn get_error_message(code: c_int) -> String {
  let mut buf = [0 as std::os::raw::c_char; 256];
  unsafe {
    super::avutil::av_strerror(code, buf.as_mut_ptr(), buf.len());
    CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_error_codes() {
    assert!(AVERROR_EOF < 0);
    assert!(AVERROR_EAGAIN < 0);
    assert!(AVERROR_EINVAL < 0);
  }

  #[test]
  fn test_check_error() {
    assert!(check_error(0).is_ok());
    assert!(check_error(100).is_ok());
    assert!(check_error(-1).is_err());
    assert!(check_error(AVERROR_EAGAIN).is_err());
  }

  #[test]
  fn test_check_error_except_eagain() {
    assert_eq!(check_error_except_eagain(0).unwrap(), Some(0));
    assert_eq!(check_error_except_eagain(AVERROR_EAGAIN).unwrap(), None);
    assert!(check_error_except_eagain(AVERROR_EINVAL).is_err());
  }
}
