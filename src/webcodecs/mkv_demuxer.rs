//! MkvDemuxer - WebCodecs-style demuxer for Matroska containers
//!
//! Provides a JavaScript-friendly API for demuxing MKV container files.
//! MKV is a flexible container that supports almost any video and audio codec.

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
// MkvFormat - Format-specific behavior for Matroska containers
// ============================================================================

/// MKV format implementation
pub struct MkvFormat;

impl DemuxerFormat for MkvFormat {
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
// MkvDemuxerInit - Initialization options
// ============================================================================

/// Initialization options for MkvDemuxer
pub struct MkvDemuxerInit {
  pub video_output: Option<VideoOutputCallback>,
  pub audio_output: Option<AudioOutputCallback>,
  pub error: ErrorCallback,
}

impl FromNapiValue for MkvDemuxerInit {
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

    Ok(MkvDemuxerInit {
      video_output,
      audio_output,
      error,
    })
  }
}

// ============================================================================
// MkvDemuxer - NAPI class wrapper
// ============================================================================

/// MKV Demuxer for reading encoded video and audio from Matroska container
///
/// MKV supports almost any video and audio codec.
#[napi(async_iterator)]
pub struct MkvDemuxer {
  inner: Arc<Mutex<DemuxerInner<MkvFormat>>>,
}

impl AsyncGenerator for MkvDemuxer {
  type Yield = DemuxerChunk;
  type Next = ();
  type Return = ();

  fn next(
    &mut self,
    _value: Option<Self::Next>,
  ) -> impl std::future::Future<Output = Result<Option<Self::Yield>>> + Send + 'static {
    let inner = self.inner.clone();

    async move {
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
impl MkvDemuxer {
  #[napi(constructor)]
  pub fn new(init: MkvDemuxerInit) -> Result<Self> {
    Ok(Self {
      inner: Arc::new(Mutex::new(DemuxerInner::new(
        init.video_output,
        init.audio_output,
        init.error,
      ))),
    })
  }

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

  /// Load an MKV from a buffer
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

  #[napi(getter)]
  pub fn tracks(&self) -> Result<Vec<DemuxerTrackInfo>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_tracks())
  }

  #[napi(getter)]
  pub fn duration(&self) -> Result<Option<i64>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_duration())
  }

  #[napi(getter)]
  pub fn video_decoder_config(&self) -> Result<Option<DemuxerVideoDecoderConfig>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_video_decoder_config())
  }

  #[napi(getter)]
  pub fn audio_decoder_config(&self) -> Result<Option<DemuxerAudioDecoderConfig>> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.get_audio_decoder_config())
  }

  #[napi]
  pub fn select_video_track(&self, track_index: i32) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.select_video_track(track_index)
  }

  #[napi]
  pub fn select_audio_track(&self, track_index: i32) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.select_audio_track(track_index)
  }

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

  #[napi]
  pub fn seek(&self, timestamp_us: i64) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.seek(timestamp_us)
  }

  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut guard = with_demuxer_inner_mut!(self);
    guard.close();
    Ok(())
  }

  #[napi(getter)]
  pub fn state(&self) -> Result<String> {
    let guard = with_demuxer_inner!(self);
    Ok(guard.state_string().to_string())
  }
}
