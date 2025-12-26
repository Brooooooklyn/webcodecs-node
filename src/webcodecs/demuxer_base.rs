//! Demuxer Base - Shared types and traits for container demuxers
//!
//! This module provides common functionality for Mp4Demuxer, WebMDemuxer, and MkvDemuxer
//! to eliminate code duplication across the three implementations.

use crate::codec::demuxer::{DemuxerContext, MediaType, StreamInfo};
use crate::codec::io_buffer::BufferSource;
use crate::ffi::AVCodecID;
use crate::webcodecs::encoded_audio_chunk::{
  EncodedAudioChunk, EncodedAudioChunkInit, EncodedAudioChunkType,
};
use crate::webcodecs::encoded_video_chunk::{
  EncodedVideoChunk, EncodedVideoChunkInit, EncodedVideoChunkType,
};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::marker::PhantomData;

// ============================================================================
// BufferSource implementation for Uint8Array (zero-copy support)
// ============================================================================

/// Implement BufferSource for Uint8Array to enable zero-copy buffer loading.
///
/// This allows passing Uint8Array directly from JavaScript to the demuxer
/// without an intermediate copy to Vec<u8>.
impl BufferSource for Uint8Array {
  fn buffer_data(&self) -> (*const u8, usize) {
    let slice: &[u8] = self.as_ref();
    (slice.as_ptr(), slice.len())
  }
}

// ============================================================================
// Lock Helper Macros
// ============================================================================

/// Helper macro to acquire mutable lock, returning error on failure.
/// Use in methods that modify demuxer state.
macro_rules! with_demuxer_inner_mut {
  ($self:expr) => {
    $self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?
  };
}

/// Helper macro to acquire immutable lock, returning error on failure.
/// Use in methods that only read demuxer state.
macro_rules! with_demuxer_inner {
  ($self:expr) => {
    $self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?
  };
}

pub(crate) use with_demuxer_inner;
pub(crate) use with_demuxer_inner_mut;

// ============================================================================
// Callback Type Aliases
// ============================================================================

/// Type alias for video output callback
pub type VideoOutputCallback =
  ThreadsafeFunction<EncodedVideoChunk, UnknownReturnValue, EncodedVideoChunk, Status, false, true>;

/// Type alias for audio output callback
pub type AudioOutputCallback =
  ThreadsafeFunction<EncodedAudioChunk, UnknownReturnValue, EncodedAudioChunk, Status, false, true>;

/// Type alias for error callback
pub type ErrorCallback = ThreadsafeFunction<Error, UnknownReturnValue, Error, Status, false, true>;

// ============================================================================
// Shared State Types
// ============================================================================

/// Demuxer state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemuxerState {
  /// Initial state - not yet loaded
  Unloaded,
  /// File/buffer loaded, ready to demux
  Ready,
  /// Currently demuxing
  Demuxing,
  /// All packets read
  EndOfStream,
  /// Closed
  Closed,
}

impl DemuxerState {
  /// Convert state to string representation
  pub fn as_str(&self) -> &'static str {
    match self {
      DemuxerState::Unloaded => "unloaded",
      DemuxerState::Ready => "ready",
      DemuxerState::Demuxing => "demuxing",
      DemuxerState::EndOfStream => "ended",
      DemuxerState::Closed => "closed",
    }
  }
}

// ============================================================================
// JavaScript-facing Types
// ============================================================================

/// Track information exposed to JavaScript
#[napi(object)]
#[derive(Debug, Clone)]
pub struct DemuxerTrackInfo {
  /// Track index
  pub index: i32,
  /// Track type ("video" or "audio")
  pub track_type: String,
  /// Codec string (WebCodecs format)
  pub codec: String,
  /// Duration in microseconds
  pub duration: Option<i64>,
  /// Coded width (video only)
  pub coded_width: Option<u32>,
  /// Coded height (video only)
  pub coded_height: Option<u32>,
  /// Sample rate (audio only)
  pub sample_rate: Option<u32>,
  /// Number of channels (audio only)
  pub number_of_channels: Option<u32>,
}

/// Video decoder configuration exposed to JavaScript
#[napi(object)]
pub struct DemuxerVideoDecoderConfig {
  /// Codec string
  pub codec: String,
  /// Coded width
  pub coded_width: u32,
  /// Coded height
  pub coded_height: u32,
  /// Codec-specific description data (avcC/hvcC)
  pub description: Option<Uint8Array>,
}

