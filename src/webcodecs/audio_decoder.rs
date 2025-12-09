//! AudioDecoder - WebCodecs API implementation
//!
//! Provides audio decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#audiodecoder-interface

use crate::codec::{AudioDecoderConfig as InternalAudioDecoderConfig, CodecContext, Frame, Packet};
use crate::ffi::AVCodecID;
use crate::webcodecs::error::{invalid_state_error, throw_type_error_unit};
use crate::webcodecs::promise_reject::reject_with_type_error;
use crate::webcodecs::{AudioData, AudioDecoderConfig, AudioDecoderSupport, EncodedAudioChunk};
use crossbeam::channel::{self, Receiver, Sender};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::video_encoder::CodecState;

/// Type alias for output callback (takes AudioData)
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback =
  ThreadsafeFunction<AudioData, UnknownReturnValue, AudioData, Status, false, true>;

/// Type alias for error callback (takes Error object)
/// Using CalleeHandled: false because WebCodecs error callback receives Error directly,
/// not error-first (err, result) style
type ErrorCallback = ThreadsafeFunction<Error, UnknownReturnValue, Error, Status, false, true>;

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
  /// Output callback - called when decoded audio is available (ThreadsafeFunction for worker)
  pub output: OutputCallback,
  /// Output callback reference - stored for synchronous calls from main thread
  pub output_ref: FunctionRef<AudioData, UnknownReturnValue>,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for AudioDecoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = Object::from_napi_value(env, value)?;

    // W3C spec: throw TypeError if required callbacks are missing
    // Get output callback as Function first, then create both FunctionRef and ThreadsafeFunction
    let output_func: Function<AudioData, UnknownReturnValue> =
      match obj.get_named_property("output") {
        Ok(cb) => cb,
        Err(_) => {
          env_wrapper.throw_type_error("output callback is required", None)?;
          return Err(Error::new(
            Status::InvalidArg,
            "output callback is required",
          ));
        }
      };

    // Create FunctionRef for synchronous calls from main thread (in flush resolver)
    let output_ref = output_func.create_ref()?;

    // Create ThreadsafeFunction for async calls from worker thread
    let output: OutputCallback = output_func
      .build_threadsafe_function()
      .callee_handled::<false>()
      .weak::<true>()
      .build()?;

    let error: ErrorCallback = match obj.get_named_property("error") {
      Ok(cb) => cb,
      Err(_) => {
        env_wrapper.throw_type_error("error callback is required", None)?;
        return Err(Error::new(Status::InvalidArg, "error callback is required"));
      }
    };

    Ok(AudioDecoderInit {
      output,
      output_ref,
      error,
    })
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
  /// Whether an error has occurred during decoding (for flush error propagation)
  had_error: bool,
  /// Pending flush response senders (for AbortError on reset)
  pending_flush_senders: Vec<Sender<Result<()>>>,
  /// Atomic flag for flush abort - set by reset() to signal pending flush to abort
  flush_abort_flag: Option<Arc<AtomicBool>>,
  /// Queue of decoded audio data waiting to be delivered via output callback
  /// Worker pushes data here during flush; flush() drains them synchronously via FunctionRef
  pending_data: Vec<AudioData>,
  /// Flag indicating whether a flush operation is in progress
  /// When true, worker queues data to pending_data instead of calling NonBlocking callback
  inside_flush: bool,
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
  /// Output callback reference - stored for synchronous calls from main thread (in flush resolver)
  /// Wrapped in Rc to allow sharing with spawn_future_with_callback closure
  /// (Rc is !Send but that's OK - the callback runs on the main thread)
  output_callback_ref: Rc<FunctionRef<AudioData, UnknownReturnValue>>,
  /// Channel sender for worker commands
  command_sender: Option<Sender<DecoderCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
}

