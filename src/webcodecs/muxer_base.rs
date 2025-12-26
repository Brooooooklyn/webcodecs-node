//! Muxer Base - Shared types and traits for container muxers
//!
//! This module provides common functionality for Mp4Muxer, WebMMuxer, and MkvMuxer
//! to eliminate code duplication across the three implementations.

use crate::codec::io_buffer::StreamingBufferHandle;
use crate::codec::muxer::{
  AudioStreamConfig, ContainerFormat, MuxerContext, MuxerOptions, MuxerOutput, VideoStreamConfig,
};
use crate::ffi::{AVCodecID, AVPixelFormat, AVRational, AVSampleFormat};
use crate::webcodecs::encoded_audio_chunk::EncodedAudioChunk;
use crate::webcodecs::encoded_video_chunk::{EncodedVideoChunk, EncodedVideoChunkType};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::marker::PhantomData;

// ============================================================================
// Lock Helper Macros
// ============================================================================

/// Helper macro to acquire mutable lock and unwrap inner, declaring bindings in caller's scope.
/// Use in methods that modify muxer state.
///
/// Usage: `lock_muxer_inner_mut!(self => guard, inner);`
macro_rules! lock_muxer_inner_mut {
  ($self:expr => $guard:ident, $inner:ident) => {
    let mut $guard = $self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    let $inner = $guard
      .as_mut()
      .ok_or_else(|| Error::new(Status::GenericFailure, "Muxer is closed"))?;
  };
}

/// Helper macro to acquire immutable lock and unwrap inner, declaring bindings in caller's scope.
/// Use in methods that only read muxer state.
///
/// Usage: `lock_muxer_inner!(self => guard, inner);`
macro_rules! lock_muxer_inner {
  ($self:expr => $guard:ident, $inner:ident) => {
    let $guard = $self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    let $inner = $guard
      .as_ref()
      .ok_or_else(|| Error::new(Status::GenericFailure, "Muxer is closed"))?;
  };
}

pub(crate) use lock_muxer_inner;
pub(crate) use lock_muxer_inner_mut;

// ============================================================================
// Shared State Types
// ============================================================================

/// Muxer state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuxerState {
  /// Initial state - tracks can be added
  ConfiguringTracks,
  /// Tracks configured, accepting chunks
  Muxing,
  /// Finalized - no more operations allowed
  Finalized,
  /// Closed
  Closed,
}

impl MuxerState {
  /// Convert state to string representation
  pub fn as_str(&self) -> &'static str {
    match self {
      MuxerState::ConfiguringTracks => "configuring",
      MuxerState::Muxing => "muxing",
      MuxerState::Finalized => "finalized",
      MuxerState::Closed => "closed",
    }
  }
}

/// Stored video track info (extracted from config)
#[derive(Debug, Clone)]
pub struct StoredVideoTrackInfo {
  pub codec: String,
  pub width: u32,
  pub height: u32,
}

/// Stored audio track info (extracted from config)
#[derive(Debug, Clone)]
pub struct StoredAudioTrackInfo {
  pub codec: String,
  pub sample_rate: u32,
  pub channels: u32,
}

// ============================================================================
// JavaScript-facing Metadata Types (shared across all muxers)
// ============================================================================

/// JavaScript-facing metadata type for video chunks
#[napi(object)]
#[derive(Default)]
pub struct EncodedVideoChunkMetadataJs {
  /// Decoder configuration from encoder
  pub decoder_config: Option<VideoDecoderConfigJs>,
  /// SVC output metadata
  pub svc: Option<SvcOutputMetadataJs>,
  /// Alpha channel side data (for VP9 alpha support)
  /// This contains the encoded alpha channel data that should be written
  /// as BlockAdditions in WebM/MKV containers.
  pub alpha_side_data: Option<Uint8Array>,
}

/// JavaScript-facing decoder config type
#[napi(object)]
#[derive(Default)]
pub struct VideoDecoderConfigJs {
  /// Codec string
  pub codec: Option<String>,
  /// Codec-specific description
  pub description: Option<Uint8Array>,
  /// Coded width
  pub coded_width: Option<u32>,
  /// Coded height
  pub coded_height: Option<u32>,
}