/// Audio decoder configuration exposed to JavaScript
#[napi(object)]
pub struct DemuxerAudioDecoderConfig {
  /// Codec string
  pub codec: String,
  /// Sample rate
  pub sample_rate: u32,
  /// Number of channels
  pub number_of_channels: u32,
  /// Codec-specific description data
  pub description: Option<Uint8Array>,
}

// ============================================================================
// DemuxerFormat Trait - Format-specific behavior
// ============================================================================

/// Trait for format-specific demuxer behavior
pub trait DemuxerFormat: Send + Sync + 'static {
  /// Convert video codec ID to WebCodecs codec string
  ///
  /// The extradata parameter contains codec-specific configuration data
  /// (e.g., avcC for H.264, hvcC for HEVC) that can be parsed to extract
  /// profile/level information for more accurate codec strings.
  fn codec_id_to_video_string(codec_id: AVCodecID, extradata: Option<&[u8]>) -> String;

  /// Convert audio codec ID to WebCodecs codec string
  ///
  /// The extradata parameter contains codec-specific configuration data
  /// (e.g., AudioSpecificConfig for AAC) that can be parsed to extract
  /// profile information.
  fn codec_id_to_audio_string(codec_id: AVCodecID, extradata: Option<&[u8]>) -> String;
}

// ============================================================================
// DemuxerInner - Generic demuxer implementation
// ============================================================================

/// Internal state for generic demuxer
pub struct DemuxerInner<F: DemuxerFormat> {
  /// FFmpeg demuxer context
  pub demuxer: Option<DemuxerContext>,
  /// Current state
  pub state: DemuxerState,
  /// Parsed track information
  pub tracks: Vec<DemuxerTrackInfo>,
  /// Selected video track index
  pub selected_video_track: Option<i32>,
  /// Selected audio track index
  pub selected_audio_track: Option<i32>,
  /// Video output callback
  pub video_callback: Option<VideoOutputCallback>,
  /// Audio output callback
  pub audio_callback: Option<AudioOutputCallback>,
  /// Error callback
  pub error_callback: Option<ErrorCallback>,
  /// Phantom data for format type
  _format: PhantomData<F>,
}

impl<F: DemuxerFormat> DemuxerInner<F> {
  /// Create a new demuxer inner state
  pub fn new(
    video_callback: Option<VideoOutputCallback>,
    audio_callback: Option<AudioOutputCallback>,
    error_callback: ErrorCallback,
  ) -> Self {
    Self {
      demuxer: None,
      state: DemuxerState::Unloaded,
      tracks: Vec::new(),
      selected_video_track: None,
      selected_audio_track: None,
      video_callback,
      audio_callback,
      error_callback: Some(error_callback),
      _format: PhantomData,
    }
  }

