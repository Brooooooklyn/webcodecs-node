//! Audio sample buffer for accumulating samples before encoding
//!
//! Different audio codecs require different frame sizes:
//! - AAC: 1024 samples
//! - MP3: 1152 samples
//! - Opus: 120, 240, 480, 960, 1920, or 2880 samples
//! - FLAC/Vorbis: variable
//!
//! This buffer accumulates input samples and produces frames of the required size.

use crate::ffi::AVSampleFormat;

use super::{CodecError, CodecResult, Frame};

/// Buffer for accumulating audio samples
pub struct AudioSampleBuffer {
  /// Sample buffer (interleaved format)
  buffer: Vec<u8>,
  /// Number of samples currently in buffer
  samples_in_buffer: usize,
  /// Target frame size (samples per channel)
  frame_size: usize,
  /// Number of channels
  channels: u32,
  /// Sample rate
  sample_rate: u32,
  /// Sample format
  format: AVSampleFormat,
  /// Bytes per sample
  bytes_per_sample: usize,
}

impl AudioSampleBuffer {
  /// Create a new audio sample buffer
  ///
  /// # Arguments
  /// * `frame_size` - Number of samples per channel required per frame
  /// * `channels` - Number of audio channels
  /// * `sample_rate` - Sample rate in Hz
  /// * `format` - Sample format (should be interleaved for simplicity)
  pub fn new(frame_size: usize, channels: u32, sample_rate: u32, format: AVSampleFormat) -> Self {
    let bytes_per_sample = format.bytes_per_sample();
    // Allocate buffer for 2x frame size to handle overflow
    let buffer_size = frame_size * channels as usize * bytes_per_sample * 2;

    Self {
      buffer: vec![0u8; buffer_size],
      samples_in_buffer: 0,
      frame_size,
      channels,
      sample_rate,
      format,
      bytes_per_sample,
    }
  }

  /// Get the required frame size for common codecs
  pub fn frame_size_for_codec(codec: &str) -> usize {
    let codec_lower = codec.to_lowercase();
    if codec_lower.contains("aac") || codec_lower.starts_with("mp4a.40") {
      1024
    } else if codec_lower == "mp3" || codec_lower == "mp4a.6b" {
      1152
    } else if codec_lower == "opus" {
      960 // 20ms at 48kHz
    } else if codec_lower == "flac" || codec_lower == "vorbis" {
      4096 // Variable, but use a reasonable default
    } else {
      1024 // Default
    }
  }

  /// Add samples to the buffer
  ///
  /// # Arguments
  /// * `samples` - Interleaved sample data
  /// * `num_samples` - Number of samples per channel
  pub fn add_samples(&mut self, samples: &[u8], num_samples: usize) -> CodecResult<()> {
    let sample_bytes = num_samples * self.channels as usize * self.bytes_per_sample;

    if samples.len() < sample_bytes {
      return Err(CodecError::InvalidConfig(
        "Sample data too short for specified sample count".into(),
      ));
    }

    let current_bytes = self.samples_in_buffer * self.channels as usize * self.bytes_per_sample;
    let new_total = current_bytes + sample_bytes;

    // Grow buffer if needed
    if new_total > self.buffer.len() {
      self.buffer.resize(new_total * 2, 0);
    }

    // Copy samples to buffer
    self.buffer[current_bytes..current_bytes + sample_bytes]
      .copy_from_slice(&samples[..sample_bytes]);
    self.samples_in_buffer += num_samples;

    Ok(())
  }

  /// Add samples from a Frame
  pub fn add_frame(&mut self, frame: &Frame) -> CodecResult<()> {
    if !frame.is_audio() {
      return Err(CodecError::InvalidConfig("Frame is not audio".into()));
    }

    let frame_format = frame.sample_format();
    let frame_channels = frame.channels();

    if frame_channels != self.channels {
      return Err(CodecError::InvalidConfig(format!(
        "Channel count mismatch: expected {}, got {}",
        self.channels, frame_channels
      )));
    }

    let nb_samples = frame.nb_samples() as usize;
    let sample_bytes = nb_samples * self.channels as usize * self.bytes_per_sample;

    // Handle format conversion if needed
    if frame_format.is_planar() && !self.format.is_planar() {
      // Need to interleave the data
      let mut interleaved = vec![0u8; sample_bytes];
      for sample in 0..nb_samples {
        for ch in 0..self.channels as usize {
          let src_offset = sample * self.bytes_per_sample;
          let dst_offset = (sample * self.channels as usize + ch) * self.bytes_per_sample;

          if let Some(ch_data) = frame.audio_channel_data(ch) {
            interleaved[dst_offset..dst_offset + self.bytes_per_sample]
              .copy_from_slice(&ch_data[src_offset..src_offset + self.bytes_per_sample]);
          }
        }
      }
      self.add_samples(&interleaved, nb_samples)
    } else if !frame_format.is_planar() {
      // Already interleaved
      if let Some(data) = frame.audio_channel_data(0) {
        self.add_samples(data, nb_samples)
      } else {
        Err(CodecError::InvalidState("No audio data in frame".into()))
      }
    } else {
      // Source is planar, target is planar - still need to interleave for buffer storage
      // (buffer always stores interleaved, deinterleaves when creating output frame)
      let mut interleaved = vec![0u8; sample_bytes];
      for sample in 0..nb_samples {
        for ch in 0..self.channels as usize {
          let src_offset = sample * self.bytes_per_sample;
          let dst_offset = (sample * self.channels as usize + ch) * self.bytes_per_sample;

          if let Some(ch_data) = frame.audio_channel_data(ch)
            && src_offset + self.bytes_per_sample <= ch_data.len()
          {
            interleaved[dst_offset..dst_offset + self.bytes_per_sample]
              .copy_from_slice(&ch_data[src_offset..src_offset + self.bytes_per_sample]);
          }
        }
      }
      self.add_samples(&interleaved, nb_samples)
    }
  }

