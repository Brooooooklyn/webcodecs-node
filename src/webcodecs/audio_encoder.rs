//! AudioEncoder - WebCodecs API implementation
//!
//! Provides audio encoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#audioencoder-interface

use crate::codec::{
  context::get_audio_encoder_name, AudioEncoderConfig as InternalAudioEncoderConfig,
  AudioSampleBuffer, CodecContext, Frame, Resampler,
};
use crate::ffi::{AVCodecID, AVSampleFormat};
use crate::webcodecs::error::{invalid_state_error, throw_type_error_unit};
use crate::webcodecs::promise_reject::reject_with_type_error;
use crate::webcodecs::{AudioData, AudioEncoderConfig, AudioEncoderSupport, EncodedAudioChunk};
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

/// Type alias for output callback (takes chunk and metadata as separate args)
/// Using FnArgs to spread tuple as separate callback arguments per WebCodecs spec
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback = ThreadsafeFunction<
  FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
  UnknownReturnValue,
  FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
  Status,
  false,
  true,
>;

/// Type alias for error callback (takes Error object)
/// Using CalleeHandled: false because WebCodecs error callback receives Error directly,
/// not error-first (err, result) style
type ErrorCallback = ThreadsafeFunction<Error, UnknownReturnValue, Error, Status, false, true>;

/// AudioEncoder init dictionary per WebCodecs spec
pub struct AudioEncoderInit {
  /// Output callback - called when encoded chunk is available (ThreadsafeFunction for worker)
  pub output: OutputCallback,
  /// Output callback reference - stored for synchronous calls from main thread
  pub output_ref:
    FunctionRef<FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>, UnknownReturnValue>,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for AudioEncoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = Object::from_napi_value(env, value)?;

    // W3C spec: throw TypeError if required callbacks are missing
    // Get output callback as Function first, then create both FunctionRef and ThreadsafeFunction
    let output_func: Function<
      FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
      UnknownReturnValue,
    > = match obj.get_named_property("output") {
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

    Ok(AudioEncoderInit {
      output,
      output_ref,
      error,
    })
  }
}

/// Output callback metadata for audio
#[napi(object)]
pub struct EncodedAudioChunkMetadata {
  /// Decoder configuration for this chunk
  pub decoder_config: Option<AudioDecoderConfigOutput>,
}

/// Decoder configuration output (for passing to decoder)
#[napi(object)]
pub struct AudioDecoderConfigOutput {
  /// Codec string
  pub codec: String,
  /// Sample rate - W3C spec uses float
  pub sample_rate: Option<f64>,
  /// Number of channels
  pub number_of_channels: Option<u32>,
  /// Codec description (e.g., AudioSpecificConfig for AAC) - Uint8Array per spec
  pub description: Option<Uint8Array>,
}

/// Encode options for audio
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AudioEncoderEncodeOptions {
  // Currently no options defined in WebCodecs spec for audio
}

/// Commands sent to the worker thread
enum EncoderCommand {
  /// Encode an audio frame
  Encode { frame: Frame, timestamp: i64 },
  /// Flush the encoder and send result back via response channel
  Flush(Sender<Result<()>>),
}

