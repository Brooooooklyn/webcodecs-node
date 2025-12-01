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

// Re-export WebCodecs types at crate root
pub use webcodecs::{
    // Video types
    CodecState, EncodedVideoChunk, EncodedVideoChunkInit, EncodedVideoChunkMetadata,
    EncodedVideoChunkType, HardwareAccelerator, VideoColorSpace, VideoDecoder, VideoDecoderConfig,
    VideoDecoderConfigOutput, VideoDecoderSupport, VideoEncoder, VideoEncoderConfig,
    VideoEncoderEncodeOptions, VideoEncoderSupport, VideoFrame, VideoFrameCopyToOptions,
    VideoFrameInit, VideoFrameRect, VideoPixelFormat,
    // Audio types
    AudioData, AudioDataCopyToOptions, AudioDataInit, AudioDecoder, AudioDecoderConfig,
    AudioDecoderConfigOutput, AudioDecoderSupport, AudioEncoder, AudioEncoderConfig,
    AudioEncoderEncodeOptions, AudioEncoderSupport, AudioSampleFormat, EncodedAudioChunk,
    EncodedAudioChunkInit, EncodedAudioChunkMetadata, EncodedAudioChunkType,
    // Hardware acceleration utilities
    get_available_hardware_accelerators, get_hardware_accelerators,
    get_preferred_hardware_accelerator, is_hardware_accelerator_available,
};