/// JavaScript-facing SVC metadata
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct SvcOutputMetadataJs {
  /// Temporal layer ID
  pub temporal_layer_id: Option<u32>,
}

/// JavaScript-facing metadata type for audio chunks
#[napi(object)]
#[derive(Default)]
pub struct EncodedAudioChunkMetadataJs {
  /// Decoder configuration from encoder
  pub decoder_config: Option<AudioDecoderConfigJs>,
}

/// JavaScript-facing audio decoder config type
#[napi(object)]
#[derive(Default)]
pub struct AudioDecoderConfigJs {
  /// Codec string
  pub codec: Option<String>,
  /// Sample rate
  pub sample_rate: Option<u32>,
  /// Number of channels
  pub number_of_channels: Option<u32>,
  /// Codec-specific description
  pub description: Option<Uint8Array>,
}

// ============================================================================
// Streaming Options
// ============================================================================

/// Streaming mode options for muxers
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct StreamingMuxerOptions {
  /// Buffer capacity for streaming output (default: 256KB)
  pub buffer_capacity: Option<u32>,
}

// ============================================================================
// Generic Track Config (used by base implementation)
// ============================================================================

/// Generic video track configuration passed to base implementation
pub struct GenericVideoTrackConfig {
  pub codec: String,
  pub codec_id: AVCodecID,
  pub width: u32,
  pub height: u32,
  pub extradata: Option<Vec<u8>>,
  /// Whether this track has alpha channel (VP9 alpha support)
  pub has_alpha: bool,
}

/// Generic audio track configuration passed to base implementation
pub struct GenericAudioTrackConfig {
  pub codec: String,
  pub codec_id: AVCodecID,
  pub sample_rate: u32,
  pub channels: u32,
  pub frame_size: Option<u32>,
  pub extradata: Option<Vec<u8>>,
}

// ============================================================================
// MuxerFormat Trait - Format-specific behavior
// ============================================================================

/// Trait for format-specific muxer behavior
pub trait MuxerFormat: Send + Sync + 'static {
  /// Container format for this muxer
  const FORMAT: ContainerFormat;

  /// Get default muxer options for this format
  fn default_muxer_options() -> MuxerOptions {
    MuxerOptions::default()
  }

  /// Parse video codec string to AVCodecID
  fn parse_video_codec(codec: &str) -> Result<AVCodecID>;

  /// Parse audio codec string to AVCodecID
  fn parse_audio_codec(codec: &str) -> Result<AVCodecID>;

  /// Get audio frame size for a codec (if known)
  fn get_audio_frame_size(codec_id: AVCodecID) -> Option<u32> {
    match codec_id {
      AVCodecID::Aac => Some(1024),
      AVCodecID::Opus => Some(960), // 20ms at 48kHz
      AVCodecID::Mp3 => Some(1152),
      _ => None,
    }
  }
}

// ============================================================================
// MuxerInner - Generic muxer implementation
// ============================================================================

/// Internal state for generic muxer
pub struct MuxerInner<F: MuxerFormat> {
  /// FFmpeg muxer context
  pub muxer: MuxerContext,
  /// Current state
  pub state: MuxerState,
  /// Stored video track info
  pub video_track_info: Option<StoredVideoTrackInfo>,
  /// Stored audio track info
  pub audio_track_info: Option<StoredAudioTrackInfo>,
  /// Streaming buffer handle (for streaming mode)
  pub streaming_handle: Option<StreamingBufferHandle>,
  /// Whether streaming mode is enabled
  pub is_streaming: bool,
  /// Format-specific options holder
  pub muxer_options: MuxerOptions,
  /// Last video PTS written (to ensure monotonically increasing)
  last_video_pts: i64,
  /// Last audio PTS written (to ensure monotonically increasing)
  last_audio_pts: i64,
  /// Phantom data for format type
  _format: PhantomData<F>,
}

