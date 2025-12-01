//! Safe Rust wrappers for FFmpeg codec operations
//!
//! This module provides RAII wrappers around FFmpeg's C structures,
//! ensuring proper resource cleanup and memory safety.

pub mod context;
pub mod frame;
pub mod hwdevice;
pub mod packet;
pub mod scaler;

pub use context::{CodecContext, CodecType};
pub use frame::Frame;
pub use hwdevice::HwDeviceContext;
pub use packet::Packet;
pub use scaler::Scaler;

use crate::ffi::{AVCodecID, AVPixelFormat};

/// Encoder configuration
#[derive(Debug, Clone)]
pub struct EncoderConfig {
    /// Video width in pixels
    pub width: u32,
    /// Video height in pixels
    pub height: u32,
    /// Pixel format
    pub pixel_format: AVPixelFormat,
    /// Target bitrate in bits per second (0 for CRF mode)
    pub bitrate: u64,
    /// Frames per second (numerator)
    pub framerate_num: u32,
    /// Frames per second (denominator)
    pub framerate_den: u32,
    /// Group of pictures size (keyframe interval)
    pub gop_size: u32,
    /// Maximum B-frames between non-B frames
    pub max_b_frames: u32,
    /// Number of threads (0 for auto)
    pub thread_count: u32,
    /// Codec profile (codec-specific)
    pub profile: Option<i32>,
    /// Codec level (codec-specific)
    pub level: Option<i32>,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            pixel_format: AVPixelFormat::Yuv420p,
            bitrate: 5_000_000, // 5 Mbps
            framerate_num: 30,
            framerate_den: 1,
            gop_size: 60, // 2 seconds at 30fps
            max_b_frames: 2,
            thread_count: 0, // Auto
            profile: None,
            level: None,
        }
    }
}

/// Decoder configuration
#[derive(Debug, Clone)]
pub struct DecoderConfig {
    /// Codec ID
    pub codec_id: AVCodecID,
    /// Number of threads (0 for auto)
    pub thread_count: u32,
    /// Extra data (codec-specific, e.g., SPS/PPS for H.264)
    pub extradata: Option<Vec<u8>>,
}

impl Default for DecoderConfig {
    fn default() -> Self {
        Self {
            codec_id: AVCodecID::H264,
            thread_count: 0,
            extradata: None,
        }
    }
}

/// Codec error type
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("FFmpeg error: {0}")]
    Ffmpeg(#[from] crate::ffi::FFmpegError),

    #[error("Codec not found: {0}")]
    CodecNotFound(String),

    #[error("Encoder not found for codec: {0:?}")]
    EncoderNotFound(AVCodecID),

    #[error("Decoder not found for codec: {0:?}")]
    DecoderNotFound(AVCodecID),

    #[error("Failed to allocate {0}")]
    AllocationFailed(&'static str),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Codec not configured")]
    NotConfigured,

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Unsupported pixel format: {0:?}")]
    UnsupportedPixelFormat(AVPixelFormat),

    #[error("Hardware acceleration error: {0}")]
    HardwareError(String),
}

pub type CodecResult<T> = Result<T, CodecError>;
