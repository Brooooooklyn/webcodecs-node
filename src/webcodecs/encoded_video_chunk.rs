//! EncodedVideoChunk - WebCodecs API implementation
//!
//! Represents a chunk of encoded video data.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/EncodedVideoChunk

use crate::codec::Packet;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, RwLock};

/// Type of encoded video chunk
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedVideoChunkType {
  /// Keyframe - can be decoded independently
  Key,
  /// Delta frame - depends on previous frames
  Delta,
}

/// Options for creating an EncodedVideoChunk
#[napi(object)]
pub struct EncodedVideoChunkInit {
  /// Chunk type (key or delta)
  #[napi(js_name = "type")]
  pub chunk_type: EncodedVideoChunkType,
  /// Timestamp in microseconds
  pub timestamp: i64,
  /// Duration in microseconds (optional)
  pub duration: Option<i64>,
  /// Encoded data
  pub data: Buffer,
}

/// Internal state for EncodedVideoChunk
struct EncodedVideoChunkInner {
  data: Vec<u8>,
  chunk_type: EncodedVideoChunkType,
  timestamp_us: i64,
  duration_us: Option<i64>,
}

/// EncodedVideoChunk - represents encoded video data
///
/// This is a WebCodecs-compliant EncodedVideoChunk implementation.
#[napi]
pub struct EncodedVideoChunk {
  inner: Arc<RwLock<Option<EncodedVideoChunkInner>>>,
}

#[napi]
impl EncodedVideoChunk {
  /// Create a new EncodedVideoChunk
  #[napi(constructor)]
  pub fn new(init: EncodedVideoChunkInit) -> Result<Self> {
    let inner = EncodedVideoChunkInner {
      data: init.data.to_vec(),
      chunk_type: init.chunk_type,
      timestamp_us: init.timestamp,
      duration_us: init.duration,
    };

    Ok(Self {
      inner: Arc::new(RwLock::new(Some(inner))),
    })
  }

  /// Create from internal Packet (for encoder output)
  pub fn from_packet(packet: &Packet) -> Self {
    let chunk_type = if packet.is_key() {
      EncodedVideoChunkType::Key
    } else {
      EncodedVideoChunkType::Delta
    };

    let inner = EncodedVideoChunkInner {
      data: packet.to_vec(),
      chunk_type,
      timestamp_us: packet.pts(),
      duration_us: if packet.duration() > 0 {
        Some(packet.duration())
      } else {
        None
      },
    };

    Self {
      inner: Arc::new(RwLock::new(Some(inner))),
    }
  }

  /// Get the chunk type
  #[napi(getter, js_name = "type")]
  pub fn chunk_type(&self) -> Result<EncodedVideoChunkType> {
    self.with_inner(|inner| Ok(inner.chunk_type))
  }

  /// Get the timestamp in microseconds
  #[napi(getter)]
  pub fn timestamp(&self) -> Result<i64> {
    self.with_inner(|inner| Ok(inner.timestamp_us))
  }

  /// Get the duration in microseconds
  #[napi(getter)]
  pub fn duration(&self) -> Result<Option<i64>> {
    self.with_inner(|inner| Ok(inner.duration_us))
  }

  /// Get the byte length of the encoded data
  #[napi(getter)]
  pub fn byte_length(&self) -> Result<u32> {
    self.with_inner(|inner| Ok(inner.data.len() as u32))
  }

  /// Copy the encoded data to a Uint8Array
  #[napi]
  pub fn copy_to(&self, destination: Uint8Array) -> Result<()> {
    self.with_inner(|inner| {
      if destination.len() < inner.data.len() {
        return Err(Error::new(
          Status::GenericFailure,
          format!(
            "Buffer too small: need {} bytes, got {}",
            inner.data.len(),
            destination.len()
          ),
        ));
      }

      // Uint8Array is already mutable
      let dest_slice = destination.as_ref();
      // We need to use unsafe here to get mutable access
      let dest_ptr = dest_slice.as_ptr() as *mut u8;
      unsafe {
        std::ptr::copy_nonoverlapping(inner.data.as_ptr(), dest_ptr, inner.data.len());
      }
      Ok(())
    })
  }