impl Drop for AudioDecoder {
  fn drop(&mut self) {
    // Signal worker to stop
    self.command_sender = None;

    // Wait for worker to finish (brief block, necessary for safety)
    if let Some(handle) = self.worker_handle.take() {
      let _ = handle.join();
    }

    // Drain decoder to ensure codec threads finish before context drops.
    // This prevents potential SIGSEGV with codecs that use internal threads.
    if let Ok(mut inner) = self.inner.lock() {
      if let Some(ctx) = inner.context.as_mut() {
        ctx.flush();
        let _ = ctx.send_packet(None);
        while ctx.receive_frame().ok().flatten().is_some() {}
      }
    }
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
      had_error: false,
      pending_flush_senders: Vec::new(),
      flush_abort_flag: None,
      pending_data: Vec::new(),
      inside_flush: false,
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
      output_callback_ref: Rc::new(init.output_ref),
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
      let old_size = guard.decode_queue_size;
      guard.decode_queue_size = old_size.saturating_sub(1);
      if old_size > 0 {
        let _ = Self::fire_dequeue_event(&guard);
      }
      Self::report_error(&mut guard, "Decoder not configured");
      return;
    }

    // W3C spec: Empty data should trigger EncodingError
    if data.is_empty() {
      let old_size = guard.decode_queue_size;
      guard.decode_queue_size = old_size.saturating_sub(1);
      if old_size > 0 {
        let _ = Self::fire_dequeue_event(&guard);
      }
      Self::report_error(
        &mut guard,
        "EncodingError: Cannot decode empty audio chunk data",
      );
      return;
    }

