//! EncodedAudioChunk - WebCodecs API implementation
//!
//! Represents a chunk of encoded audio data.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/EncodedAudioChunk

use crate::codec::Packet;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, RwLock};

/// Type of encoded audio chunk
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodedAudioChunkType {
  /// Key chunk - can be decoded independently
  Key,
  /// Delta chunk - depends on previous chunks
  Delta,
}

/// Options for creating an EncodedAudioChunk
#[napi(object)]
pub struct EncodedAudioChunkInit {
  /// Chunk type (key or delta)
  #[napi(js_name = "type")]
  pub chunk_type: EncodedAudioChunkType,
  /// Timestamp in microseconds
  pub timestamp: i64,
  /// Duration in microseconds (optional)
  pub duration: Option<i64>,
  /// Encoded data
  pub data: Buffer,
}

/// Internal state for EncodedAudioChunk
struct EncodedAudioChunkInner {
  data: Vec<u8>,
  chunk_type: EncodedAudioChunkType,
  timestamp_us: i64,
  duration_us: Option<i64>,
}

/// EncodedAudioChunk - represents encoded audio data
///
/// This is a WebCodecs-compliant EncodedAudioChunk implementation.
#[napi]
pub struct EncodedAudioChunk {
  inner: Arc<RwLock<Option<EncodedAudioChunkInner>>>,
}

#[napi]
impl EncodedAudioChunk {
  /// Create a new EncodedAudioChunk
  #[napi(constructor)]
  pub fn new(init: EncodedAudioChunkInit) -> Result<Self> {
    let inner = EncodedAudioChunkInner {
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
  pub fn from_packet(packet: &Packet, duration_us: Option<i64>) -> Self {
    // Audio packets are typically all key frames (for most codecs)
    // Some codecs like AAC-LD might have dependencies, but most don't
    let chunk_type = if packet.is_key() {
      EncodedAudioChunkType::Key
    } else {
      EncodedAudioChunkType::Delta
    };

    let inner = EncodedAudioChunkInner {
      data: packet.to_vec(),
      chunk_type,
      timestamp_us: packet.pts(),
      duration_us: duration_us.or_else(|| {
        if packet.duration() > 0 {
          Some(packet.duration())
        } else {
          None
        }
      }),
    };

    Self {
      inner: Arc::new(RwLock::new(Some(inner))),
    }
  }

  /// Get the chunk type
  #[napi(getter, js_name = "type")]
  pub fn chunk_type(&self) -> Result<EncodedAudioChunkType> {
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

      // Use unsafe to get mutable access
      let dest_ptr = destination.as_ref().as_ptr() as *mut u8;
      unsafe {
        std::ptr::copy_nonoverlapping(inner.data.as_ptr(), dest_ptr, inner.data.len());
      }
      Ok(())
    })
  }

  /// Get the raw data as a Uint8Array (extension, not in spec)
  #[napi(getter)]
  pub fn get_data(&self) -> Result<Uint8Array> {
    self.with_inner(|inner| Ok(inner.data.clone().into()))
  }

  // ========================================================================
  // Internal helpers
  // ========================================================================

  /// Get data for decoder input
  pub(crate) fn get_data_vec(&self) -> Result<Vec<u8>> {
    self.with_inner(|inner| Ok(inner.data.clone()))
  }

  /// Get timestamp for decoder
  pub fn get_timestamp(&self) -> Result<i64> {
    self.with_inner(|inner| Ok(inner.timestamp_us))
  }

  /// Check if this is a key frame
  pub fn is_key(&self) -> bool {
    self
      .with_inner(|inner| Ok(inner.chunk_type == EncodedAudioChunkType::Key))
      .unwrap_or(false)
  }

  fn with_inner<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&EncodedAudioChunkInner) -> Result<R>,
  {
    let guard = self
      .inner
      .read()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => f(inner),
      None => Err(Error::new(
        Status::GenericFailure,
        "EncodedAudioChunk is closed",
      )),
    }
  }

  /// Convert to serializable output for callbacks
  pub fn to_output(&self) -> Result<EncodedAudioChunkOutput> {
    self.with_inner(|inner| {
      Ok(EncodedAudioChunkOutput {
        chunk_type: inner.chunk_type,
        timestamp: inner.timestamp_us,
        duration: inner.duration_us,
        data: Buffer::from(inner.data.clone()),
        byte_length: inner.data.len() as u32,
      })
    })
  }
}

/// Serializable output for callbacks (used with ThreadsafeFunction)
///
/// NAPI-RS class instances can't be passed through ThreadsafeFunction,
/// so we use this plain object struct for callback output.
#[napi(object)]
pub struct EncodedAudioChunkOutput {
  /// Chunk type (key or delta)
  #[napi(js_name = "type")]
  pub chunk_type: EncodedAudioChunkType,
  /// Timestamp in microseconds
  pub timestamp: i64,
  /// Duration in microseconds (optional)
  pub duration: Option<i64>,
  /// Encoded data
  pub data: Buffer,
  /// Byte length of the encoded data
  pub byte_length: u32,
}

/// Audio encoder configuration (WebCodecs spec)
#[napi(object)]
#[derive(Debug, Clone)]
pub struct AudioEncoderConfig {
  /// Codec string (e.g., "mp4a.40.2" for AAC-LC, "opus")
  pub codec: String,
  /// Sample rate in Hz
  pub sample_rate: Option<u32>,
  /// Number of channels
  pub number_of_channels: Option<u32>,
  /// Target bitrate in bits per second
  pub bitrate: Option<f64>,
  /// Opus-specific: complexity (0-10)
  pub complexity: Option<u32>,
  /// Opus-specific: application type ("voip", "audio", "lowdelay")
  pub opus_application: Option<String>,
  /// Opus-specific: signal type ("auto", "music", "voice")
  pub opus_signal: Option<String>,
  /// Opus-specific: frame duration preference in microseconds
  pub opus_frame_duration: Option<u32>,
  /// AAC-specific: format ("aac", "adts")
  pub aac_format: Option<String>,
}

/// Audio decoder configuration (WebCodecs spec)
#[napi(object)]
pub struct AudioDecoderConfig {
  /// Codec string (e.g., "mp4a.40.2" for AAC-LC, "opus")
  pub codec: String,
  /// Sample rate in Hz (optional, may be in description)
  pub sample_rate: Option<u32>,
  /// Number of channels (optional, may be in description)
  pub number_of_channels: Option<u32>,
  /// Codec-specific description data (e.g., AudioSpecificConfig for AAC)
  pub description: Option<Buffer>,
}

/// Audio encoder support information
#[napi(object)]
pub struct AudioEncoderSupport {
  /// Whether the configuration is supported
  pub supported: bool,
  /// The configuration that was tested
  pub config: AudioEncoderConfig,
}

/// Audio decoder support information
#[napi(object)]
pub struct AudioDecoderSupport {
  /// Whether the configuration is supported
  pub supported: bool,
  /// The configuration that was tested
  pub config: AudioDecoderConfig,
}

impl std::fmt::Debug for EncodedAudioChunk {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if let Ok(guard) = self.inner.read() {
      if let Some(ref inner) = *guard {
        return f
          .debug_struct("EncodedAudioChunk")
          .field("type", &inner.chunk_type)
          .field("timestamp", &inner.timestamp_us)
          .field("duration", &inner.duration_us)
          .field("byte_length", &inner.data.len())
          .finish();
      }
    }
    f.debug_struct("EncodedAudioChunk")
      .field("closed", &true)
      .finish()
  }
}