  /// Get the raw data as a Buffer (extension, not in spec)
  #[napi]
  pub fn get_data(&self) -> Result<Buffer> {
    self.with_inner(|inner| Ok(Buffer::from(inner.data.clone())))
  }

  // ========================================================================
  // Internal helpers
  // ========================================================================

  /// Get data for decoder input
  pub fn get_data_vec(&self) -> Result<Vec<u8>> {
    self.with_inner(|inner| Ok(inner.data.clone()))
  }

  /// Check if this is a key frame
  pub fn is_key(&self) -> bool {
    self
      .with_inner(|inner| Ok(inner.chunk_type == EncodedVideoChunkType::Key))
      .unwrap_or(false)
  }

  fn with_inner<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&EncodedVideoChunkInner) -> Result<R>,
  {
    let guard = self
      .inner
      .read()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => f(inner),
      None => Err(Error::new(
        Status::GenericFailure,
        "EncodedVideoChunk is closed",
      )),
    }
  }

  /// Convert to serializable output for callbacks
  pub fn to_output(&self) -> Result<EncodedVideoChunkOutput> {
    self.with_inner(|inner| {
      let data = inner.data.clone();
      let byte_length = data.len() as u32;
      Ok(EncodedVideoChunkOutput {
        chunk_type: inner.chunk_type,
        timestamp: inner.timestamp_us,
        duration: inner.duration_us,
        data: Buffer::from(data),
        byte_length,
      })
    })
  }
}

/// Serializable output for callbacks (used with ThreadsafeFunction)
///
/// NAPI-RS class instances can't be passed through ThreadsafeFunction,
/// so we use this plain object struct for callback output.
#[napi(object)]
pub struct EncodedVideoChunkOutput {
  /// Chunk type (key or delta)
  #[napi(js_name = "type")]
  pub chunk_type: EncodedVideoChunkType,
  /// Timestamp in microseconds
  pub timestamp: i64,
  /// Duration in microseconds (optional)
  pub duration: Option<i64>,
  /// Encoded data
  pub data: Buffer,
  /// Byte length of the encoded data
  pub byte_length: u32,
}

/// Decode configuration for AVC (H.264)
#[allow(dead_code)]
#[napi(object)]
pub struct AvcDecoderConfig {
  /// AVC configuration record (avcC box content)
  pub avc_c: Option<Buffer>,
}

/// Encode configuration for AVC (H.264)
#[allow(dead_code)]
#[napi(object)]
#[derive(Debug, Clone)]
pub struct AvcEncoderConfig {
  /// AVC profile (e.g., "baseline", "main", "high")
  pub profile: Option<String>,
  /// AVC level (e.g., "3.0", "4.0", "5.1")
  pub level: Option<String>,
  /// Output format: "annexb" or "avc"
  pub format: Option<String>,
}

/// Encode configuration for VP9
#[napi(object)]
#[derive(Debug, Clone)]
pub struct Vp9EncoderConfig {
  /// VP9 profile: 0 (8-bit 4:2:0), 1 (8-bit 4:2:2/4:4:4), 2 (10/12-bit 4:2:0), 3 (10/12-bit 4:2:2/4:4:4)
  pub profile: Option<u8>,
  /// Encoding speed preset (0 = best quality/slowest, 8 = fastest)
  pub speed: Option<u32>,
  /// Tile columns (log2 value: 0-6)
  pub tile_columns: Option<u32>,
  /// Tile rows (log2 value: 0-2)
  pub tile_rows: Option<u32>,
  /// Enable row-based multithreading
  pub row_mt: Option<bool>,
  /// Keyframe placement mode: "auto", "disabled"
  pub keyframe_mode: Option<String>,
}

