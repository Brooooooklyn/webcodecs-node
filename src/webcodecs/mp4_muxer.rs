//! Mp4Muxer - WebCodecs-style muxer for MP4 containers
//!
//! Provides a JavaScript-friendly API for muxing encoded video and audio
//! chunks into MP4 container format.

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
// MP4 Format Implementation
// ============================================================================

/// MP4-specific format implementation
pub struct Mp4Format;

impl MuxerFormat for Mp4Format {
  const FORMAT: ContainerFormat = ContainerFormat::Mp4;

  fn default_muxer_options() -> MuxerOptions {
    MuxerOptions {
      fast_start: false,
      fragmented: false,
      live: false,
    }
  }

  fn parse_video_codec(codec: &str) -> Result<AVCodecID> {
    let parsed = parse_codec_string(codec).ok_or_else(|| {
      Error::new(
        Status::GenericFailure,
        format!("Invalid codec string: {}", codec),
      )
    })?;

    let codec_id = parsed.codec_id;

    // MP4 supports H.264, H.265, AV1 (VP8/VP9 are WebM-only codecs)
    match codec_id {
      AVCodecID::H264 | AVCodecID::Hevc | AVCodecID::Av1 => Ok(codec_id),
      AVCodecID::Vp8 | AVCodecID::Vp9 => Err(Error::new(
        Status::GenericFailure,
        format!(
          "VP8/VP9 are not supported in MP4 container. Use WebM or MKV instead: {}",
          codec
        ),
      )),
      _ => Err(Error::new(
        Status::GenericFailure,
        format!("Unsupported video codec for MP4: {}", codec),
      )),
    }
  }

  fn parse_audio_codec(codec: &str) -> Result<AVCodecID> {
    let codec_lower = codec.to_lowercase();

    // MP4 supports AAC, Opus, MP3, FLAC (Vorbis is WebM-only, PCM is rarely used in MP4)
    if codec_lower.starts_with("mp4a") || codec_lower == "aac" {
      Ok(AVCodecID::Aac)
    } else if codec_lower == "opus" {
      Ok(AVCodecID::Opus)
    } else if codec_lower == "mp3" || codec_lower.starts_with("mp3") {
      Ok(AVCodecID::Mp3)
    } else if codec_lower == "flac" {
      Ok(AVCodecID::Flac)
    } else if codec_lower == "vorbis" {
      Err(Error::new(
        Status::GenericFailure,
        "Vorbis is not supported in MP4 container. Use WebM or MKV instead",
      ))
    } else if codec_lower == "pcm" || codec_lower.starts_with("pcm-") {
      Err(Error::new(
        Status::GenericFailure,
        "PCM audio is not supported in MP4 container. Use MKV or WAV instead",
      ))
    } else {
      Err(Error::new(
        Status::GenericFailure,
        format!("Unsupported audio codec for MP4: {}", codec),
      ))
    }
  }

  fn get_audio_frame_size(codec_id: AVCodecID) -> Option<u32> {
    match codec_id {
      AVCodecID::Aac => Some(1024),
      AVCodecID::Opus => Some(960),
      AVCodecID::Mp3 => Some(1152),
      _ => None,
    }
  }
}

// ============================================================================
// MP4 Muxer Options
// ============================================================================

/// MP4 muxer options
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct Mp4MuxerOptions {
  /// Move moov atom to beginning for better streaming (default: false)
  /// Note: Not compatible with streaming output mode
  pub fast_start: Option<bool>,
  /// Use fragmented MP4 for streaming output
  /// When true, uses frag_keyframe+empty_moov+default_base_moof
  pub fragmented: Option<bool>,
  /// Enable streaming output mode
  pub streaming: Option<StreamingMuxerOptions>,
}

// ============================================================================
// Track Configuration Types
// ============================================================================

/// Video track configuration for MP4 muxer
#[napi(object)]
pub struct Mp4VideoTrackConfig {
  /// Codec string (e.g., "avc1.42001E", "hev1.1.6.L93.B0", "av01.0.04M.08")
  pub codec: String,
  /// Video width in pixels
  pub width: u32,
  /// Video height in pixels
  pub height: u32,
  /// Codec-specific description data (avcC/hvcC/av1C from encoder metadata)
  pub description: Option<Uint8Array>,
}

/// Audio track configuration for MP4 muxer
#[napi(object)]
pub struct Mp4AudioTrackConfig {
  /// Codec string (e.g., "mp4a.40.2" for AAC-LC, "opus")
  pub codec: String,
  /// Sample rate in Hz
  pub sample_rate: u32,
  /// Number of audio channels
  pub number_of_channels: u32,
  /// Codec-specific description data (esds for AAC, etc.)
  pub description: Option<Uint8Array>,
}

// ============================================================================
// MP4 Muxer Implementation
// ============================================================================

/// MP4 Muxer for combining encoded video and audio into MP4 container
///
/// Usage:
/// ```javascript
/// const muxer = new Mp4Muxer({ fastStart: true });
/// muxer.addVideoTrack({ codec: 'avc1.42001E', width: 1920, height: 1080 });
/// muxer.addAudioTrack({ codec: 'mp4a.40.2', sampleRate: 48000, numberOfChannels: 2 });
///
/// // Add encoded chunks from VideoEncoder/AudioEncoder
/// encoder.configure({
///   output: (chunk, metadata) => muxer.addVideoChunk(chunk, metadata)
/// });
///
/// // Finalize and get MP4 data
/// const mp4Data = muxer.finalize();
/// ```
#[napi]
pub struct Mp4Muxer {
  inner: Mutex<Option<MuxerInner<Mp4Format>>>,
}

