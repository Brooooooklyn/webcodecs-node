//! AudioData - WebCodecs API implementation
//!
//! Represents uncompressed audio data that can be encoded or played.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/AudioData

use crate::codec::Frame;
use crate::ffi::AVSampleFormat;
use crate::webcodecs::error::{
  enforce_range_long_long, enforce_range_unsigned_long_long_optional, invalid_state_error,
  native_range_error, throw_invalid_state_error,
};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use parking_lot::RwLock as ParkingLotRwLock;
use std::sync::{Arc, Mutex};

/// Audio sample format (WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleFormat {
  /// Unsigned 8-bit integer samples, interleaved
  #[napi(value = "u8")]
  U8,
  /// Signed 16-bit integer samples, interleaved
  #[napi(value = "s16")]
  S16,
  /// Signed 32-bit integer samples, interleaved
  #[napi(value = "s32")]
  S32,
  /// 32-bit float samples, interleaved
  #[napi(value = "f32")]
  F32,
  /// Unsigned 8-bit integer samples, planar
  #[napi(value = "u8-planar")]
  U8Planar,
  /// Signed 16-bit integer samples, planar
  #[napi(value = "s16-planar")]
  S16Planar,
  /// Signed 32-bit integer samples, planar
  #[napi(value = "s32-planar")]
  S32Planar,
  /// 32-bit float samples, planar
  #[napi(value = "f32-planar")]
  F32Planar,
}

impl AudioSampleFormat {
  /// Convert from FFmpeg sample format
  pub fn from_av_format(format: AVSampleFormat) -> Option<Self> {
    match format {
      AVSampleFormat::U8 => Some(AudioSampleFormat::U8),
      AVSampleFormat::S16 => Some(AudioSampleFormat::S16),
      AVSampleFormat::S32 => Some(AudioSampleFormat::S32),
      AVSampleFormat::Flt => Some(AudioSampleFormat::F32),
      AVSampleFormat::U8p => Some(AudioSampleFormat::U8Planar),
      AVSampleFormat::S16p => Some(AudioSampleFormat::S16Planar),
      AVSampleFormat::S32p => Some(AudioSampleFormat::S32Planar),
      AVSampleFormat::Fltp => Some(AudioSampleFormat::F32Planar),
      _ => None,
    }
  }

  /// Convert to FFmpeg sample format
  pub fn to_av_format(&self) -> AVSampleFormat {
    match self {
      AudioSampleFormat::U8 => AVSampleFormat::U8,
      AudioSampleFormat::S16 => AVSampleFormat::S16,
      AudioSampleFormat::S32 => AVSampleFormat::S32,
      AudioSampleFormat::F32 => AVSampleFormat::Flt,
      AudioSampleFormat::U8Planar => AVSampleFormat::U8p,
      AudioSampleFormat::S16Planar => AVSampleFormat::S16p,
      AudioSampleFormat::S32Planar => AVSampleFormat::S32p,
      AudioSampleFormat::F32Planar => AVSampleFormat::Fltp,
    }
  }

  /// Get bytes per sample
  pub fn bytes_per_sample(&self) -> usize {
    match self {
      AudioSampleFormat::U8 | AudioSampleFormat::U8Planar => 1,
      AudioSampleFormat::S16 | AudioSampleFormat::S16Planar => 2,
      AudioSampleFormat::S32
      | AudioSampleFormat::S32Planar
      | AudioSampleFormat::F32
      | AudioSampleFormat::F32Planar => 4,
    }
  }

  /// Check if this is a planar format
  pub fn is_planar(&self) -> bool {
    matches!(
      self,
      AudioSampleFormat::U8Planar
        | AudioSampleFormat::S16Planar
        | AudioSampleFormat::S32Planar
        | AudioSampleFormat::F32Planar
    )
  }

