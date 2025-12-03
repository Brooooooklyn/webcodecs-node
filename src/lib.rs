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

/// Module initialization - called when the native module is loaded
#[module_init]
fn init() {
  // Suppress verbose FFmpeg logging (only show errors)
  unsafe {
    ffi::avutil::av_log_set_level(ffi::avutil::log_level::ERROR);
  }
}

// Re-export WebCodecs types at crate root
pub use webcodecs::{
  // Hardware acceleration utilities
  get_available_hardware_accelerators,
  get_hardware_accelerators,
  get_preferred_hardware_accelerator,
  is_hardware_accelerator_available,
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
};