/// Internal encoder state
struct AudioEncoderInner {
  state: CodecState,
  config: Option<AudioEncoderConfig>,
  context: Option<CodecContext>,
  resampler: Option<Resampler>,
  sample_buffer: Option<AudioSampleBuffer>,
  frame_count: u64,
  extradata_sent: bool,
  /// Target sample format for encoder
  target_format: AVSampleFormat,
  /// Number of pending encode operations (for encodeQueueSize)
  encode_queue_size: u32,
  /// Output callback (required per spec)
  output_callback: OutputCallback,
  /// Error callback (required per spec)
  error_callback: ErrorCallback,
  /// Optional dequeue event callback (Arc for cloning across mutex boundary)
  dequeue_callback:
    Option<Arc<ThreadsafeFunction<(), UnknownReturnValue, (), Status, false, true>>>,
  /// Pending flush response senders (for AbortError on reset)
  pending_flush_senders: Vec<Sender<Result<()>>>,
  /// Queue of input timestamps for correlation with output packets
  /// (needed because FFmpeg may buffer frames internally)
  timestamp_queue: std::collections::VecDeque<i64>,
  /// Base timestamp from the first input AudioData (for timestamp calculation)
  base_timestamp: Option<i64>,
  /// Abort channel senders - reset() sends abort signal through these
  pending_abort_senders: Vec<Sender<()>>,
  /// Atomic flag for flush abort - set by reset() to signal pending flush to abort
  flush_abort_flag: Option<Arc<AtomicBool>>,
  /// W3C spec: Flag to suppress output delivery after reset()
  /// When true, pending outputs are not delivered to the user callback
  #[allow(dead_code)]
  output_suppressed: bool,
  /// Queue of encoded chunks waiting to be delivered via output callback
  /// Worker pushes chunks here during flush; flush() drains them synchronously via FunctionRef
  pending_chunks: Vec<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
  /// Flag indicating whether a flush operation is in progress
  /// When true, worker queues chunks to pending_chunks instead of calling NonBlocking callback
  inside_flush: bool,
  /// Flag indicating whether the encoder was closed due to an error
  /// Used to return EncodingError from flush() instead of InvalidStateError
  had_error: bool,
}

/// AudioEncoder - WebCodecs-compliant audio encoder
///
/// Encodes AudioData objects into EncodedAudioChunk objects using FFmpeg.
///
/// Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
///
/// Example:
/// ```javascript
/// const encoder = new AudioEncoder({
///   output: (chunk, metadata) => { console.log('encoded chunk', chunk); },
///   error: (e) => { console.error('error', e); }
/// });
///
/// encoder.configure({
///   codec: 'opus',
///   sampleRate: 48000,
///   numberOfChannels: 2
/// });
///
/// encoder.encode(audioData);
/// await encoder.flush();
/// ```
#[napi]
pub struct AudioEncoder {
  inner: Arc<Mutex<AudioEncoderInner>>,
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
  /// Output callback reference - stored for synchronous calls from main thread (in flush resolver)
  /// Wrapped in Rc to allow sharing with spawn_future_with_callback closure
  /// (Rc is !Send but that's OK - the callback runs on the main thread)
  output_callback_ref:
    Rc<FunctionRef<FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>, UnknownReturnValue>>,
  /// Channel sender for worker commands
  command_sender: Option<Sender<EncoderCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
}

impl Drop for AudioEncoder {
  fn drop(&mut self) {
    // Signal worker to stop
    self.command_sender = None;

    // Wait for worker to finish (brief block, necessary for safety)
    if let Some(handle) = self.worker_handle.take() {
      let _ = handle.join();
    }

    // Drain encoder to ensure codec threads finish before context drops.
    // This prevents potential SIGSEGV with codecs that use internal threads.
    if let Ok(mut inner) = self.inner.lock() {
      if let Some(ctx) = inner.context.as_mut() {
        ctx.flush();
        let _ = ctx.send_frame(None);
        while ctx.receive_packet().ok().flatten().is_some() {}
      }
    }
  }
}

