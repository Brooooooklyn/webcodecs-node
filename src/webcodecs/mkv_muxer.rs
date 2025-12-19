//! MkvMuxer - WebCodecs-style muxer for Matroska containers
//!
//! Provides a JavaScript-friendly API for muxing encoded video and audio
//! chunks into MKV container format.

use crate::codec::muxer::{ContainerFormat, MuxerOptions};
use crate::ffi::AVCodecID;
use crate::webcodecs::codec_string::parse_codec_string;
use crate::webcodecs::encoded_audio_chunk::EncodedAudioChunk;
use crate::webcodecs::encoded_video_chunk::EncodedVideoChunk;
use crate::webcodecs::muxer_base::{
  EncodedAudioChunkMetadataJs, EncodedVideoChunkMetadataJs, GenericAudioTrackConfig,
  GenericVideoTrackConfig, MuxerFormat, MuxerInner, StreamingMuxerOptions, lock_muxer_inner,
  lock_muxer_inner_mut,
};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::Mutex;

// ============================================================================
// MKV Format Implementation
// ============================================================================

/// MKV-specific format implementation
pub struct MkvFormat;

impl MuxerFormat for MkvFormat {
  const FORMAT: ContainerFormat = ContainerFormat::Mkv;

  fn default_muxer_options() -> MuxerOptions {
    MuxerOptions::default()
  }

  fn parse_video_codec(codec: &str) -> Result<AVCodecID> {
    let parsed = parse_codec_string(codec).ok_or_else(|| {
      Error::new(
        Status::GenericFailure,
        format!("Invalid codec string: {}", codec),
      )
    })?;

    // MKV accepts most video codecs
    Ok(parsed.codec_id)
  }

  fn parse_audio_codec(codec: &str) -> Result<AVCodecID> {
    let codec_lower = codec.to_lowercase();

    // MKV accepts most audio codecs
    if codec_lower.starts_with("mp4a") || codec_lower == "aac" {
      Ok(AVCodecID::Aac)
    } else if codec_lower == "opus" {
      Ok(AVCodecID::Opus)
    } else if codec_lower == "mp3" || codec_lower.starts_with("mp3") {
      Ok(AVCodecID::Mp3)
    } else if codec_lower == "flac" {
      Ok(AVCodecID::Flac)
    } else if codec_lower == "vorbis" {
      Ok(AVCodecID::Vorbis)
    } else if codec_lower == "pcm" || codec_lower.starts_with("pcm-") {
      Ok(AVCodecID::PcmS16le)
    } else if codec_lower == "alac" {
      Ok(AVCodecID::Alac)
    } else if codec_lower == "ac3" || codec_lower == "ac-3" {
      // Fixed: Use proper AC3 codec ID instead of falling back to AAC
      Ok(AVCodecID::Ac3)
    } else {
      Err(Error::new(
        Status::GenericFailure,
        format!("Unsupported audio codec: {}", codec),
      ))
    }
  }

  fn get_audio_frame_size(codec_id: AVCodecID) -> Option<u32> {
    match codec_id {
      AVCodecID::Aac => Some(1024),
      AVCodecID::Opus => Some(960),
      AVCodecID::Mp3 => Some(1152),
      AVCodecID::Ac3 => Some(1536), // AC3 frame size
      _ => None,
    }
  }
}

// ============================================================================
// MKV Muxer Options
// ============================================================================

/// MKV muxer options
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct MkvMuxerOptions {
  /// Enable live streaming mode
  pub live: Option<bool>,
  /// Enable streaming output mode
  pub streaming: Option<StreamingMuxerOptions>,
}

// ============================================================================
// Track Configuration Types
// ============================================================================

/// Video track configuration for MKV muxer
#[napi(object)]
pub struct MkvVideoTrackConfig {
  /// Codec string (e.g., "avc1.42001E", "hev1.1.6.L93.B0", "vp09.00.10.08", "av01.0.04M.08")
  pub codec: String,
  /// Video width in pixels
  pub width: u32,
  /// Video height in pixels
  pub height: u32,
  /// Codec-specific description data
  pub description: Option<Uint8Array>,
}