  /// Base sample type with planarity stripped (u8-planar → u8, etc.)
  fn base(&self) -> AudioSampleFormat {
    match self {
      AudioSampleFormat::U8 | AudioSampleFormat::U8Planar => AudioSampleFormat::U8,
      AudioSampleFormat::S16 | AudioSampleFormat::S16Planar => AudioSampleFormat::S16,
      AudioSampleFormat::S32 | AudioSampleFormat::S32Planar => AudioSampleFormat::S32,
      AudioSampleFormat::F32 | AudioSampleFormat::F32Planar => AudioSampleFormat::F32,
    }
  }
}

/// Read one sample normalized to the [-1, 1] domain per the spec's
/// "Magnitude of the audio samples" table (u8 bias 128, s16/s32 signed,
/// f32 native). All format conversion goes through this representation.
fn read_sample(buf: &[u8], index: usize, format: AudioSampleFormat) -> f64 {
  match format.base() {
    AudioSampleFormat::U8 => (f64::from(buf[index]) - 128.0) / 128.0,
    AudioSampleFormat::S16 => {
      f64::from(i16::from_le_bytes([buf[index * 2], buf[index * 2 + 1]])) / 32768.0
    }
    AudioSampleFormat::S32 => {
      i32::from_le_bytes(buf[index * 4..index * 4 + 4].try_into().unwrap()) as f64 / 2147483648.0
    }
    AudioSampleFormat::F32 => {
      f32::from_le_bytes(buf[index * 4..index * 4 + 4].try_into().unwrap()) as f64
    }
    _ => unreachable!(),
  }
}