  /// Check if there are enough samples for a full frame
  pub fn has_full_frame(&self) -> bool {
    self.samples_in_buffer >= self.frame_size
  }

  /// Get number of samples currently in buffer
  pub fn samples_available(&self) -> usize {
    self.samples_in_buffer
  }

  /// Get number of complete frames available
  pub fn frames_available(&self) -> usize {
    self.samples_in_buffer / self.frame_size
  }

  /// Take a full frame of samples from the buffer
  ///
  /// Returns None if there aren't enough samples for a full frame
  pub fn take_frame(&mut self) -> CodecResult<Option<Frame>> {
    if !self.has_full_frame() {
      return Ok(None);
    }

    // Create output frame
    let mut frame = Frame::new_audio(
      self.frame_size as u32,
      self.channels,
      self.sample_rate,
      self.format,
    )?;

    let frame_bytes = self.frame_size * self.channels as usize * self.bytes_per_sample;

    // Copy data to frame
    if self.format.is_planar() {
      // Need to deinterleave
      for ch in 0..self.channels as usize {
        if let Some(ch_data) = frame.audio_channel_data_mut(ch) {
          for sample in 0..self.frame_size {
            let src_offset = (sample * self.channels as usize + ch) * self.bytes_per_sample;
            let dst_offset = sample * self.bytes_per_sample;
            ch_data[dst_offset..dst_offset + self.bytes_per_sample]
              .copy_from_slice(&self.buffer[src_offset..src_offset + self.bytes_per_sample]);
          }
        }
      }
    } else {
      // Direct copy for interleaved
      if let Some(data) = frame.audio_channel_data_mut(0) {
        data[..frame_bytes].copy_from_slice(&self.buffer[..frame_bytes]);
      }
    }

    // Shift remaining samples to front of buffer
    let remaining_bytes =
      (self.samples_in_buffer - self.frame_size) * self.channels as usize * self.bytes_per_sample;

    if remaining_bytes > 0 {
      self
        .buffer
        .copy_within(frame_bytes..frame_bytes + remaining_bytes, 0);
    }
    self.samples_in_buffer -= self.frame_size;

    Ok(Some(frame))
  }

  /// Flush remaining samples as a partial frame
  ///
  /// Returns None if buffer is empty
  pub fn flush(&mut self) -> CodecResult<Option<Frame>> {
    if self.samples_in_buffer == 0 {
      return Ok(None);
    }

    // Create frame with remaining samples
    let mut frame = Frame::new_audio(
      self.samples_in_buffer as u32,
      self.channels,
      self.sample_rate,
      self.format,
    )?;

    let remaining_bytes = self.samples_in_buffer * self.channels as usize * self.bytes_per_sample;

    // Copy data to frame
    if self.format.is_planar() {
      for ch in 0..self.channels as usize {
        if let Some(ch_data) = frame.audio_channel_data_mut(ch) {
          for sample in 0..self.samples_in_buffer {
            let src_offset = (sample * self.channels as usize + ch) * self.bytes_per_sample;
            let dst_offset = sample * self.bytes_per_sample;
            if dst_offset + self.bytes_per_sample <= ch_data.len()
              && src_offset + self.bytes_per_sample <= self.buffer.len()
            {
              ch_data[dst_offset..dst_offset + self.bytes_per_sample]
                .copy_from_slice(&self.buffer[src_offset..src_offset + self.bytes_per_sample]);
            }
          }
        }
      }
    } else if let Some(data) = frame.audio_channel_data_mut(0) {
      data[..remaining_bytes].copy_from_slice(&self.buffer[..remaining_bytes]);
    }

    self.samples_in_buffer = 0;
    Ok(Some(frame))
  }

  /// Clear the buffer
  pub fn clear(&mut self) {
    self.samples_in_buffer = 0;
  }

  // ========================================================================
  // Accessors
  // ========================================================================

  /// Get frame size
  pub fn frame_size(&self) -> usize {
    self.frame_size
  }

  /// Get number of channels
  pub fn channels(&self) -> u32 {
    self.channels
  }

  /// Get sample rate
  pub fn sample_rate(&self) -> u32 {
    self.sample_rate
  }

  /// Get sample format
  pub fn format(&self) -> AVSampleFormat {
    self.format
  }
}

impl std::fmt::Debug for AudioSampleBuffer {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("AudioSampleBuffer")
      .field("frame_size", &self.frame_size)
      .field("channels", &self.channels)
      .field("sample_rate", &self.sample_rate)
      .field("format", &self.format)
      .field("samples_in_buffer", &self.samples_in_buffer)
      .field("frames_available", &self.frames_available())
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_buffer_creation() {
    let buffer = AudioSampleBuffer::new(1024, 2, 48000, AVSampleFormat::S16);
    assert_eq!(buffer.frame_size(), 1024);
    assert_eq!(buffer.channels(), 2);
    assert_eq!(buffer.sample_rate(), 48000);
    assert!(!buffer.has_full_frame());
  }

  #[test]
  fn test_frame_size_detection() {
    assert_eq!(AudioSampleBuffer::frame_size_for_codec("aac"), 1024);
    assert_eq!(AudioSampleBuffer::frame_size_for_codec("mp4a.40.2"), 1024);
    assert_eq!(AudioSampleBuffer::frame_size_for_codec("mp3"), 1152);
    assert_eq!(AudioSampleBuffer::frame_size_for_codec("opus"), 960);
  }
}
