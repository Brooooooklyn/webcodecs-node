//! Hand-written FFmpeg C bindings (no bindgen)
//!
//! This module provides minimal, safe FFmpeg bindings for video encoding/decoding.
//! All FFmpeg structs are opaque - we access fields via the thin C accessor library.

pub mod accessors;
pub mod avcodec;
pub mod avformat;
pub mod avutil;
pub mod error;
pub mod hwaccel;
pub mod swresample;
pub mod swscale;
pub mod types;

pub use error::{FFmpegError, FFmpegResult, check_error};
pub use types::*;
