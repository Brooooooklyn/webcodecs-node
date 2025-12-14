//! AudioData - WebCodecs API implementation
//!
//! Represents uncompressed audio data that can be encoded or played.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/AudioData

use crate::codec::Frame;
use crate::ffi::AVSampleFormat;
use crate::webcodecs::error::{invalid_state_error, throw_invalid_state_error};
use napi::bindgen_prelude::*;
use napi_derive::napi;
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

    // Get timestamp (required)
    let timestamp: i64 = match obj.get("timestamp")? {
      Some(v) => v,
      None => return Err(throw_type_error(env, "timestamp is required")),
    };

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
  frame: Frame,
  format: AudioSampleFormat,
  timestamp_us: i64,
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
  pub fn new(
    env: Env,
    #[napi(ts_arg_type = "import('./standard').AudioDataInit")] init: AudioDataInit,
  ) -> Result<Self> {
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
      frame,
      format: init.format,
      timestamp_us: init.timestamp,
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
      frame,
      format,
      timestamp_us,
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
      Some(i) => Ok(i.frame.sample_rate() as f64),
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
      Some(i) => Ok(i.frame.nb_samples()),
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
      Some(i) => Ok(i.frame.channels()),
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
        let frames = i.frame.nb_samples() as i64;
        let sample_rate = i.frame.sample_rate() as i64;
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
          Ok(i.frame.channels())
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

    let format = options.format.unwrap_or(inner.format);

    // Validate planeIndex (throws RangeError per W3C spec)
    let num_planes = if format.is_planar() {
      inner.frame.channels()
    } else {
      1
    };
    if options.plane_index >= num_planes {
      env.throw_range_error(
        &format!(
          "planeIndex {} is out of bounds (numberOfPlanes is {})",
          options.plane_index, num_planes
        ),
        None,
      )?;
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "planeIndex {} is out of bounds (numberOfPlanes is {})",
          options.plane_index, num_planes
        ),
      ));
    }

    let frame_offset = options.frame_offset.unwrap_or(0);
    let num_frames = options
      .frame_count
      .unwrap_or(inner.frame.nb_samples() - frame_offset);

    let bytes_per_sample = format.bytes_per_sample() as u32;

    if format.is_planar() {
      // Planar: one plane per channel
      Ok(num_frames * bytes_per_sample)
    } else {
      // Interleaved: all channels in one buffer
      Ok(num_frames * inner.frame.channels() * bytes_per_sample)
    }
  }

  /// Copy audio data to a buffer (W3C WebCodecs spec)
  /// Note: Per spec, this is SYNCHRONOUS and returns undefined
  /// Accepts AllowSharedBufferSource (any TypedArray, DataView, or ArrayBuffer)
  #[napi(
    ts_args_type = "destination: import('./standard').AllowSharedBufferSource, options: AudioDataCopyToOptions"
  )]
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

    let format = options.format.unwrap_or(inner.format);
    let plane_index = options.plane_index as usize;
    let frame_offset = options.frame_offset.unwrap_or(0) as usize;
    let num_frames = options
      .frame_count
      .unwrap_or(inner.frame.nb_samples() - frame_offset as u32) as usize;

    let bytes_per_sample = format.bytes_per_sample();
    let channels = inner.frame.channels() as usize;

    // Validate planeIndex (throws RangeError per W3C spec)
    let num_planes = if format.is_planar() { channels } else { 1 };
    if plane_index >= num_planes {
      env.throw_range_error(
        &format!(
          "planeIndex {} is out of bounds (numberOfPlanes is {})",
          plane_index, num_planes
        ),
        None,
      )?;
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "planeIndex {} is out of bounds (numberOfPlanes is {})",
          plane_index, num_planes
        ),
      ));
    }

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

    if format.is_planar() {
      let copy_size = num_frames * bytes_per_sample;
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

      // Get source data
      if inner.format.is_planar() {
        // Source is planar too
        if let Some(src) = inner.frame.audio_channel_data(plane_index) {
          let src_offset = frame_offset * bytes_per_sample;
          dest_slice[..copy_size].copy_from_slice(&src[src_offset..src_offset + copy_size]);
        }
      } else {
        // Source is interleaved, need to extract one channel
        if let Some(src) = inner.frame.audio_channel_data(0) {
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
      let copy_size = num_frames * channels * bytes_per_sample;
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

      if inner.format.is_planar() {
        // Source is planar, need to interleave
        for i in 0..num_frames {
          for ch in 0..channels {
            if let Some(src) = inner.frame.audio_channel_data(ch) {
              let src_offset = (frame_offset + i) * bytes_per_sample;
              let dst_offset = (i * channels + ch) * bytes_per_sample;
              dest_slice[dst_offset..dst_offset + bytes_per_sample]
                .copy_from_slice(&src[src_offset..src_offset + bytes_per_sample]);
            }
          }
        }
      } else {
        // Both interleaved
        if let Some(src) = inner.frame.audio_channel_data(0) {
          let src_offset = frame_offset * channels * bytes_per_sample;
          dest_slice[..copy_size].copy_from_slice(&src[src_offset..src_offset + copy_size]);
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

    let cloned_frame = inner.frame.try_clone().map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to clone frame: {}", e),
      )
    })?;

    Ok(AudioData {
      inner: Arc::new(Mutex::new(Some(AudioDataInner {
        frame: cloned_frame,
        format: inner.format,
        timestamp_us: inner.timestamp_us,
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
      Some(i) if !i.closed => Ok(f(&i.frame)),
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

    let num_frames = inner.frame.nb_samples() as usize;
    let channels = inner.frame.channels() as usize;
    let bytes_per_sample = inner.format.bytes_per_sample();
    let total_size = num_frames * channels * bytes_per_sample;

    let mut buffer = vec![0u8; total_size];
    inner.frame.copy_audio_to_buffer(&mut buffer).map_err(|e| {
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
      return f
        .debug_struct("AudioData")
        .field("format", &i.format)
        .field("sample_rate", &i.frame.sample_rate())
        .field("number_of_frames", &i.frame.nb_samples())
        .field("number_of_channels", &i.frame.channels())
        .field("timestamp", &i.timestamp_us)
        .finish();
    }
    f.debug_struct("AudioData").field("closed", &true).finish()
  }
}
