//! Hand-written FFmpeg C bindings (no bindgen)
//!
//! This module provides minimal, safe FFmpeg bindings for video encoding/decoding.
//! All FFmpeg structs are opaque - we access fields via the thin C accessor library.

pub mod accessors;
pub mod avcodec;
pub mod avutil;
pub mod error;
pub mod hwaccel;
pub mod swresample;
pub mod swscale;
pub mod types;

pub use error::{check_error, FFmpegError, FFmpegResult};
pub use types::*;
