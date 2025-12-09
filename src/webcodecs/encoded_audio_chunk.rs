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
  #[napi(value = "key")]
  Key,
  /// Delta chunk - depends on previous chunks
  #[napi(value = "delta")]
  Delta,
}

/// Bitrate mode for audio encoding (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BitrateMode {
  /// Variable bitrate (default)
  #[default]
  #[napi(value = "variable")]
  Variable,
  /// Constant bitrate
  #[napi(value = "constant")]
  Constant,
}

/// Options for creating an EncodedAudioChunk
/// W3C spec: https://w3c.github.io/webcodecs/#dictdef-encodedaudiochunkinit
pub struct EncodedAudioChunkInit {
  /// Chunk type (key or delta)
  pub chunk_type: EncodedAudioChunkType,
  /// Timestamp in microseconds
  pub timestamp: i64,
  /// Duration in microseconds (optional)
  pub duration: Option<i64>,
  /// Encoded data (BufferSource per spec)
  pub data: Vec<u8>,
}

impl FromNapiValue for EncodedAudioChunkInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = Object::from_napi_value(env, value)?;

    // Validate type - required field, must throw TypeError if missing
    let chunk_type_value: Option<String> = obj.get("type")?;
    let chunk_type = match chunk_type_value {
      Some(ref s) if s == "key" => EncodedAudioChunkType::Key,
      Some(ref s) if s == "delta" => EncodedAudioChunkType::Delta,
      Some(s) => {
        env_wrapper.throw_type_error(&format!("Invalid chunk type: {}", s), None)?;
        return Err(Error::new(Status::InvalidArg, "Invalid chunk type"));
      }
      None => {
        env_wrapper.throw_type_error("type is required", None)?;
        return Err(Error::new(Status::InvalidArg, "type is required"));
      }
    };

    // Validate timestamp - required field
    let timestamp: Option<i64> = obj.get("timestamp")?;
    let timestamp = match timestamp {
      Some(ts) => ts,
      None => {
        env_wrapper.throw_type_error("timestamp is required", None)?;
        return Err(Error::new(Status::InvalidArg, "timestamp is required"));
      }
    };

    // Duration is optional
    let duration: Option<i64> = obj.get("duration")?;

    // Validate data - required field, accept BufferSource (ArrayBuffer, TypedArray, DataView)
    let data: Vec<u8> = if let Ok(Some(buffer)) = obj.get::<Buffer>("data") {
      buffer.to_vec()
    } else if let Ok(Some(array)) = obj.get::<Uint8Array>("data") {
      array.to_vec()
    } else if let Ok(Some(array_buffer)) = obj.get::<ArrayBuffer>("data") {
      array_buffer.to_vec()
    } else {
      // Check if data property exists but is undefined/null
      let has_data = obj.has_named_property("data")?;
      if !has_data {
        env_wrapper.throw_type_error("data is required", None)?;
        return Err(Error::new(Status::InvalidArg, "data is required"));
      }

      // Try getting as object and check for buffer/byteLength properties (DataView, other TypedArrays)
      if let Ok(Some(data_obj)) = obj.get::<Object>("data") {
        let byte_length: Option<u32> = data_obj.get("byteLength").ok().flatten();
        let byte_offset: u32 = data_obj.get("byteOffset").ok().flatten().unwrap_or(0);

        if let (Some(len), Ok(Some(buffer))) = (byte_length, data_obj.get::<ArrayBuffer>("buffer"))
        {
          let full_data = buffer.to_vec();
          let offset = byte_offset as usize;
          let length = len as usize;
          if offset + length <= full_data.len() {
            full_data[offset..offset + length].to_vec()
          } else {
            env_wrapper.throw_type_error("data must be a valid BufferSource", None)?;
            return Err(Error::new(
              Status::InvalidArg,
              "data must be a valid BufferSource",
            ));
          }
        } else {
          env_wrapper.throw_type_error("data must be a BufferSource", None)?;
          return Err(Error::new(
            Status::InvalidArg,
            "data must be a BufferSource",
          ));
        }
      } else {
        env_wrapper.throw_type_error("data is required", None)?;
        return Err(Error::new(Status::InvalidArg, "data is required"));
      }
    };

    Ok(EncodedAudioChunkInit {
      chunk_type,
      timestamp,
      duration,
      data,
    })
  }
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
  pub fn new(
    #[napi(ts_arg_type = "import('./standard').EncodedAudioChunkInit")] init: EncodedAudioChunkInit,
  ) -> Result<Self> {
    let inner = EncodedAudioChunkInner {
      data: init.data,
      chunk_type: init.chunk_type,
      timestamp_us: init.timestamp,
      duration_us: init.duration,
    };

    Ok(Self {
      inner: Arc::new(RwLock::new(Some(inner))),
    })
  }

  /// Create from internal Packet (for encoder output)
  /// If explicit_timestamp is provided, it overrides the packet's PTS (for timestamp preservation)
  pub fn from_packet(
    packet: &Packet,
    duration_us: Option<i64>,
    explicit_timestamp: Option<i64>,
  ) -> Self {
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
      // Use explicit timestamp if provided, otherwise fall back to packet PTS
      timestamp_us: explicit_timestamp.unwrap_or_else(|| packet.pts()),
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

  /// Copy the encoded data to a BufferSource
  /// W3C spec: throws TypeError if destination is too small
  #[napi(ts_args_type = "destination: import('./standard').BufferSource")]
  pub fn copy_to(&self, env: Env, destination: Unknown) -> Result<()> {
    self.with_inner(|inner| {
      // Try to get it as a TypedArray first (most common case)
      if let Ok(typed_array) = destination.coerce_to_object() {
        // Check if it has a buffer property (TypedArray/DataView)
        if let Ok(true) = typed_array.has_named_property("buffer") {
          // It's a TypedArray or DataView - get its underlying buffer info
          let byte_length: u32 = typed_array.get("byteLength").ok().flatten().unwrap_or(0);
          let byte_offset: u32 = typed_array.get("byteOffset").ok().flatten().unwrap_or(0);
          let buffer: ArrayBuffer = typed_array
            .get("buffer")?
            .ok_or_else(|| Error::new(Status::InvalidArg, "Invalid BufferSource"))?;
          if (byte_length as usize) < inner.data.len() {
            env.throw_type_error(
              &format!(
                "destination is too small: need {} bytes, got {}",
                inner.data.len(),
                byte_length
              ),
              None,
            )?;
            return Ok(());
          }

          // Copy data to the view's portion of the buffer
          let dest_ptr = unsafe { buffer.as_ptr().add(byte_offset as usize) as *mut u8 };
          unsafe {
            std::ptr::copy_nonoverlapping(inner.data.as_ptr(), dest_ptr, inner.data.len());
          }
        } else {
          // It's likely an ArrayBuffer directly
          let byte_length: Option<u32> = typed_array.get("byteLength").ok().flatten();
          if let Some(len) = byte_length {
            if (len as usize) < inner.data.len() {
              env.throw_type_error(
                &format!(
                  "destination is too small: need {} bytes, got {}",
                  inner.data.len(),
                  len
                ),
                None,
              )?;
              return Ok(());
            }

            // Get the ArrayBuffer data pointer
            let array_buffer = ArrayBuffer::from_unknown(destination)?;
            let dest_ptr = array_buffer.as_ptr() as *mut u8;
            unsafe {
              std::ptr::copy_nonoverlapping(inner.data.as_ptr(), dest_ptr, inner.data.len());
            }
          } else {
            return Err(Error::new(Status::InvalidArg, "Invalid BufferSource"));
          }
        }
      } else {
        return Err(Error::new(Status::InvalidArg, "Invalid BufferSource"));
      }
      Ok(())
    })
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
}

// ============================================================================
// Codec-Specific Audio Encoder Configurations (W3C WebCodecs Codec Registry)
// ============================================================================

/// Opus bitstream format (W3C WebCodecs Opus Registration)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpusBitstreamFormat {
  /// Opus packets (RFC 6716) - no metadata needed for decoding
  #[default]
  #[napi(value = "opus")]
  Opus,
  /// Ogg encapsulation (RFC 7845) - metadata in description
  #[napi(value = "ogg")]
  Ogg,
}

