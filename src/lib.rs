#![deny(clippy::all)]

//! WebCodecs API implementation for Node.js
//!
//! This crate provides a spec-compliant implementation of the WebCodecs API
//! using FFmpeg for video encoding/decoding.

// FFmpeg C bindings (hand-written, no bindgen)
pub mod ffi;

// Safe codec wrappers (RAII)
pub mod codec;

// WebCodecs API surface (NAPI classes)
pub mod webcodecs;

use napi_derive::module_init;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

/// FFmpeg log callback that redirects to tracing
///
/// This callback is called by FFmpeg for all log messages. It formats the message
/// using FFmpeg's internal formatter and then dispatches to the appropriate
/// tracing macro based on the log level.
///
/// Log messages are filtered by the `ffmpeg` target, so users can control
/// FFmpeg logging independently via tracing subscriber configuration.
unsafe extern "C" fn ffmpeg_log_callback(
  ptr: *mut c_void,
  level: c_int,
  fmt: *const c_char,
  vl: *mut c_void,
) {
  // Skip if level is below our threshold (higher number = more verbose)
  let current_level = unsafe { ffi::avutil::av_log_get_level() };
  if level > current_level {
    return;
  }

  // Format the message using FFmpeg's internal formatter
  // Buffer size of 1024 is standard for FFmpeg log lines
  let mut line_buf = [0u8; 1024];
  let mut print_prefix: c_int = 1;

  let len = unsafe {
    ffi::avutil::av_log_format_line2(
      ptr,
      level,
      fmt,
      vl,
      line_buf.as_mut_ptr() as *mut c_char,
      line_buf.len() as c_int,
      &mut print_prefix,
    )
  };

  if len <= 0 {
    return;
  }

  // Convert to string, trimming trailing newline
  let msg = match CStr::from_bytes_until_nul(&line_buf) {
    Ok(cstr) => cstr.to_string_lossy(),
    Err(_) => return,
  };
  let msg = msg.trim_end();

  if msg.is_empty() {
    return;
  }

  // Dispatch to appropriate tracing level
  // FFmpeg log levels: QUIET=-8, PANIC=0, FATAL=8, ERROR=16, WARNING=24, INFO=32, VERBOSE=40, DEBUG=48, TRACE=56
  match level {
    l if l <= ffi::avutil::log_level::FATAL => {
      tracing::error!(target: "ffmpeg", "{}", msg);
    }
    l if l <= ffi::avutil::log_level::ERROR => {
      tracing::error!(target: "ffmpeg", "{}", msg);
    }
    l if l <= ffi::avutil::log_level::WARNING => {
      tracing::warn!(target: "ffmpeg", "{}", msg);
    }
    l if l <= ffi::avutil::log_level::INFO => {
      tracing::info!(target: "ffmpeg", "{}", msg);
    }
    l if l <= ffi::avutil::log_level::VERBOSE => {
      tracing::debug!(target: "ffmpeg", "{}", msg);
    }
    _ => {
      tracing::trace!(target: "ffmpeg", "{}", msg);
    }
  }
}

/// Module initialization - called when the native module is loaded
#[module_init]
fn init() {
  use tracing_subscriber::filter::Targets;
  use tracing_subscriber::prelude::*;
  use tracing_subscriber::util::SubscriberInitExt;

  // Set FFmpeg log level to allow all messages through to our callback
  // The tracing subscriber will handle actual filtering
  unsafe {
    ffi::avutil::av_log_set_level(ffi::avutil::log_level::INFO);
    ffi::avutil::av_log_set_callback(Some(ffmpeg_log_callback));
  }

  // Usage without the `regex` feature.
  // <https://github.com/tokio-rs/tracing/issues/1436#issuecomment-918528013>
  tracing_subscriber::registry()
    .with(std::env::var("WEBCODECS_LOG").map_or_else(
      |_| Targets::new(),
      |env_var| {
        use std::str::FromStr;
        Targets::from_str(&env_var).unwrap()
      },
    ))
    .with(tracing_subscriber::fmt::layer())
    .init();
}

// Re-export WebCodecs types at crate root
pub use webcodecs::{
  // Audio types
  AudioData,
  AudioDataCopyToOptions,
  AudioDataInit,
  AudioDecoder,
  AudioDecoderConfig,
  AudioDecoderConfigOutput,
  AudioDecoderSupport,
  AudioEncoder,
  AudioEncoderConfig,
  AudioEncoderEncodeOptions,
  AudioEncoderSupport,
  AudioSampleFormat,
  // Video types
  CodecState,
  EncodedAudioChunk,
  EncodedAudioChunkInit,
  EncodedAudioChunkMetadata,
  EncodedAudioChunkType,
  EncodedVideoChunk,
  EncodedVideoChunkInit,
  EncodedVideoChunkMetadata,
  EncodedVideoChunkType,
  HardwareAccelerator,
  VideoColorPrimaries,
  VideoColorSpace,
  VideoColorSpaceInit,
  VideoDecoder,
  VideoDecoderConfig,
  VideoDecoderConfigOutput,
  VideoDecoderSupport,
  VideoEncoder,
  VideoEncoderConfig,
  VideoEncoderEncodeOptions,
  VideoEncoderSupport,
  VideoFrame,
  VideoFrameCopyToOptions,
  VideoFrameInit,
  VideoFrameRect,
  VideoMatrixCoefficients,
  VideoPixelFormat,
  VideoTransferCharacteristics,
  // Hardware acceleration utilities
  get_available_hardware_accelerators,
  get_hardware_accelerators,
  get_preferred_hardware_accelerator,
  is_hardware_accelerator_available,
  reset_hardware_fallback_state,
};
