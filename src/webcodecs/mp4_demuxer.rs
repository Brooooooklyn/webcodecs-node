//! Mp4Demuxer - WebCodecs-style demuxer for MP4 containers
//!
//! Provides a JavaScript-friendly API for demuxing MP4 container files
//! into encoded video and audio chunks.

use crate::ffi::AVCodecID;
use crate::webcodecs::demuxer_base::{
  AudioOutputCallback, DemuxerAudioDecoderConfig, DemuxerChunk, DemuxerFormat, DemuxerInner,
  DemuxerTrackInfo, DemuxerVideoDecoderConfig, ErrorCallback, VideoOutputCallback,
  parse_aac_codec_string, parse_h264_codec_string, parse_hevc_codec_string, parse_vp9_codec_string,
  with_demuxer_inner, with_demuxer_inner_mut,
};
use crate::webcodecs::encoded_audio_chunk::EncodedAudioChunk;
use crate::webcodecs::encoded_video_chunk::EncodedVideoChunk;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::UnknownReturnValue;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

// ============================================================================
// Mp4Format - Format-specific behavior for MP4 containers
// ============================================================================

/// MP4 format implementation
pub struct Mp4Format;

impl DemuxerFormat for Mp4Format {
  fn codec_id_to_video_string(codec_id: AVCodecID, extradata: Option<&[u8]>) -> String {
    match codec_id {
      AVCodecID::H264 => parse_h264_codec_string(extradata),
      AVCodecID::Hevc => parse_hevc_codec_string(extradata),
      AVCodecID::Vp8 => "vp8".to_string(),
      AVCodecID::Vp9 => parse_vp9_codec_string(extradata),
      AVCodecID::Av1 => "av01.0.04M.08".to_string(),
      _ => format!("{:?}", codec_id).to_lowercase(),
    }
  }

  fn codec_id_to_audio_string(codec_id: AVCodecID, extradata: Option<&[u8]>) -> String {
    match codec_id {
      AVCodecID::Aac => parse_aac_codec_string(extradata),
      AVCodecID::Opus => "opus".to_string(),
      AVCodecID::Mp3 => "mp3".to_string(),
      AVCodecID::Flac => "flac".to_string(),
      AVCodecID::Vorbis => "vorbis".to_string(),
      _ => format!("{:?}", codec_id).to_lowercase(),
    }
  }
}

// ============================================================================
// Mp4DemuxerInit - Initialization options
// ============================================================================

/// Initialization options for Mp4Demuxer
pub struct Mp4DemuxerInit {
  pub video_output: Option<VideoOutputCallback>,
  pub audio_output: Option<AudioOutputCallback>,
  pub error: ErrorCallback,
}

impl FromNapiValue for Mp4DemuxerInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // Get optional video output callback
    let video_output: Option<VideoOutputCallback> = match obj
      .get_named_property::<Option<Function<EncodedVideoChunk, UnknownReturnValue>>>("videoOutput")
    {
      Ok(Some(func)) => Some(
        func
          .build_threadsafe_function()
          .callee_handled::<false>()
          .weak::<true>()
          .build()?,
      ),
      _ => None,
    };

    // Get optional audio output callback
    let audio_output: Option<AudioOutputCallback> = match obj
      .get_named_property::<Option<Function<EncodedAudioChunk, UnknownReturnValue>>>("audioOutput")
    {
      Ok(Some(func)) => Some(
        func
          .build_threadsafe_function()
          .callee_handled::<false>()
          .weak::<true>()
          .build()?,
      ),
      _ => None,
    };

    // Get required error callback
    let error_func: Function<Error, UnknownReturnValue> = match obj.get_named_property("error") {
      Ok(cb) => cb,
      Err(_) => {
        env_wrapper.throw_type_error("error callback is required", None)?;
        return Err(Error::new(Status::InvalidArg, "error callback is required"));
      }
    };

    let error: ErrorCallback = error_func
      .build_threadsafe_function()
      .callee_handled::<false>()
      .weak::<true>()
      .build()?;

    Ok(Mp4DemuxerInit {
      video_output,
      audio_output,
      error,
    })
  }
}

// ============================================================================
// Mp4Demuxer - NAPI class wrapper
// ============================================================================

/// MP4 Demuxer for reading encoded video and audio from MP4 container
///
/// Usage:
/// ```javascript
/// const demuxer = new Mp4Demuxer({
///   videoOutput: (chunk) => videoDecoder.decode(chunk),
///   audioOutput: (chunk) => audioDecoder.decode(chunk),
///   error: (err) => console.error(err)
/// });
///
/// await demuxer.load('./video.mp4');
///
/// // Get decoder configs
/// const videoConfig = demuxer.videoDecoderConfig;
/// const audioConfig = demuxer.audioDecoderConfig;
///
/// // Configure decoders
/// videoDecoder.configure(videoConfig);
/// audioDecoder.configure(audioConfig);
///
/// // Start demuxing
/// demuxer.demux();
///
/// // Seek to 5 seconds
/// demuxer.seek(5_000_000);
///
/// demuxer.close();
/// ```
#[napi(async_iterator)]
pub struct Mp4Demuxer {
  inner: Arc<Mutex<DemuxerInner<Mp4Format>>>,
}

impl AsyncGenerator for Mp4Demuxer {
  type Yield = DemuxerChunk;
  type Next = ();
  type Return = ();

  fn next(
    &mut self,
    _value: Option<Self::Next>,
  ) -> impl Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
    let inner = self.inner.clone();