/// Opus signal type hint (W3C WebCodecs Opus Registration)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpusSignal {
  /// Auto-detect signal type
  #[default]
  #[napi(value = "auto")]
  Auto,
  /// Music signal
  #[napi(value = "music")]
  Music,
  /// Voice/speech signal
  #[napi(value = "voice")]
  Voice,
}

/// Opus application mode (W3C WebCodecs Opus Registration)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpusApplication {
  /// Optimize for VoIP (speech intelligibility)
  #[napi(value = "voip")]
  Voip,
  /// Optimize for audio fidelity (default)
  #[default]
  #[napi(value = "audio")]
  Audio,
  /// Minimize coding delay
  #[napi(value = "lowdelay")]
  Lowdelay,
}

/// Opus encoder configuration (W3C WebCodecs Opus Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct OpusEncoderConfig {
  /// Bitstream format (default: "opus")
  pub format: Option<OpusBitstreamFormat>,
  /// Signal type hint (default: "auto")
  pub signal: Option<OpusSignal>,
  /// Application mode (default: "audio")
  pub application: Option<OpusApplication>,
  /// Frame duration in microseconds (default: 20000)
  /// Note: W3C spec uses unsigned long long, but NAPI-RS uses f64 for JS compatibility
  pub frame_duration: Option<f64>,
  /// Encoder complexity 0-10 (default: 5 mobile, 9 desktop)
  pub complexity: Option<u32>,
  /// Expected packet loss percentage 0-100 (default: 0)
  pub packetlossperc: Option<u32>,
  /// Enable in-band FEC (default: false)
  pub useinbandfec: Option<bool>,
  /// Enable DTX (default: false)
  pub usedtx: Option<bool>,
}

