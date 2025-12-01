//! AudioData - WebCodecs API implementation
//!
//! Represents uncompressed audio data that can be encoded or played.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/AudioData

use crate::codec::Frame;
use crate::ffi::AVSampleFormat;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// Audio sample format (WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSampleFormat {
  /// Unsigned 8-bit integer samples, interleaved
  U8,
  /// Signed 16-bit integer samples, interleaved
  S16,
  /// Signed 32-bit integer samples, interleaved
  S32,
  /// 32-bit float samples, interleaved
  F32,
  /// Unsigned 8-bit integer samples, planar
  U8Planar,
  /// Signed 16-bit integer samples, planar
  S16Planar,
  /// Signed 32-bit integer samples, planar
  S32Planar,
  /// 32-bit float samples, planar
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

/// Options for creating an AudioData
#[napi(object)]
#[derive(Debug, Clone)]
pub struct AudioDataInit {
  /// Sample format
  pub format: AudioSampleFormat,
  /// Sample rate in Hz
  pub sample_rate: u32,
  /// Number of frames (samples per channel)
  pub number_of_frames: u32,
  /// Number of channels
  pub number_of_channels: u32,
  /// Timestamp in microseconds
  pub timestamp: i64,
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
}

#[napi]
impl AudioData {
  /// Create a new AudioData from raw sample data
  #[napi(constructor)]
  pub fn new(data: &[u8], init: AudioDataInit) -> Result<Self> {
    let av_format = init.format.to_av_format();

    // Create internal frame
    let mut frame = Frame::new_audio(
      init.number_of_frames,
      init.number_of_channels,
      init.sample_rate,
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
    }
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

  /// Get sample rate in Hz
  #[napi(getter)]
  pub fn sample_rate(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => Ok(i.frame.sample_rate()),
      None => Err(Error::new(Status::GenericFailure, "AudioData is closed")),
    }
  }

  /// Get number of frames (samples per channel)
  #[napi(getter)]
  pub fn number_of_frames(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => Ok(i.frame.nb_samples()),
      None => Err(Error::new(Status::GenericFailure, "AudioData is closed")),
    }
  }

  /// Get number of channels
  #[napi(getter)]
  pub fn number_of_channels(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => Ok(i.frame.channels()),
      None => Err(Error::new(Status::GenericFailure, "AudioData is closed")),
    }
  }

  /// Get duration in microseconds
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
      None => Err(Error::new(Status::GenericFailure, "AudioData is closed")),
    }
  }

  /// Get timestamp in microseconds
  #[napi(getter)]
  pub fn timestamp(&self) -> Result<i64> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match &*inner {
      Some(i) => Ok(i.timestamp_us),
      None => Err(Error::new(Status::GenericFailure, "AudioData is closed")),
    }
  }

  // ========================================================================
  // Methods (WebCodecs spec)
  // ========================================================================

  /// Get the buffer size required for allocationSize
  #[napi]
  pub fn allocation_size(&self, options: Option<AudioDataCopyToOptions>) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let inner = inner
      .as_ref()
      .ok_or_else(|| Error::new(Status::GenericFailure, "AudioData is closed"))?;

    let format = options
      .as_ref()
      .and_then(|o| o.format)
      .unwrap_or(inner.format);
    let frame_offset = options.as_ref().and_then(|o| o.frame_offset).unwrap_or(0);
    let num_frames = options
      .as_ref()
      .and_then(|o| o.frame_count)
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

  /// Copy audio data to a buffer
  #[napi]
  pub fn copy_to(
    &self,
    destination: Uint8Array,
    options: Option<AudioDataCopyToOptions>,
  ) -> Result<()> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let inner = inner
      .as_ref()
      .ok_or_else(|| Error::new(Status::GenericFailure, "AudioData is closed"))?;

    let format = options
      .as_ref()
      .and_then(|o| o.format)
      .unwrap_or(inner.format);
    let plane_index = options
      .as_ref()
      .map(|o| o.plane_index as usize)
      .unwrap_or(0);
    let frame_offset = options.as_ref().and_then(|o| o.frame_offset).unwrap_or(0) as usize;
    let num_frames = options
      .as_ref()
      .and_then(|o| o.frame_count)
      .unwrap_or(inner.frame.nb_samples() - frame_offset as u32) as usize;

    let bytes_per_sample = format.bytes_per_sample();
    let channels = inner.frame.channels() as usize;

    // Get mutable access to the destination buffer
    let dest_ptr = destination.as_ref().as_ptr() as *mut u8;

    if format.is_planar() {
      // Copy single plane
      if plane_index >= channels {
        return Err(Error::new(Status::InvalidArg, "Invalid plane index"));
      }

      let copy_size = num_frames * bytes_per_sample;
      if destination.len() < copy_size {
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
          unsafe {
            std::ptr::copy_nonoverlapping(src[src_offset..].as_ptr(), dest_ptr, copy_size);
          }
        }
      } else {
        // Source is interleaved, need to extract one channel
        if let Some(src) = inner.frame.audio_channel_data(0) {
          for i in 0..num_frames {
            let src_offset = ((frame_offset + i) * channels + plane_index) * bytes_per_sample;
            let dst_offset = i * bytes_per_sample;
            unsafe {
              std::ptr::copy_nonoverlapping(
                src[src_offset..].as_ptr(),
                dest_ptr.add(dst_offset),
                bytes_per_sample,
              );
            }
          }
        }
      }
    } else {
      // Interleaved output
      let copy_size = num_frames * channels * bytes_per_sample;
      if destination.len() < copy_size {
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
              unsafe {
                std::ptr::copy_nonoverlapping(
                  src[src_offset..].as_ptr(),
                  dest_ptr.add(dst_offset),
                  bytes_per_sample,
                );
              }
            }
          }
        }
      } else {
        // Both interleaved
        if let Some(src) = inner.frame.audio_channel_data(0) {
          let src_offset = frame_offset * channels * bytes_per_sample;
          unsafe {
            std::ptr::copy_nonoverlapping(src[src_offset..].as_ptr(), dest_ptr, copy_size);
          }
        }
      }
    }

    Ok(())
  }

  /// Create a copy of this AudioData
  #[napi(js_name = "clone")]
  pub fn clone_audio_data(&self) -> Result<AudioData> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let inner = inner
      .as_ref()
      .ok_or_else(|| Error::new(Status::GenericFailure, "AudioData is closed"))?;

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
      Some(_) => Err(Error::new(Status::GenericFailure, "AudioData is closed")),
      None => Err(Error::new(Status::GenericFailure, "AudioData is closed")),
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
      .ok_or_else(|| Error::new(Status::GenericFailure, "AudioData is closed"))?;

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
    if let Ok(inner) = self.inner.lock() {
      if let Some(ref i) = *inner {
        return f
          .debug_struct("AudioData")
          .field("format", &i.format)
          .field("sample_rate", &i.frame.sample_rate())
          .field("number_of_frames", &i.frame.nb_samples())
          .field("number_of_channels", &i.frame.channels())
          .field("timestamp", &i.timestamp_us)
          .finish();
      }
    }
    f.debug_struct("AudioData").field("closed", &true).finish()
  }
}
