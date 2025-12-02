//! AudioDecoder - WebCodecs API implementation
//!
//! Provides audio decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#audiodecoder-interface

use crate::codec::{AudioDecoderConfig as InternalAudioDecoderConfig, CodecContext, Frame, Packet};
use crate::ffi::AVCodecID;
use crate::webcodecs::{AudioData, AudioDecoderConfig, AudioDecoderSupport, EncodedAudioChunk};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::sync::{Arc, Mutex};

use super::video_encoder::CodecState;

/// Type alias for output callback (takes AudioData)
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback =
  ThreadsafeFunction<AudioData, UnknownReturnValue, AudioData, Status, false, true>;

/// Type alias for error callback (takes error message)
/// Still using default CalleeHandled: true for error-first convention
type ErrorCallback = ThreadsafeFunction<String, UnknownReturnValue, String, Status, true, true>;

// Note: For ondequeue, we use FunctionRef instead of ThreadsafeFunction
// to support both getter and setter per WebCodecs spec

/// AudioDecoder init dictionary per WebCodecs spec
pub struct AudioDecoderInit {
  /// Output callback - called when decoded audio is available
  pub output: OutputCallback,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for AudioDecoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let obj = Object::from_napi_value(env, value)?;

    let output: OutputCallback = obj
      .get_named_property("output")
      .map_err(|_| Error::new(Status::InvalidArg, "Missing required 'output' callback"))?;

    let error: ErrorCallback = obj
      .get_named_property("error")
      .map_err(|_| Error::new(Status::InvalidArg, "Missing required 'error' callback"))?;

    Ok(AudioDecoderInit { output, error })
  }
}

/// Internal decoder state
struct AudioDecoderInner {
  state: CodecState,
  config: Option<InternalAudioDecoderConfig>,
  context: Option<CodecContext>,
  codec_string: String,
  frame_count: u64,
  /// Number of pending decode operations (for decodeQueueSize)
  decode_queue_size: u32,
  /// Output callback (required per spec)
  output_callback: OutputCallback,
  /// Error callback (required per spec)
  error_callback: ErrorCallback,
  /// Optional dequeue event callback (ThreadsafeFunction for multi-thread support)
  dequeue_callback: Option<ThreadsafeFunction<(), UnknownReturnValue, (), Status, false, true>>,
}

/// AudioDecoder - WebCodecs-compliant audio decoder
///
/// Decodes EncodedAudioChunk objects into AudioData objects using FFmpeg.
///
/// Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
///
/// Example:
/// ```javascript
/// const decoder = new AudioDecoder({
///   output: (data) => { console.log('decoded audio', data); },
///   error: (e) => { console.error('error', e); }
/// });
///
/// decoder.configure({
///   codec: 'opus',
///   sampleRate: 48000,
///   numberOfChannels: 2
/// });
///
/// decoder.decode(chunk);
/// await decoder.flush();
/// ```
#[napi]
pub struct AudioDecoder {
  inner: Arc<Mutex<AudioDecoderInner>>,
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
}

#[napi]
impl AudioDecoder {
  /// Create a new AudioDecoder with init dictionary (per WebCodecs spec)
  ///
  /// @param init - Init dictionary containing output and error callbacks
  #[napi(constructor)]
  pub fn new(
    #[napi(ts_arg_type = "{ output: (data: AudioData) => void, error: (error: Error) => void }")]
    init: AudioDecoderInit,
  ) -> Result<Self> {
    let inner = AudioDecoderInner {
      state: CodecState::Unconfigured,
      config: None,
      context: None,
      codec_string: String::new(),
      frame_count: 0,
      decode_queue_size: 0,
      output_callback: init.output,
      error_callback: init.error,
      dequeue_callback: None,
    };

    Ok(Self {
      inner: Arc::new(Mutex::new(inner)),
      dequeue_callback: None,
    })
  }

  /// Report an error via callback and close the decoder
  fn report_error(inner: &mut AudioDecoderInner, error_msg: &str) {
    inner.error_callback.call(
      Ok(error_msg.to_string()),
      ThreadsafeFunctionCallMode::NonBlocking,
    );
    inner.state = CodecState::Closed;
  }

  /// Fire dequeue event if callback is set
  fn fire_dequeue_event(inner: &AudioDecoderInner) -> Result<()> {
    if let Some(ref callback) = inner.dequeue_callback {
      callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
    }
    Ok(())
  }