    async move {
      // Use spawn_blocking for the FFmpeg read operation (blocking I/O)
      tokio::task::spawn_blocking(move || {
        let mut guard = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
        guard.read_next_chunk()
      })
      .await
      .map_err(|e| Error::new(Status::GenericFailure, format!("Task error: {}", e)))?
    }
  }
}

#[napi]
impl Mp4Demuxer {
  /// Create a new MP4 demuxer
  #[napi(constructor)]
  pub fn new(init: Mp4DemuxerInit) -> Result<Self> {
    Ok(Self {
      inner: Arc::new(Mutex::new(DemuxerInner::new(
        init.video_output,
        init.audio_output,
        init.error,
      ))),
    })
  }

  /// Load an MP4 file from a path
  #[napi]
  pub async fn load(&self, path: String) -> Result<()> {
    let inner = self.inner.clone();

    tokio::task::spawn_blocking(move || {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      guard.load_file(&path)
    })
    .await
    .map_err(|e| Error::new(Status::GenericFailure, format!("Task error: {}", e)))?
  }

  /// Load an MP4 from a buffer
  ///
  /// This method uses zero-copy buffer loading - the Uint8Array data is passed
  /// directly to the demuxer without an intermediate copy.
  #[napi]
  pub async fn load_buffer(&self, data: Uint8Array) -> Result<()> {
    let inner = self.inner.clone();
    // Zero-copy: pass Uint8Array directly (it implements BufferSource)

    tokio::task::spawn_blocking(move || {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      guard.load_buffer(data)
    })
    .await
    .map_err(|e| Error::new(Status::GenericFailure, format!("Task error: {}", e)))?
  }

  /// Get all tracks
  #[napi(getter)]
  pub fn tracks(&self) -> Result<Vec<DemuxerTrackInfo>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_tracks())
  }

  /// Get container duration in microseconds
  #[napi(getter)]
  pub fn duration(&self) -> Result<Option<i64>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_duration())
  }

  /// Get video decoder configuration for the selected video track
  #[napi(getter)]
  pub fn video_decoder_config(&self) -> Result<Option<DemuxerVideoDecoderConfig>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_video_decoder_config())
  }

  /// Get audio decoder configuration for the selected audio track
  #[napi(getter)]
  pub fn audio_decoder_config(&self) -> Result<Option<DemuxerAudioDecoderConfig>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_audio_decoder_config())
  }

  /// Select a video track by index
  #[napi]
  pub fn select_video_track(&self, track_index: i32) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.select_video_track(track_index)
  }

  /// Select an audio track by index
  #[napi]
  pub fn select_audio_track(&self, track_index: i32) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.select_audio_track(track_index)
  }

  /// Start demuxing packets
  ///
  /// If count is specified, reads up to that many packets.
  /// Otherwise, reads all packets until end of stream.
  #[napi]
  pub fn demux(&self, count: Option<u32>) -> Result<()> {
    let inner = self.inner.clone();
    let max_packets = count.unwrap_or(u32::MAX);

    std::thread::spawn(move || {
      let mut guard = match inner.lock() {
        Ok(g) => g,
        Err(_) => return,
      };
      guard.demux_sync(max_packets);
    });

    Ok(())
  }

  /// Demux packets asynchronously (awaitable version of demux)
  ///
  /// If count is specified, reads up to that many packets.
  /// Otherwise, reads all packets until end of stream.
  /// Returns a Promise that resolves when demuxing is complete.
  ///
  /// This method is useful when you want to wait for demuxing to finish
  /// before proceeding with other operations.
  ///
  /// Note: For streaming use cases, prefer the async iterator pattern:
  /// `for await (const chunk of demuxer) { ... }`
  #[napi]
  pub async fn demux_async(&self, count: Option<u32>) -> Result<()> {
    let inner = self.inner.clone();
    let max_packets = count.unwrap_or(u32::MAX);

    tokio::task::spawn_blocking(move || {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      guard.demux_sync(max_packets);
      Ok(())
    })
    .await
    .map_err(|e| Error::new(Status::GenericFailure, format!("Task error: {}", e)))?
  }

  /// Seek to a timestamp in microseconds
  #[napi]
  pub fn seek(&self, timestamp_us: i64) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.seek(timestamp_us)
  }

  /// Close the demuxer and release resources
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.close();
    Ok(())
  }

  /// Get the current state of the demuxer
  #[napi(getter)]
  pub fn state(&self) -> Result<String> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.state_string().to_string())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_mp4_codec_id_to_string() {
    assert!(Mp4Format::codec_id_to_video_string(AVCodecID::H264, None).starts_with("avc1"));
    assert!(Mp4Format::codec_id_to_video_string(AVCodecID::Hevc, None).starts_with("hev1"));
    assert_eq!(
      Mp4Format::codec_id_to_video_string(AVCodecID::Vp8, None),
      "vp8"
    );
    assert!(Mp4Format::codec_id_to_video_string(AVCodecID::Vp9, None).starts_with("vp09"));
    assert!(Mp4Format::codec_id_to_video_string(AVCodecID::Av1, None).starts_with("av01"));
  }

  #[test]
  fn test_mp4_audio_codec_id_to_string() {
    assert_eq!(
      Mp4Format::codec_id_to_audio_string(AVCodecID::Aac, None),
      "mp4a.40.2"
    );
    assert_eq!(
      Mp4Format::codec_id_to_audio_string(AVCodecID::Opus, None),
      "opus"
    );
    assert_eq!(
      Mp4Format::codec_id_to_audio_string(AVCodecID::Mp3, None),
      "mp3"
    );
  }
}