impl<F: MuxerFormat> MuxerInner<F> {
  /// Create a new muxer with buffer output mode
  pub fn new_buffer(options: MuxerOptions) -> Result<Self> {
    let muxer = MuxerContext::new(F::FORMAT, MuxerOutput::Buffer).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create muxer: {}", e),
      )
    })?;

    Ok(Self {
      muxer,
      state: MuxerState::ConfiguringTracks,
      video_track_info: None,
      audio_track_info: None,
      streaming_handle: None,
      is_streaming: false,
      muxer_options: options,
      last_video_pts: -1,
      last_audio_pts: -1,
      _format: PhantomData,
    })
  }

  /// Create a new muxer with streaming output mode
  pub fn new_streaming(options: MuxerOptions, buffer_capacity: usize) -> Result<Self> {
    let muxer =
      MuxerContext::new(F::FORMAT, MuxerOutput::Streaming(buffer_capacity)).map_err(|e| {
        Error::new(
          Status::GenericFailure,
          format!("Failed to create muxer: {}", e),
        )
      })?;

    // Get the streaming handle
    let streaming_handle = muxer.get_streaming_handle();

    Ok(Self {
      muxer,
      state: MuxerState::ConfiguringTracks,
      video_track_info: None,
      audio_track_info: None,
      streaming_handle,
      is_streaming: true,
      muxer_options: options,
      last_video_pts: -1,
      last_audio_pts: -1,
      _format: PhantomData,
    })
  }

  /// Add a video track to the muxer
  pub fn add_video_track(&mut self, config: GenericVideoTrackConfig) -> Result<()> {
    if self.state != MuxerState::ConfiguringTracks {
      return Err(Error::new(
        Status::GenericFailure,
        "Cannot add track after muxing has started",
      ));
    }

    if self.video_track_info.is_some() {
      return Err(Error::new(
        Status::GenericFailure,
        "Video track already added",
      ));
    }

    // Use YUVA420P for VP9 with alpha, otherwise use YUV420P
    let pixel_format = if config.has_alpha && config.codec_id == AVCodecID::Vp9 {
      AVPixelFormat::Yuva420p
    } else {
      AVPixelFormat::Yuv420p
    };

    // Create video stream config
    let stream_config = VideoStreamConfig {
      codec_id: config.codec_id,
      width: config.width,
      height: config.height,
      pixel_format,
      time_base: AVRational::MICROSECONDS,
      bitrate: None,
      extradata: config.extradata,
    };

    self.muxer.add_video_stream(&stream_config).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to add video stream: {}", e),
      )
    })?;

    self.video_track_info = Some(StoredVideoTrackInfo {
      codec: config.codec,
      width: config.width,
      height: config.height,
    });

    Ok(())
  }

  /// Add an audio track to the muxer
  pub fn add_audio_track(&mut self, config: GenericAudioTrackConfig) -> Result<()> {
    if self.state != MuxerState::ConfiguringTracks {
      return Err(Error::new(
        Status::GenericFailure,
        "Cannot add track after muxing has started",
      ));
    }

    if self.audio_track_info.is_some() {
      return Err(Error::new(
        Status::GenericFailure,
        "Audio track already added",
      ));
    }

    // Create audio stream config
    let stream_config = AudioStreamConfig {
      codec_id: config.codec_id,
      sample_rate: config.sample_rate,
      channels: config.channels,
      sample_format: AVSampleFormat::Fltp,
      time_base: AVRational::new(1, config.sample_rate as i32),
      bitrate: None,
      frame_size: config.frame_size,
      extradata: config.extradata,
    };

    self.muxer.add_audio_stream(&stream_config).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to add audio stream: {}", e),
      )
    })?;

    self.audio_track_info = Some(StoredAudioTrackInfo {
      codec: config.codec,
      sample_rate: config.sample_rate,
      channels: config.channels,
    });

    Ok(())
  }

  /// Ensure header is written, transitioning state if needed
  fn ensure_header_written(&mut self) -> Result<()> {
    if self.state == MuxerState::ConfiguringTracks {
      self
        .muxer
        .write_header(Some(&self.muxer_options))
        .map_err(|e| {
          Error::new(
            Status::GenericFailure,
            format!("Failed to write header: {}", e),
          )
        })?;
      self.state = MuxerState::Muxing;
    }
    Ok(())
  }

  /// Add an encoded video chunk to the muxer
  pub fn add_video_chunk(
    &mut self,
    chunk: &EncodedVideoChunk,
    metadata: Option<&EncodedVideoChunkMetadataJs>,
  ) -> Result<()> {
    // Ensure we have a video track
    let video_index = self
      .muxer
      .video_stream_index()
      .ok_or_else(|| Error::new(Status::GenericFailure, "No video track added"))?;

    // Write header if needed
    self.ensure_header_written()?;

    if self.state != MuxerState::Muxing {
      return Err(Error::new(
        Status::GenericFailure,
        "Muxer is not in muxing state",
      ));
    }

    // Get chunk data and metadata
    let chunk_type = chunk.chunk_type()?;
    let timestamp = chunk.timestamp()?;
    let duration = chunk.duration()?;

    // Get packet using optimized path:
    // - If chunk has Packet (from encoder): shallow_clone shares buffer (zero-copy)
    // - If chunk has Vec<u8> (from JS): copy data into new packet
    let mut packet = chunk.get_packet_for_muxing()?;

    // Set packet properties
    packet.set_stream_index(video_index);

    // Ensure monotonically increasing PTS (video time base is microseconds)
    let pts = if timestamp <= self.last_video_pts {
      self.last_video_pts + 1
    } else {
      timestamp
    };
    self.last_video_pts = pts;

    packet.set_pts(pts);
    packet.set_dts(pts);
    if let Some(dur) = duration {
      packet.set_duration(dur);
    }

    // Set keyframe flag
    if chunk_type == EncodedVideoChunkType::Key {
      packet.set_flags(crate::ffi::pkt_flag::KEY);
    }

    // Handle metadata - extract description if present
    if let Some(description) = metadata
      .as_ref()
      .and_then(|m| m.decoder_config.as_ref())
      .and_then(|c| c.description.as_ref())
    {
      let desc_data: &[u8] = description;
      if !desc_data.is_empty() {
        // Update extradata dynamically if available
        if let Err(e) = self.muxer.update_video_extradata(desc_data) {
          tracing::warn!(target: "webcodecs", "Failed to update video extradata: {}", e);
        }
      }
    }

    // Handle alpha side data for VP9 alpha support
    // This adds the alpha channel data as BlockAdditional side data
    if let Some(alpha_data) = metadata.as_ref().and_then(|m| m.alpha_side_data.as_ref()) {
      let alpha_bytes: &[u8] = alpha_data;
      if !alpha_bytes.is_empty()
        && let Err(e) = packet.add_matroska_blockadditional(alpha_bytes)
      {
        tracing::warn!(target: "webcodecs", "Failed to add alpha side data: {}", e);
      }
    }

    // Write packet
    self.muxer.write_packet(&mut packet).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to write packet: {}", e),
      )
    })?;

    Ok(())
  }

  /// Add an encoded audio chunk to the muxer
  pub fn add_audio_chunk(
    &mut self,
    chunk: &EncodedAudioChunk,
    metadata: Option<&EncodedAudioChunkMetadataJs>,
  ) -> Result<()> {
    // Ensure we have an audio track
    let audio_index = self
      .muxer
      .audio_stream_index()
      .ok_or_else(|| Error::new(Status::GenericFailure, "No audio track added"))?;

    // Write header if needed
    self.ensure_header_written()?;

    if self.state != MuxerState::Muxing {
      return Err(Error::new(
        Status::GenericFailure,
        "Muxer is not in muxing state",
      ));
    }

    // Get chunk data
    let timestamp = chunk.timestamp()?;
    let duration = chunk.duration()?;

    // Get packet using optimized path:
    // - If chunk has Packet (from encoder): shallow_clone shares buffer (zero-copy)
    // - If chunk has Vec<u8> (from JS): copy data into new packet
    let mut packet = chunk.get_packet_for_muxing()?;

    // Set packet properties
    packet.set_stream_index(audio_index);

    // Convert timestamp from microseconds to audio time base (1/sample_rate)
    let sample_rate = self
      .audio_track_info
      .as_ref()
      .map(|c| c.sample_rate)
      .unwrap_or(48000) as i64;
    let pts_in_samples = timestamp * sample_rate / 1_000_000;

    // Ensure monotonically increasing PTS (audio time base is 1/sample_rate)
    let pts = if pts_in_samples <= self.last_audio_pts {
      self.last_audio_pts + 1
    } else {
      pts_in_samples
    };
    self.last_audio_pts = pts;

    packet.set_pts(pts);
    packet.set_dts(pts);

    if let Some(dur) = duration {
      let duration_in_samples = dur * sample_rate / 1_000_000;
      packet.set_duration(duration_in_samples);
    }

    // Handle metadata - extract description if present
    if let Some(description) = metadata
      .and_then(|m| m.decoder_config.as_ref())
      .and_then(|c| c.description.as_ref())
    {
      let desc_data = description.to_vec();
      if !desc_data.is_empty() {
        // Update extradata dynamically if available
        if let Err(e) = self.muxer.update_audio_extradata(&desc_data) {
          tracing::warn!(target: "webcodecs", "Failed to update audio extradata: {}", e);
        }
      }
    }

    // Audio packets are typically all keyframes
    packet.set_flags(crate::ffi::pkt_flag::KEY);

    // Write packet
    self.muxer.write_packet(&mut packet).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to write packet: {}", e),
      )
    })?;

    Ok(())
  }

  /// Flush any buffered data
  pub fn flush(&mut self) -> Result<()> {
    if self.state == MuxerState::Muxing {
      self
        .muxer
        .flush()
        .map_err(|e| Error::new(Status::GenericFailure, format!("Failed to flush: {}", e)))?;
    }
    Ok(())
  }

  /// Finalize the muxer and return the buffer data
  ///
  /// For buffer mode: returns the complete muxed data as a Vec<u8>
  /// For streaming mode: signals EOF and returns an empty Vec (use read() to get remaining data)
  pub fn finalize(&mut self) -> Result<Vec<u8>> {
    // If still configuring, write header first
    if self.state == MuxerState::ConfiguringTracks {
      if self.video_track_info.is_none() && self.audio_track_info.is_none() {
        return Err(Error::new(
          Status::GenericFailure,
          "No tracks added to muxer",
        ));
      }
      self.ensure_header_written()?;
    }

    if self.state == MuxerState::Finalized {
      return Err(Error::new(
        Status::GenericFailure,
        "Muxer already finalized",
      ));
    }

    // Finalize the muxer (writes trailer)
    self
      .muxer
      .finalize()
      .map_err(|e| Error::new(Status::GenericFailure, format!("Failed to finalize: {}", e)))?;

    self.state = MuxerState::Finalized;

    // In streaming mode, signal EOF and return empty vec
    // Remaining data should be read via read()
    if self.is_streaming {
      self.muxer.finish_streaming();
      return Ok(Vec::new());
    }

    // In buffer mode, return the complete buffer
    let data = self
      .muxer
      .take_buffer()
      .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get output buffer"))?;

    Ok(data)
  }

  /// Read available data from streaming buffer (for streaming mode)
  pub fn read_streaming(&self) -> Result<Option<Vec<u8>>> {
    if !self.is_streaming {
      return Err(Error::new(Status::GenericFailure, "Not in streaming mode"));
    }

    if let Some(ref handle) = self.streaming_handle {
      Ok(handle.read_available())
    } else {
      Err(Error::new(
        Status::GenericFailure,
        "Streaming handle not available",
      ))
    }
  }

  /// Check if streaming is finished (EOF reached)
  pub fn is_streaming_finished(&self) -> bool {
    if let Some(ref handle) = self.streaming_handle {
      handle.is_eof()
    } else {
      true
    }
  }

  /// Get current state as string
  pub fn state_string(&self) -> &'static str {
    self.state.as_str()
  }
}