/// Audio track configuration for MKV muxer
#[napi(object)]
pub struct MkvAudioTrackConfig {
  /// Codec string (e.g., "mp4a.40.2", "opus", "flac", "vorbis", "ac3")
  pub codec: String,
  /// Sample rate in Hz
  pub sample_rate: u32,
  /// Number of audio channels
  pub number_of_channels: u32,
  /// Codec-specific description data
  pub description: Option<Uint8Array>,
}

// ============================================================================
// MKV Muxer Implementation
// ============================================================================

/// MKV Muxer for combining encoded video and audio into Matroska container
///
/// MKV (Matroska) supports virtually all video and audio codecs.
///
/// Usage:
/// ```javascript
/// const muxer = new MkvMuxer();
/// muxer.addVideoTrack({ codec: 'avc1.42001E', width: 1920, height: 1080 });
/// muxer.addAudioTrack({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
///
/// // Add encoded chunks from VideoEncoder/AudioEncoder
/// encoder.configure({
///   output: (chunk, metadata) => muxer.addVideoChunk(chunk, metadata)
/// });
///
/// // Finalize and get MKV data
/// const mkvData = muxer.finalize();
/// ```
#[napi]
pub struct MkvMuxer {
  inner: Mutex<Option<MuxerInner<MkvFormat>>>,
}

#[napi]
impl MkvMuxer {
  /// Create a new MKV muxer
  #[napi(constructor)]
  pub fn new(options: Option<MkvMuxerOptions>) -> Result<Self> {
    let opts = options.unwrap_or_default();

    // Create muxer options with live streaming support
    let muxer_options = MuxerOptions {
      live: opts.live.unwrap_or(false),
      ..Default::default()
    };

    // Create inner based on output mode
    let inner = if let Some(streaming_opts) = opts.streaming {
      let capacity = streaming_opts.buffer_capacity.unwrap_or(256 * 1024) as usize;
      MuxerInner::<MkvFormat>::new_streaming(muxer_options, capacity)?
    } else {
      MuxerInner::<MkvFormat>::new_buffer(muxer_options)?
    };

    Ok(Self {
      inner: Mutex::new(Some(inner)),
    })
  }