  /// Load from a file path
  pub fn load_file(&mut self, path: &str) -> Result<()> {
    if self.state != DemuxerState::Unloaded {
      return Err(Error::new(
        Status::GenericFailure,
        "Demuxer already loaded. Call close() first.",
      ));
    }

    let demuxer = DemuxerContext::open_file(path).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to open file: {}", e),
      )
    })?;

    self.finish_load(demuxer);
    Ok(())
  }

  /// Load from a buffer
  ///
  /// This method accepts any type implementing `BufferSource`, enabling
  /// zero-copy buffer loading from `Uint8Array` without intermediate copies.
  pub fn load_buffer(&mut self, source: impl BufferSource + 'static) -> Result<()> {
    if self.state != DemuxerState::Unloaded {
      return Err(Error::new(
        Status::GenericFailure,
        "Demuxer already loaded. Call close() first.",
      ));
    }

    let demuxer = DemuxerContext::open_buffer(source).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to open buffer: {}", e),
      )
    })?;

    self.finish_load(demuxer);
    Ok(())
  }

  /// Complete the load process (shared between file and buffer loading)
  fn finish_load(&mut self, demuxer: DemuxerContext) {
    // Parse track info using format-specific codec string conversion
    let tracks = parse_tracks::<F>(demuxer.streams());

    // Select first video and audio tracks by default
    let selected_video_track = tracks
      .iter()
      .find(|t| t.track_type == "video")
      .map(|t| t.index);
    let selected_audio_track = tracks
      .iter()
      .find(|t| t.track_type == "audio")
      .map(|t| t.index);

    self.demuxer = Some(demuxer);
    self.tracks = tracks;
    self.selected_video_track = selected_video_track;
    self.selected_audio_track = selected_audio_track;
    self.state = DemuxerState::Ready;
  }

  /// Get all tracks
  pub fn get_tracks(&self) -> Vec<DemuxerTrackInfo> {
    self.tracks.clone()
  }

  /// Get container duration in microseconds
  pub fn get_duration(&self) -> Option<i64> {
    self.demuxer.as_ref().and_then(|d| d.duration_us())
  }

  /// Get video decoder configuration for the selected video track
  pub fn get_video_decoder_config(&self) -> Option<DemuxerVideoDecoderConfig> {
    let demuxer = self.demuxer.as_ref()?;
    let video_index = self.selected_video_track?;

    demuxer.get_stream(video_index).map(|s| {
      let codec = F::codec_id_to_video_string(s.codec_id, s.extradata.as_deref());
      let description = s.extradata.as_ref().map(|d| Uint8Array::new(d.clone()));

      DemuxerVideoDecoderConfig {
        codec,
        coded_width: s.width.unwrap_or(0),
        coded_height: s.height.unwrap_or(0),
        description,
      }
    })
  }

  /// Get audio decoder configuration for the selected audio track
  pub fn get_audio_decoder_config(&self) -> Option<DemuxerAudioDecoderConfig> {
    let demuxer = self.demuxer.as_ref()?;
    let audio_index = self.selected_audio_track?;

    demuxer.get_stream(audio_index).map(|s| {
      let codec = F::codec_id_to_audio_string(s.codec_id, s.extradata.as_deref());
      let description = s.extradata.as_ref().map(|d| Uint8Array::new(d.clone()));

      DemuxerAudioDecoderConfig {
        codec,
        sample_rate: s.sample_rate.unwrap_or(0),
        number_of_channels: s.channels.unwrap_or(0),
        description,
      }
    })
  }

  /// Select a video track by index
  pub fn select_video_track(&mut self, track_index: i32) -> Result<()> {
    let track = self.tracks.iter().find(|t| t.index == track_index);
    match track {
      Some(t) if t.track_type == "video" => {
        self.selected_video_track = Some(track_index);
        Ok(())
      }
      Some(_) => Err(Error::new(
        Status::GenericFailure,
        format!("Track {} is not a video track", track_index),
      )),
      None => Err(Error::new(
        Status::GenericFailure,
        format!("Track {} not found", track_index),
      )),
    }
  }

  /// Select an audio track by index
  pub fn select_audio_track(&mut self, track_index: i32) -> Result<()> {
    let track = self.tracks.iter().find(|t| t.index == track_index);
    match track {
      Some(t) if t.track_type == "audio" => {
        self.selected_audio_track = Some(track_index);
        Ok(())
      }
      Some(_) => Err(Error::new(
        Status::GenericFailure,
        format!("Track {} is not an audio track", track_index),
      )),
      None => Err(Error::new(
        Status::GenericFailure,
        format!("Track {} not found", track_index),
      )),
    }
  }

  /// Demux packets (runs in the calling thread context)
  ///
  /// This method should be called from within a spawned thread.
  /// It holds the mutex for the duration of the demux operation.
  pub fn demux_sync(&mut self, max_packets: u32) {
    if self.state != DemuxerState::Ready && self.state != DemuxerState::Demuxing {
      if let Some(ref error_cb) = self.error_callback {
        let _ = error_cb.call(
          Error::new(
            Status::GenericFailure,
            "Demuxer is not ready. Call load() first.",
          ),
          ThreadsafeFunctionCallMode::NonBlocking,
        );
      }
      return;
    }

    self.state = DemuxerState::Demuxing;

    let video_index = self.selected_video_track;
    let audio_index = self.selected_audio_track;
    let mut packets_read = 0u32;

    // Get stream time bases for timestamp conversion
    let video_time_base = video_index.and_then(|idx| {
      self
        .demuxer
        .as_ref()
        .and_then(|d| d.get_stream(idx).map(|s| s.time_base))
    });
    let audio_time_base = audio_index.and_then(|idx| {
      self
        .demuxer
        .as_ref()
        .and_then(|d| d.get_stream(idx).map(|s| s.time_base))
    });

    while packets_read < max_packets {
      let demuxer = match self.demuxer.as_mut() {
        Some(d) => d,
        None => break,
      };

      match demuxer.read_packet() {
        Ok(Some((packet, stream_index))) => {
          if Some(stream_index) == video_index {
            // Process video packet
            let timestamp = convert_timestamp(packet.pts(), video_time_base);
            let duration = if packet.duration() > 0 {
              Some(convert_timestamp(packet.duration(), video_time_base))
            } else {
              None
            };

            let chunk_type = if packet.is_key() {
              EncodedVideoChunkType::Key
            } else {
              EncodedVideoChunkType::Delta
            };

            let init = EncodedVideoChunkInit {
              chunk_type,
              timestamp,
              duration,
              data: Either::B(packet),
            };

            match EncodedVideoChunk::new(init) {
              Ok(chunk) => {
                if let Some(ref cb) = self.video_callback {
                  let _ = cb.call(chunk, ThreadsafeFunctionCallMode::NonBlocking);
                }
              }
              Err(e) => {
                if let Some(ref err_cb) = self.error_callback {
                  let _ = err_cb.call(
                    Error::new(
                      Status::GenericFailure,
                      format!("Failed to create video chunk: {}", e),
                    ),
                    ThreadsafeFunctionCallMode::NonBlocking,
                  );
                }
              }
            }
          } else if Some(stream_index) == audio_index {
            // Process audio packet
            let timestamp = convert_timestamp(packet.pts(), audio_time_base);
            let duration = if packet.duration() > 0 {
              Some(convert_timestamp(packet.duration(), audio_time_base))
            } else {
              None
            };

            let init = EncodedAudioChunkInit {
              chunk_type: EncodedAudioChunkType::Key, // Audio packets are typically keyframes
              timestamp,
              duration,
              data: Either::B(packet),
            };

            match EncodedAudioChunk::new(init) {
              Ok(chunk) => {
                if let Some(ref cb) = self.audio_callback {
                  let _ = cb.call(chunk, ThreadsafeFunctionCallMode::NonBlocking);
                }
              }
              Err(e) => {
                if let Some(ref err_cb) = self.error_callback {
                  let _ = err_cb.call(
                    Error::new(
                      Status::GenericFailure,
                      format!("Failed to create audio chunk: {}", e),
                    ),
                    ThreadsafeFunctionCallMode::NonBlocking,
                  );
                }
              }
            }
          }
          // Ignore packets from other tracks

          packets_read += 1;
        }
        Ok(None) => {
          // End of stream
          self.state = DemuxerState::EndOfStream;
          break;
        }
        Err(e) => {
          if let Some(ref err_cb) = self.error_callback {
            let _ = err_cb.call(
              Error::new(Status::GenericFailure, format!("Demuxer error: {}", e)),
              ThreadsafeFunctionCallMode::NonBlocking,
            );
          }
          break;
        }
      }
    }
  }

  /// Seek to a timestamp in microseconds
  pub fn seek(&mut self, timestamp_us: i64) -> Result<()> {
    let stream_index = self.selected_video_track.unwrap_or(-1);

    let demuxer = self
      .demuxer
      .as_mut()
      .ok_or_else(|| Error::new(Status::GenericFailure, "Demuxer not loaded"))?;

    // Convert microseconds to stream time base if a specific stream is selected
    // FFmpeg's av_seek_frame expects:
    // - AV_TIME_BASE (microseconds) when stream_index is -1
    // - Stream time base units when stream_index >= 0
    let timestamp = if stream_index >= 0 {
      if let Some(stream) = demuxer.get_stream(stream_index) {
        let (num, den) = stream.time_base;
        if num > 0 && den > 0 {
          // Convert: timestamp_us * den / (1_000_000 * num)
          // Use i128 to avoid overflow for large timestamps
          let ts = (timestamp_us as i128) * (den as i128) / (1_000_000i128 * num as i128);
          ts as i64
        } else {
          timestamp_us
        }
      } else {
        timestamp_us
      }
    } else {
      // stream_index is -1, FFmpeg uses AV_TIME_BASE (microseconds)
      timestamp_us
    };

    demuxer
      .seek(stream_index, timestamp, true)
      .map_err(|e| Error::new(Status::GenericFailure, format!("Seek failed: {}", e)))?;

    // Reset state to ready for more demuxing
    if self.state == DemuxerState::EndOfStream {
      self.state = DemuxerState::Ready;
    }

    Ok(())
  }

  /// Close the demuxer and release resources
  pub fn close(&mut self) {
    self.demuxer = None;
    self.tracks.clear();
    self.selected_video_track = None;
    self.selected_audio_track = None;
    self.state = DemuxerState::Closed;
  }

  /// Get current state as string
  pub fn state_string(&self) -> &'static str {
    self.state.as_str()
  }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parse stream info into track info using format-specific codec conversion