/// AAC bitstream format (W3C WebCodecs AAC Registration)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AacBitstreamFormat {
  /// Raw AAC frames - metadata in description
  #[default]
  #[napi(value = "aac")]
  Aac,
  /// ADTS frames - metadata in each frame
  #[napi(value = "adts")]
  Adts,
}

/// AAC encoder configuration (W3C WebCodecs AAC Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AacEncoderConfig {
  /// Bitstream format (default: "aac")
  pub format: Option<AacBitstreamFormat>,
}

/// FLAC encoder configuration (W3C WebCodecs FLAC Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct FlacEncoderConfig {
  /// Block size (0 = auto, default: 0)
  pub block_size: Option<u32>,
  /// Compression level 0-8 (default: 5)
  pub compress_level: Option<u32>,
}

/// Audio encoder configuration (WebCodecs spec)
///
/// Note: codec, sample_rate, and number_of_channels are Option to support
/// the W3C spec requirement that isConfigSupported() rejects with TypeError
/// (returns rejected Promise) for missing fields, rather than throwing synchronously.
#[derive(Debug, Clone)]
pub struct AudioEncoderConfig {
  /// Codec string (e.g., "mp4a.40.2" for AAC-LC, "opus")
  /// W3C spec: required, but stored as Option for proper error handling
  pub codec: Option<String>,
  /// Sample rate in Hz - W3C spec uses float
  /// W3C spec: required, but stored as Option for proper error handling
  pub sample_rate: Option<f64>,
  /// Number of channels
  /// W3C spec: required, but stored as Option for proper error handling
  pub number_of_channels: Option<u32>,
  /// Target bitrate in bits per second
  pub bitrate: Option<f64>,
  /// Bitrate mode (W3C spec enum)
  pub bitrate_mode: Option<BitrateMode>,
  /// Opus codec-specific configuration
  pub opus: Option<OpusEncoderConfig>,
  /// AAC codec-specific configuration
  pub aac: Option<AacEncoderConfig>,
  /// FLAC codec-specific configuration
  pub flac: Option<FlacEncoderConfig>,
}

impl FromNapiValue for AudioEncoderConfig {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let obj = Object::from_napi_value(env, value)?;