/// Write one normalized [-1, 1] sample in `format`, clipping to the type's
/// minimum/maximum values. f32 output is not clipped per the spec note that
/// implementations should not clip internally when handling f32 samples.
fn write_sample(buf: &mut [u8], index: usize, format: AudioSampleFormat, value: f64) {
  match format.base() {
    AudioSampleFormat::U8 => {
      buf[index] = (value * 128.0 + 128.0).round().clamp(0.0, 255.0) as u8;
    }
    AudioSampleFormat::S16 => {
      buf[index * 2..index * 2 + 2].copy_from_slice(
        &((value * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes(),
      );
    }
    AudioSampleFormat::S32 => {
      buf[index * 4..index * 4 + 4].copy_from_slice(
        &((value * 2147483648.0)
          .round()
          .clamp(-2147483648.0, 2147483647.0) as i32)
          .to_le_bytes(),
      );
    }
    AudioSampleFormat::F32 => {
      buf[index * 4..index * 4 + 4].copy_from_slice(&(value as f32).to_le_bytes());
    }
    _ => unreachable!(),
  }
}

/// Result of the spec's "Compute Copy Element Count" algorithm, plus the
/// resolved destination format. Shared by allocationSize() and copyTo().
struct CopyPlan {
  /// Destination sample format (options.format ?? the AudioData's format)
  dest_format: AudioSampleFormat,
  /// Number of frames to copy (validated against frameOffset/frameCount)
  copy_frames: usize,
  /// Total sample count to copy — frames × channels for interleaved dests
  element_count: usize,
}

/// Compute Copy Element Count per W3C WebCodecs spec: planeIndex is bounded
/// by the *destination* format's planarity (interleaved dests have exactly
/// one plane, planar dests one per channel); frameOffset >= numberOfFrames
/// and oversized frameCount are RangeErrors.
fn compute_copy_plan(
  env: &Env,
  inner: &AudioDataInner,
  frame: &Frame,
  options: &AudioDataCopyToOptions,
) -> Result<CopyPlan> {
  let dest_format = options.format.unwrap_or(inner.format);
  let channels = frame.channels();

  let num_planes = if dest_format.is_planar() { channels } else { 1 };
  if options.plane_index >= num_planes {
    return Err(native_range_error(
      env,
      &format!(
        "planeIndex {} is out of bounds (numberOfPlanes is {})",
        options.plane_index, num_planes
      ),
    )?);
  }

  let frame_count = frame.nb_samples();
  let frame_offset = options.frame_offset.unwrap_or(0);
  if frame_offset >= frame_count {
    return Err(native_range_error(
      env,
      &format!(
        "frameOffset {} is out of bounds (numberOfFrames is {})",
        frame_offset, frame_count
      ),
    )?);
  }
  let mut copy_frames = frame_count - frame_offset;
  if let Some(frame_count_opt) = options.frame_count {
    if frame_count_opt > copy_frames {
      return Err(native_range_error(
        env,
        &format!(
          "frameCount {} exceeds the {} frames available after frameOffset",
          frame_count_opt, copy_frames
        ),
      )?);
    }
    copy_frames = frame_count_opt;
  }

  let element_count = if dest_format.is_planar() {
    copy_frames as usize
  } else {
    copy_frames as usize * channels as usize
  };

  Ok(CopyPlan {
    dest_format,
    copy_frames: copy_frames as usize,
    element_count,
  })
}

/// Options for creating an AudioData (W3C WebCodecs spec)
/// Note: Per spec, data is included in the init object
pub struct AudioDataInit {
  /// Sample format (required)
  pub format: AudioSampleFormat,
  /// Sample rate in Hz (required) - W3C spec uses float
  pub sample_rate: f64,
  /// Number of frames (samples per channel) (required)
  pub number_of_frames: u32,
  /// Number of channels (required)
  pub number_of_channels: u32,
  /// Timestamp in microseconds (required)
  pub timestamp: i64,
  /// Duration in microseconds (optional) - [EnforceRange] unsigned long long per spec
  pub duration: Option<i64>,
  /// Raw audio sample data (required) - BufferSource per spec
  pub data: Vec<u8>,
}

/// Helper to throw TypeError and return an error
fn throw_type_error(env: napi::sys::napi_env, message: &str) -> Error {
  let env_wrapper = Env::from_raw(env);
  let _ = env_wrapper.throw_type_error(message, None);
  Error::new(Status::InvalidArg, message)
}

impl FromNapiValue for AudioDataInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // Get format (required) - first check if it's a valid string
    let format_str: Option<String> = obj.get("format")?;
    let format = match format_str {
      Some(s) => match s.as_str() {
        "u8" => AudioSampleFormat::U8,
        "s16" => AudioSampleFormat::S16,
        "s32" => AudioSampleFormat::S32,
        "f32" => AudioSampleFormat::F32,
        "u8-planar" => AudioSampleFormat::U8Planar,
        "s16-planar" => AudioSampleFormat::S16Planar,
        "s32-planar" => AudioSampleFormat::S32Planar,
        "f32-planar" => AudioSampleFormat::F32Planar,
        _ => return Err(throw_type_error(env, &format!("Invalid format: {}", s))),
      },
      None => return Err(throw_type_error(env, "format is required")),
    };

    // Get sample_rate (required) - W3C spec uses float
    let sample_rate: f64 = match obj.get("sampleRate")? {
      Some(v) => v,
      None => return Err(throw_type_error(env, "sampleRate is required")),
    };

    // Get numberOfFrames (required)
    let number_of_frames: u32 = match obj.get("numberOfFrames")? {
      Some(v) => v,
      None => return Err(throw_type_error(env, "numberOfFrames is required")),
    };

    // Get numberOfChannels (required)
    let number_of_channels: u32 = match obj.get("numberOfChannels")? {
      Some(v) => v,
      None => return Err(throw_type_error(env, "numberOfChannels is required")),
    };

    // Get timestamp (required) per WebIDL [EnforceRange] long long
    // Accept f64 and manually convert per WebIDL spec to handle floating-point values
    let env_wrapper = Env::from_raw(env);
    let timestamp_f64: Option<f64> = obj.get("timestamp")?;
    let timestamp = match timestamp_f64 {
      Some(ts) => enforce_range_long_long(&env_wrapper, ts, "timestamp")?,
      None => return Err(throw_type_error(env, "timestamp is required")),
    };

    // Duration is optional per WebIDL [EnforceRange] unsigned long long
    let duration_f64: Option<f64> = obj.get("duration")?;
    let duration =
      enforce_range_unsigned_long_long_optional(&env_wrapper, duration_f64, "duration")?
        .map(|v| v.min(i64::MAX as u64) as i64);

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
        return Err(throw_type_error(env, "data is required"));
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
            return Err(throw_type_error(env, "data must be a valid BufferSource"));
          }
        } else {
          return Err(throw_type_error(env, "data must be a BufferSource"));
        }
      } else {
        return Err(throw_type_error(env, "data is required"));
      }
    };

    Ok(AudioDataInit {
      format,
      sample_rate,
      number_of_frames,
      number_of_channels,
      timestamp,
      duration,
      data,
    })
  }
}