fn parse_tracks<F: DemuxerFormat>(streams: &[StreamInfo]) -> Vec<DemuxerTrackInfo> {
  streams
    .iter()
    .map(|s| {
      let track_type = match s.media_type {
        MediaType::Video => "video".to_string(),
        MediaType::Audio => "audio".to_string(),
        MediaType::Subtitle => "subtitle".to_string(),
        MediaType::Data => "data".to_string(),
      };

      let codec = if s.media_type == MediaType::Video {
        F::codec_id_to_video_string(s.codec_id, s.extradata.as_deref())
      } else {
        F::codec_id_to_audio_string(s.codec_id, s.extradata.as_deref())
      };

      // Calculate duration in microseconds from stream duration and time base
      // Use checked arithmetic to prevent overflow
      let duration = s.duration.and_then(|d| {
        let (num, den) = s.time_base;
        if den != 0 {
          // d * 1_000_000 * num / den - use checked arithmetic
          d.checked_mul(1_000_000)
            .and_then(|v| v.checked_mul(num as i64))
            .map(|v| v / den as i64)
        } else {
          Some(d)
        }
      });

      DemuxerTrackInfo {
        index: s.index,
        track_type,
        codec,
        duration,
        coded_width: s.width,
        coded_height: s.height,
        sample_rate: s.sample_rate,
        number_of_channels: s.channels,
      }
    })
    .collect()
}

