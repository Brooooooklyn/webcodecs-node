//! AudioDecoder - WebCodecs API implementation
//!
//! Provides audio decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#audiodecoder-interface

use crate::codec::{AudioDecoderConfig as InternalAudioDecoderConfig, CodecContext, Frame, Packet};
use crate::ffi::AVCodecID;
use crate::webcodecs::{AudioData, AudioDecoderConfig, AudioDecoderSupport, EncodedAudioChunk};
use crossbeam::channel::{self, Receiver, Sender};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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

/// Commands sent to the worker thread
enum DecoderCommand {
  /// Decode an audio chunk
  Decode { data: Vec<u8>, timestamp: i64 },
  /// Flush the decoder and send result back via response channel
  Flush(Sender<Result<()>>),
}

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
  /// Channel sender for worker commands
  command_sender: Option<Sender<DecoderCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
}

impl Drop for AudioDecoder {
  fn drop(&mut self) {
    // Drop the sender to signal the worker to stop.
    // The worker will see the channel disconnect and exit its loop.
    self.command_sender = None;

    // Don't join the worker thread here - it would block the JS thread during GC.
    // Instead, let the thread become detached and finish on its own.
    // Safety: The Arc<Mutex<AudioDecoderInner>> ensures the inner state (including
    // callbacks and FFmpeg context) stays alive until the worker exits.
  }
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

    let inner = Arc::new(Mutex::new(inner));

    // Create channel for worker commands
    let (sender, receiver) = channel::unbounded();

    // Spawn worker thread
    let worker_inner = inner.clone();
    let worker_handle = std::thread::spawn(move || {
      Self::worker_loop(worker_inner, receiver);
    });

    Ok(Self {
      inner,
      dequeue_callback: None,
      command_sender: Some(sender),
      worker_handle: Some(worker_handle),
    })
  }

  /// Worker loop that processes commands from the channel
  fn worker_loop(inner: Arc<Mutex<AudioDecoderInner>>, receiver: Receiver<DecoderCommand>) {
    while let Ok(command) = receiver.recv() {
      match command {
        DecoderCommand::Decode { data, timestamp } => {
          Self::process_decode(&inner, data, timestamp);
        }
        DecoderCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner);
          let _ = response_sender.send(result);
        }
      }
    }
  }

  /// Process a decode command on the worker thread
  fn process_decode(inner: &Arc<Mutex<AudioDecoderInner>>, data: Vec<u8>, timestamp: i64) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Check if decoder is still configured
    if guard.state != CodecState::Configured {
      guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
      let _ = Self::fire_dequeue_event(&guard);
      Self::report_error(&mut guard, "Decoder not configured");
      return;
    }

    // Get context
    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
        Self::report_error(&mut guard, "No decoder context");
        return;
      }
    };

    // Decode using the internal implementation
    let frames = match decode_audio_chunk_data(context, &data, timestamp) {
      Ok(f) => f,
      Err(e) => {
        guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
        Self::report_error(&mut guard, &format!("Decode failed: {}", e));
        return;
      }
    };

    guard.frame_count += 1;

    // Decrement queue size and fire dequeue event
    guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
    let _ = Self::fire_dequeue_event(&guard);

    // Convert internal frames to AudioData and call output callback
    for frame in frames {
      let pts = frame.pts();
      let audio_data = AudioData::from_internal(frame, pts);

      // Call output callback (CalleeHandled: false means direct value, not Result)
      guard
        .output_callback
        .call(audio_data, ThreadsafeFunctionCallMode::NonBlocking);
    }
  }

  /// Process a flush command on the worker thread
  fn process_flush(inner: &Arc<Mutex<AudioDecoderInner>>) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if guard.state != CodecState::Configured {
      Self::report_error(&mut guard, "Decoder not configured");
      return Ok(());
    }

    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut guard, "No decoder context");
        return Ok(());
      }
    };

    // Flush decoder
    let frames = match context.flush_decoder() {
      Ok(f) => f,
      Err(e) => {
        Self::report_error(&mut guard, &format!("Flush failed: {}", e));
        return Ok(());
      }
    };

    // Convert and call output callback for remaining frames
    for frame in frames {
      let pts = frame.pts();
      let audio_data = AudioData::from_internal(frame, pts);

      // Call output callback (CalleeHandled: false means direct value, not Result)
      guard
        .output_callback
        .call(audio_data, ThreadsafeFunctionCallMode::NonBlocking);
    }

    Ok(())
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

    // Configure decoder (cast f64 sample_rate to u32 for FFmpeg)
    let decoder_config = InternalAudioDecoderConfig {
      codec_id,
      sample_rate: config.sample_rate as u32,
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
    // Extract data and timestamp on main thread (brief lock)
    let (data, timestamp) = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state != CodecState::Configured {
        Self::report_error(&mut inner, "Decoder not configured");
        return Ok(());
      }

      // Get chunk data
      let data = match chunk.get_data_vec() {
        Ok(d) => d,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get chunk data: {}", e));
          return Ok(());
        }
      };
      let timestamp = match chunk.get_timestamp() {
        Ok(ts) => ts,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get timestamp: {}", e));
          return Ok(());
        }
      };

      // Increment queue size (pending operation)
      inner.decode_queue_size += 1;

      (data, timestamp)
    };

    // Send decode command to worker thread
    if let Some(ref sender) = self.command_sender {
      sender
        .send(DecoderCommand::Decode { data, timestamp })
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?;
    } else {
      return Err(Error::new(
        Status::GenericFailure,
        "Decoder has been closed",
      ));
    }

    Ok(())
  }

  /// Flush the decoder
  /// Returns a Promise that resolves when flushing is complete
  #[napi]
  pub async fn flush(&self) -> Result<()> {
    // Create a response channel
    let (response_sender, response_receiver) = channel::bounded::<Result<()>>(1);

    // Send flush command through the channel to ensure it's processed after all pending decodes
    if let Some(ref sender) = self.command_sender {
      sender
        .send(DecoderCommand::Flush(response_sender))
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?;
    } else {
      return Err(Error::new(
        Status::GenericFailure,
        "Decoder has been closed",
      ));
    }

    // Wait for response in a blocking thread to not block the event loop
    spawn_blocking(move || {
      response_receiver
        .recv()
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?
    })
    .await
    .map_err(|join_error| {
      Error::new(
        Status::GenericFailure,
        format!("Flush failed: {}", join_error),
      )
    })
    .flatten()
  }

  /// Reset the decoder
  #[napi]
  pub fn reset(&mut self) -> Result<()> {
    // Check state first before touching the worker
    {
      let inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        return Err(Error::new(Status::GenericFailure, "Decoder is closed"));
      }
    }

    // Drop sender to signal worker to stop (must drop before join!)
    drop(self.command_sender.take());

    // Wait for worker to finish processing remaining commands
    if let Some(handle) = self.worker_handle.take() {
      let _ = handle.join();
    }

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Drop existing context
    inner.context = None;
    inner.config = None;
    inner.codec_string.clear();
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    inner.decode_queue_size = 0;

    // Create new channel and worker for future decode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(sender);
    let worker_inner = self.inner.clone();
    drop(inner); // Release lock before spawning thread
    self.worker_handle = Some(std::thread::spawn(move || {
      Self::worker_loop(worker_inner, receiver);
    }));

    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&mut self) -> Result<()> {
    // Drop sender to stop accepting new commands
    self.command_sender = None;

    // Wait for worker to finish processing remaining tasks
    if let Some(handle) = self.worker_handle.take() {
      let _ = handle.join();
    }

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