/// Options for copyTo operation
#[napi(object)]
#[derive(Debug, Clone)]
pub struct AudioDataCopyToOptions {
  /// The index of the audio plane to copy
  pub plane_index: u32,
  /// The offset in frames to start copying from (optional)
  pub frame_offset: Option<u32>,
  /// The number of frames to copy (optional, defaults to all remaining)
  pub frame_count: Option<u32>,
  /// Target format for conversion (optional)
  pub format: Option<AudioSampleFormat>,
}

/// Internal state for AudioData
struct AudioDataInner {
  /// Shared reference to the frame data (via Arc for Rust-level sharing)
  frame: Arc<ParkingLotRwLock<Frame>>,
  format: AudioSampleFormat,
  timestamp_us: i64,
  /// Duration in microseconds when provided at construction; otherwise computed
  /// from frames and sample rate in the getter (spec: [[duration]])
  duration_us: Option<i64>,
  closed: bool,
}

/// AudioData - represents uncompressed audio data
///
/// This is a WebCodecs-compliant AudioData implementation backed by FFmpeg.
#[napi]
pub struct AudioData {
  inner: Arc<Mutex<Option<AudioDataInner>>>,
  /// Timestamp is preserved after close per W3C spec
  timestamp_us: i64,
}

#[napi]
impl AudioData {
  /// Create a new AudioData (W3C WebCodecs spec)
  /// Per spec, the constructor takes a single init object containing all parameters including data
  #[napi(constructor)]
  pub fn new(env: Env, init: AudioDataInit) -> Result<Self> {
    // Validate zero values
    if init.sample_rate == 0.0 {
      env.throw_type_error("sampleRate must be greater than 0", None)?;
      return Err(Error::new(
        Status::InvalidArg,
        "sampleRate must be greater than 0",
      ));
    }
    if init.number_of_frames == 0 {
      env.throw_type_error("numberOfFrames must be greater than 0", None)?;
      return Err(Error::new(
        Status::InvalidArg,
        "numberOfFrames must be greater than 0",
      ));
    }
    if init.number_of_channels == 0 {
      env.throw_type_error("numberOfChannels must be greater than 0", None)?;
      return Err(Error::new(
        Status::InvalidArg,
        "numberOfChannels must be greater than 0",
      ));
    }

    // Validate buffer size
    let expected_size =
      Self::calculate_buffer_size(init.format, init.number_of_frames, init.number_of_channels);
    if init.data.len() < expected_size {
      env.throw_type_error(
        &format!(
          "data buffer too small: need {} bytes, got {}",
          expected_size,
          init.data.len()
        ),
        None,
      )?;
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "data buffer too small: need {} bytes, got {}",
          expected_size,
          init.data.len()
        ),
      ));
    }

    let av_format = init.format.to_av_format();
    let data = &init.data;
    // Convert sample_rate from f64 to u32 for FFmpeg (internally uses integer)
    let sample_rate_u32 = init.sample_rate as u32;

    // Create internal frame
    let mut frame = Frame::new_audio(
      init.number_of_frames,
      init.number_of_channels,
      sample_rate_u32,
      av_format,
    )
    .map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create frame: {}", e),
      )
    })?;

    // Copy data into the frame
    Self::copy_data_to_frame(
      &mut frame,
      data,
      init.format,
      init.number_of_frames,
      init.number_of_channels,
    )?;

    // Set timestamps
    frame.set_pts(init.timestamp);

    let inner = AudioDataInner {
      frame: frame.into_shared(),
      format: init.format,
      timestamp_us: init.timestamp,
      duration_us: init.duration,
      closed: false,
    };

    Ok(Self {
      inner: Arc::new(Mutex::new(Some(inner))),
      timestamp_us: init.timestamp,
    })
  }

  /// Create an AudioData from an internal Frame (for decoder output)
  pub fn from_internal(frame: Frame, timestamp_us: i64) -> Self {
    let av_format = frame.sample_format();
    let format = AudioSampleFormat::from_av_format(av_format).unwrap_or(AudioSampleFormat::F32);

    let inner = AudioDataInner {
      frame: frame.into_shared(),
      format,
      timestamp_us,
      duration_us: None,
      closed: false,
    };

    Self {
      inner: Arc::new(Mutex::new(Some(inner))),
      timestamp_us,
    }
  }

  /// Calculate required buffer size for audio data
  fn calculate_buffer_size(format: AudioSampleFormat, num_frames: u32, channels: u32) -> usize {
    let bytes_per_sample = format.bytes_per_sample();
    num_frames as usize * channels as usize * bytes_per_sample
  }

  /// Copy data into frame
  fn copy_data_to_frame(
    frame: &mut Frame,
    data: &[u8],
    format: AudioSampleFormat,
    num_frames: u32,
    channels: u32,
  ) -> Result<()> {
    let bytes_per_sample = format.bytes_per_sample();
    let is_planar = format.is_planar();

    if is_planar {
      // Planar: data is organized as [ch0_samples][ch1_samples]...
      let plane_size = num_frames as usize * bytes_per_sample;
      for ch in 0..channels as usize {
        let src_offset = ch * plane_size;
        if src_offset + plane_size > data.len() {
          return Err(Error::new(
            Status::InvalidArg,
            "Data buffer too small for planar format",
          ));
        }

        if let Some(dest) = frame.audio_channel_data_mut(ch) {
          dest[..plane_size].copy_from_slice(&data[src_offset..src_offset + plane_size]);
        }
      }
    } else {
      // Interleaved: all samples are in one buffer
      let total_size = num_frames as usize * channels as usize * bytes_per_sample;
      if data.len() < total_size {
        return Err(Error::new(
          Status::InvalidArg,
          "Data buffer too small for interleaved format",
        ));
      }

      if let Some(dest) = frame.audio_channel_data_mut(0) {
        dest[..total_size].copy_from_slice(&data[..total_size]);
      }
    }

    Ok(())
  }

  // ========================================================================
  // Properties (WebCodecs spec)
  // ========================================================================

  /// Get sample format
  #[napi(getter)]
  pub fn format(&self) -> Result<Option<AudioSampleFormat>> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    Ok(inner.as_ref().map(|i| i.format))
  }

  /// Get sample rate in Hz (W3C spec uses float)
  /// Returns 0 after close per W3C spec
  #[napi(getter)]
  pub fn sample_rate(&self) -> Result<f64> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => Ok(i.frame.read().sample_rate() as f64),
      None => Ok(0.0), // Return 0 after close per W3C spec
    }
  }

  /// Get number of frames (samples per channel)
  /// Returns 0 after close per W3C spec
  #[napi(getter)]
  pub fn number_of_frames(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => Ok(i.frame.read().nb_samples()),
      None => Ok(0), // Return 0 after close per W3C spec
    }
  }

  /// Get number of channels
  /// Returns 0 after close per W3C spec
  #[napi(getter)]
  pub fn number_of_channels(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => Ok(i.frame.read().channels()),
      None => Ok(0), // Return 0 after close per W3C spec
    }
  }

  /// Get duration in microseconds
  /// Returns 0 after close per W3C spec
  #[napi(getter)]
  pub fn duration(&self) -> Result<i64> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => {
        if let Some(duration) = i.duration_us {
          return Ok(duration);
        }
        let frame_guard = i.frame.read();
        let frames = frame_guard.nb_samples() as i64;
        let sample_rate = frame_guard.sample_rate() as i64;
        if sample_rate > 0 {
          Ok((frames * 1_000_000) / sample_rate)
        } else {
          Ok(0)
        }
      }
      None => Ok(0), // Return 0 after close per W3C spec
    }
  }

  /// Get timestamp in microseconds
  /// Timestamp is preserved after close per W3C spec
  #[napi(getter)]
  pub fn timestamp(&self) -> Result<i64> {
    // Timestamp is preserved after close per W3C spec
    Ok(self.timestamp_us)
  }

  /// Get whether this AudioData has been closed (W3C WebCodecs spec)
  #[napi(getter)]
  pub fn closed(&self) -> Result<bool> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    Ok(inner.is_none())
  }

  /// Get the number of planes in this AudioData (W3C WebCodecs spec)
  /// For interleaved formats: 1
  /// For planar formats: numberOfChannels
  #[napi(getter)]
  pub fn number_of_planes(&self, env: Env) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => {
        if i.format.is_planar() {
          Ok(i.frame.read().channels())
        } else {
          Ok(1)
        }
      }
      None => throw_invalid_state_error(&env, "AudioData is closed"),
    }
  }

  // ========================================================================
  // Methods (WebCodecs spec)
  // ========================================================================

  /// Get the buffer size required for copyTo (W3C WebCodecs spec)
  /// Note: options is REQUIRED per spec
  #[napi]
  pub fn allocation_size(&self, env: Env, options: AudioDataCopyToOptions) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let inner = match inner.as_ref() {
      Some(i) => i,
      None => return throw_invalid_state_error(&env, "AudioData is closed"),
    };

    // Acquire read lock on the shared frame
    let frame_guard = inner.frame.read();

    // Compute Copy Element Count: validates planeIndex against the
    // destination layout, frameOffset/frameCount against the frame's
    // numberOfFrames (throws RangeError per W3C spec)
    let plan = compute_copy_plan(&env, inner, &frame_guard, &options)?;

    let byte_size = plan.element_count * plan.dest_format.bytes_per_sample();
    u32::try_from(byte_size).map_err(|_| {
      Error::new(
        Status::InvalidArg,
        "RangeError: allocation size does not fit in u32",
      )
    })
  }

  /// Copy audio data to a buffer (W3C WebCodecs spec)
  /// Note: Per spec, this is SYNCHRONOUS and returns undefined
  /// Accepts AllowSharedBufferSource (any TypedArray, DataView, or ArrayBuffer)
  #[napi(ts_args_type = "destination: AllowSharedBufferSource, options: AudioDataCopyToOptions")]
  pub fn copy_to(
    &self,
    env: Env,
    destination: Unknown,
    options: AudioDataCopyToOptions,
  ) -> Result<()> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let inner = match inner.as_ref() {
      Some(i) => i,
      None => return throw_invalid_state_error(&env, "AudioData is closed"),
    };

    // Acquire read lock on the shared frame
    let frame_guard = inner.frame.read();

    // Compute Copy Element Count: validates planeIndex against the
    // destination layout, frameOffset/frameCount against the frame's
    // numberOfFrames (throws RangeError per W3C spec)
    let plan = compute_copy_plan(&env, inner, &frame_guard, &options)?;

    let dest_format = plan.dest_format;
    let plane_index = options.plane_index as usize;
    let frame_offset = options.frame_offset.unwrap_or(0) as usize;
    let num_frames = plan.copy_frames;
    let channels = frame_guard.channels() as usize;

    // Extract the underlying buffer from AllowSharedBufferSource (TypedArray, DataView, or ArrayBuffer)
    let typed_array = destination
      .coerce_to_object()
      .map_err(|_| Error::new(Status::InvalidArg, "Invalid AllowSharedBufferSource"))?;

    // Get buffer info - handle both TypedArray/DataView and direct ArrayBuffer
    let (mut buffer, byte_offset, byte_length): (ArrayBuffer, usize, usize) =
      if let Ok(true) = typed_array.has_named_property("buffer") {
        // It's a TypedArray or DataView - get its underlying buffer info
        let byte_length: u32 = typed_array.get("byteLength").ok().flatten().unwrap_or(0);
        let byte_offset: u32 = typed_array.get("byteOffset").ok().flatten().unwrap_or(0);
        let buffer: ArrayBuffer = typed_array
          .get("buffer")?
          .ok_or_else(|| Error::new(Status::InvalidArg, "Invalid AllowSharedBufferSource"))?;
        (buffer, byte_offset as usize, byte_length as usize)
      } else {
        // It's likely an ArrayBuffer directly
        let byte_length: Option<u32> = typed_array.get("byteLength").ok().flatten();
        if let Some(len) = byte_length {
          let buffer = ArrayBuffer::from_unknown(destination)?;
          (buffer, 0, len as usize)
        } else {
          return Err(Error::new(
            Status::InvalidArg,
            "Invalid AllowSharedBufferSource",
          ));
        }
      };

    // Get mutable access to the destination buffer at the correct offset
    let full_buffer = unsafe { buffer.as_mut() };
    let dest_slice = &mut full_buffer[byte_offset..byte_offset + byte_length];

    // The destination must hold element_count samples in the destination
    // format — not the source format's byte size (RangeError per W3C spec)
    let copy_size = plan.element_count * dest_format.bytes_per_sample();
    if dest_slice.len() < copy_size {
      env.throw_range_error(
        &format!(
          "destination buffer too small: need {} bytes, got {}",
          copy_size,
          dest_slice.len()
        ),
        None,
      )?;
      return Err(Error::new(
        Status::InvalidArg,
        "Destination buffer too small",
      ));
    }

    let src_format = inner.format;
    if src_format.base() == dest_format.base() {
      // Same base sample type (planarity may still differ) — verbatim byte
      // copy; source indexing uses the source format's byte size
      let bytes_per_sample = dest_format.bytes_per_sample();
      if dest_format.is_planar() {
        if src_format.is_planar() {
          // Source is planar too
          if let Some(src) = frame_guard.audio_channel_data(plane_index) {
            let src_offset = frame_offset * bytes_per_sample;
            dest_slice[..copy_size].copy_from_slice(&src[src_offset..src_offset + copy_size]);
          }
        } else {
          // Source is interleaved, need to extract one channel
          if let Some(src) = frame_guard.audio_channel_data(0) {
            for i in 0..num_frames {
              let src_offset = ((frame_offset + i) * channels + plane_index) * bytes_per_sample;
              let dst_offset = i * bytes_per_sample;
              dest_slice[dst_offset..dst_offset + bytes_per_sample]
                .copy_from_slice(&src[src_offset..src_offset + bytes_per_sample]);
            }
          }
        }
      } else {
        // Interleaved output
        if src_format.is_planar() {
          // Source is planar, need to interleave
          for i in 0..num_frames {
            for ch in 0..channels {
              if let Some(src) = frame_guard.audio_channel_data(ch) {
                let src_offset = (frame_offset + i) * bytes_per_sample;
                let dst_offset = (i * channels + ch) * bytes_per_sample;
                dest_slice[dst_offset..dst_offset + bytes_per_sample]
                  .copy_from_slice(&src[src_offset..src_offset + bytes_per_sample]);
              }
            }
          }
        } else {
          // Both interleaved
          if let Some(src) = frame_guard.audio_channel_data(0) {
            let src_offset = frame_offset * channels * bytes_per_sample;
            dest_slice[..copy_size].copy_from_slice(&src[src_offset..src_offset + copy_size]);
          }
        }
      }
    } else {
      // Base sample types differ — per-sample conversion through the
      // normalized [-1, 1] domain per the spec's magnitude table
      if dest_format.is_planar() {
        let src = frame_guard
          .audio_channel_data(if src_format.is_planar() {
            plane_index
          } else {
            0
          })
          .ok_or_else(|| Error::new(Status::GenericFailure, "missing audio channel data"))?;
        for i in 0..num_frames {
          let src_index = if src_format.is_planar() {
            frame_offset + i
          } else {
            (frame_offset + i) * channels + plane_index
          };
          write_sample(
            dest_slice,
            i,
            dest_format,
            read_sample(src, src_index, src_format),
          );
        }
      } else {
        // Interleaved destination — all channels are written
        for ch in 0..channels {
          let src = frame_guard
            .audio_channel_data(if src_format.is_planar() { ch } else { 0 })
            .ok_or_else(|| Error::new(Status::GenericFailure, "missing audio channel data"))?;
          for i in 0..num_frames {
            let src_index = if src_format.is_planar() {
              frame_offset + i
            } else {
              (frame_offset + i) * channels + ch
            };
            write_sample(
              dest_slice,
              i * channels + ch,
              dest_format,
              read_sample(src, src_index, src_format),
            );
          }
        }
      }
    }

    Ok(())
  }

  /// Create a copy of this AudioData
  #[napi(js_name = "clone")]
  pub fn clone_audio_data(&self, env: Env) -> Result<AudioData> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let inner = match inner.as_ref() {
      Some(i) => i,
      None => return throw_invalid_state_error(&env, "AudioData is closed"),
    };

    // Clone the Arc to share the underlying frame data (no pixel copy)
    let cloned_frame = inner.frame.clone();

    Ok(AudioData {
      inner: Arc::new(Mutex::new(Some(AudioDataInner {
        frame: cloned_frame,
        format: inner.format,
        timestamp_us: inner.timestamp_us,
        duration_us: inner.duration_us,
        closed: false,
      }))),
      timestamp_us: self.timestamp_us,
    })
  }

  /// Close and release resources
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    *inner = None;
    Ok(())
  }

  // ========================================================================
  // Internal methods for codec integration
  // ========================================================================

  /// Access the internal frame for encoding
  pub fn with_frame<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&Frame) -> R,
  {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) if !i.closed => {
        let frame_guard = i.frame.read();
        Ok(f(&frame_guard))
      }
      Some(_) => Err(invalid_state_error("AudioData is closed")),
      None => Err(invalid_state_error("AudioData is closed")),
    }
  }

  /// Get raw sample data for encoding (copies to interleaved format)
  pub fn get_data_vec(&self) -> Result<Vec<u8>> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let inner = inner
      .as_ref()
      .ok_or_else(|| invalid_state_error("AudioData is closed"))?;

    // Acquire read lock on the shared frame
    let frame_guard = inner.frame.read();

    let num_frames = frame_guard.nb_samples() as usize;
    let channels = frame_guard.channels() as usize;
    let bytes_per_sample = inner.format.bytes_per_sample();
    let total_size = num_frames * channels * bytes_per_sample;

    let mut buffer = vec![0u8; total_size];
    frame_guard.copy_audio_to_buffer(&mut buffer).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to copy audio: {}", e),
      )
    })?;

    Ok(buffer)
  }
}

impl std::fmt::Debug for AudioData {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    if let Ok(inner) = self.inner.lock()
      && let Some(ref i) = *inner
    {
      let frame_guard = i.frame.read();
      return f
        .debug_struct("AudioData")
        .field("format", &i.format)
        .field("sample_rate", &frame_guard.sample_rate())
        .field("number_of_frames", &frame_guard.nb_samples())
        .field("number_of_channels", &frame_guard.channels())
        .field("timestamp", &i.timestamp_us)
        .finish();
    }
    f.debug_struct("AudioData").field("closed", &true).finish()
  }
}
