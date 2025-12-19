//! WebMMuxer - WebCodecs-style muxer for WebM containers
//!
//! Provides a JavaScript-friendly API for muxing encoded video and audio
//! chunks into WebM container format.

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
// WebM Format Implementation
// ============================================================================

/// WebM-specific format implementation
pub struct WebMFormat;

impl MuxerFormat for WebMFormat {
  const FORMAT: ContainerFormat = ContainerFormat::WebM;

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

    let codec_id = parsed.codec_id;

    // WebM only supports VP8, VP9, and AV1
    match codec_id {
      AVCodecID::Vp8 | AVCodecID::Vp9 | AVCodecID::Av1 => Ok(codec_id),
      _ => Err(Error::new(
        Status::GenericFailure,
        format!(
          "Unsupported WebM video codec: {}. WebM supports VP8, VP9, and AV1.",
          codec
        ),
      )),
    }
  }

  fn parse_audio_codec(codec: &str) -> Result<AVCodecID> {
    let codec_lower = codec.to_lowercase();

    // WebM only supports Opus and Vorbis
    if codec_lower == "opus" {
      Ok(AVCodecID::Opus)
    } else if codec_lower == "vorbis" {
      Ok(AVCodecID::Vorbis)
    } else {
      Err(Error::new(
        Status::GenericFailure,
        format!(
          "Unsupported WebM audio codec: {}. WebM supports Opus and Vorbis.",
          codec
        ),
      ))
    }
  }

  fn get_audio_frame_size(codec_id: AVCodecID) -> Option<u32> {
    match codec_id {
      AVCodecID::Opus => Some(960), // 20ms at 48kHz
      AVCodecID::Vorbis => None,
      _ => None,
    }
  }
}

// ============================================================================
// WebM Muxer Options
// ============================================================================

/// WebM muxer options
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct WebMMuxerOptions {
  /// Enable live streaming mode (cluster-at-a-time output)
  pub live: Option<bool>,
  /// Enable streaming output mode
  pub streaming: Option<StreamingMuxerOptions>,
}

// ============================================================================
// Track Configuration Types
// ============================================================================

/// Video track configuration for WebM muxer
#[napi(object)]
pub struct WebMVideoTrackConfig {
  /// Codec string (e.g., "vp8", "vp09.00.10.08", "av01.0.04M.08")
  pub codec: String,
  /// Video width in pixels
  pub width: u32,
  /// Video height in pixels
  pub height: u32,
  /// Codec-specific description data
  pub description: Option<Uint8Array>,
}

/// Audio track configuration for WebM muxer
#[napi(object)]
pub struct WebMAudioTrackConfig {
  /// Codec string (e.g., "opus", "vorbis")
  pub codec: String,
  /// Sample rate in Hz
  pub sample_rate: u32,
  /// Number of audio channels
  pub number_of_channels: u32,
  /// Codec-specific description data
  pub description: Option<Uint8Array>,
}

// ============================================================================
// WebM Muxer Implementation
// ============================================================================

/// WebM Muxer for combining encoded video and audio into WebM container
///
/// WebM supports VP8, VP9, AV1 video codecs and Opus, Vorbis audio codecs.
///
/// Usage:
/// ```javascript
/// const muxer = new WebMMuxer();
/// muxer.addVideoTrack({ codec: 'vp09.00.10.08', width: 1920, height: 1080 });
/// muxer.addAudioTrack({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
///
/// // Add encoded chunks from VideoEncoder/AudioEncoder
/// encoder.configure({
///   output: (chunk, metadata) => muxer.addVideoChunk(chunk, metadata)
/// });
///
/// // Finalize and get WebM data
/// const webmData = muxer.finalize();
/// ```
#[napi]
pub struct WebMMuxer {
  inner: Mutex<Option<MuxerInner<WebMFormat>>>,
}

#[napi]
impl WebMMuxer {
  /// Create a new WebM muxer
  #[napi(constructor)]
  pub fn new(options: Option<WebMMuxerOptions>) -> Result<Self> {
    let opts = options.unwrap_or_default();

    // Create muxer options with live streaming support
    let muxer_options = MuxerOptions {
      live: opts.live.unwrap_or(false),
      ..Default::default()
    };

    // Create inner based on output mode
    let inner = if let Some(streaming_opts) = opts.streaming {
      let capacity = streaming_opts.buffer_capacity.unwrap_or(256 * 1024) as usize;
      MuxerInner::<WebMFormat>::new_streaming(muxer_options, capacity)?
    } else {
      MuxerInner::<WebMFormat>::new_buffer(muxer_options)?
    };

    Ok(Self {
      inner: Mutex::new(Some(inner)),
    })
  }

  /// Add a video track to the muxer
  ///
  /// WebM supports VP8, VP9, and AV1 video codecs.
  #[napi]
  pub fn add_video_track(&self, config: WebMVideoTrackConfig) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);

    // Parse codec and validate
    let codec_id = WebMFormat::parse_video_codec(&config.codec)?;

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
  /// WebM supports Opus and Vorbis audio codecs.
  #[napi]
  pub fn add_audio_track(&self, config: WebMAudioTrackConfig) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);

    // Parse codec and validate
    let codec_id = WebMFormat::parse_audio_codec(&config.codec)?;

    let generic_config = GenericAudioTrackConfig {
      codec: config.codec,
      codec_id,
      sample_rate: config.sample_rate,
      channels: config.number_of_channels,
      frame_size: WebMFormat::get_audio_frame_size(codec_id),
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

  /// Finalize the muxer and return the WebM data
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
  fn test_parse_webm_video_codec() {
    assert!(matches!(
      WebMFormat::parse_video_codec("vp8"),
      Ok(AVCodecID::Vp8)
    ));
    assert!(matches!(
      WebMFormat::parse_video_codec("vp9"),
      Ok(AVCodecID::Vp9)
    ));
    assert!(matches!(
      WebMFormat::parse_video_codec("vp09.00.10.08"),
      Ok(AVCodecID::Vp9)
    ));
    assert!(matches!(
      WebMFormat::parse_video_codec("av01.0.04M.08"),
      Ok(AVCodecID::Av1)
    ));
    // H.264 is not supported in WebM
    assert!(WebMFormat::parse_video_codec("avc1.42001E").is_err());
  }

  #[test]
  fn test_parse_webm_audio_codec() {
    assert!(matches!(
      WebMFormat::parse_audio_codec("opus"),
      Ok(AVCodecID::Opus)
    ));
    assert!(matches!(
      WebMFormat::parse_audio_codec("vorbis"),
      Ok(AVCodecID::Vorbis)
    ));
    // AAC is not supported in WebM
    assert!(WebMFormat::parse_audio_codec("mp4a.40.2").is_err());
  }
}