#[napi]
impl Mp4Muxer {
  /// Create a new MP4 muxer
  #[napi(constructor)]
  pub fn new(options: Option<Mp4MuxerOptions>) -> Result<Self> {
    let opts = options.unwrap_or_default();

    // Validate incompatible options
    if opts.fast_start.unwrap_or(false) && opts.streaming.is_some() {
      return Err(Error::new(
        Status::GenericFailure,
        "fastStart is not compatible with streaming mode. Use fragmented: true for streaming.",
      ));
    }

    // Create muxer options
    let muxer_options = MuxerOptions {
      fast_start: opts.fast_start.unwrap_or(false),
      fragmented: opts.fragmented.unwrap_or(false),
      live: false, // Not applicable for MP4
    };

    // Create inner based on output mode
    let inner = if let Some(streaming_opts) = opts.streaming {
      let capacity = streaming_opts.buffer_capacity.unwrap_or(256 * 1024) as usize;
      MuxerInner::<Mp4Format>::new_streaming(muxer_options, capacity)?
    } else {
      MuxerInner::<Mp4Format>::new_buffer(muxer_options)?
    };

    Ok(Self {
      inner: Mutex::new(Some(inner)),
    })
  }

  /// Add a video track to the muxer
  ///
  /// Must be called before adding any chunks.
  #[napi]
  pub fn add_video_track(&self, config: Mp4VideoTrackConfig) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);

    // Parse codec and validate
    let codec_id = Mp4Format::parse_video_codec(&config.codec)?;

    // Validate MP4-specific codec support (H.264, H.265, AV1 only for MP4)
    if !matches!(codec_id, AVCodecID::H264 | AVCodecID::Hevc | AVCodecID::Av1) {
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "Video codec {} is not supported in MP4 container",
          config.codec
        ),
      ));
    }

    let generic_config = GenericVideoTrackConfig {
      codec: config.codec,
      codec_id,
      width: config.width,
      height: config.height,
      extradata: config.description.as_ref().map(|d| d.to_vec()),
      has_alpha: false, // MP4 doesn't support VP9 alpha
    };

    inner.add_video_track(generic_config)
  }

  /// Add an audio track to the muxer
  ///
  /// Must be called before adding any chunks.
  #[napi]
  pub fn add_audio_track(&self, config: Mp4AudioTrackConfig) -> Result<()> {
    lock_muxer_inner_mut!(self => _guard, inner);

    // Parse codec and validate
    let codec_id = Mp4Format::parse_audio_codec(&config.codec)?;

    // Validate MP4-specific codec support
    if !matches!(
      codec_id,
      AVCodecID::Aac | AVCodecID::Mp3 | AVCodecID::Flac | AVCodecID::Opus
    ) {
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "Audio codec {} is not supported in MP4 container",
          config.codec
        ),
      ));
    }

    let generic_config = GenericAudioTrackConfig {
      codec: config.codec,
      codec_id,
      sample_rate: config.sample_rate,
      channels: config.number_of_channels,
      frame_size: Mp4Format::get_audio_frame_size(codec_id),
      extradata: config.description.as_ref().map(|d| d.to_vec()),
    };

    inner.add_audio_track(generic_config)
  }

  /// Add an encoded video chunk to the muxer
  ///
  /// The chunk should come from a VideoEncoder's output callback.
  /// If metadata contains decoderConfig.description, it will be used to update
  /// the codec extradata (useful for extracting avcC/hvcC from the encoder).
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
  ///
  /// The chunk should come from an AudioEncoder's output callback.
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

  /// Finalize the muxer and return the MP4 data
  ///
  /// After calling this, no more chunks can be added.
  /// Returns the complete MP4 file as a Uint8Array.
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
  ///
  /// This is called automatically when the muxer is garbage collected,
  /// but can be called explicitly to release resources early.
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if guard.is_none() {
      return Ok(()); // Already closed
    }

    // Drop the inner state
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
  fn test_parse_video_codec() {
    assert!(matches!(
      Mp4Format::parse_video_codec("avc1.42001E"),
      Ok(AVCodecID::H264)
    ));
    assert!(matches!(
      Mp4Format::parse_video_codec("hev1.1.6.L93.B0"),
      Ok(AVCodecID::Hevc)
    ));
    assert!(matches!(
      Mp4Format::parse_video_codec("av01.0.04M.08"),
      Ok(AVCodecID::Av1)
    ));
    assert!(matches!(
      Mp4Format::parse_video_codec("vp9"),
      Ok(AVCodecID::Vp9)
    ));
    assert!(matches!(
      Mp4Format::parse_video_codec("vp09.00.10.08"),
      Ok(AVCodecID::Vp9)
    ));
  }

  #[test]
  fn test_parse_audio_codec() {
    assert!(matches!(
      Mp4Format::parse_audio_codec("mp4a.40.2"),
      Ok(AVCodecID::Aac)
    ));
    assert!(matches!(
      Mp4Format::parse_audio_codec("opus"),
      Ok(AVCodecID::Opus)
    ));
    assert!(matches!(
      Mp4Format::parse_audio_codec("mp3"),
      Ok(AVCodecID::Mp3)
    ));
    assert!(matches!(
      Mp4Format::parse_audio_codec("flac"),
      Ok(AVCodecID::Flac)
    ));
  }
}