    // All fields stored as Option - validation happens in configure() or isConfigSupported()
    let codec: Option<String> = obj.get("codec")?;
    let sample_rate: Option<f64> = obj.get("sampleRate")?;
    let number_of_channels: Option<u32> = obj.get("numberOfChannels")?;
    let bitrate: Option<f64> = obj.get("bitrate")?;
    let bitrate_mode: Option<BitrateMode> = obj.get("bitrateMode")?;
    let opus: Option<OpusEncoderConfig> = obj.get("opus")?;
    let aac: Option<AacEncoderConfig> = obj.get("aac")?;
    let flac: Option<FlacEncoderConfig> = obj.get("flac")?;

    Ok(AudioEncoderConfig {
      codec,
      sample_rate,
      number_of_channels,
      bitrate,
      bitrate_mode,
      opus,
      aac,
      flac,
    })
  }
}

/// Audio decoder configuration (WebCodecs spec)
///
/// Note: codec, sample_rate, and number_of_channels are Option to support
/// the W3C spec requirement that isConfigSupported() rejects with TypeError
/// (returns rejected Promise) for missing fields, rather than throwing synchronously.
pub struct AudioDecoderConfig {
  /// Codec string (e.g., "mp4a.40.2" for AAC-LC, "opus")
  /// W3C spec: required, but stored as Option for proper error handling
  pub codec: Option<String>,
  /// Sample rate in Hz - W3C spec uses float
  /// W3C spec: required, but stored as Option for proper error handling
  pub sample_rate: Option<f64>,
  /// Number of channels
  /// W3C spec: required, but stored as Option for proper error handling
  pub number_of_channels: Option<u32>,
  /// Codec-specific description data (e.g., AudioSpecificConfig for AAC) - BufferSource per spec
  pub description: Option<Uint8Array>,
}

impl FromNapiValue for AudioDecoderConfig {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let obj = Object::from_napi_value(env, value)?;

    // All fields stored as Option - validation happens in configure() or isConfigSupported()
    let codec: Option<String> = obj.get("codec")?;
    let sample_rate: Option<f64> = obj.get("sampleRate")?;
    let number_of_channels: Option<u32> = obj.get("numberOfChannels")?;
    let description: Option<Uint8Array> = obj.get("description")?;

    Ok(AudioDecoderConfig {
      codec,
      sample_rate,
      number_of_channels,
      description,
    })
  }
}

impl ToNapiValue for AudioEncoderConfig {
  unsafe fn to_napi_value(env: napi::sys::napi_env, val: Self) -> Result<napi::sys::napi_value> {
    let env_wrapper = Env::from_raw(env);
    let mut obj = Object::new(&env_wrapper)?;

    if let Some(codec) = val.codec {
      obj.set("codec", codec)?;
    }
    if let Some(sample_rate) = val.sample_rate {
      obj.set("sampleRate", sample_rate)?;
    }
    if let Some(number_of_channels) = val.number_of_channels {
      obj.set("numberOfChannels", number_of_channels)?;
    }
    if let Some(bitrate) = val.bitrate {
      obj.set("bitrate", bitrate)?;
    }
    if let Some(bitrate_mode) = val.bitrate_mode {
      obj.set("bitrateMode", bitrate_mode)?;
    }
    if let Some(opus) = val.opus {
      obj.set("opus", opus)?;
    }
    if let Some(aac) = val.aac {
      obj.set("aac", aac)?;
    }
    if let Some(flac) = val.flac {
      obj.set("flac", flac)?;
    }

    Object::to_napi_value(env, obj)
  }
}

impl ToNapiValue for AudioDecoderConfig {
  unsafe fn to_napi_value(env: napi::sys::napi_env, val: Self) -> Result<napi::sys::napi_value> {
    let env_wrapper = Env::from_raw(env);
    let mut obj = Object::new(&env_wrapper)?;

    if let Some(codec) = val.codec {
      obj.set("codec", codec)?;
    }
    if let Some(sample_rate) = val.sample_rate {
      obj.set("sampleRate", sample_rate)?;
    }
    if let Some(number_of_channels) = val.number_of_channels {
      obj.set("numberOfChannels", number_of_channels)?;
    }
    if let Some(description) = val.description {
      obj.set("description", description)?;
    }

    Object::to_napi_value(env, obj)
  }
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