    // Get context
    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        let old_size = guard.decode_queue_size;
        guard.decode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(&guard);
        }
        Self::report_error(&mut guard, "No decoder context");
        return;
      }
    };

    // Decode using the internal implementation
    let frames = match decode_audio_chunk_data(context, &data, timestamp) {
      Ok(f) => f,
      Err(e) => {
        let old_size = guard.decode_queue_size;
        guard.decode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(&guard);
        }
        Self::report_error(&mut guard, &format!("Decode failed: {}", e));
        return;
      }
    };

    guard.frame_count += 1;

    // Decrement queue size and fire dequeue event (only if queue was not empty)
    let old_size = guard.decode_queue_size;
    guard.decode_queue_size = old_size.saturating_sub(1);
    if old_size > 0 {
      let _ = Self::fire_dequeue_event(&guard);
    }

    // Convert internal frames to AudioData and deliver
    for frame in frames {
      let pts = frame.pts();
      let audio_data = AudioData::from_internal(frame, pts);

      // During flush, queue data for synchronous delivery in resolver
      // Otherwise, use NonBlocking callback for immediate delivery
      if guard.inside_flush {
        guard.pending_data.push(audio_data);
      } else {
        guard
          .output_callback
          .call(audio_data, ThreadsafeFunctionCallMode::NonBlocking);
      }
    }
  }

  /// Process a flush command on the worker thread
  fn process_flush(inner: &Arc<Mutex<AudioDecoderInner>>) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: flush() should reject if decoder is not in configured state
    if guard.state != CodecState::Configured {
      // If closed due to error, return EncodingError; otherwise InvalidStateError
      if guard.state == CodecState::Closed && guard.had_error {
        return Err(Error::new(
          Status::GenericFailure,
          "EncodingError: Decode error occurred",
        ));
      }
      return Err(Error::new(
        Status::GenericFailure,
        "InvalidStateError: Decoder is not configured",
      ));
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

    // Queue remaining frames for delivery (always queue during flush for synchronous delivery)
    for frame in frames {
      let pts = frame.pts();
      let audio_data = AudioData::from_internal(frame, pts);
      // Always queue during flush for synchronous delivery in resolver
      guard.pending_data.push(audio_data);
    }

    // Reset decoder state so it can accept more data (per W3C spec, flush should leave
    // decoder in configured state, ready for more decode() calls)
    if let Some(ref mut context) = guard.context {
      context.flush();
    }

    Ok(())
  }

  /// Report an error via callback and close the decoder
  fn report_error(inner: &mut AudioDecoderInner, error_msg: &str) {
    // Create an Error object that will be passed directly to the JS callback
    let error = Error::new(Status::GenericFailure, error_msg);
    inner
      .error_callback
      .call(error, ThreadsafeFunctionCallMode::NonBlocking);
    inner.had_error = true;
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
  pub fn configure(&self, env: Env, config: AudioDecoderConfig) -> Result<()> {
    // W3C WebCodecs spec: Validate config synchronously, throw TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-audiodecoder-configure

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      _ => return throw_type_error_unit(&env, "codec is required"),
    };

    // Validate sample rate - must be present and greater than 0
    let sample_rate = match config.sample_rate {
      Some(sr) if sr > 0.0 => sr,
      Some(_) => return throw_type_error_unit(&env, "sampleRate must be greater than 0"),
      None => return throw_type_error_unit(&env, "sampleRate is required"),
    };

    // Validate number of channels - must be present and greater than 0
    let number_of_channels = match config.number_of_channels {
      Some(nc) if nc > 0 => nc,
      Some(_) => return throw_type_error_unit(&env, "numberOfChannels must be greater than 0"),
      None => return throw_type_error_unit(&env, "numberOfChannels is required"),
    };

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: throw InvalidStateError if closed
    if inner.state == CodecState::Closed {
      return Err(invalid_state_error("Decoder is closed"));
    }

    // Parse codec string to determine codec ID
    let codec_id = match parse_audio_codec_string(&codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_error(
          &mut inner,
          &format!("NotSupportedError: Invalid codec: {}", e),
        );
        return Ok(());
      }
    };

    // W3C WebCodecs spec: FLAC codec requires description (contains STREAMINFO)
    let codec_lower = codec.to_lowercase();
    if codec_lower == "flac" && config.description.is_none() {
      Self::report_error(
        &mut inner,
        "NotSupportedError: FLAC codec requires a description (STREAMINFO)",
      );
      return Ok(());
    }

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
      sample_rate: sample_rate as u32,
      channels: number_of_channels,
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
    inner.codec_string = codec;
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

      // W3C spec: throw InvalidStateError if not configured or closed
      if inner.state == CodecState::Closed {
        return Err(invalid_state_error("Cannot decode with a closed codec"));
      }
      if inner.state != CodecState::Configured {
        return Err(invalid_state_error(
          "Cannot decode with an unconfigured codec",
        ));
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
  ///
  /// Uses spawn_future_with_callback to check abort flag synchronously in the resolver.
  /// This ensures that if reset() is called from a callback, the abort flag is checked
  /// AFTER the callback returns, allowing flush() to return AbortError.
  #[napi(ts_return_type = "Promise<void>")]
  pub fn flush<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
    // Create abort flag for this flush operation
    let flush_abort_flag = Arc::new(AtomicBool::new(false));

    // W3C spec: Check state upfront and return rejected promise with appropriate error
    // (not throw synchronously - flush() should always return a promise)
    {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        // If closed due to error, return EncodingError; otherwise InvalidStateError
        let error_msg = if inner.had_error {
          "EncodingError: Decode error occurred"
        } else {
          "InvalidStateError: Cannot flush a closed codec"
        };
        // Return rejected promise via async to allow error callback to run first
        return env
          .spawn_future_with_callback(async move { Ok(()) }, move |_env, _| -> Result<()> {
            Err(Error::new(Status::GenericFailure, error_msg))
          });
      }
      if inner.state == CodecState::Unconfigured {
        // Return rejected promise via async to allow error callback to run first
        return env.spawn_future_with_callback(
          async move { Ok(()) },
          move |_env, _| -> Result<()> {
            Err(invalid_state_error("Cannot flush an unconfigured codec"))
          },
        );
      }

      // Store abort flag for reset() to access
      inner.flush_abort_flag = Some(flush_abort_flag.clone());
      // Set inside_flush flag so worker queues data instead of calling NonBlocking callback
      inner.inside_flush = true;
    }

    // Create a response channel
    let (response_sender, response_receiver) = channel::bounded::<Result<()>>(1);

    // Track this flush for AbortError on reset()
    {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      inner.pending_flush_senders.push(response_sender.clone());
    }

    // Send flush command through the channel to ensure it's processed after all pending decodes
    if let Some(ref sender) = self.command_sender {
      sender
        .send(DecoderCommand::Flush(response_sender))
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?;
    } else {
      return Err(invalid_state_error("Cannot flush a closed codec"));
    }

    // Clone references for the callback closure
    let inner_clone = self.inner.clone();
    let output_callback_ref = self.output_callback_ref.clone();

    env.spawn_future_with_callback(
      async move {
        // Wait for worker response in a blocking thread
        let result = spawn_blocking(move || {
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
        .flatten();

        Ok((result, inner_clone, flush_abort_flag))
      },
      move |env, (result, inner, abort_flag)| {
        // Drain pending data and call output callback SYNCHRONOUSLY
        // This runs on the main thread with Env access
        let data_items = {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          std::mem::take(&mut guard.pending_data)
        };

        // Call output callback for each data item synchronously
        // If callback calls reset(), abort_flag will be set before next iteration
        let callback = output_callback_ref.borrow_back(env)?;
        for data in data_items {
          // Check abort flag before each callback - exit early if reset() was called
          if abort_flag.load(Ordering::SeqCst) {
            break;
          }
          callback.call(data)?;
        }

        // Clean up flags
        {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          guard.flush_abort_flag = None;
          guard.inside_flush = false;
        }

        // Check abort flag after draining all data
        if abort_flag.load(Ordering::SeqCst) {
          return Err(Error::new(
            Status::GenericFailure,
            "AbortError: The operation was aborted",
          ));
        }

        // Return worker result
        result
      },
    )
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

      // W3C spec: throw InvalidStateError if closed
      if inner.state == CodecState::Closed {
        return Err(invalid_state_error("Cannot reset a closed codec"));
      }

      // Set abort flag FIRST (synchronously, before any other reset logic)
      // This signals any pending flush() that is yielding to return AbortError
      if let Some(ref flag) = inner.flush_abort_flag {
        flag.store(true, Ordering::SeqCst);
      }
    }

    // W3C spec: Abort all pending flushes with AbortError BEFORE dropping sender
    {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      for sender in inner.pending_flush_senders.drain(..) {
        let _ = sender.send(Err(Error::new(
          Status::GenericFailure,
          "AbortError: The operation was aborted",
        )));
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
    inner.had_error = false;

    // Clear flush-related state
    inner.inside_flush = false;
    inner.pending_data.clear();

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
    // Check state first - W3C spec: throw InvalidStateError if already closed
    {
      let inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        return Err(invalid_state_error("Cannot close an already closed codec"));
      }
    }

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
  ///
  /// W3C WebCodecs spec: Rejects with TypeError for invalid configs,
  /// returns { supported: false } for valid but unsupported configs.
  #[napi]
  pub fn is_config_supported<'env>(
    env: &'env Env,
    config: AudioDecoderConfig,
  ) -> Result<PromiseRaw<'env, AudioDecoderSupport>> {
    // W3C WebCodecs spec: Validate config, reject with TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-audiodecoder-isconfigsupported

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      Some(_) => return reject_with_type_error(env, "codec is required"),
      None => return reject_with_type_error(env, "codec is required"),
    };

    // Validate sample rate - must be present and greater than 0
    match config.sample_rate {
      Some(sr) if sr > 0.0 => {}
      Some(_) => return reject_with_type_error(env, "sampleRate must be greater than 0"),
      None => return reject_with_type_error(env, "sampleRate is required"),
    };

    // Validate number of channels - must be present and greater than 0
    match config.number_of_channels {
      Some(nc) if nc > 0 => {}
      Some(_) => return reject_with_type_error(env, "numberOfChannels must be greater than 0"),
      None => return reject_with_type_error(env, "numberOfChannels is required"),
    };

    env.spawn_future(async move {
      // Parse codec string
      let codec_id = match parse_audio_codec_string(&codec) {
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