/// Convert timestamp from stream time base to microseconds
///
/// Uses checked arithmetic to prevent overflow for large timestamps.
/// On overflow, saturates to i64::MAX or i64::MIN based on sign.
pub fn convert_timestamp(ts: i64, time_base: Option<(i32, i32)>) -> i64 {
  match time_base {
    Some((num, den)) if den != 0 => {
      // ts * 1_000_000 * num / den - use checked arithmetic
      ts.checked_mul(1_000_000)
        .and_then(|v| v.checked_mul(num as i64))
        .map(|v| v / den as i64)
        .unwrap_or_else(|| {
          tracing::warn!(target: "webcodecs", "Timestamp overflow during conversion, saturating");
          if ts > 0 { i64::MAX } else { i64::MIN }
        })
    }
    _ => ts, // Assume already in microseconds
  }
}

// ============================================================================
// Common Codec String Parsing Functions
// ============================================================================

/// Parse H.264 avcC extradata to generate codec string
///
/// avcC format: [version, profile_idc, profile_compat, level_idc, ...]
pub fn parse_h264_codec_string(extradata: Option<&[u8]>) -> String {
  if let Some(data) = extradata.filter(|d| d.len() >= 4 && d[0] == 1) {
    // avcC format: version(1), profile(1), compat(1), level(1)
    let profile = data[1];
    let compat = data[2];
    let level = data[3];
    return format!("avc1.{:02X}{:02X}{:02X}", profile, compat, level);
  }
  "avc1.42001E".to_string() // Default: Baseline profile, level 3.0
}

/// Parse HEVC hvcC extradata to generate codec string
///
/// hvcC format: [configurationVersion, general_profile_space/tier_flag/profile_idc, ...]
pub fn parse_hevc_codec_string(extradata: Option<&[u8]>) -> String {
  // hvcC structure: configurationVersion (1) + general_profile_space/tier/idc (1) + ...
  // Minimum 23 bytes for full header, but we can parse partial
  if let Some(data) = extradata.filter(|d| d.len() >= 13 && d[0] == 1) {
    // Byte 1: general_profile_space (2 bits) | general_tier_flag (1 bit) | general_profile_idc (5 bits)
    let general_profile_idc = data[1] & 0x1F;
    let general_tier_flag = (data[1] >> 5) & 0x01;
    // Byte 12 (index 12): general_level_idc
    let general_level_idc = data[12];
    let tier = if general_tier_flag == 1 { "H" } else { "L" };
    return format!(
      "hev1.{}.6.{}{}.B0",
      general_profile_idc, tier, general_level_idc
    );
  }
  "hev1.1.6.L93.B0".to_string() // Default fallback
}

/// Parse VP9 extradata to generate codec string
///
/// VP9CodecConfigurationRecord: [profile, level, bitDepth, ...]
pub fn parse_vp9_codec_string(extradata: Option<&[u8]>) -> String {
  // VP9CodecConfigurationRecord in MP4 (vpcC box):
  // version (1) + flags (3) + profile (1) + level (1) + bitDepth (4 bits) + ...
  if let Some(data) = extradata.filter(|d| d.len() >= 8) {
    let profile = data[4];
    let level = data[5];
    // bitDepth is in the high 4 bits of byte 6
    let bit_depth = (data[6] >> 4) & 0x0F;
    // Use 08/10/12 bit depth encoding
    let bit_depth_code = match bit_depth {
      8 => 8,
      10 => 10,
      12 => 12,
      _ => 8, // default to 8-bit
    };
    return format!("vp09.{:02}.{:02}.{:02}", profile, level, bit_depth_code);
  }
  "vp09.00.10.08".to_string() // Default: Profile 0, level 1.0, 8-bit
}

