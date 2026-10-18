//! AudioDecoder - WebCodecs API implementation
//!
//! Provides audio decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#audiodecoder-interface

use crate::codec::{AudioDecoderConfig as InternalAudioDecoderConfig, CodecContext, Frame, Packet};
use crate::ffi::AVCodecID;
use crate::webcodecs::codec_epoch::{
  CancelToken, CodecEpoch, ErrorOrigin, GatedError, GatedOutput, buffered_output, gated_error,
  gated_output,
};
use crate::webcodecs::encoded_audio_chunk::EncodedAudioChunkInner;
use crate::webcodecs::error::{
  DOMExceptionName, dom_exception_rejection_error, native_dom_exception_error, throw_data_error,
  throw_invalid_state_error, throw_type_error_unit,
};
use crate::webcodecs::flush_tracker::{FlushAbort, FlushTracker};
use crate::webcodecs::promise_reject::{reject_with_dom_exception_async, reject_with_type_error};
use crate::webcodecs::{AudioData, AudioDecoderConfig, AudioDecoderSupport, EncodedAudioChunk};
use crossbeam::channel::{self, Receiver, Sender};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::UnknownReturnValue;
use napi_derive::napi;
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use super::event_target::CodecEventState;
use super::video_encoder::CodecState;

/// Output callback: wakes the gate for one output queued with
/// `CodecEpoch::send_output` (see codec_epoch.rs).
type OutputCallback = GatedOutput;

/// Type alias for error callback (takes the codec generation and an Error)
/// Using CalleeHandled: false because WebCodecs error callback receives the
/// error directly, not error-first (err, result) style. The Error payload is
/// converted to a native DOMException in the JS-side callback.
type ErrorCallback = GatedError;

/// Options for addEventListener (W3C DOM spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AudioDecoderAddEventListenerOptions {
  pub capture: Option<bool>,
  pub once: Option<bool>,
  pub passive: Option<bool>,
}

/// Options for removeEventListener (W3C DOM spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AudioDecoderEventListenerOptions {
  pub capture: Option<bool>,
}

/// Commands sent to the worker thread
enum DecoderCommand {
  /// Decode an audio chunk
  Decode {
    chunk: Arc<RwLock<Option<EncodedAudioChunkInner>>>,
    timestamp: i64,
    duration: Option<i64>,
  },
  /// Flush the decoder and send result back via response channel
  Flush(Sender<Result<()>>),
  /// Reconfigure the decoder with a new configuration
  Reconfigure(AudioDecoderConfig),
}

/// AudioDecoder init dictionary per WebCodecs spec
pub struct AudioDecoderInit {
  /// Output callback - called when decoded audio is available (ThreadsafeFunction for worker)
  pub output: OutputCallback,
  /// Output callback reference - stored for synchronous calls from main thread
  pub output_ref: FunctionRef<AudioData, UnknownReturnValue>,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
  /// Error callback reference - prevents GC from collecting the error callback
  pub error_ref: FunctionRef<Error, UnknownReturnValue>,
  /// Cancellation shared with the gated callbacks above
  pub(crate) epoch: CodecEpoch,
}