#[napi]
impl AudioEncoder {
  /// Create a new AudioEncoder with init dictionary (per WebCodecs spec)
  ///
  /// @param init - Init dictionary containing output and error callbacks
  #[napi(constructor)]
  pub fn new(
    #[napi(
      ts_arg_type = "{ output: (chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata) => void, error: (error: Error) => void }"
    )]
    init: AudioEncoderInit,
  ) -> Result<Self> {
    let inner = AudioEncoderInner {
      state: CodecState::Unconfigured,
      config: None,
      context: None,
      resampler: None,
      sample_buffer: None,
      frame_count: 0,
      extradata_sent: false,
      target_format: AVSampleFormat::Fltp,
      encode_queue_size: 0,
      output_callback: init.output,
      error_callback: init.error,
      dequeue_callback: None,
      pending_flush_senders: Vec::new(),
      timestamp_queue: std::collections::VecDeque::new(),
      base_timestamp: None,
      pending_abort_senders: Vec::new(),
      flush_abort_flag: None,
      output_suppressed: false,
      pending_chunks: Vec::new(),
      inside_flush: false,
      had_error: false,
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
  fn worker_loop(inner: Arc<Mutex<AudioEncoderInner>>, receiver: Receiver<EncoderCommand>) {
    while let Ok(command) = receiver.recv() {
      match command {
        EncoderCommand::Encode { frame, timestamp } => {
          Self::process_encode(&inner, frame, timestamp);
        }
        EncoderCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner);
          let _ = response_sender.send(result);
        }
      }
    }
  }

  /// Process an encode command on the worker thread
  fn process_encode(inner: &Arc<Mutex<AudioEncoderInner>>, frame: Frame, timestamp: i64) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Check if encoder is still configured
    if guard.state != CodecState::Configured {
      let old_size = guard.encode_queue_size;
      guard.encode_queue_size = old_size.saturating_sub(1);
      if old_size > 0 {
        let _ = Self::fire_dequeue_event(&guard);
      }
      Self::report_error(&mut guard, "Encoder not configured");
      return;
    }

    // Track base timestamp from first input for output timestamp calculation
    if guard.base_timestamp.is_none() {
      guard.base_timestamp = Some(timestamp);
    }

    // Get config info (unwrap validated config values)
    let codec_string = match guard.config.as_ref() {
      Some(config) => config.codec.clone().unwrap_or_default(),
      None => {
        let old_size = guard.encode_queue_size;
        guard.encode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(&guard);
        }
        Self::report_error(&mut guard, "No encoder config");
        return;
      }
    };

    // Add frame to sample buffer
    {
      let sample_buffer = match guard.sample_buffer.as_mut() {
        Some(buf) => buf,
        None => {
          let old_size = guard.encode_queue_size;
          guard.encode_queue_size = old_size.saturating_sub(1);
          if old_size > 0 {
            let _ = Self::fire_dequeue_event(&guard);
          }
          Self::report_error(&mut guard, "No sample buffer");
          return;
        }
      };

      if let Err(e) = sample_buffer.add_frame(&frame) {
        let old_size = guard.encode_queue_size;
        guard.encode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(&guard);
        }
        Self::report_error(&mut guard, &format!("Failed to add samples: {}", e));
        return;
      }
    }

    // Get extradata before encoding first frame
    let extradata = if !guard.extradata_sent {
      guard
        .context
        .as_ref()
        .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
    } else {
      None
    };

    // Process complete frames
    loop {
      // Check if we have a full frame and get buffer info
      let (has_frame, frame_size, sample_rate) = match guard.sample_buffer.as_ref() {
        Some(buf) => (
          buf.has_full_frame(),
          buf.frame_size() as i64,
          buf.sample_rate() as i64,
        ),
        None => {
          let old_size = guard.encode_queue_size;
          guard.encode_queue_size = old_size.saturating_sub(1);
          if old_size > 0 {
            let _ = Self::fire_dequeue_event(&guard);
          }
          Self::report_error(&mut guard, "No sample buffer");
          return;
        }
      };

      if !has_frame {
        break;
      }

      // Take frame from buffer
      let mut frame_to_encode = {
        let sample_buffer = match guard.sample_buffer.as_mut() {
          Some(buf) => buf,
          None => {
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(&guard);
            }
            Self::report_error(&mut guard, "No sample buffer");
            return;
          }
        };
        match sample_buffer.take_frame() {
          Ok(Some(f)) => f,
          Ok(None) => {
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(&guard);
            }
            Self::report_error(&mut guard, "No frame available");
            return;
          }
          Err(e) => {
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(&guard);
            }
            Self::report_error(&mut guard, &format!("Failed to get frame: {}", e));
            return;
          }
        }
      };

      // Calculate timestamp based on base_timestamp (from first input) + frame offset
      // This ensures timestamps are continuous and the first output has the first input's timestamp
      let base_ts = guard.base_timestamp.unwrap_or(0);
      let frame_timestamp =
        base_ts + (guard.frame_count as i64 * frame_size * 1_000_000) / sample_rate;
      frame_to_encode.set_pts(frame_timestamp);

      // Push timestamp to queue BEFORE encoding (for output correlation)
      guard.timestamp_queue.push_back(frame_timestamp);

      // Encode the frame
      let context = match guard.context.as_mut() {
        Some(ctx) => ctx,
        None => {
          let old_size = guard.encode_queue_size;
          guard.encode_queue_size = old_size.saturating_sub(1);
          if old_size > 0 {
            let _ = Self::fire_dequeue_event(&guard);
          }
          Self::report_error(&mut guard, "No encoder context");
          return;
        }
      };

      let packets = match context.encode(Some(&frame_to_encode)) {
        Ok(pkts) => pkts,
        Err(e) => {
          let old_size = guard.encode_queue_size;
          guard.encode_queue_size = old_size.saturating_sub(1);
          if old_size > 0 {
            let _ = Self::fire_dequeue_event(&guard);
          }
          Self::report_error(&mut guard, &format!("Encode failed: {}", e));
          return;
        }
      };

      guard.frame_count += 1;

      // Calculate duration per frame in microseconds
      let duration_us = (frame_size * 1_000_000) / sample_rate;

      // Process output packets - call callback for each
      // Pop timestamp from queue to preserve original input timestamp
      for packet in packets {
        let output_timestamp = guard.timestamp_queue.pop_front();
        let chunk = EncodedAudioChunk::from_packet(&packet, Some(duration_us), output_timestamp);

        // Create metadata
        let metadata = if !guard.extradata_sent {
          guard.extradata_sent = true;
          let (target_sample_rate, target_channels) = guard
            .config
            .as_ref()
            .map(|c| {
              (
                c.sample_rate.unwrap_or(48000.0),
                c.number_of_channels.unwrap_or(2),
              )
            })
            .unwrap_or((48000.0, 2));

          EncodedAudioChunkMetadata {
            decoder_config: Some(AudioDecoderConfigOutput {
              codec: codec_string.clone(),
              sample_rate: Some(target_sample_rate),
              number_of_channels: Some(target_channels),
              description: extradata.clone().map(Uint8Array::from),
            }),
          }
        } else {
          EncodedAudioChunkMetadata {
            decoder_config: None,
          }
        };

        // During flush, queue chunks for synchronous delivery in resolver
        // Otherwise, use NonBlocking callback for immediate delivery
        if guard.inside_flush {
          guard.pending_chunks.push((chunk, metadata));
        } else {
          guard.output_callback.call(
            (chunk, metadata).into(),
            ThreadsafeFunctionCallMode::NonBlocking,
          );
        }
      }
    }

    // Decrement queue size and prepare to fire dequeue event (only if queue was not empty)
    let old_size = guard.encode_queue_size;
    guard.encode_queue_size = old_size.saturating_sub(1);
    let dequeue_callback = if old_size > 0 {
      guard.dequeue_callback.clone()
    } else {
      None
    };

    // Release mutex BEFORE calling dequeue callback to avoid deadlock
    // (callback may read encodeQueueSize which needs the mutex)
    drop(guard);

    // Fire dequeue event with NonBlocking mode
    // Note: With NonBlocking, callbacks may not see the exact queue size at time of dequeue.
    // This is acceptable per W3C spec which only requires the event to fire when an item
    // is removed, not that the callback sees a specific queue state.
    if let Some(ref callback) = dequeue_callback {
      callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
    }
  }

  /// Process a flush command on the worker thread
  fn process_flush(inner: &Arc<Mutex<AudioEncoderInner>>) -> Result<()> {
    {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      // W3C spec: flush() should reject if encoder is not in configured state
      if guard.state != CodecState::Configured {
        // If closed due to error, return EncodingError; otherwise InvalidStateError
        if guard.state == CodecState::Closed && guard.had_error {
          return Err(Error::new(
            Status::GenericFailure,
            "EncodingError: Encode error occurred",
          ));
        }
        return Err(Error::new(
          Status::GenericFailure,
          "InvalidStateError: Encoder is not configured",
        ));
      }

      // Flush any remaining samples in buffer
      if let Some(ref mut sample_buffer) = guard.sample_buffer {
        if let Ok(Some(mut frame)) = sample_buffer.flush() {
          // Set timestamp using base_timestamp
          let frame_size = sample_buffer.frame_size() as i64;
          let sample_rate = sample_buffer.sample_rate() as i64;
          let base_ts = guard.base_timestamp.unwrap_or(0);
          let frame_timestamp =
            base_ts + (guard.frame_count as i64 * frame_size * 1_000_000) / sample_rate;
          frame.set_pts(frame_timestamp);

          // Push timestamp to queue for output correlation
          guard.timestamp_queue.push_back(frame_timestamp);

          let context = match guard.context.as_mut() {
            Some(ctx) => ctx,
            None => {
              Self::report_error(&mut guard, "No encoder context");
              return Ok(());
            }
          };

          if let Ok(packets) = context.encode(Some(&frame)) {
            let duration_us = (frame.nb_samples() as i64 * 1_000_000) / sample_rate;
            for packet in packets {
              let output_timestamp = guard.timestamp_queue.pop_front();
              let chunk =
                EncodedAudioChunk::from_packet(&packet, Some(duration_us), output_timestamp);
              let metadata = EncodedAudioChunkMetadata {
                decoder_config: None,
              };
              // Always queue during flush for synchronous delivery
              guard.pending_chunks.push((chunk, metadata));
            }
          }
        }
      }

      // Flush encoder
      let context = match guard.context.as_mut() {
        Some(ctx) => ctx,
        None => {
          Self::report_error(&mut guard, "No encoder context");
          return Ok(());
        }
      };

      let packets = match context.flush_encoder() {
        Ok(pkts) => pkts,
        Err(e) => {
          Self::report_error(&mut guard, &format!("Flush failed: {}", e));
          return Ok(());
        }
      };

      // Queue remaining packets for synchronous delivery in resolver
      for packet in packets {
        let output_timestamp = guard.timestamp_queue.pop_front();
        let chunk = EncodedAudioChunk::from_packet(&packet, None, output_timestamp);
        let metadata = EncodedAudioChunkMetadata {
          decoder_config: None,
        };
        // Always queue during flush for synchronous delivery
        guard.pending_chunks.push((chunk, metadata));
      }
    } // mutex released here

    // Reset encoder state so it can accept more data (per W3C spec, flush should leave
    // encoder in configured state, ready for more encode() calls)
    {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      if let Some(ref mut context) = guard.context {
        context.flush();
      }
    }

    Ok(())
  }

  /// Report an error via callback and close the encoder
  fn report_error(inner: &mut AudioEncoderInner, error_msg: &str) {
    // Create an Error object that will be passed directly to the JS callback
    let error = Error::new(Status::GenericFailure, error_msg);
    inner
      .error_callback
      .call(error, ThreadsafeFunctionCallMode::NonBlocking);
    inner.had_error = true;
    inner.state = CodecState::Closed;
  }

  /// Fire dequeue event if callback is set
  fn fire_dequeue_event(inner: &AudioEncoderInner) -> Result<()> {
    if let Some(ref callback) = inner.dequeue_callback {
      callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
    }
    Ok(())
  }

  /// Get encoder state
  #[napi(getter)]
  pub fn state(&self) -> Result<CodecState> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.state)
  }

  /// Get number of pending encode operations (per WebCodecs spec)
  #[napi(getter)]
  pub fn encode_queue_size(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.encode_queue_size)
  }

  /// Set the dequeue event handler (per WebCodecs spec)
  ///
  /// The dequeue event fires when encodeQueueSize decreases,
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
      inner.dequeue_callback = Some(Arc::new(
        callback
          .borrow_back(env)?
          .build_threadsafe_function()
          .callee_handled::<false>()
          .weak::<true>()
          .build()?,
      ));
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

  /// Configure the encoder
  #[napi]
  pub fn configure(&self, env: Env, config: AudioEncoderConfig) -> Result<()> {
    // W3C WebCodecs spec: Validate config synchronously, throw TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-audioencoder-configure

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

    // Validate bitrate if specified
    if let Some(bitrate) = config.bitrate {
      if bitrate <= 0.0 {
        return throw_type_error_unit(&env, "bitrate must be greater than 0");
      }
    }

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: throw InvalidStateError if closed
    if inner.state == CodecState::Closed {
      return Err(invalid_state_error("Encoder is closed"));
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

    // Get encoder name (prefer external libraries for better quality)
    let encoder_name = get_audio_encoder_name(codec_id);

    // Create encoder context
    let mut context = match if let Some(name) = encoder_name {
      CodecContext::new_encoder_by_name(name).or_else(|_| CodecContext::new_encoder(codec_id))
    } else {
      CodecContext::new_encoder(codec_id)
    } {
      Ok(ctx) => ctx,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Failed to create encoder: {}", e));
        return Ok(());
      }
    };

    // Determine target sample format based on codec
    let target_format = get_encoder_sample_format(codec_id);

    // Configure encoder (cast f64 sample_rate to u32 for FFmpeg)
    let encoder_config = InternalAudioEncoderConfig {
      sample_rate: sample_rate as u32,
      channels: number_of_channels,
      sample_format: target_format,
      bitrate: config.bitrate.unwrap_or(128_000.0) as u64,
      thread_count: 0,
    };

    if let Err(e) = context.configure_audio_encoder(&encoder_config) {
      Self::report_error(&mut inner, &format!("Failed to configure encoder: {}", e));
      return Ok(());
    }

    // Open the encoder
    if let Err(e) = context.open() {
      Self::report_error(&mut inner, &format!("Failed to open encoder: {}", e));
      return Ok(());
    }

    // Get the actual frame size from the encoder
    let frame_size = context.frame_size();
    let frame_size = if frame_size == 0 {
      // Some encoders don't set frame_size, use codec default
      AudioSampleBuffer::frame_size_for_codec(&codec)
    } else {
      frame_size as usize
    };

    // Create sample buffer
    let sample_buffer = AudioSampleBuffer::new(
      frame_size,
      number_of_channels,
      sample_rate as u32,
      target_format,
    );

    inner.context = Some(context);
    inner.config = Some(config);
    inner.sample_buffer = Some(sample_buffer);
    inner.target_format = target_format;
    inner.state = CodecState::Configured;
    inner.extradata_sent = false;
    inner.frame_count = 0;
    inner.resampler = None;
    inner.encode_queue_size = 0;

    Ok(())
  }

  /// Encode audio data
  #[napi]
  pub fn encode(&self, data: &AudioData) -> Result<()> {
    // Clone frame, resample if needed, and get timestamp on main thread
    let (frame_to_send, timestamp) = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      // W3C spec: throw InvalidStateError if not configured or closed
      if inner.state == CodecState::Closed {
        return Err(invalid_state_error("Cannot encode with a closed codec"));
      }
      if inner.state != CodecState::Configured {
        return Err(invalid_state_error(
          "Cannot encode with an unconfigured codec",
        ));
      }

      // Get config info (unwrap validated config values)
      let (target_sample_rate, target_channels) = match inner.config.as_ref() {
        Some(config) => (
          config.sample_rate.unwrap_or(48000.0),
          config.number_of_channels.unwrap_or(2),
        ),
        None => {
          Self::report_error(&mut inner, "No encoder config");
          return Ok(());
        }
      };

      // Get audio data properties
      let src_format = match data.format() {
        Ok(Some(fmt)) => fmt,
        Ok(None) => {
          Self::report_error(&mut inner, "AudioData has no format");
          return Ok(());
        }
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get format: {}", e));
          return Ok(());
        }
      };
      let src_sample_rate = match data.sample_rate() {
        Ok(sr) => sr,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get sample rate: {}", e));
          return Ok(());
        }
      };
      let src_channels = match data.number_of_channels() {
        Ok(ch) => ch,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get channels: {}", e));
          return Ok(());
        }
      };
      let timestamp = match data.timestamp() {
        Ok(ts) => ts,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get timestamp: {}", e));
          return Ok(());
        }
      };

      // Check if we need resampling (compare as u32 for FFmpeg)
      let src_sample_rate_u32 = src_sample_rate as u32;
      let target_sample_rate_u32 = target_sample_rate as u32;
      let needs_resampling = src_sample_rate_u32 != target_sample_rate_u32
        || src_channels != target_channels
        || src_format.to_av_format() != inner.target_format;

      // Create resampler if needed and not already created
      if needs_resampling && inner.resampler.is_none() {
        match Resampler::new(
          src_channels,
          src_sample_rate_u32,
          src_format.to_av_format(),
          target_channels,
          target_sample_rate_u32,
          inner.target_format,
        ) {
          Ok(resampler) => inner.resampler = Some(resampler),
          Err(e) => {
            Self::report_error(&mut inner, &format!("Failed to create resampler: {}", e));
            return Ok(());
          }
        }
      }

      // Get frame from AudioData
      let frame = match data.with_frame(|f| f.try_clone()) {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => {
          Self::report_error(&mut inner, &format!("Failed to clone frame: {}", e));
          return Ok(());
        }
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get frame: {}", e));
          return Ok(());
        }
      };

      // Resample if needed
      let frame_to_send = if let Some(ref mut resampler) = inner.resampler {
        match resampler.convert_alloc(&frame) {
          Ok(f) => f,
          Err(e) => {
            Self::report_error(&mut inner, &format!("Resampling failed: {}", e));
            return Ok(());
          }
        }
      } else {
        frame
      };

      // Increment queue size (pending operation)
      inner.encode_queue_size += 1;

      (frame_to_send, timestamp)
    };

    // Send encode command to worker thread
    if let Some(ref sender) = self.command_sender {
      sender
        .send(EncoderCommand::Encode {
          frame: frame_to_send,
          timestamp,
        })
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?;
    } else {
      return Err(Error::new(
        Status::GenericFailure,
        "Encoder has been closed",
      ));
    }

    Ok(())
  }

  /// Flush the encoder
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
        // Return rejected promise via async to allow error callback to run first
        return env
          .spawn_future_with_callback(async move { Ok(()) }, move |_env, _| -> Result<()> {
            Err(invalid_state_error("Cannot flush a closed codec"))
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
      // Set inside_flush flag so worker queues chunks instead of calling NonBlocking callback
      inner.inside_flush = true;
    }

    // Create a response channel for worker result
    let (response_sender, response_receiver) = channel::bounded::<Result<()>>(1);

    // Track this flush - store sender in Inner for reset() to use
    {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      inner.pending_flush_senders.push(response_sender.clone());
    }

    // Send flush command through the channel to ensure it's processed after all pending encodes
    if let Some(ref sender) = self.command_sender {
      sender
        .send(EncoderCommand::Flush(response_sender))
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
        // Drain pending chunks and call output callback SYNCHRONOUSLY
        // This runs on the main thread with Env access
        let chunks = {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          std::mem::take(&mut guard.pending_chunks)
        };

        // Call output callback for each chunk synchronously
        // If callback calls reset(), abort_flag will be set before next iteration
        let callback = output_callback_ref.borrow_back(env)?;
        for (chunk, metadata) in chunks {
          // Check abort flag before each callback - exit early if reset() was called
          if abort_flag.load(Ordering::SeqCst) {
            break;
          }
          callback.call((chunk, metadata).into())?;
        }

        // Clean up flags
        {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          guard.flush_abort_flag = None;
          guard.inside_flush = false;
        }

        // Check abort flag after draining all chunks
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

  /// Reset the encoder
  #[napi]
  pub fn reset(&mut self) -> Result<()> {
    // Check state first before touching the worker
    {
      let mut inner = self
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

      // W3C spec: Abort all pending flushes with AbortError BEFORE dropping sender

      // Send abort signal through all abort channels - this causes pending flush()
      // calls to return AbortError via the select! in spawn_blocking
      for sender in inner.pending_abort_senders.drain(..) {
        let _ = sender.send(());
      }

      // Also try to send through the response channel (fallback)
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
    inner.resampler = None;
    inner.sample_buffer = None;
    inner.config = None;
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    inner.extradata_sent = false;
    inner.encode_queue_size = 0;
    inner.timestamp_queue.clear();
    inner.base_timestamp = None;
    // Clear any remaining abort senders (shouldn't be any, but just in case)
    inner.pending_abort_senders.clear();

    // Clear flush-related state
    inner.inside_flush = false;
    inner.pending_chunks.clear();

    // Create new channel and worker for future encode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(sender);
    let worker_inner = self.inner.clone();
    drop(inner); // Release lock before spawning thread
    self.worker_handle = Some(std::thread::spawn(move || {
      Self::worker_loop(worker_inner, receiver);
    }));

    Ok(())
  }

  /// Close the encoder
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
    inner.resampler = None;
    inner.sample_buffer = None;
    inner.config = None;
    inner.state = CodecState::Closed;
    inner.encode_queue_size = 0;

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
    config: AudioEncoderConfig,
  ) -> Result<PromiseRaw<'env, AudioEncoderSupport>> {
    // W3C WebCodecs spec: Validate config, reject with TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-audioencoder-isconfigsupported

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      Some(_) => return reject_with_type_error(env, "codec is required"),
      None => return reject_with_type_error(env, "codec is required"),
    };

    // Validate sample rate - must be present and greater than 0
    let _sample_rate = match config.sample_rate {
      Some(sr) if sr > 0.0 => sr,
      Some(_) => return reject_with_type_error(env, "sampleRate must be greater than 0"),
      None => return reject_with_type_error(env, "sampleRate is required"),
    };

    // Validate number of channels - must be present and greater than 0
    let _number_of_channels = match config.number_of_channels {
      Some(nc) if nc > 0 => nc,
      Some(_) => return reject_with_type_error(env, "numberOfChannels must be greater than 0"),
      None => return reject_with_type_error(env, "numberOfChannels is required"),
    };

    // Validate bitrate if specified
    if let Some(bitrate) = config.bitrate {
      if bitrate <= 0.0 {
        return reject_with_type_error(env, "bitrate must be greater than 0");
      }
    }

    env.spawn_future(async move {
      // Parse codec string
      let codec_id = match parse_audio_codec_string(&codec) {
        Ok(id) => id,
        Err(_) => {
          return Ok(AudioEncoderSupport {
            supported: false,
            config,
          });
        }
      };

      // Try to find encoder
      let encoder_name = get_audio_encoder_name(codec_id);
      let result = if let Some(name) = encoder_name {
        CodecContext::new_encoder_by_name(name).or_else(|_| CodecContext::new_encoder(codec_id))
      } else {
        CodecContext::new_encoder(codec_id)
      };

      Ok(AudioEncoderSupport {
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

/// Get the preferred sample format for an encoder
fn get_encoder_sample_format(codec_id: AVCodecID) -> AVSampleFormat {
  match codec_id {
    AVCodecID::Aac => AVSampleFormat::Fltp, // AAC prefers float planar
    AVCodecID::Opus => AVSampleFormat::Flt, // Opus prefers float interleaved
    AVCodecID::Mp3 => AVSampleFormat::S16p, // MP3 prefers s16 planar
    AVCodecID::Flac => AVSampleFormat::S16, // FLAC prefers s16
    AVCodecID::Vorbis => AVSampleFormat::Fltp, // Vorbis prefers float planar
    AVCodecID::PcmS16le => AVSampleFormat::S16,
    AVCodecID::PcmS16be => AVSampleFormat::S16,
    AVCodecID::PcmF32le => AVSampleFormat::Flt,
    AVCodecID::PcmF32be => AVSampleFormat::Flt,
    AVCodecID::Ac3 => AVSampleFormat::Fltp,
    AVCodecID::Alac => AVSampleFormat::S16p,
    _ => AVSampleFormat::Fltp, // Default to float planar
  }
}