  /// Get decoder state
  #[napi(getter)]
  pub fn state(&self) -> Result<CodecState> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.state)
  }

  /// Get number of pending decode operations (per WebCodecs spec)
  #[napi(getter)]
  pub fn decode_queue_size(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.decode_queue_size)
  }

  /// Set the dequeue event handler (per WebCodecs spec)
  ///
  /// The dequeue event fires when decodeQueueSize decreases,
  /// allowing backpressure management.
  #[napi(setter)]
  pub fn set_ondequeue(
    &mut self,
    env: &Env,
    callback: Option<FunctionRef<(), UnknownReturnValue>>,
  ) -> Result<()> {
    if let Some(ref callback) = callback {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      inner.dequeue_callback = Some(
        callback
          .borrow_back(env)?
          .build_threadsafe_function()
          .callee_handled::<false>()
          .weak::<true>()
          .build()?,
      );
    }
    self.dequeue_callback = callback;

    Ok(())
  }

  /// Get the dequeue event handler (per WebCodecs spec)
  #[napi(getter)]
  pub fn get_ondequeue<'env>(
    &self,
    env: &'env Env,
  ) -> Result<Option<Function<'env, (), UnknownReturnValue>>> {
    if let Some(ref callback) = self.dequeue_callback {
      let cb = callback.borrow_back(env)?;
      Ok(Some(cb))
    } else {
      Ok(None)
    }
  }

  /// Configure the decoder
  #[napi]
  pub fn configure(&self, config: AudioDecoderConfig) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state == CodecState::Closed {
      Self::report_error(&mut inner, "Decoder is closed");
      return Ok(());
    }

    // Parse codec string to determine codec ID
    let codec_id = match parse_audio_codec_string(&config.codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Invalid codec: {}", e));
        return Ok(());
      }
    };

    // Create decoder context
    let mut context = match CodecContext::new_decoder(codec_id) {
      Ok(ctx) => ctx,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Failed to create decoder: {}", e));
        return Ok(());
      }
    };

    // Configure decoder
    let decoder_config = InternalAudioDecoderConfig {
      codec_id,
      sample_rate: config.sample_rate,
      channels: config.number_of_channels,
      thread_count: 0, // Auto
      extradata: config.description.as_ref().map(|d| d.to_vec()),
    };

    if let Err(e) = context.configure_audio_decoder(&decoder_config) {
      Self::report_error(&mut inner, &format!("Failed to configure decoder: {}", e));
      return Ok(());
    }

    // Open the decoder
    if let Err(e) = context.open() {
      Self::report_error(&mut inner, &format!("Failed to open decoder: {}", e));
      return Ok(());
    }

    inner.context = Some(context);
    inner.config = Some(decoder_config);
    inner.codec_string = config.codec;
    inner.state = CodecState::Configured;
    inner.frame_count = 0;
    inner.decode_queue_size = 0;

    Ok(())
  }

  /// Decode an encoded audio chunk
  #[napi]
  pub fn decode(&self, chunk: &EncodedAudioChunk) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state != CodecState::Configured {
      Self::report_error(&mut inner, "Decoder not configured");
      return Ok(());
    }

    // Increment queue size (pending operation)
    inner.decode_queue_size += 1;

    // Get chunk data
    let data = match chunk.get_data_vec() {
      Ok(d) => d,
      Err(e) => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(&inner)?;
        Self::report_error(&mut inner, &format!("Failed to get chunk data: {}", e));
        return Ok(());
      }
    };
    let timestamp = match chunk.get_timestamp() {
      Ok(ts) => ts,
      Err(e) => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(&inner)?;
        Self::report_error(&mut inner, &format!("Failed to get timestamp: {}", e));
        return Ok(());
      }
    };

    // Get context
    let context = match inner.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(&inner)?;
        Self::report_error(&mut inner, "No decoder context");
        return Ok(());
      }
    };

    // Decode using the internal implementation
    let frames = match decode_audio_chunk_data(context, &data, timestamp) {
      Ok(f) => f,
      Err(e) => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(&inner)?;
        Self::report_error(&mut inner, &format!("Decode failed: {}", e));
        return Ok(());
      }
    };

    inner.frame_count += 1;

    // Decrement queue size and fire dequeue event
    inner.decode_queue_size -= 1;
    Self::fire_dequeue_event(&inner)?;

    // Convert internal frames to AudioData and call output callback
    for frame in frames {
      let pts = frame.pts();
      let audio_data = AudioData::from_internal(frame, pts);

      // Call output callback (CalleeHandled: false means direct value, not Result)
      inner
        .output_callback
        .call(audio_data, ThreadsafeFunctionCallMode::NonBlocking);
    }

    Ok(())
  }

  /// Flush the decoder
  /// Returns a Promise that resolves when flushing is complete
  #[napi]
  pub async fn flush(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state != CodecState::Configured {
      Self::report_error(&mut inner, "Decoder not configured");
      return Ok(());
    }

    let context = match inner.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut inner, "No decoder context");
        return Ok(());
      }
    };

    // Flush decoder
    let frames = match context.flush_decoder() {
      Ok(f) => f,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Flush failed: {}", e));
        return Ok(());
      }
    };

    // Convert and call output callback for remaining frames
    for frame in frames {
      let pts = frame.pts();
      let audio_data = AudioData::from_internal(frame, pts);

      // Call output callback (CalleeHandled: false means direct value, not Result)
      inner
        .output_callback
        .call(audio_data, ThreadsafeFunctionCallMode::NonBlocking);
    }

    Ok(())
  }

  /// Reset the decoder
  #[napi]
  pub fn reset(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state == CodecState::Closed {
      Self::report_error(&mut inner, "Decoder is closed");
      return Ok(());
    }

    // Drop existing context
    inner.context = None;
    inner.config = None;
    inner.codec_string.clear();
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    inner.decode_queue_size = 0;

    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    inner.context = None;
    inner.config = None;
    inner.codec_string.clear();
    inner.state = CodecState::Closed;
    inner.decode_queue_size = 0;

    Ok(())
  }

  /// Check if a configuration is supported
  /// Returns a Promise that resolves with support information
  #[napi]
  pub async fn is_config_supported(config: AudioDecoderConfig) -> Result<AudioDecoderSupport> {
    // Parse codec string
    let codec_id = match parse_audio_codec_string(&config.codec) {
      Ok(id) => id,
      Err(_) => {
        return Ok(AudioDecoderSupport {
          supported: false,
          config,
        });
      }
    };

    // Try to create decoder
    let result = CodecContext::new_decoder(codec_id);

    Ok(AudioDecoderSupport {
      supported: result.is_ok(),
      config,
    })
  }
}