/// Parse AAC AudioSpecificConfig to generate codec string
///
/// AudioSpecificConfig: first 5 bits are audioObjectType
pub fn parse_aac_codec_string(extradata: Option<&[u8]>) -> String {
  // AudioSpecificConfig: audioObjectType in first 5 bits
  if let Some(data) = extradata.filter(|d| !d.is_empty()) {
    let object_type = (data[0] >> 3) & 0x1F;
    return match object_type {
      2 => "mp4a.40.2".to_string(),   // AAC-LC
      5 => "mp4a.40.5".to_string(),   // HE-AAC (SBR)
      29 => "mp4a.40.29".to_string(), // HE-AAC v2 (PS)
      _ => format!("mp4a.40.{}", object_type),
    };
  }
  "mp4a.40.2".to_string() // Default: AAC-LC
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_convert_timestamp_normal() {
    // 30fps: time_base = 1/30, so 90 ticks = 3 seconds = 3_000_000 us
    assert_eq!(convert_timestamp(90, Some((1, 30))), 3_000_000);
  }

  #[test]
  fn test_convert_timestamp_microseconds() {
    // Already in microseconds (1/1000000)
    assert_eq!(convert_timestamp(1_000_000, Some((1, 1_000_000))), 1);
  }

  #[test]
  fn test_convert_timestamp_overflow_protection() {
    // Large timestamp that would overflow without checked arithmetic
    // i64::MAX / 1_000_000 is still huge, but this tests the path
    let result = convert_timestamp(i64::MAX / 1000, Some((1, 1)));
    // Should return original on overflow
    assert_eq!(result, i64::MAX / 1000);
  }

  #[test]
  fn test_convert_timestamp_zero_denominator() {
    // Zero denominator should return original
    assert_eq!(convert_timestamp(1000, Some((1, 0))), 1000);
  }

  #[test]
  fn test_parse_h264_codec_string() {
    // Valid avcC with High profile, level 4.0
    let extradata = vec![1, 0x64, 0x00, 0x28]; // version, profile=100, compat=0, level=40
    assert_eq!(parse_h264_codec_string(Some(&extradata)), "avc1.640028");

    // Invalid extradata
    assert_eq!(parse_h264_codec_string(None), "avc1.42001E");
    assert_eq!(parse_h264_codec_string(Some(&[0, 0, 0, 0])), "avc1.42001E");
  }

  #[test]
  fn test_parse_hevc_codec_string() {
    // Default without extradata
    assert_eq!(parse_hevc_codec_string(None), "hev1.1.6.L93.B0");
  }

  #[test]
  fn test_parse_vp9_codec_string() {
    // Default without extradata
    assert_eq!(parse_vp9_codec_string(None), "vp09.00.10.08");
  }

  #[test]
  fn test_parse_aac_codec_string() {
    // AAC-LC (object type 2)
    let aac_lc = vec![0x10]; // 0x10 >> 3 = 2
    assert_eq!(parse_aac_codec_string(Some(&aac_lc)), "mp4a.40.2");

    // HE-AAC (object type 5)
    let he_aac = vec![0x28]; // 0x28 >> 3 = 5
    assert_eq!(parse_aac_codec_string(Some(&he_aac)), "mp4a.40.5");

    // HE-AAC v2 (object type 29)
    let he_aac_v2 = vec![0xE8]; // 0xE8 >> 3 = 29
    assert_eq!(parse_aac_codec_string(Some(&he_aac_v2)), "mp4a.40.29");

    // Default
    assert_eq!(parse_aac_codec_string(None), "mp4a.40.2");
  }

  #[test]
  fn test_demuxer_state_strings() {
    assert_eq!(DemuxerState::Unloaded.as_str(), "unloaded");
    assert_eq!(DemuxerState::Ready.as_str(), "ready");
    assert_eq!(DemuxerState::Demuxing.as_str(), "demuxing");
    assert_eq!(DemuxerState::EndOfStream.as_str(), "ended");
    assert_eq!(DemuxerState::Closed.as_str(), "closed");
  }
}