impl FromNapiValue for AudioDecoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = unsafe { Object::from_napi_value(env, value)? };

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

    // Create ThreadsafeFunction for async calls from worker thread. It targets
    // a gated wrapper so calls queued before close()/reset() are dropped.
    let epoch = CodecEpoch::default();
    let output: OutputCallback = gated_output(&env_wrapper, output_func.raw(), &epoch)?;

    // Get error callback as Function first, then create both FunctionRef and ThreadsafeFunction
    let error_func: Function<Error, UnknownReturnValue> = match obj.get_named_property("error") {
      Ok(cb) => cb,
      Err(_) => {
        env_wrapper.throw_type_error("error callback is required", None)?;
        return Err(Error::new(Status::InvalidArg, "error callback is required"));
      }
    };

    // Create FunctionRef to prevent GC from collecting the error callback
    let error_ref = error_func.create_ref()?;

    // Create ThreadsafeFunction for async calls from worker thread.
    // The JS-side callback converts the Error payload into a native
    // DOMException (WebCodecsErrorCallback receives DOMException per spec).
    let error: ErrorCallback =
      gated_error(&env_wrapper, error_func.raw(), output_func.raw(), &epoch)?;

    Ok(AudioDecoderInit {
      output,
      output_ref,
      error,
      error_ref,
      epoch,
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
  /// The codec's event state: report_error orders the error callback before
  /// the dequeue events that come due meanwhile.
  event_state: Arc<RwLock<CodecEventState>>,
  /// Name of the error that closed the decoder (for flush error propagation)
  failure_name: Option<DOMExceptionName>,
  /// Whether a key chunk has been decoded since the last configure (spec: [[key chunk required]])
  keyframe_received: bool,
  /// Pending flush operations, tracked independently for overlapping calls.
  flushes: FlushTracker,
  /// Queue of decoded audio data waiting to be delivered via output callback
  /// Worker pushes data here during flush; flush() drains them synchronously via FunctionRef
  pending_data: Vec<AudioData>,
  /// Queue of timestamps from input chunks (to preserve original timestamps)
  /// FFmpeg may return AV_NOPTS_VALUE for frame.pts(), so we track input timestamps
  timestamp_queue: std::collections::VecDeque<(i64, Option<i64>)>,
  /// Shared with the codec, its workers and its gated callbacks
  epoch: CodecEpoch,
  /// Generation this state serves. Outputs and errors are tagged with it;
  /// dequeue events are not (see `CodecEventState::clear_queue`). reset()
  /// moves it to the new generation once it holds the lock.
  generation: u32,
}

impl AudioDecoderInner {
  /// close()/reset() moved the codec past the generation this state serves.
  /// A worker still finishing old work leaves the codec to them.
  fn is_stale(&self) -> bool {
    !self.epoch.is_current(self.generation)
  }

  /// Worker: one queued decode finished; fire its dequeue event.
  fn dequeue_one(&mut self, event_state: &Arc<RwLock<CodecEventState>>) {
    if self.is_stale() {
      return;
    }
    let old_size = self.decode_queue_size;
    self.decode_queue_size = old_size.saturating_sub(1);
    if old_size > 0 {
      CodecEventState::fire(event_state);
    }
  }

  /// Worker: every queued decode is dropped at once; one dequeue event covers
  /// them all.
  fn dequeue_all(&mut self, event_state: &Arc<RwLock<CodecEventState>>) {
    if self.is_stale() {
      return;
    }
    CodecEventState::dequeue_all(event_state, &mut self.decode_queue_size);
  }

  /// JS thread: close the decoder. `failure` is the error that closed it: pending
  /// flushes reject with it, or with AbortError when close() ran.
  fn close_codec(
    &mut self,
    env: &Env,
    event_state: &Arc<RwLock<CodecEventState>>,
    failure: Option<DOMExceptionName>,
  ) {
    match failure {
      Some(name) => self.flushes.fail_all(name),
      None => self.flushes.abort_all(),
    }
    self.context = None;
    self.config = None;
    self.codec_string.clear();
    self.state = CodecState::Closed;
    self.failure_name = failure;
    self.pending_data.clear();
    let cleared_queue = self.decode_queue_size as usize;
    self.decode_queue_size = 0;
    CodecEventState::clear_queue(event_state, env, cleared_queue);
  }
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
  /// Separate lock for event listeners to avoid contention with decode operations
  event_state: Arc<RwLock<CodecEventState>>,
  /// Output callback reference - stored for synchronous calls from main thread (in flush resolver)
  /// Wrapped in Rc to allow sharing with spawn_future_with_callback closure
  /// (Rc is !Send but that's OK - the callback runs on the main thread)
  output_callback_ref: Rc<FunctionRef<AudioData, UnknownReturnValue>>,
  /// Error callback reference - prevents GC from collecting the error callback
  /// (weak ThreadsafeFunction alone can be collected on slow platforms like armv7 QEMU)
  #[allow(dead_code)]
  error_callback_ref: Rc<FunctionRef<Error, UnknownReturnValue>>,
  /// Channel sender for worker commands (wrapped in Arc for Weak references in microtasks)
  command_sender: Option<Arc<Sender<DecoderCommand>>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
  /// Shared cancellation: close()/reset() advance it before taking the lock
  epoch: CodecEpoch,
  /// The current worker's token; cancelled once close()/reset() runs
  cancel: CancelToken,
  /// close() ran. Together with a delivered error, this is "closed" as JS sees it.
  closed: bool,
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
    if let Ok(mut inner) = self.inner.lock()
      && let Some(ctx) = inner.context.as_mut()
    {
      ctx.flush();
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
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
    #[napi(
      ts_arg_type = "{ output: (data: AudioData) => void, error: (error: DOMException) => void }"
    )]
    init: AudioDecoderInit,
  ) -> Result<Self> {
    let event_state = Arc::new(RwLock::new(CodecEventState::default()));
    let inner = AudioDecoderInner {
      state: CodecState::Unconfigured,
      config: None,
      context: None,
      codec_string: String::new(),
      frame_count: 0,
      decode_queue_size: 0,
      output_callback: init.output,
      error_callback: init.error,
      event_state: event_state.clone(),
      failure_name: None,
      keyframe_received: false,
      flushes: FlushTracker::default(),
      pending_data: Vec::new(),
      timestamp_queue: std::collections::VecDeque::new(),
      epoch: init.epoch.clone(),
      generation: init.epoch.current(),
    };

    let inner = Arc::new(Mutex::new(inner));

    // An unsupported config that configure() reported still closes the decoder
    // when its error reaches JS after reset(), as in Chrome.
    init
      .epoch
      .set_close_on_error(&inner, &event_state, |inner, env, events, name| {
        inner.close_codec(env, events, Some(name));
      });

    // Outputs a pending flush buffered reach JS before an error callback, as in
    // Chrome, instead of after the error closed the decoder.
    init.epoch.set_take_buffered(&inner, |inner| {
      std::mem::take(&mut inner.pending_data)
        .into_iter()
        .map(buffered_output)
        .collect()
    });

    // Create channel for worker commands
    let (sender, receiver) = channel::unbounded();

    // Token the worker checks between commands
    let cancel = init.epoch.token();

    // Spawn worker thread
    let worker_inner = inner.clone();
    let worker_event_state = event_state.clone();
    let worker_cancel = cancel.clone();
    let worker_handle = std::thread::spawn(move || {
      Self::worker_loop(worker_inner, worker_event_state, receiver, worker_cancel);
    });

    Ok(Self {
      inner,
      event_state,
      output_callback_ref: Rc::new(init.output_ref),
      error_callback_ref: Rc::new(init.error_ref),
      command_sender: Some(Arc::new(sender)),
      worker_handle: Some(worker_handle),
      epoch: init.epoch,
      cancel,
      closed: false,
    })
  }

  /// Worker loop that processes commands from the channel
  fn worker_loop(
    inner: Arc<Mutex<AudioDecoderInner>>,
    event_state: Arc<RwLock<CodecEventState>>,
    receiver: Receiver<DecoderCommand>,
    cancel: CancelToken,
  ) {
    while let Ok(command) = receiver.recv() {
      // close()/reset() cancelled this worker: drop the rest of its queue
      if cancel.is_cancelled() {
        // Still process flush commands to send responses, but skip decodes
        if let DecoderCommand::Flush(response_sender) = command {
          let _ = response_sender.send(Err(Error::new(
            Status::GenericFailure,
            "AbortError: The operation was aborted",
          )));
        }
        continue;
      }

      match command {
        DecoderCommand::Decode {
          chunk,
          timestamp,
          duration,
        } => {
          Self::process_decode(&inner, &event_state, chunk, timestamp, duration, &cancel);
        }
        DecoderCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner, &event_state, &cancel);
          let _ = response_sender.send(result);
        }
        DecoderCommand::Reconfigure(config) => {
          Self::process_reconfigure(&inner, &config, &cancel);
        }
      }
    }
  }

  /// Process a decode command on the worker thread
  fn process_decode(
    inner: &Arc<Mutex<AudioDecoderInner>>,
    event_state: &Arc<RwLock<CodecEventState>>,
    chunk: Arc<RwLock<Option<EncodedAudioChunkInner>>>,
    timestamp: i64,
    duration: Option<i64>,
    cancel: &CancelToken,
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    if cancel.is_cancelled() {
      return;
    }

    // Check if decoder is still configured
    if guard.state != CodecState::Configured {
      // Per W3C spec: "cease producing output" - silently discard pending work
      // State could be Unconfigured (reset called) or Closed (close called)
      // Don't call report_error() - that would set state to Closed and invoke error callback
      // Every decode still queued is discarded the same way, so dequeue them all
      // now: one dequeue event, as Chrome fires when it resets the queue.
      guard.dequeue_all(event_state);
      return;
    }

    let inner_lock = match chunk.read() {
      Ok(lock) => lock,
      Err(_) => {
        Self::close_with_error(
          &mut guard,
          event_state,
          "Failed to read chunk: lock poisoned",
        );
        return;
      }
    };

    let inner = match inner_lock.as_ref() {
      Some(inner) => inner,
      None => {
        Self::close_with_error(
          &mut guard,
          event_state,
          "Failed to read chunk: chunk is closed",
        );
        return;
      }
    };

    let data = match &inner.data {
      Either::A(data) => data.as_slice(),
      Either::B(packet) => packet.as_slice(),
    };

    // W3C spec: Empty data should trigger EncodingError
    if data.is_empty() {
      Self::close_with_error(
        &mut guard,
        event_state,
        "EncodingError: Cannot decode empty audio chunk data",
      );
      return;
    }

    // Store the original timestamp for output (FFmpeg may return AV_NOPTS_VALUE)
    // Must be done before borrowing context to satisfy borrow checker
    guard.timestamp_queue.push_back((timestamp, duration));

    // Get context
    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        // Remove the timestamp we just pushed since we're not decoding
        guard.timestamp_queue.pop_back();
        Self::close_with_error(&mut guard, event_state, "No decoder context");
        return;
      }
    };

    // Decode using the internal implementation
    let frames = match decode_audio_chunk_data(context, data, timestamp) {
      Ok(f) => f,
      Err(e) => {
        guard.timestamp_queue.pop_back();
        Self::close_with_error(&mut guard, event_state, &format!("Decode failed: {}", e));
        return;
      }
    };

    drop(inner_lock);

    guard.frame_count += 1;

    // Decrement queue size and fire dequeue event (only if queue was not empty)
    guard.dequeue_one(event_state);

    // A buffering decoder may not return a frame until a later packet or
    // flush. Preserve this input's timestamp for that eventual output.
    if frames.is_empty() {
      return;
    }

    // Convert internal frames to AudioData and deliver
    // Pop the original timestamp (fallback to frame.pts() if queue empty)
    let (output_timestamp, output_duration) =
      guard.timestamp_queue.pop_front().unwrap_or_else(|| {
        let pts = frames
          .first()
          .map(|frame| frame.pts())
          .filter(|pts| *pts != crate::ffi::types::AV_NOPTS_VALUE)
          .unwrap_or(0);
        (pts, None)
      });
    let mut next_timestamp = output_timestamp;
    for (i, frame) in frames.into_iter().enumerate() {
      // For the first frame, use the original input timestamp
      // For subsequent frames, calculate based on sample count or use frame.pts()
      let pts = if i == 0 {
        output_timestamp
      } else {
        let frame_pts = frame.pts();
        // Use frame.pts() if valid, otherwise estimate from first frame timestamp
        if frame_pts != crate::ffi::types::AV_NOPTS_VALUE {
          frame_pts
        } else {
          next_timestamp
        }
      };
      let frame_duration = if frame.sample_rate() > 0 {
        (frame.nb_samples() as i64)
          .saturating_mul(1_000_000)
          .checked_div(frame.sample_rate() as i64)
          .unwrap_or(0)
      } else {
        output_duration.unwrap_or(0)
      };
      next_timestamp = pts.saturating_add(frame_duration);
      let audio_data = AudioData::from_internal(frame, pts);

      // During flush, queue data for synchronous delivery in resolver
      // Otherwise, use NonBlocking callback for immediate delivery
      if guard.flushes.should_buffer_outputs() {
        guard.pending_data.push(audio_data);
      } else if !guard.is_stale() {
        guard.epoch.send_output(
          &guard.output_callback,
          guard.generation,
          buffered_output(audio_data),
        );
      }
    }
  }

  /// Process a flush command on the worker thread
  fn process_flush(
    inner: &Arc<Mutex<AudioDecoderInner>>,
    _event_state: &Arc<RwLock<CodecEventState>>,
    cancel: &CancelToken,
  ) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if cancel.is_cancelled() {
      return Err(Error::new(
        Status::GenericFailure,
        "AbortError: The operation was aborted",
      ));
    }

    // W3C spec: a flush pending when an error closed the decoder rejects with
    // that error. This must be checked first to return the correct error type.
    if guard.state == CodecState::Closed
      && let Some(name) = guard.failure_name
    {
      return Err(Error::new(
        Status::GenericFailure,
        format!("{}: Decode error occurred", name.as_str()),
      ));
    }

    // Per W3C spec: state check happens on main thread (in flush() method).
    // If state changed after that check (e.g., reconfigure failed but no decode error),
    // silently succeed. The error callback has already been invoked by the failing operation.
    // This is consistent with process_decode() which silently discards when state is wrong.
    if guard.state != CodecState::Configured {
      return Ok(());
    }

    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        let message = "No decoder context";
        Self::report_error(&mut guard, message);
        return Err(Error::new(
          Status::GenericFailure,
          format!("EncodingError: {}", message),
        ));
      }
    };

    // Flush decoder
    let frames = match context.flush_decoder() {
      Ok(f) => f,
      Err(e) => {
        let message = format!("Flush failed: {}", e);
        Self::report_error(&mut guard, &message);
        return Err(Error::new(
          Status::GenericFailure,
          format!("EncodingError: {}", message),
        ));
      }
    };

    // Queue remaining frames for delivery (always queue during flush for synchronous delivery)
    for frame in frames {
      let (timestamp, _duration) = guard.timestamp_queue.pop_front().unwrap_or_else(|| {
        let pts = frame.pts();
        let timestamp = if pts == crate::ffi::types::AV_NOPTS_VALUE {
          0
        } else {
          pts
        };
        let duration = (frame.duration() > 0).then_some(frame.duration());
        (timestamp, duration)
      });
      let audio_data = AudioData::from_internal(frame, timestamp);
      // Always queue during flush for synchronous delivery in resolver
      guard.pending_data.push(audio_data);
    }
    guard.timestamp_queue.clear();

    // Reset decoder state so it can accept more data (per W3C spec, flush should leave
    // decoder in configured state, ready for more decode() calls)
    if let Some(ref mut context) = guard.context {
      context.flush();
    }

    Ok(())
  }

  /// Process a reconfigure command on the worker thread
  /// Drains old context, clears work state, and creates new decoder context
  fn process_reconfigure(
    inner: &Arc<Mutex<AudioDecoderInner>>,
    config: &AudioDecoderConfig,
    cancel: &CancelToken,
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    if cancel.is_cancelled() {
      return;
    }

    // Drain old context (codec thread safety)
    if let Some(ctx) = guard.context.as_mut() {
      ctx.flush();
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
    }

    // Clear codec-local work state. Do not reset decode_queue_size here:
    // main-thread decode() calls after this FIFO command are already counted.
    // keyframe_received is intentionally not cleared here: configure() already
    // resets it synchronously on the main thread, and clearing it again on the
    // worker would clobber a key chunk accepted after configure() returned.
    guard.timestamp_queue.clear();
    guard.frame_count = 0;

    // Parse codec string
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      _ => {
        Self::report_error(&mut guard, "codec is required");
        return;
      }
    };

    let sample_rate = match config.sample_rate {
      Some(sr) if sr > 0.0 => sr,
      _ => {
        Self::report_error(&mut guard, "sampleRate is required and must be > 0");
        return;
      }
    };

    let number_of_channels = match config.number_of_channels {
      Some(nc) if nc > 0 => nc,
      _ => {
        Self::report_error(&mut guard, "numberOfChannels is required and must be > 0");
        return;
      }
    };

    let codec_id = match parse_audio_codec_string(&codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_error(
          &mut guard,
          &format!("NotSupportedError: Invalid codec: {}", e),
        );
        return;
      }
    };

    // configure() already rejected FLAC without a description.

    // Create decoder context
    let mut context = match CodecContext::new_decoder(codec_id) {
      Ok(ctx) => ctx,
      Err(e) => {
        Self::report_error(&mut guard, &format!("Failed to create decoder: {}", e));
        return;
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
      Self::report_error(&mut guard, &format!("Failed to configure decoder: {}", e));
      return;
    }

    // Open the decoder
    if let Err(e) = context.open() {
      Self::report_error(&mut guard, &format!("Failed to open decoder: {}", e));
      return;
    }

    // Update state
    guard.context = Some(context);
    guard.config = Some(decoder_config);
    guard.codec_string = codec;
  }

  /// Report an error via callback and close the decoder
  fn report_error(inner: &mut AudioDecoderInner, error_msg: &str) {
    Self::report_error_from(inner, error_msg, ErrorOrigin::Work);
  }

  /// Report an error configure() found on the JS thread
  fn report_configure_error(inner: &mut AudioDecoderInner, error_msg: &str) {
    Self::report_error_from(inner, error_msg, ErrorOrigin::of_configure_error(error_msg));
  }

  /// Report an error; `origin` decides whether reset() drops it
  fn report_error_from(inner: &mut AudioDecoderInner, error_msg: &str, origin: ErrorOrigin) {
    // A worker cancelled by close()/reset() leaves the codec to them.
    if inner.is_stale() {
      return;
    }

    // Log the error at warn level for debugging (visible even if JS callback fails)
    tracing::warn!(target: "webcodecs", codec = "AudioDecoder", error = error_msg, "Codec error reported");

    // Create an Error object that will be passed directly to the JS callback
    let error = Error::new(Status::GenericFailure, error_msg);
    CodecEventState::report_error(
      &inner.event_state,
      &inner.error_callback,
      (inner.generation, origin, error),
    );
    // A flush issued before the error callback runs fails with this same error.
    inner.failure_name = Some(DOMExceptionName::from_error_message(error_msg));
    inner.state = CodecState::Closed;
  }

  /// Worker: an error closes the decoder and drops every queued decode. As in
  /// Chrome, decodeQueueSize is already 0 when the error callback runs, and one
  /// dequeue event follows for all the dropped decodes.
  fn close_with_error(
    inner: &mut AudioDecoderInner,
    event_state: &Arc<RwLock<CodecEventState>>,
    message: &str,
  ) {
    Self::report_error(inner, message);
    inner.dequeue_all(event_state);
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
  #[napi(
    setter,
    ts_args_type = "callback: ((event: Event) => unknown) | undefined | null"
  )]
  pub fn set_ondequeue(
    &self,
    env: &Env,
    this: This,
    callback: Option<FunctionRef<Unknown<'static>, UnknownReturnValue>>,
  ) -> Result<()> {
    CodecEventState::set_ondequeue_handler(&self.event_state, env, this.object, callback)
  }

  /// Get the dequeue event handler (per WebCodecs spec)
  #[napi(getter, ts_return_type = "((event: Event) => unknown) | null")]
  pub fn get_ondequeue<'env>(
    &self,
    env: &'env Env,
  ) -> Result<Option<Function<'env, Unknown<'static>, UnknownReturnValue>>> {
    let state = self
      .event_state
      .read()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    match state.ondequeue() {
      Some(refr) => Ok(
        crate::webcodecs::event_target::upgrade_ref(env, refr)?
          .map(|v| unsafe { v.cast::<Function<'env, Unknown<'static>, UnknownReturnValue>>() })
          .transpose()?,
      ),
      None => Ok(None),
    }
  }

  /// Configure the decoder
  #[napi]
  pub fn configure(&mut self, env: Env, config: AudioDecoderConfig) -> Result<()> {
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

    if config.description_detached {
      return throw_type_error_unit(&env, "Invalid config; description is detached.");
    }

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: throw InvalidStateError if closed
    if inner.state == CodecState::Closed {
      return throw_invalid_state_error(&env, "Decoder is closed");
    }

    // W3C spec: configure() sets [[key chunk required]] = true synchronously,
    // so a delta chunk decoded right after configure() must throw DataError
    // before the worker's reconfigure command runs.
    inner.keyframe_received = false;

    // If already configured, queue reconfigure via microtask for W3C spec FIFO ordering
    // This ensures pending decode commands are processed before reconfiguration
    if inner.state == CodecState::Configured {
      // Validate codec synchronously before queueing
      let _codec_id = match parse_audio_codec_string(&codec) {
        Ok(id) => id,
        Err(e) => {
          Self::report_configure_error(
            &mut inner,
            &format!("NotSupportedError: Invalid codec: {}", e),
          );
          return Ok(());
        }
      };

      // As for a first configure(), so reset() keeps this error either way
      if codec.to_lowercase() == "flac" && config.description.is_none() {
        Self::report_configure_error(
          &mut inner,
          "NotSupportedError: FLAC codec requires a description (STREAMINFO)",
        );
        return Ok(());
      }

      // Queue reconfigure via microtask (runs AFTER pending decode microtasks)
      // Use Weak reference to allow close() to immediately close channel without deadlock
      drop(inner); // Release lock before microtask
      if let Some(ref sender) = self.command_sender {
        let weak_sender = Arc::downgrade(sender);
        PromiseRaw::resolve(&env, ())?.then(move |_| {
          // Only send if decoder hasn't been closed (weak reference can still upgrade)
          if let Some(sender) = weak_sender.upgrade() {
            let _ = sender.send(DecoderCommand::Reconfigure(config));
          }
          Ok(())
        })?;
      }
      return Ok(());
    }

    // Parse codec string to determine codec ID
    let codec_id = match parse_audio_codec_string(&codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_configure_error(
          &mut inner,
          &format!("NotSupportedError: Invalid codec: {}", e),
        );
        return Ok(());
      }
    };

    // W3C WebCodecs spec: FLAC codec requires description (contains STREAMINFO)
    let codec_lower = codec.to_lowercase();
    if codec_lower == "flac" && config.description.is_none() {
      Self::report_configure_error(
        &mut inner,
        "NotSupportedError: FLAC codec requires a description (STREAMINFO)",
      );
      return Ok(());
    }

    // Create decoder context
    let mut context = match CodecContext::new_decoder(codec_id) {
      Ok(ctx) => ctx,
      Err(e) => {
        Self::report_configure_error(&mut inner, &format!("Failed to create decoder: {}", e));
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
      Self::report_configure_error(&mut inner, &format!("Failed to configure decoder: {}", e));
      return Ok(());
    }

    // Open the decoder
    if let Err(e) = context.open() {
      Self::report_configure_error(&mut inner, &format!("Failed to open decoder: {}", e));
      return Ok(());
    }

    inner.context = Some(context);
    inner.config = Some(decoder_config);
    inner.codec_string = codec;
    inner.state = CodecState::Configured;
    inner.frame_count = 0;
    let cleared_queue = inner.decode_queue_size as usize;
    inner.decode_queue_size = 0;
    if cleared_queue > 0
      && let Ok(mut es) = self.event_state.write()
    {
      es.note_queue_cleared(&env, cleared_queue);
    }
    inner.timestamp_queue.clear();

    // Create new channel and worker for decode operations
    if self.command_sender.is_none() {
      let (sender, receiver) = channel::unbounded();
      self.command_sender = Some(Arc::new(sender));
      let worker_inner = self.inner.clone();
      let worker_event_state = self.event_state.clone();
      let worker_cancel = self.cancel.clone();
      drop(inner);
      self.worker_handle = Some(std::thread::spawn(move || {
        Self::worker_loop(worker_inner, worker_event_state, receiver, worker_cancel);
      }));
    }

    Ok(())
  }

  /// Decode an encoded audio chunk
  #[napi]
  pub fn decode(&self, env: Env, chunk: &EncodedAudioChunk) -> Result<()> {
    // Extract data and timestamp on main thread (brief lock)
    let (chunk, timestamp, duration) = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      // W3C spec: throw InvalidStateError if not configured or closed
      if inner.state == CodecState::Closed {
        return throw_invalid_state_error(&env, "Cannot decode with a closed codec");
      }
      if inner.state != CodecState::Configured {
        return throw_invalid_state_error(&env, "Cannot decode with an unconfigured codec");
      }

      let timestamp = match chunk.get_timestamp() {
        Ok(ts) => ts,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get timestamp: {}", e));
          return Ok(());
        }
      };
      let duration = match chunk.duration() {
        Ok(duration) => duration,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get duration: {}", e));
          return Ok(());
        }
      };

      // W3C spec: throw DataError if first chunk is not a keyframe
      let is_key = chunk.is_key();
      if !inner.keyframe_received {
        if is_key {
          inner.keyframe_received = true;
        } else {
          // Trying to decode a delta chunk before any key chunk
          return throw_data_error(&env, "First chunk must be a keyframe");
        }
      }

      // Increment queue size (pending operation)
      inner.decode_queue_size += 1;
      if let Ok(mut es) = self.event_state.write() {
        es.note_enqueue(&env);
      }

      (Arc::clone(&chunk.inner), timestamp, duration)
    };

    // Send decode command to worker thread (deferred to microtask for W3C spec compliance)
    // This ensures decodeQueueSize check happens before the worker receives the command
    // Use Weak reference to allow close() to immediately close channel without deadlock
    if let Some(ref sender) = self.command_sender {
      let weak_sender = Arc::downgrade(sender);
      let cancel = self.cancel.clone();
      PromiseRaw::resolve(&env, ())?.then(move |_| {
        // Skip if reset()/close() ran since (JS is single-threaded, so it
        // must have completed before this microtask runs)
        if !cancel.is_cancelled() {
          // Only send if decoder hasn't been closed (weak reference can still upgrade)
          if let Some(sender) = weak_sender.upgrade() {
            let _ = sender.send(DecoderCommand::Decode {
              chunk,
              timestamp,
              duration,
            });
          }
        }
        Ok(())
      })?;
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
    let flush_abort = Arc::new(FlushAbort::default());
    let (response_sender, response_receiver) = channel::bounded::<Result<()>>(1);

    // W3C spec: Check state upfront and return rejected promise with appropriate error
    // (not throw synchronously - flush() should always return a promise)
    let flush_id = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        // Chrome closes the codec when its error callback runs. Before that,
        // a flush is still accepted and fails with the codec's error; after
        // it, the codec is closed and flush() fails with InvalidStateError.
        let (error_name, error_msg) = match inner.failure_name {
          Some(name) if !self.epoch.error_delivered() => (name, "Decode error occurred"),
          _ => (
            DOMExceptionName::InvalidStateError,
            "Cannot flush a closed codec",
          ),
        };
        // Return rejected promise with native DOMException (async to allow error callback to run)
        return reject_with_dom_exception_async(env, error_name, error_msg);
      }
      if inner.state == CodecState::Unconfigured {
        // Return rejected promise with native DOMException (async to allow error callback to run)
        return reject_with_dom_exception_async(
          env,
          DOMExceptionName::InvalidStateError,
          "Cannot flush an unconfigured codec",
        );
      }

      // W3C spec: flush() sets [[key chunk required]] = true synchronously,
      // so a delta chunk decoded after flush() must throw DataError.
      inner.keyframe_received = false;

      inner
        .flushes
        .register(response_sender.clone(), flush_abort.clone())
    };

    // Send flush command through the channel (deferred to microtask for W3C spec compliance)
    // This ensures flush is processed after all pending decode microtasks complete (FIFO order)
    // Use Weak reference to allow close() to immediately close channel without deadlock
    if let Some(ref sender) = self.command_sender {
      let weak_sender = Arc::downgrade(sender);
      let cancel = self.cancel.clone();
      PromiseRaw::resolve(env, ())?.then(move |_| {
        // Skip if reset()/close() ran since
        // (flush Promise is already rejected with AbortError by reset())
        if !cancel.is_cancelled() {
          // Only send if decoder hasn't been closed (weak reference can still upgrade)
          if let Some(sender) = weak_sender.upgrade() {
            let _ = sender.send(DecoderCommand::Flush(response_sender));
          }
        }
        Ok(())
      })?;
    } else {
      if let Ok(mut inner) = self.inner.lock() {
        inner.flushes.finish(flush_id);
      }
      return reject_with_dom_exception_async(
        env,
        DOMExceptionName::InvalidStateError,
        "Cannot flush a closed codec",
      );
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

        Ok((result, inner_clone, flush_abort, flush_id))
      },
      move |env, (result, inner, abort, flush_id)| {
        // Drain pending data and call output callback SYNCHRONOUSLY
        // This runs on the main thread with Env access
        let data_items = {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          guard.flushes.begin_output_delivery(flush_id);
          std::mem::take(&mut guard.pending_data)
        };

        // Call output callback for each data item synchronously
        // If callback calls reset(), abort will be set before next iteration
        let callback_result = (|| -> Result<()> {
          let callback = output_callback_ref.borrow_back(env)?;
          for data in data_items {
            // Check abort flag before each callback - exit early if reset() was called
            if abort.is_aborted() {
              break;
            }
            callback.call(data)?;
          }
          Ok(())
        })();

        // Always release this operation, including when the JS callback throws.
        {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          guard.flushes.finish(flush_id);
        }
        callback_result?;

        // Check abort flag after draining all data
        // close()/reset() abort the flush; an error that closed the codec fails it
        if abort.is_aborted() {
          let (name, message) = abort.rejection();
          return Err(native_dom_exception_error(env, name, message)?);
        }

        // Worker errors carry their DOMException name as a message prefix
        match result {
          Ok(()) => Ok(()),
          Err(error) => Err(dom_exception_rejection_error(env, error)?),
        }
      },
    )
  }

  /// Reset the decoder
  #[napi]
  pub fn reset(&mut self, env: Env) -> Result<()> {
    // W3C spec: throw InvalidStateError if closed. A worker error JS has not
    // seen yet does not count: Chrome only closes the codec when the error
    // callback runs, and reset() drops that error.
    if self.closed || self.epoch.error_delivered() {
      return throw_invalid_state_error(&env, "Cannot reset a closed codec");
    }

    // Cancel before waiting for the lock: the worker finishes at most the
    // operation in progress, drops the rest of its queue, and nothing it
    // queued for JS (outputs, errors) is delivered. A dequeue event already
    // scheduled still runs, as in Chrome (see CodecEventState::clear_queue).
    let generation = self.epoch.advance();

    // Drop sender to signal worker to stop. Pending microtasks hold only Weak
    // references, so the channel disconnects after they observe the cancelled token.
    // Do not join here: the old worker may be waiting on a JS callback.
    drop(self.command_sender.take());
    drop(self.worker_handle.take()); // Detach old worker thread

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    inner.flushes.abort_all();

    // Drop existing context
    inner.context = None;
    inner.config = None;
    inner.codec_string.clear();
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    let cleared_queue = inner.decode_queue_size as usize;
    inner.decode_queue_size = 0;
    CodecEventState::clear_queue(&self.event_state, &env, cleared_queue);
    inner.failure_name = None;
    inner.keyframe_received = false;

    // Clear flush-related state
    inner.flushes.clear();
    inner.pending_data.clear();
    inner.timestamp_queue.clear();

    // Serve the new generation. The detached pre-reset worker keeps its old,
    // now cancelled token, so it cannot resume against the new state.
    inner.generation = generation;
    self.cancel = self.epoch.token();

    // Create new channel and worker for future decode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(Arc::new(sender));
    let worker_inner = self.inner.clone();
    let worker_event_state = self.event_state.clone();
    let worker_cancel = self.cancel.clone();

    // Create synchronization channel to wait for worker to be ready
    let (ready_sender, ready_receiver) = channel::bounded::<()>(1);

    drop(inner); // Release lock before spawning thread
    self.worker_handle = Some(std::thread::spawn(move || {
      // Signal that worker is ready before entering the loop
      let _ = ready_sender.send(());
      Self::worker_loop(worker_inner, worker_event_state, receiver, worker_cancel);
    }));

    // Wait for worker to be ready (prevents race condition)
    let _ = ready_receiver.recv();

    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&mut self, env: Env) -> Result<()> {
    // W3C spec: throw InvalidStateError if already closed. A worker error JS
    // has not seen yet does not count (see reset()); close() drops that error.
    if self.closed || self.epoch.error_delivered() {
      return throw_invalid_state_error(&env, "Cannot close an already closed codec");
    }
    self.closed = true;
    self.epoch.mark_closed();

    // Cancel before waiting for the worker: it finishes at most the operation
    // in progress and drops the rest of its queue, and nothing it queued for
    // JS (outputs, errors) is delivered. A dequeue event already scheduled
    // still runs, as in Chrome (see CodecEventState::clear_queue).
    self.epoch.advance();

    // Drop sender to stop accepting new commands
    // This immediately closes the channel since Weak references in microtasks
    // can't keep it alive (they fail to upgrade after Arc is dropped)
    self.command_sender = None;

    // Now safe to join - the worker is cancelled and sees the channel disconnect
    // after the operation in progress
    if let Some(handle) = self.worker_handle.take() {
      let _ = handle.join();
    }

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Aborts the flush promises too, even when their deferred send microtask
    // lost the command sender above.
    inner.close_codec(&env, &self.event_state, None);

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

    if config.description_detached {
      return reject_with_type_error(env, "Invalid config; description is detached.");
    }

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

  // ============================================================================
  // EventTarget interface (W3C DOM spec)
  // ============================================================================

  /// Add an event listener for the specified event type
  /// Uses separate RwLock to avoid blocking on decode operations
  #[napi(
    ts_args_type = "eventType: string, callback: (event: Event) => unknown, options?: AudioDecoderAddEventListenerOptions | undefined | null"
  )]
  pub fn add_event_listener(
    &self,
    env: Env,
    this: This,
    event_type: String,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    options: Option<AudioDecoderAddEventListenerOptions>,
  ) -> Result<()> {
    let once = options.as_ref().and_then(|o| o.once).unwrap_or(false);
    let capture = options.as_ref().and_then(|o| o.capture).unwrap_or(false);
    CodecEventState::add_event_listener(
      &self.event_state,
      &env,
      this.object,
      &event_type,
      callback,
      once,
      capture,
    )
  }

  /// Remove an event listener for the specified event type
  #[napi(
    ts_args_type = "eventType: string, callback: (event: Event) => unknown, options?: AudioDecoderEventListenerOptions | undefined | null"
  )]
  pub fn remove_event_listener(
    &self,
    env: Env,
    event_type: String,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    options: Option<AudioDecoderEventListenerOptions>,
  ) -> Result<()> {
    let capture = options.as_ref().and_then(|o| o.capture).unwrap_or(false);
    CodecEventState::remove_event_listener(&self.event_state, &env, &event_type, &callback, capture)
  }

  /// Dispatch an event to all registered listeners
  #[napi]
  pub fn dispatch_event(&self, env: Env, event_type: String) -> Result<bool> {
    crate::webcodecs::event_target::dispatch(&env, &self.event_state, &event_type);
    Ok(true) // Event was not cancelled
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

  // Allocate and copy data to packet using safe wrapper
  // NOTE: This must be done BEFORE setting timestamps because copy_data_from
  // calls unref() internally which would reset timestamps to AV_NOPTS_VALUE.
  packet.copy_data_from(data).map_err(|e| {
    Error::new(
      Status::GenericFailure,
      format!("Failed to copy packet data: {}", e),
    )
  })?;

  // Set packet timestamps AFTER copying data (unref in copy_data_from resets timestamps)
  packet.set_pts(timestamp);
  packet.set_dts(timestamp);

  // Decode
  let frames = context
    .decode(Some(&packet))
    .map_err(|e| Error::new(Status::GenericFailure, format!("Decode failed: {}", e)))?;

  Ok(frames)
}