/// Parse WebCodecs audio codec string to FFmpeg codec ID
fn parse_audio_codec_string(codec: &str) -> Result<AVCodecID> {
  let codec_lower = codec.to_lowercase();

  // AAC variants
  if codec_lower.starts_with("mp4a.40") || codec_lower == "aac" {
    return Ok(AVCodecID::Aac);
  }

  // Opus
  if codec_lower == "opus" {
    return Ok(AVCodecID::Opus);
  }

  // MP3
  if codec_lower == "mp3" || codec_lower == "mp4a.6b" {
    return Ok(AVCodecID::Mp3);
  }

  // FLAC
  if codec_lower == "flac" {
    return Ok(AVCodecID::Flac);
  }

  // Vorbis
  if codec_lower == "vorbis" {
    return Ok(AVCodecID::Vorbis);
  }

  // PCM variants
  if codec_lower == "pcm-s16" || codec_lower == "pcm_s16le" {
    return Ok(AVCodecID::PcmS16le);
  }
  if codec_lower == "pcm-f32" || codec_lower == "pcm_f32le" {
    return Ok(AVCodecID::PcmF32le);
  }

  // AC3/E-AC3
  if codec_lower == "ac3" || codec_lower == "ac-3" {
    return Ok(AVCodecID::Ac3);
  }

  // ALAC (Apple Lossless)
  if codec_lower == "alac" {
    return Ok(AVCodecID::Alac);
  }

  Err(Error::new(
    Status::GenericFailure,
    format!("Unsupported audio codec: {}", codec),
  ))
}

/// Decode audio chunk data using FFmpeg
fn decode_audio_chunk_data(
  context: &mut CodecContext,
  data: &[u8],
  timestamp: i64,
) -> Result<Vec<Frame>> {
  // Create a packet and fill it with data
  let mut packet = Packet::new().map_err(|e| {
    Error::new(
      Status::GenericFailure,
      format!("Failed to create packet: {}", e),
    )
  })?;

  // Set packet timestamps
  packet.set_pts(timestamp);
  packet.set_dts(timestamp);

  // Allocate and copy data to packet using safe wrapper
  packet.copy_data_from(data).map_err(|e| {
    Error::new(
      Status::GenericFailure,
      format!("Failed to copy packet data: {}", e),
    )
  })?;

  // Decode
  let frames = context
    .decode(Some(&packet))
    .map_err(|e| Error::new(Status::GenericFailure, format!("Decode failed: {}", e)))?;

  Ok(frames)
}