  /// Add a video track to the muxer
  ///
  /// MKV supports H.264, H.265, VP8, VP9, AV1, and many other video codecs.
  #[napi]
  pub fn add_video_track(&self, config: MkvVideoTrackConfig) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);

    // Parse codec (MKV accepts most codecs)
    let codec_id = MkvFormat::parse_video_codec(&config.codec)?;

    let generic_config = GenericVideoTrackConfig {
      codec: config.codec,
      codec_id,
      width: config.width,
      height: config.height,
      extradata: config.description.as_ref().map(|d| d.to_vec()),
    };

    inner.add_video_track(generic_config)
  }

  /// Add an audio track to the muxer
  ///
  /// MKV supports AAC, Opus, Vorbis, FLAC, MP3, AC3, and many other audio codecs.
  #[napi]
  pub fn add_audio_track(&self, config: MkvAudioTrackConfig) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);

    // Parse codec (MKV accepts most codecs)
    let codec_id = MkvFormat::parse_audio_codec(&config.codec)?;

    let generic_config = GenericAudioTrackConfig {
      codec: config.codec,
      codec_id,
      sample_rate: config.sample_rate,
      channels: config.number_of_channels,
      frame_size: MkvFormat::get_audio_frame_size(codec_id),
      extradata: config.description.as_ref().map(|d| d.to_vec()),
    };

    inner.add_audio_track(generic_config)
  }

  /// Add an encoded video chunk to the muxer
  #[napi]
  pub fn add_video_chunk(
    &self,
    chunk: &EncodedVideoChunk,
    metadata: Option<EncodedVideoChunkMetadataJs>,
  ) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);
    inner.add_video_chunk(chunk, metadata.as_ref())
  }

  /// Add an encoded audio chunk to the muxer
  #[napi]
  pub fn add_audio_chunk(
    &self,
    chunk: &EncodedAudioChunk,
    metadata: Option<EncodedAudioChunkMetadataJs>,
  ) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);
    inner.add_audio_chunk(chunk, metadata.as_ref())
  }

  /// Flush any buffered data
  #[napi]
  pub fn flush(&self) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);
    inner.flush()
  }

  /// Finalize the muxer and return the MKV data
  #[napi]
  pub fn finalize(&self) -> Result<Uint8Array> {
    lock_muxer_inner_mut!(self => _guard, inner);
    let data = inner.finalize()?;
    Ok(Uint8Array::new(data))
  }

  /// Read available data from streaming buffer (streaming mode only)
  ///
  /// Returns available data, or null if no data is ready yet.
  /// Returns empty Uint8Array when streaming is finished.
  #[napi]
  pub fn read(&self) -> Result<Option<Uint8Array>> {
    lock_muxer_inner!(self => _guard, inner);
    match inner.read_streaming() {
      Ok(Some(data)) => Ok(Some(Uint8Array::new(data))),
      Ok(None) => Ok(None),
      Err(e) => Err(e),
    }
  }

  /// Check if muxer is in streaming mode
  #[napi(getter)]
  pub fn is_streaming(&self) -> Result<bool> {
    lock_muxer_inner!(self => _guard, inner);
    Ok(inner.is_streaming)
  }

  /// Check if streaming is finished (streaming mode only)
  #[napi(getter)]
  pub fn is_finished(&self) -> Result<bool> {
    lock_muxer_inner!(self => _guard, inner);
    Ok(inner.is_streaming_finished())
  }

  /// Close the muxer and release resources
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if guard.is_none() {
      return Ok(());
    }

    *guard = None;
    Ok(())
  }

  /// Get the current state of the muxer
  #[napi(getter)]
  pub fn state(&self) -> Result<String> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let state = match guard.as_ref() {
      Some(inner) => inner.state_string(),
      None => "closed",
    };

    Ok(state.to_string())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_parse_mkv_video_codec() {
    assert!(matches!(
      MkvFormat::parse_video_codec("avc1.42001E"),
      Ok(AVCodecID::H264)
    ));
    assert!(matches!(
      MkvFormat::parse_video_codec("hev1.1.6.L93.B0"),
      Ok(AVCodecID::Hevc)
    ));
    assert!(matches!(
      MkvFormat::parse_video_codec("vp9"),
      Ok(AVCodecID::Vp9)
    ));
    assert!(matches!(
      MkvFormat::parse_video_codec("av01.0.04M.08"),
      Ok(AVCodecID::Av1)
    ));
  }

  #[test]
  fn test_parse_mkv_audio_codec() {
    assert!(matches!(
      MkvFormat::parse_audio_codec("mp4a.40.2"),
      Ok(AVCodecID::Aac)
    ));
    assert!(matches!(
      MkvFormat::parse_audio_codec("opus"),
      Ok(AVCodecID::Opus)
    ));
    assert!(matches!(
      MkvFormat::parse_audio_codec("flac"),
      Ok(AVCodecID::Flac)
    ));
    assert!(matches!(
      MkvFormat::parse_audio_codec("vorbis"),
      Ok(AVCodecID::Vorbis)
    ));
    // AC3 should now work correctly (was previously broken)
    assert!(matches!(
      MkvFormat::parse_audio_codec("ac3"),
      Ok(AVCodecID::Ac3)
    ));
    assert!(matches!(
      MkvFormat::parse_audio_codec("alac"),
      Ok(AVCodecID::Alac)
    ));
  }
}