/// Encode configuration for AV1
#[napi(object)]
#[derive(Debug, Clone)]
pub struct Av1EncoderConfig {
  /// AV1 profile: 0 (Main), 1 (High), 2 (Professional)
  pub profile: Option<u8>,
  /// CPU usage preset (0 = slowest/best quality, 8 = fastest)
  pub cpu_used: Option<u32>,
  /// Tile columns (1, 2, 4, 8, 16, 32, 64)
  pub tile_columns: Option<u32>,
  /// Tile rows
  pub tile_rows: Option<u32>,
  /// CQ level for quantizer mode (0-63, lower = better quality)
  pub cq_level: Option<u32>,
  /// Enable screen content coding tools
  pub screen_content: Option<bool>,
}

/// Encode configuration for HEVC (H.265)
#[napi(object)]
#[derive(Debug, Clone)]
pub struct HevcEncoderConfig {
  /// HEVC profile: "main", "main10", "main-still-picture", "rext"
  pub profile: Option<String>,
  /// Tier: "main" or "high"
  pub tier: Option<String>,
  /// Level (e.g., "4.0", "5.1", "6.2")
  pub level: Option<String>,
  /// Encoding preset: "ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow", "placebo"
  pub preset: Option<String>,
  /// Tuning: "psnr", "ssim", "grain", "zerolatency", "fastdecode"
  pub tune: Option<String>,
}

/// Video encoder configuration (WebCodecs spec)
#[allow(dead_code)]
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
  /// Codec string (e.g., "avc1.42001E", "vp8", "vp09.00.10.08")
  pub codec: String,
  /// Coded width in pixels
  pub width: u32,
  /// Coded height in pixels
  pub height: u32,
  /// Display width (optional, defaults to width)
  pub display_width: Option<u32>,
  /// Display height (optional, defaults to height)
  pub display_height: Option<u32>,
  /// Target bitrate in bits per second
  pub bitrate: Option<f64>,
  /// Framerate
  pub framerate: Option<f64>,
  /// Hardware acceleration preference
  pub hardware_acceleration: Option<String>,
  /// Latency mode: "quality" or "realtime"
  pub latency_mode: Option<String>,
  /// Bitrate mode: "constant", "variable", "quantizer"
  pub bitrate_mode: Option<String>,
  /// AVC-specific configuration (H.264)
  pub avc: Option<AvcEncoderConfig>,
  /// VP9-specific configuration
  pub vp9: Option<Vp9EncoderConfig>,
  /// AV1-specific configuration
  pub av1: Option<Av1EncoderConfig>,
  /// HEVC-specific configuration (H.265)
  pub hevc: Option<HevcEncoderConfig>,
  /// Alpha handling: "discard" or "keep"
  pub alpha: Option<String>,
  /// Scalability mode (SVC) - e.g., "L1T1", "L1T2", "L1T3"
  pub scalability_mode: Option<String>,
}

/// Video decoder configuration (WebCodecs spec)
#[allow(dead_code)]
#[napi(object)]
pub struct VideoDecoderConfig {
  /// Codec string (e.g., "avc1.42001E", "vp8", "vp09.00.10.08")
  pub codec: String,
  /// Coded width in pixels (optional for some codecs)
  pub coded_width: Option<u32>,
  /// Coded height in pixels (optional for some codecs)
  pub coded_height: Option<u32>,
  /// Display aspect width
  pub display_aspect_width: Option<u32>,
  /// Display aspect height
  pub display_aspect_height: Option<u32>,
  /// Color space parameters
  pub color_space: Option<crate::webcodecs::video_frame::VideoColorSpace>,
  /// Hardware acceleration preference
  pub hardware_acceleration: Option<String>,
  /// Optimization preference: "prefer-accuracy" or "prefer-speed"
  pub optimize_for_latency: Option<bool>,
  /// Codec-specific description data (e.g., avcC for H.264)
  pub description: Option<Buffer>,
}
