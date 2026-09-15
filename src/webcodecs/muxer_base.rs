//! Muxer Base - Shared types and traits for container muxers
//!
//! This module provides common functionality for Mp4Muxer, WebMMuxer, and MkvMuxer
//! to eliminate code duplication across the three implementations.

use crate::codec::Packet;
use crate::codec::io_buffer::StreamingBufferHandle;
use crate::codec::muxer::{
  AudioStreamConfig, ContainerFormat, Hvc1Context, MuxerContext, MuxerOptions, MuxerOutput,
  VideoStreamConfig,
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

fn next_reordered_video_timestamp(last_dts: i64, last_duration: i64) -> i64 {
  if last_dts == i64::MIN {
    0
  } else {
    last_dts.saturating_add(last_duration.max(1))
  }
}

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
  pub framerate: f64,
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
  /// Maximum bytes returned by each streaming read (default: 256KB).
  /// Muxer writes are queued without blocking the JavaScript consumer.
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
  pub framerate: f64,
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
  /// Format-specific options holder (without fast_start - that's handled separately)
  pub muxer_options: MuxerOptions,
  /// Whether to apply fastStart post-processing (MP4 only)
  /// We handle this ourselves because FFmpeg's faststart doesn't work with custom I/O
  apply_faststart: bool,
  /// Last audio PTS written (to ensure monotonically increasing)
  last_audio_pts: i64,
  /// Ticks per video frame in stream time base (set after header written)
  /// Used only as a duration fallback when a chunk has no explicit duration.
  video_ticks_per_frame: Option<u64>,
  /// Running DTS offset used when MP4 rejects a codec-provided B-frame pair.
  video_dts_shift: i64,
  /// Last written video DTS (to ensure monotonically increasing decode order)
  last_video_dts: i64,
  /// Duration of the previously written video packet. The Matroska/WebM
  /// reordered-frame compatibility path advances from the previous packet's
  /// end, not by the duration of the packet currently being written.
  last_video_duration: i64,
  /// HEVC 'hvc1' selection context. In-band parameter sets that duplicate the
  /// hvcC are stripped from samples because 'hvc1' forbids them; None when
  /// 'hev1' is in use (in-band sets allowed).
  strip_hevc_ps: Option<Hvc1Context>,
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

    // Extract fast_start option - we handle this ourselves via post-processing
    // because FFmpeg's faststart doesn't work with custom I/O contexts
    let apply_faststart = options.fast_start && F::FORMAT == ContainerFormat::Mp4;

    // Create options for FFmpeg without fast_start (we'll apply it in post-processing)
    let ffmpeg_options = MuxerOptions {
      fast_start: false, // Never pass to FFmpeg - we handle it ourselves
      ..options
    };

    Ok(Self {
      muxer,
      state: MuxerState::ConfiguringTracks,
      video_track_info: None,
      audio_track_info: None,
      streaming_handle: None,
      is_streaming: false,
      muxer_options: ffmpeg_options,
      apply_faststart,
      last_audio_pts: -1,
      video_ticks_per_frame: None,
      video_dts_shift: 0,
      last_video_dts: i64::MIN,
      last_video_duration: 0,
      strip_hevc_ps: None,
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

    // Note: fastStart is not supported in streaming mode - it requires the complete
    // file to rearrange atoms. Use fragmented MP4 for streaming instead.
    let ffmpeg_options = MuxerOptions {
      fast_start: false, // Not supported in streaming mode
      ..options
    };

    Ok(Self {
      muxer,
      state: MuxerState::ConfiguringTracks,
      video_track_info: None,
      audio_track_info: None,
      streaming_handle,
      is_streaming: true,
      muxer_options: ffmpeg_options,
      apply_faststart: false, // Never apply in streaming mode
      last_audio_pts: -1,
      video_ticks_per_frame: None,
      video_dts_shift: 0,
      last_video_dts: i64::MIN,
      last_video_duration: 0,
      strip_hevc_ps: None,
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

    // Calculate time_base for precise timing using FFmpeg's algorithm:
    // Start with fps as timescale, then double until >= 10000
    // This ensures millisecond-level precision while keeping timescale reasonable
    // For 30fps: 30 -> 60 -> 120 -> 240 -> 480 -> 960 -> 1920 -> 3840 -> 7680 -> 15360
    let time_base = if config.framerate > 0.0 && config.framerate.is_finite() {
      let fps = config.framerate;
      const MIN_FPS: f64 = 1.0;
      if fps >= MIN_FPS {
        // FFmpeg's algorithm: double until >= 10000
        let mut timescale = fps.round() as i32;
        while timescale < 10000 {
          timescale *= 2;
        }
        AVRational::new(1, timescale)
      } else {
        // Fallback to microseconds for very low framerates
        AVRational::MICROSECONDS
      }
    } else {
      AVRational::MICROSECONDS
    };

    // Create video stream config
    let stream_config = VideoStreamConfig {
      codec_id: config.codec_id,
      width: config.width,
      height: config.height,
      pixel_format,
      time_base,
      bitrate: None,
      extradata: config.extradata,
    };

    self.muxer.add_video_stream(&stream_config).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to add video stream: {}", e),
      )
    })?;

    // 'hvc1' forbids in-band parameter sets; strip hvcC duplicates below
    self.strip_hevc_ps = self.muxer.hvc1_context().cloned();

    self.video_track_info = Some(StoredVideoTrackInfo {
      codec: config.codec,
      width: config.width,
      height: config.height,
      framerate: config.framerate,
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

      // Calculate ticks per frame for precise PTS calculation
      // This avoids floating point cumulative errors
      if let (Some(tb), Some(track_info)) = (self.muxer.video_time_base(), &self.video_track_info) {
        // ticks_per_frame = time_base_den / fps
        // For 30fps with tb=1/57600: ticks_per_frame = 57600 / 30 = 1920
        let fps = track_info.framerate;
        // Use minimum fps threshold to avoid extremely large tick values from division
        // 1.0 fps is a reasonable lower bound for any practical video
        // Also check is_finite() to guard against NaN/Infinity
        const MIN_FPS: f64 = 1.0;
        if fps.is_finite() && fps >= MIN_FPS {
          self.video_ticks_per_frame = Some((tb.den as f64 / fps).round() as u64);
        }
      }
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
    // Get internal DTS if available (for B-frame support)
    let chunk_dts = chunk.dts()?;
    // Get original PTS from encoder (for B-frame support)
    let chunk_original_pts = chunk.original_pts()?;

    // Get packet using optimized path:
    // - If chunk has Packet (from encoder): shallow_clone shares buffer (zero-copy)
    // - If chunk has Vec<u8> (from JS): copy data into new packet
    let mut packet = chunk.get_packet_for_muxing()?;

    // 'hvc1' forbids in-band VPS/SPS/PPS; drop the ones already carried by the
    // hvcC. Done before any packet metadata is set so the swap is a plain data
    // replace. Malformed samples and in-band parameter-set updates (which
    // 'hvc1' cannot represent) are rejected.
    if let Some(hvc1) = &self.strip_hevc_ps {
      // A packet carrying a new description (AV_PKT_DATA_NEW_EXTRADATA) means
      // the stream's parameter sets changed mid-stream. The stripping context
      // is fixed at track-add time and cannot follow the change — and the
      // packet replacement below would drop the side data — so reject
      // anything but a redundant re-send of the same hvcC.
      if let Some(new_extradata) = packet.new_extradata()
        && new_extradata != hvc1.extradata.as_slice()
      {
        return Err(Error::new(
          Status::GenericFailure,
          "HEVC description changed mid-stream, which the 'hvc1' sample entry cannot represent",
        ));
      }
      let stripped =
        strip_hevc_parameter_sets(packet.as_slice(), hvc1.nal_len_size, &hvc1.parameter_sets)
          .map_err(|e| Error::new(Status::GenericFailure, e))?;
      if let Some(stripped) = stripped {
        let mut stripped_packet = Packet::new()
          .map_err(|e| Error::new(Status::GenericFailure, format!("Packet alloc: {}", e)))?;
        stripped_packet
          .copy_data_from(&stripped)
          .map_err(|e| Error::new(Status::GenericFailure, format!("Packet copy: {}", e)))?;
        packet = stripped_packet;
      }
    }

    // Set packet properties
    packet.set_stream_index(video_index);

    // Encoder/demuxer chunks may carry a private DTS/PTS pair for frame reordering.
    // Public chunks only expose a presentation timestamp, so DTS defaults to PTS.
    // Never synthesize presentation timestamps from frame count: doing so destroys
    // variable frame rate, gaps, and discontinuities.
    let pts_us = chunk_original_pts.unwrap_or(timestamp);
    let dts_us = chunk_dts.unwrap_or(pts_us);

    let (pts, dts, dur) = if let Some(dst_tb) = self.muxer.video_time_base() {
      use crate::ffi::avutil::av_rescale_q;
      let src_tb = AVRational::MICROSECONDS;
      let pts = unsafe { av_rescale_q(pts_us, src_tb, dst_tb) };
      let dts = unsafe { av_rescale_q(dts_us, src_tb, dst_tb) };
      let dur = duration
        .map(|value| unsafe { av_rescale_q(value, src_tb, dst_tb) })
        .or_else(|| self.video_ticks_per_frame.map(|value| value as i64))
        .unwrap_or(0);
      (pts, dts, dur)
    } else {
      (pts_us, dts_us, duration.unwrap_or(0))
    };

    let has_reordered_timestamps =
      chunk_original_pts.is_some() && chunk_dts.is_some() && dts != pts;
    let (final_pts, final_dts) = if F::FORMAT == ContainerFormat::Mp4 {
      // FFmpeg's MP4 muxer requires monotonically increasing DTS and PTS >=
      // DTS. Some hardware encoders emit B-frame pairs that violate the
      // latter at the API boundary. Shift only decode timestamps, preserving
      // source PTS unless the two constraints conflict for this packet.
      let mut shifted_dts = dts.saturating_add(self.video_dts_shift);
      let min_dts = self.last_video_dts.saturating_add(1);
      if shifted_dts < min_dts {
        shifted_dts = min_dts;
      }

      if pts < shifted_dts {
        self.video_dts_shift = self
          .video_dts_shift
          .saturating_sub(shifted_dts.saturating_sub(pts));
        shifted_dts = pts;
      }

      if shifted_dts <= self.last_video_dts {
        shifted_dts = self.last_video_dts.saturating_add(1);
      }
      (pts.max(shifted_dts), shifted_dts)
    } else if has_reordered_timestamps {
      // FFmpeg's Matroska muxer rejects decode-order packets once a B-frame
      // presentation timestamp falls below its DTS. Keep the established
      // compatibility behavior for that constrained case; non-reordered
      // streams retain their source timestamps below.
      let sequential =
        next_reordered_video_timestamp(self.last_video_dts, self.last_video_duration);
      (sequential, sequential)
    } else {
      let monotonic_dts = if dts <= self.last_video_dts {
        self.last_video_dts.saturating_add(1)
      } else {
        dts
      };
      let constrained_pts = pts.max(monotonic_dts);
      if monotonic_dts != dts {
        tracing::warn!(target: "webcodecs", "Adjusted non-monotonic video PTS/DTS from {}/{} to {}/{}", pts, dts, constrained_pts, monotonic_dts);
      }
      (constrained_pts, monotonic_dts)
    };
    tracing::trace!(target: "ffmpeg", "video packet: pts={}, dts={}, dur={}", final_pts, final_dts, dur);
    packet.set_pts(final_pts);
    self.last_video_dts = final_dts;
    self.last_video_duration = dur;
    packet.set_dts(final_dts);
    packet.set_duration(dur);

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
    let audio_time_base = AVRational {
      num: 1,
      den: sample_rate as i32,
    };
    let pts_in_samples = unsafe {
      crate::ffi::avutil::av_rescale_q(timestamp, AVRational::MICROSECONDS, audio_time_base)
    };

    // Ensure monotonically increasing PTS (audio time base is 1/sample_rate)
    let pts = if pts_in_samples <= self.last_audio_pts {
      self.last_audio_pts.saturating_add(1)
    } else {
      pts_in_samples
    };
    self.last_audio_pts = pts;

    packet.set_pts(pts);
    packet.set_dts(pts); // Audio has no B-frames, DTS always equals PTS

    if let Some(dur) = duration {
      let duration_in_samples =
        unsafe { crate::ffi::avutil::av_rescale_q(dur, AVRational::MICROSECONDS, audio_time_base) };
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
    let mut data = self
      .muxer
      .take_buffer()
      .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get output buffer"))?;

    // Apply fastStart post-processing if requested (MP4 only)
    // This moves the moov atom to the beginning of the file for faster streaming playback.
    // We do this ourselves because FFmpeg's faststart option doesn't work with custom I/O.
    if self.apply_faststart {
      data = crate::codec::mp4_faststart::apply_faststart(data);
    }

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
  pub fn is_streaming_finished(&self) -> Result<bool> {
    if !self.is_streaming {
      return Err(Error::new(Status::GenericFailure, "Not in streaming mode"));
    }

    if let Some(ref handle) = self.streaming_handle {
      Ok(handle.is_eof())
    } else {
      Err(Error::new(
        Status::GenericFailure,
        "Streaming handle not available",
      ))
    }
  }

  /// Get current state as string
  pub fn state_string(&self) -> &'static str {
    self.state.as_str()
  }
}

/// Strip VPS/SPS/PPS NAL units from a length-prefixed HEVC sample when they
/// are byte-identical to a parameter set in the hvcC sample description.
///
/// Mirrors `ff_hevc_annexb2mp4` with filter_ps=1 (FFmpeg hevc.c): drops
/// exactly NAL types 32/33/34, keeps everything else (including AUD).
/// Returns Ok(None) — pass the original data through unchanged — when the
/// sample carries no parameter sets. Errors when the buffer does not parse
/// cleanly as length-prefixed NALs, or when a parameter set differs from
/// every hvcC entry: that is an in-band parameter update, which 'hvc1' cannot
/// represent, and dropping it would silently corrupt the stream.
fn strip_hevc_parameter_sets(
  data: &[u8],
  len_size: usize,
  known: &[Vec<u8>],
) -> std::result::Result<Option<Vec<u8>>, String> {
  const NAL_TYPE_VPS: u8 = 32;
  const NAL_TYPE_PPS: u8 = 34;

  let mut offset = 0;
  let mut out: Option<Vec<u8>> = None;
  while offset < data.len() {
    if data.len() - offset < len_size {
      return Err("malformed HEVC sample: truncated NAL length prefix".to_string());
    }
    let mut nal_len: usize = 0;
    for &b in &data[offset..offset + len_size] {
      nal_len = (nal_len << 8) | usize::from(b);
    }
    let nal_start = offset + len_size;
    if nal_len == 0 {
      return Err("malformed HEVC sample: zero-length NAL".to_string());
    }
    if data.len() - nal_start < nal_len {
      return Err("malformed HEVC sample: NAL overruns end of sample".to_string());
    }
    let nal = &data[nal_start..nal_start + nal_len];
    let nal_type = (nal[0] >> 1) & 0x3f;
    let is_ps = (NAL_TYPE_VPS..=NAL_TYPE_PPS).contains(&nal_type);
    if is_ps && !known.iter().any(|k| k.as_slice() == nal) {
      return Err(
        "HEVC sample carries an in-band parameter set update absent from the hvcC \
         description, which the 'hvc1' sample entry cannot represent"
          .to_string(),
      );
    }
    match out.as_mut() {
      // Already stripping: keep non-parameter-set NALs (prefix + data)
      Some(buf) if !is_ps => buf.extend_from_slice(&data[offset..nal_start + nal_len]),
      // First parameter-set NAL: start the output with everything before it
      None if is_ps => out = Some(data[..offset].to_vec()),
      _ => {}
    }
    offset = nal_start + nal_len;
  }
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::{next_reordered_video_timestamp, strip_hevc_parameter_sets};

  #[test]
  fn reordered_vfr_timestamps_advance_by_previous_packet_duration() {
    let first = next_reordered_video_timestamp(i64::MIN, 0);
    let second = next_reordered_video_timestamp(first, 40);
    let third = next_reordered_video_timestamp(second, 20);

    assert_eq!([first, second, third], [0, 40, 60]);
  }

  #[test]
  fn reordered_timestamps_remain_monotonic_without_duration() {
    assert_eq!(next_reordered_video_timestamp(10, 0), 11);
  }

  fn lp(nal: &[u8]) -> Vec<u8> {
    let mut out = (nal.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(nal);
    out
  }

  // Valid 2-byte NAL headers: [type << 1, 0x01]
  const VPS: &[u8] = &[0x40, 0x01, 0xaa];
  const SPS: &[u8] = &[0x42, 0x01, 0xbb];
  const PPS: &[u8] = &[0x44, 0x01, 0xcc];
  const IDR: &[u8] = &[0x28, 0x01, 0xdd]; // type 20
  const AUD: &[u8] = &[0x46, 0x01, 0x50]; // type 35

  fn known_sets() -> Vec<Vec<u8>> {
    vec![VPS.to_vec(), SPS.to_vec(), PPS.to_vec()]
  }

  fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
  }

  #[test]
  fn strips_parameter_sets_matching_hvcc() {
    let sample = concat(&[&lp(VPS), &lp(SPS), &lp(PPS), &lp(IDR)]);
    let stripped = strip_hevc_parameter_sets(&sample, 4, &known_sets())
      .unwrap()
      .expect("parameter sets should be stripped");
    assert_eq!(stripped, lp(IDR));
  }

  #[test]
  fn strip_keeps_aud_and_non_ps_nals() {
    let sample = concat(&[&lp(AUD), &lp(SPS), &lp(IDR)]);
    let stripped = strip_hevc_parameter_sets(&sample, 4, &known_sets())
      .unwrap()
      .expect("SPS should be stripped");
    assert_eq!(stripped, concat(&[&lp(AUD), &lp(IDR)]));
  }

  #[test]
  fn strip_passes_through_samples_without_parameter_sets() {
    let sample = concat(&[&lp(IDR), &lp(AUD)]);
    assert_eq!(
      strip_hevc_parameter_sets(&sample, 4, &known_sets()).unwrap(),
      None
    );
    assert_eq!(
      strip_hevc_parameter_sets(&[], 4, &known_sets()).unwrap(),
      None
    );
  }

  #[test]
  fn strip_rejects_parameter_set_updates_absent_from_hvcc() {
    let updated_sps: &[u8] = &[0x42, 0x01, 0x99]; // same type, different payload
    let sample = concat(&[&lp(VPS), &lp(updated_sps), &lp(IDR)]);
    let err = strip_hevc_parameter_sets(&sample, 4, &known_sets()).unwrap_err();
    assert!(err.contains("parameter set update"), "{err}");
  }

  #[test]
  fn strip_rejects_malformed_samples() {
    // Trailing byte
    let mut trailing = lp(IDR);
    trailing.push(0xaa);
    assert!(strip_hevc_parameter_sets(&trailing, 4, &known_sets()).is_err());

    // Zero-length NAL
    let mut zero = 0u32.to_be_bytes().to_vec();
    zero.extend_from_slice(&lp(IDR));
    assert!(strip_hevc_parameter_sets(&zero, 4, &known_sets()).is_err());

    // NAL overruns end of sample
    let mut overrun = 0u32.to_be_bytes().to_vec();
    overrun[3] = 10;
    overrun.extend_from_slice(IDR);
    assert!(strip_hevc_parameter_sets(&overrun, 4, &known_sets()).is_err());
  }

  #[test]
  fn strip_supports_short_length_prefixes() {
    // 1-byte prefix
    let mut sample = vec![SPS.len() as u8];
    sample.extend_from_slice(SPS);
    sample.push(IDR.len() as u8);
    sample.extend_from_slice(IDR);
    let stripped = strip_hevc_parameter_sets(&sample, 1, &known_sets())
      .unwrap()
      .expect("SPS should be stripped");
    let mut expected = vec![IDR.len() as u8];
    expected.extend_from_slice(IDR);
    assert_eq!(stripped, expected);

    // 2-byte prefix
    let mut sample = (SPS.len() as u16).to_be_bytes().to_vec();
    sample.extend_from_slice(SPS);
    sample.extend_from_slice(&(IDR.len() as u16).to_be_bytes());
    sample.extend_from_slice(IDR);
    let stripped = strip_hevc_parameter_sets(&sample, 2, &known_sets())
      .unwrap()
      .expect("SPS should be stripped");
    let mut expected = (IDR.len() as u16).to_be_bytes().to_vec();
    expected.extend_from_slice(IDR);
    assert_eq!(stripped, expected);
  }
}
