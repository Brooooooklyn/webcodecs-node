//! EncodedVideoChunk - WebCodecs API implementation
//!
//! Represents a chunk of encoded video data.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/EncodedVideoChunk

use crate::codec::Packet;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::io::Write;
use std::sync::{Arc, RwLock};

/// Type of encoded video chunk
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedVideoChunkType {
  /// Keyframe - can be decoded independently
  #[napi(value = "key")]
  Key,
  /// Delta frame - depends on previous frames
  #[napi(value = "delta")]
  Delta,
}

/// Hardware acceleration preference (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HardwareAcceleration {
  /// No preference - may use hardware or software
  #[default]
  #[napi(value = "no-preference")]
  NoPreference,
  /// Prefer hardware acceleration
  #[napi(value = "prefer-hardware")]
  PreferHardware,
  /// Prefer software implementation
  #[napi(value = "prefer-software")]
  PreferSoftware,
}

/// Latency mode for video encoding (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LatencyMode {
  /// Optimize for quality (default)
  #[default]
  #[napi(value = "quality")]
  Quality,
  /// Optimize for low latency
  #[napi(value = "realtime")]
  Realtime,
}

/// Bitrate mode for video encoding (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoEncoderBitrateMode {
  /// Variable bitrate (default)
  #[default]
  #[napi(value = "variable")]
  Variable,
  /// Constant bitrate
  #[napi(value = "constant")]
  Constant,
  /// Use quantizer parameter from codec-specific options
  #[napi(value = "quantizer")]
  Quantizer,
}

/// Alpha channel handling option (W3C WebCodecs spec)
/// Default is "discard" per spec
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlphaOption {
  /// Keep alpha channel if present
  #[napi(value = "keep")]
  Keep,
  /// Discard alpha channel (default per W3C spec)
  #[default]
  #[napi(value = "discard")]
  Discard,
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
  /// Note: W3C spec uses unsigned long long, but JS number can represent up to 2^53 safely
  pub duration: Option<i64>,
  /// Encoded data (BufferSource per spec)
  pub data: Uint8Array,
  /// ArrayBuffers to transfer (W3C spec - ignored in Node.js, we always copy)
  #[napi(ts_type = "ArrayBuffer[]")]
  pub transfer: Option<Vec<Uint8Array>>,
}

/// Internal state for EncodedVideoChunk
pub(crate) struct EncodedVideoChunkInner {
  pub(crate) data: Vec<u8>,
  pub(crate) chunk_type: EncodedVideoChunkType,
  pub(crate) timestamp_us: i64,
  pub(crate) duration_us: Option<i64>,
}

/// EncodedVideoChunk - represents encoded video data
///
/// This is a WebCodecs-compliant EncodedVideoChunk implementation.
#[napi]
pub struct EncodedVideoChunk {
  pub(crate) inner: Arc<RwLock<Option<EncodedVideoChunkInner>>>,
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
  pub fn copy_to(&self, mut destination: Uint8Array) -> Result<()> {
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

      unsafe { destination.as_mut() }.write_all(&inner.data)?;
      Ok(())
    })
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
}

// ============================================================================
// Codec-Specific Encoder Configurations (W3C WebCodecs Codec Registry)
// ============================================================================

/// AVC (H.264) bitstream format (W3C WebCodecs AVC Registration)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AvcBitstreamFormat {
  /// AVC format with parameter sets in description (ISO 14496-15)
  #[default]
  #[napi(value = "avc")]
  Avc,
  /// Annex B format with parameter sets in bitstream
  #[napi(value = "annexb")]
  Annexb,
}

/// AVC (H.264) encoder configuration (W3C WebCodecs AVC Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AvcEncoderConfig {
  /// Bitstream format (default: "avc")
  pub format: Option<AvcBitstreamFormat>,
}

/// HEVC (H.265) bitstream format (W3C WebCodecs HEVC Registration)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HevcBitstreamFormat {
  /// HEVC format with parameter sets in description (ISO 14496-15)
  #[default]
  #[napi(value = "hevc")]
  Hevc,
  /// Annex B format with parameter sets in bitstream
  #[napi(value = "annexb")]
  Annexb,
}

/// HEVC (H.265) encoder configuration (W3C WebCodecs HEVC Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct HevcEncoderConfig {
  /// Bitstream format (default: "hevc")
  pub format: Option<HevcBitstreamFormat>,
}

/// Video encoder configuration (WebCodecs spec)
/// Note: Codec-specific options are encoded in the codec string per W3C spec
/// e.g., "avc1.42001E" encodes profile/level, "vp09.00.10.08" encodes profile/level/depth
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoEncoderConfig {
  /// Codec string (e.g., "avc1.42001E", "vp8", "vp09.00.10.08", "av01.0.04M.08")
  pub codec: String,
  /// Coded width in pixels (required)
  pub width: u32,
  /// Coded height in pixels (required)
  pub height: u32,
  /// Display width (optional, defaults to width)
  pub display_width: Option<u32>,
  /// Display height (optional, defaults to height)
  pub display_height: Option<u32>,
  /// Target bitrate in bits per second
  pub bitrate: Option<f64>,
  /// Framerate (frames per second)
  pub framerate: Option<f64>,
  /// Hardware acceleration preference (W3C spec enum)
  pub hardware_acceleration: Option<HardwareAcceleration>,
  /// Latency mode (W3C spec enum)
  pub latency_mode: Option<LatencyMode>,
  /// Bitrate mode (W3C spec enum)
  pub bitrate_mode: Option<VideoEncoderBitrateMode>,
  /// Alpha handling (W3C spec enum)
  pub alpha: Option<AlphaOption>,
  /// Scalability mode (SVC) - e.g., "L1T1", "L1T2", "L1T3"
  pub scalability_mode: Option<String>,
  /// Content hint for encoder optimization
  pub content_hint: Option<String>,
  /// AVC (H.264) codec-specific configuration
  pub avc: Option<AvcEncoderConfig>,
  /// HEVC (H.265) codec-specific configuration
  pub hevc: Option<HevcEncoderConfig>,
}

/// Video decoder configuration (WebCodecs spec)
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
  /// Color space parameters (uses init object for compatibility)
  pub color_space: Option<crate::webcodecs::video_frame::VideoColorSpaceInit>,
  /// Hardware acceleration preference (W3C spec enum)
  pub hardware_acceleration: Option<HardwareAcceleration>,
  /// Optimize for latency (W3C spec)
  pub optimize_for_latency: Option<bool>,
  /// Codec-specific description data (e.g., avcC for H.264) - BufferSource per spec
  pub description: Option<Uint8Array>,
}
