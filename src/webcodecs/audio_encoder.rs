//! AudioEncoder - WebCodecs API implementation
//!
//! Provides audio encoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#audioencoder-interface

use crate::codec::{
  AudioEncoderConfig as InternalAudioEncoderConfig, AudioSampleBuffer, CodecContext, Frame,
  Resampler, context::get_audio_encoder_name,
};
use crate::ffi::{AVCodecID, AVSampleFormat};
use crate::webcodecs::codec_epoch::{
  CancelToken, CodecEpoch, ErrorOrigin, GatedError, GatedOutput, gated_error, gated_output,
};
use crate::webcodecs::error::{
  DOMExceptionName, error_as_dom_exception, native_dom_exception_error, throw_invalid_state_error,
  throw_type_error_unit,
};
use crate::webcodecs::event_target::CodecEventState;
use crate::webcodecs::flush_tracker::{FlushAbort, FlushTracker};
use crate::webcodecs::promise_reject::{reject_with_dom_exception_async, reject_with_type_error};
use crate::webcodecs::{
  AacBitstreamFormat, AudioData, AudioEncoderConfig, AudioEncoderSupport, EncodedAudioChunk,
};
use crossbeam::channel::{self, Receiver, Sender};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunctionCallMode, UnknownReturnValue};
use napi_derive::napi;
use std::rc::Rc;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use super::video_encoder::CodecState;

/// Type alias for output callback (takes chunk and metadata as separate args)
/// Using FnArgs to spread tuple as separate callback arguments per WebCodecs spec
/// Using CalleeHandled: false for direct callbacks without error-first convention
/// The leading u32 is the codec generation; a gated wrapper drops stale calls.
type OutputCallback = GatedOutput<FnArgs<(u32, EncodedAudioChunk, EncodedAudioChunkMetadata)>>;

/// Type alias for error callback (takes the codec generation and an Error)
/// Using CalleeHandled: false because WebCodecs error callback receives the
/// error directly, not error-first (err, result) style. The Error payload is
/// converted to a native DOMException in the JS-side callback.
type ErrorCallback = GatedError;

/// Options for addEventListener (W3C DOM spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AudioEncoderAddEventListenerOptions {
  pub capture: Option<bool>,
  pub once: Option<bool>,
  pub passive: Option<bool>,
}

/// Options for removeEventListener (W3C DOM spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct AudioEncoderEventListenerOptions {
  pub capture: Option<bool>,
}

/// AudioEncoder init dictionary per WebCodecs spec
pub struct AudioEncoderInit {
  /// Output callback - called when encoded chunk is available (ThreadsafeFunction for worker)
  pub output: OutputCallback,
  /// Output callback reference - stored for synchronous calls from main thread
  pub output_ref:
    FunctionRef<FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>, UnknownReturnValue>,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
  /// Error callback reference - prevents GC from collecting the error callback
  pub error_ref: FunctionRef<Error, UnknownReturnValue>,
  /// Cancellation shared with the gated callbacks above
  pub(crate) epoch: CodecEpoch,
}

impl FromNapiValue for AudioEncoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = unsafe { Object::from_napi_value(env, value)? };

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
    let error: ErrorCallback = gated_error(&env_wrapper, error_func.raw(), &epoch)?;

    Ok(AudioEncoderInit {
      output,
      output_ref,
      error,
      error_ref,
      epoch,
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
  /// Reconfigure the encoder with a new configuration
  Reconfigure(AudioEncoderConfig),
}

/// Internal encoder state
struct AudioEncoderInner {
  state: CodecState,
  /// Most recently requested configuration. `encode()` uses this immediately
  /// so calls made after a queued reconfigure prepare input for the new codec.
  config: Option<AudioEncoderConfig>,
  /// Configuration currently installed on the worker. Output produced before
  /// a queued reconfigure must continue to advertise this configuration.
  active_config: Option<AudioEncoderConfig>,
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
  /// The codec's event state: report_error orders the error callback before
  /// the dequeue events that come due meanwhile.
  event_state: Arc<RwLock<CodecEventState>>,
  /// Pending flush operations, tracked independently for overlapping calls.
  flushes: FlushTracker,
  /// Queue of input timestamps for correlation with output packets
  /// (needed because FFmpeg may buffer frames internally)
  timestamp_queue: std::collections::VecDeque<i64>,
  /// Base timestamp from the first input AudioData (for timestamp calculation)
  base_timestamp: Option<i64>,
  /// W3C spec: Flag to suppress output delivery after reset()
  /// When true, pending outputs are not delivered to the user callback
  #[allow(dead_code)]
  output_suppressed: bool,
  /// Queue of encoded chunks waiting to be delivered via output callback
  /// Worker pushes chunks here during flush; flush() drains them synchronously via FunctionRef
  pending_chunks: Vec<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
  /// Name of the error that closed the encoder; a flush issued before the
  /// error callback runs rejects with it instead of InvalidStateError
  failure_name: Option<DOMExceptionName>,
  /// AAC-specific: Whether to use ADTS format (headers prepended to each frame)
  use_adts: bool,
  /// Cached AAC parameters for ADTS header generation
  /// (sample_rate_index, channel_config)
  adts_params: Option<(u8, u8)>,
  /// Cached decoder config for FLAC - W3C FLAC codec spec requires decoderConfig on every chunk
  /// See: https://w3c.github.io/webcodecs/flac_codec_registration.html
  /// Stores (codec, sample_rate, number_of_channels, description_bytes)
  cached_flac_decoder_config: Option<(String, f64, u32, Option<Vec<u8>>)>,
  /// Shared with the codec, its workers and its gated callbacks
  epoch: CodecEpoch,
  /// Generation this state serves. Outputs and errors are tagged with it;
  /// dequeue events are not (see `CodecEventState::clear_queue`). reset()
  /// moves it to the new generation once it holds the lock.
  generation: u32,
}

impl AudioEncoderInner {
  /// close()/reset() moved the codec past the generation this state serves.
  /// A worker still finishing old work leaves the codec to them.
  fn is_stale(&self) -> bool {
    !self.epoch.is_current(self.generation)
  }

  /// Worker: one queued encode finished; fire its dequeue event.
  fn dequeue_one(&mut self, event_state: &Arc<RwLock<CodecEventState>>) {
    if self.is_stale() {
      return;
    }
    let old_size = self.encode_queue_size;
    self.encode_queue_size = old_size.saturating_sub(1);
    if old_size > 0 {
      CodecEventState::fire(event_state);
    }
  }

  /// Worker: every queued encode is dropped at once; one dequeue event covers
  /// them all.
  fn dequeue_all(&mut self, event_state: &Arc<RwLock<CodecEventState>>) {
    if self.is_stale() {
      return;
    }
    CodecEventState::dequeue_all(event_state, &mut self.encode_queue_size);
  }

  /// JS thread: close the encoder. `failure` is the error that closed it: pending
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
    self.resampler = None;
    self.sample_buffer = None;
    self.config = None;
    self.active_config = None;
    self.state = CodecState::Closed;
    self.failure_name = failure;
    self.pending_chunks.clear();
    let cleared_queue = self.encode_queue_size as usize;
    self.encode_queue_size = 0;
    CodecEventState::clear_queue(event_state, env, cleared_queue);
  }
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
  /// Separate lock for EventTarget state to avoid lock contention with encode operations.
  /// This allows addEventListener to complete immediately even when worker holds inner lock.
  event_state: Arc<RwLock<CodecEventState>>,
  /// Output callback reference - stored for synchronous calls from main thread (in flush resolver)
  /// Wrapped in Rc to allow sharing with spawn_future_with_callback closure
  /// (Rc is !Send but that's OK - the callback runs on the main thread)
  output_callback_ref:
    Rc<FunctionRef<FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>, UnknownReturnValue>>,
  /// Error callback reference - prevents GC from collecting the error callback
  /// (weak ThreadsafeFunction alone can be collected on slow platforms like armv7 QEMU)
  #[allow(dead_code)]
  error_callback_ref: Rc<FunctionRef<Error, UnknownReturnValue>>,
  /// Channel sender for worker commands (wrapped in Arc for Weak references in microtasks)
  command_sender: Option<Arc<Sender<EncoderCommand>>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
  /// Shared cancellation: close()/reset() advance it before taking the lock
  epoch: CodecEpoch,
  /// The current worker's token; cancelled once close()/reset() runs
  cancel: CancelToken,
  /// close() ran. Together with a delivered error, this is "closed" as JS sees it.
  closed: bool,
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
    if let Ok(mut inner) = self.inner.lock()
      && let Some(ctx) = inner.context.as_mut()
    {
      ctx.flush();
      let _ = ctx.send_frame(None);
      while ctx.receive_packet().ok().flatten().is_some() {}
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
      ts_arg_type = "{ output: (chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata) => void, error: (error: DOMException) => void }"
    )]
    init: AudioEncoderInit,
  ) -> Result<Self> {
    let event_state = Arc::new(RwLock::new(CodecEventState::default()));
    let inner = AudioEncoderInner {
      state: CodecState::Unconfigured,
      config: None,
      active_config: None,
      context: None,
      resampler: None,
      sample_buffer: None,
      frame_count: 0,
      extradata_sent: false,
      target_format: AVSampleFormat::Fltp,
      encode_queue_size: 0,
      output_callback: init.output,
      error_callback: init.error,
      event_state: event_state.clone(),
      flushes: FlushTracker::default(),
      timestamp_queue: std::collections::VecDeque::new(),
      base_timestamp: None,
      output_suppressed: false,
      pending_chunks: Vec::new(),
      failure_name: None,
      use_adts: false,
      adts_params: None,
      cached_flac_decoder_config: None,
      epoch: init.epoch.clone(),
      generation: init.epoch.current(),
    };

    let inner = Arc::new(Mutex::new(inner));

    // Create separate lock for event listener state (avoids contention with encode operations)

    // An unsupported config that configure() reported still closes the encoder
    // when its error reaches JS after reset(), as in Chrome.
    init
      .epoch
      .set_close_on_error(&inner, &event_state, |inner, env, events, name| {
        inner.close_codec(env, events, Some(name));
      });

    // Create channel for worker commands
    let (sender, receiver) = channel::unbounded();

    // Token the worker and the send microtasks check
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
    inner: Arc<Mutex<AudioEncoderInner>>,
    event_state: Arc<RwLock<CodecEventState>>,
    receiver: Receiver<EncoderCommand>,
    cancel: CancelToken,
  ) {
    // Simple blocking recv - channel disconnects when all senders dropped
    // (including microtask cloned senders after they see the cancelled token and skip sending)
    while let Ok(command) = receiver.recv() {
      // close()/reset() cancelled this worker: drop the rest of its queue
      if cancel.is_cancelled() {
        // Still process flush commands to send responses, but skip encodes
        if let EncoderCommand::Flush(response_sender) = command {
          let _ = response_sender.send(Err(Error::new(
            Status::GenericFailure,
            "AbortError: The operation was aborted",
          )));
        }
        continue;
      }

      match command {
        EncoderCommand::Encode { frame, timestamp } => {
          Self::process_encode(&inner, &event_state, frame, timestamp, &cancel);
        }
        EncoderCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner, &event_state, &cancel);
          let _ = response_sender.send(result);
        }
        EncoderCommand::Reconfigure(config) => {
          Self::process_reconfigure(&inner, &config, &cancel);
        }
      }
    }
  }

  /// Process an encode command on the worker thread
  fn process_encode(
    inner: &Arc<Mutex<AudioEncoderInner>>,
    event_state: &Arc<RwLock<CodecEventState>>,
    frame: Frame,
    timestamp: i64,
    cancel: &CancelToken,
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // A detached pre-reset worker may have received this command immediately
    // before reset set its token. Recheck under the shared-state lock so it
    // cannot mutate the next generation's queue accounting or codec state.
    if cancel.is_cancelled() {
      return;
    }

    // Check if encoder is still configured
    if guard.state != CodecState::Configured {
      // Per W3C spec: "cease producing output" - silently discard pending work
      // State could be Unconfigured (reset called) or Closed (close called)
      // Don't call report_error() - that would set state to Closed and invoke error callback
      // Every encode still queued is discarded the same way, so dequeue them all
      // now: one dequeue event, as Chrome fires when it resets the queue.
      guard.dequeue_all(event_state);
      return;
    }

    // Track base timestamp from first input for output timestamp calculation
    if guard.base_timestamp.is_none() {
      guard.base_timestamp = Some(timestamp);
    }

    // Get config info (unwrap validated config values)
    let codec_string = match guard.active_config.as_ref() {
      Some(config) => config.codec.clone().unwrap_or_default(),
      None => {
        Self::close_with_error(&mut guard, event_state, "No encoder config");
        return;
      }
    };

    // Add frame to sample buffer
    {
      let sample_buffer = match guard.sample_buffer.as_mut() {
        Some(buf) => buf,
        None => {
          Self::close_with_error(&mut guard, event_state, "No sample buffer");
          return;
        }
      };

      if let Err(e) = sample_buffer.add_frame(&frame) {
        Self::close_with_error(
          &mut guard,
          event_state,
          &format!("Failed to add samples: {}", e),
        );
        return;
      }
    }

    // Check if codec is FLAC - W3C FLAC spec requires decoderConfig on every chunk
    let is_flac = codec_string.to_lowercase() == "flac";

    // Get extradata before encoding first frame
    // For FLAC, prepend the "fLaC" header since FFmpeg only returns raw STREAMINFO
    // Note: For FLAC, we always try to get extradata because it may not be available
    // until after encoding starts, but we need it on every chunk per W3C spec
    // For ADTS AAC, description is not needed (metadata embedded in frame headers)
    let extradata = if guard.use_adts {
      None // ADTS format: No description needed (self-describing)
    } else if !guard.extradata_sent || is_flac {
      let raw_extradata = guard
        .context
        .as_ref()
        .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()));

      raw_extradata.map(|data| {
        if is_flac {
          prepend_flac_header(&data)
        } else {
          data
        }
      })
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
          Self::close_with_error(&mut guard, event_state, "No sample buffer");
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
            Self::close_with_error(&mut guard, event_state, "No sample buffer");
            return;
          }
        };
        match sample_buffer.take_frame() {
          Ok(Some(f)) => f,
          Ok(None) => {
            Self::close_with_error(&mut guard, event_state, "No frame available");
            return;
          }
          Err(e) => {
            Self::close_with_error(
              &mut guard,
              event_state,
              &format!("Failed to get frame: {}", e),
            );
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
          Self::close_with_error(&mut guard, event_state, "No encoder context");
          return;
        }
      };

      let packets = match context.encode(Some(&frame_to_encode)) {
        Ok(pkts) => pkts,
        Err(e) => {
          Self::close_with_error(&mut guard, event_state, &format!("Encode failed: {}", e));
          return;
        }
      };

      guard.frame_count += 1;

      // Calculate duration per frame in microseconds
      let duration_us = (frame_size * 1_000_000) / sample_rate;

      // Process output packets - call callback for each
      // Pop timestamp from queue to preserve original input timestamp
      let adts_params = if guard.use_adts {
        guard.adts_params
      } else {
        None
      };
      for packet in packets {
        let output_timestamp = guard.timestamp_queue.pop_front();
        let chunk = EncodedAudioChunk::from_packet_with_adts(
          packet,
          Some(duration_us),
          output_timestamp,
          adts_params,
        );

        // Create metadata
        // W3C FLAC codec spec requires decoderConfig on every chunk
        // See: https://w3c.github.io/webcodecs/flac_codec_registration.html
        let metadata = if is_flac {
          // FLAC: Always include decoderConfig with fresh extradata on every chunk
          let (target_sample_rate, target_channels) = guard
            .active_config
            .as_ref()
            .map(|c| {
              (
                c.sample_rate.unwrap_or(48000.0),
                c.number_of_channels.unwrap_or(2),
              )
            })
            .unwrap_or((48000.0, 2));

          guard.extradata_sent = true;
          EncodedAudioChunkMetadata {
            decoder_config: Some(AudioDecoderConfigOutput {
              codec: codec_string.clone(),
              sample_rate: Some(target_sample_rate),
              number_of_channels: Some(target_channels),
              description: extradata.clone().map(Uint8Array::from),
            }),
          }
        } else if !guard.extradata_sent {
          // Non-FLAC: Only send decoderConfig on first chunk
          guard.extradata_sent = true;
          let (target_sample_rate, target_channels) = guard
            .active_config
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
        if guard.flushes.should_buffer_outputs() {
          guard.pending_chunks.push((chunk, metadata));
        } else if !guard.is_stale() {
          guard.output_callback.call(
            FnArgs::from((guard.generation, chunk, metadata)),
            ThreadsafeFunctionCallMode::NonBlocking,
          );
        }
      }
    }

    // Decrement queue size and fire dequeue event (only if queue was not empty)
    // Note: Using NonBlocking mode means callbacks are scheduled asynchronously,
    // so holding the mutex during the call is safe (no re-entrancy deadlock).
    guard.dequeue_one(event_state);
  }

  /// Process a flush command on the worker thread
  fn process_flush(
    inner: &Arc<Mutex<AudioEncoderInner>>,
    _event_state: &Arc<RwLock<CodecEventState>>,
    cancel: &CancelToken,
  ) -> Result<()> {
    let active_config = {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if cancel.is_cancelled() {
        return Err(Error::new(
          Status::GenericFailure,
          "AbortError: The operation was aborted",
        ));
      }

      // W3C spec: a flush pending when an error closed the encoder rejects
      // with that error. This must be checked first to return the correct error type.
      if guard.state == CodecState::Closed
        && let Some(name) = guard.failure_name
      {
        return Err(Error::new(
          Status::GenericFailure,
          format!("{}: Encode error occurred", name.as_str()),
        ));
      }

      // Per W3C spec: state check happens on main thread (in flush() method).
      // If state changed after that check (e.g., reconfigure failed but no encode error),
      // silently succeed. The error callback has already been invoked by the failing operation.
      // This is consistent with process_encode() which silently discards when state is wrong.
      if guard.state != CodecState::Configured {
        return Ok(());
      }

      // Check if FLAC - W3C FLAC spec requires decoderConfig on every chunk
      let is_flac = guard
        .active_config
        .as_ref()
        .is_some_and(|c| c.codec.as_deref().unwrap_or("").to_lowercase() == "flac");

      // Get config info for FLAC metadata
      let (codec_string, target_sample_rate, target_channels) = guard
        .active_config
        .as_ref()
        .map(|c| {
          (
            c.codec.clone().unwrap_or_default(),
            c.sample_rate.unwrap_or(48000.0),
            c.number_of_channels.unwrap_or(2),
          )
        })
        .unwrap_or_default();

      // Helper to get FLAC extradata with header
      let get_flac_extradata =
        |ctx: &CodecContext| -> Option<Vec<u8>> { ctx.extradata().map(prepend_flac_header) };

      // Flush any remaining samples in buffer.
      let (remainder, frame_size, sample_rate) = match guard.sample_buffer.as_mut() {
        Some(sample_buffer) => {
          let frame_size = sample_buffer.frame_size() as i64;
          let sample_rate = sample_buffer.sample_rate() as i64;
          let remainder = match sample_buffer.flush() {
            Ok(frame) => frame,
            Err(error) => {
              let message = format!("Failed to flush buffered audio: {}", error);
              Self::report_error(&mut guard, &message);
              return Err(Error::new(
                Status::GenericFailure,
                format!("EncodingError: {}", message),
              ));
            }
          };
          (remainder, frame_size, sample_rate)
        }
        None => (None, 0, 1),
      };
      if let Some(mut frame) = remainder {
        // Set timestamp using base_timestamp
        let base_ts = guard.base_timestamp.unwrap_or(0);
        let frame_timestamp =
          base_ts + (guard.frame_count as i64 * frame_size * 1_000_000) / sample_rate;
        frame.set_pts(frame_timestamp);

        // Push timestamp to queue for output correlation
        guard.timestamp_queue.push_back(frame_timestamp);

        let context = match guard.context.as_mut() {
          Some(ctx) => ctx,
          None => {
            let message = "No encoder context";
            Self::report_error(&mut guard, message);
            return Err(Error::new(
              Status::GenericFailure,
              format!("EncodingError: {}", message),
            ));
          }
        };

        let packets = match context.encode(Some(&frame)) {
          Ok(packets) => packets,
          Err(error) => {
            let message = format!("Failed to encode buffered audio: {}", error);
            Self::report_error(&mut guard, &message);
            return Err(Error::new(
              Status::GenericFailure,
              format!("EncodingError: {}", message),
            ));
          }
        };
        let duration_us = (frame.nb_samples() as i64 * 1_000_000) / sample_rate;
        let adts_params = if guard.use_adts {
          guard.adts_params
        } else {
          None
        };
        for packet in packets {
          let output_timestamp = guard.timestamp_queue.pop_front();
          let chunk = EncodedAudioChunk::from_packet_with_adts(
            packet,
            Some(duration_us),
            output_timestamp,
            adts_params,
          );
          // Create decoderConfig: FLAC on every chunk, others only on first chunk
          let decoder_config = if is_flac {
            // FLAC: Always include decoderConfig with fresh extradata
            let extradata = guard.context.as_ref().and_then(get_flac_extradata);
            Some(AudioDecoderConfigOutput {
              codec: codec_string.clone(),
              sample_rate: Some(target_sample_rate),
              number_of_channels: Some(target_channels),
              description: extradata.map(Uint8Array::from),
            })
          } else if !guard.extradata_sent {
            // First chunk: Include decoderConfig
            guard.extradata_sent = true;
            // ADTS format: No description (metadata in frame headers)
            // Raw AAC: Include AudioSpecificConfig
            let extradata = if guard.use_adts {
              None
            } else {
              guard
                .context
                .as_ref()
                .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
            };
            Some(AudioDecoderConfigOutput {
              codec: codec_string.clone(),
              sample_rate: Some(target_sample_rate),
              number_of_channels: Some(target_channels),
              description: extradata.map(Uint8Array::from),
            })
          } else {
            None
          };
          let metadata = EncodedAudioChunkMetadata { decoder_config };
          // Always queue during flush for synchronous delivery
          guard.pending_chunks.push((chunk, metadata));
        }
      }

      // Flush encoder
      let context = match guard.context.as_mut() {
        Some(ctx) => ctx,
        None => {
          let message = "No encoder context";
          Self::report_error(&mut guard, message);
          return Err(Error::new(
            Status::GenericFailure,
            format!("EncodingError: {}", message),
          ));
        }
      };

      let packets = match context.flush_encoder() {
        Ok(pkts) => pkts,
        Err(e) => {
          let message = format!("Flush failed: {}", e);
          Self::report_error(&mut guard, &message);
          return Err(Error::new(
            Status::GenericFailure,
            format!("EncodingError: {}", message),
          ));
        }
      };

      // Queue remaining packets for synchronous delivery in resolver
      let adts_params = if guard.use_adts {
        guard.adts_params
      } else {
        None
      };
      for packet in packets {
        let output_timestamp = guard.timestamp_queue.pop_front();
        let chunk =
          EncodedAudioChunk::from_packet_with_adts(packet, None, output_timestamp, adts_params);
        // Create decoderConfig: FLAC on every chunk, others only on first chunk
        let decoder_config = if is_flac {
          // FLAC: Always include decoderConfig with fresh extradata
          let extradata = guard.context.as_ref().and_then(get_flac_extradata);
          Some(AudioDecoderConfigOutput {
            codec: codec_string.clone(),
            sample_rate: Some(target_sample_rate),
            number_of_channels: Some(target_channels),
            description: extradata.map(Uint8Array::from),
          })
        } else if !guard.extradata_sent {
          // First chunk: Include decoderConfig
          guard.extradata_sent = true;
          // ADTS format: No description (metadata in frame headers)
          // Raw AAC: Include AudioSpecificConfig
          let extradata = if guard.use_adts {
            None
          } else {
            guard
              .context
              .as_ref()
              .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
          };
          Some(AudioDecoderConfigOutput {
            codec: codec_string.clone(),
            sample_rate: Some(target_sample_rate),
            number_of_channels: Some(target_channels),
            description: extradata.map(Uint8Array::from),
          })
        } else {
          None
        };
        let metadata = EncodedAudioChunkMetadata { decoder_config };
        // Always queue during flush for synchronous delivery
        guard.pending_chunks.push((chunk, metadata));
      }
      guard.active_config.clone()
    }; // mutex released here

    // FFmpeg audio encoders enter EOF after flush_encoder(); avcodec_flush_buffers
    // does not make all of them reusable. Recreate the active context before
    // resolving flush so the configured encoder can accept more AudioData.
    if let Some(config) = active_config {
      Self::process_reconfigure(inner, &config, cancel);

      let guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      if let Some(name) = guard.failure_name {
        return Err(Error::new(
          Status::GenericFailure,
          format!("{}: Failed to restore encoder after flush", name.as_str()),
        ));
      }
    }

    Ok(())
  }

  /// Process a reconfigure command on the worker thread
  /// Drains old context, clears work state, and creates new encoder context
  fn process_reconfigure(
    inner: &Arc<Mutex<AudioEncoderInner>>,
    config: &AudioEncoderConfig,
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
      let _ = ctx.send_frame(None);
      while ctx.receive_packet().ok().flatten().is_some() {}
    }

    // Clear codec-local work state. Do not reset encode_queue_size here:
    // main-thread encode() calls after this FIFO command are already counted.
    guard.timestamp_queue.clear();
    guard.frame_count = 0;
    guard.extradata_sent = false;
    guard.base_timestamp = None;

    // Parse codec string to determine codec ID
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
        Self::report_error(&mut guard, &format!("Failed to create encoder: {}", e));
        return;
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
      Self::report_error(&mut guard, &format!("Failed to configure encoder: {}", e));
      return;
    }

    // Open the encoder
    if let Err(e) = context.open() {
      Self::report_error(&mut guard, &format!("Failed to open encoder: {}", e));
      return;
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

    // Check if AAC ADTS format is requested
    let is_aac = codec.to_lowercase().starts_with("mp4a.40") || codec.to_lowercase() == "aac";
    let use_adts = is_aac
      && config
        .aac
        .as_ref()
        .and_then(|aac| aac.format)
        .map(|f| f == AacBitstreamFormat::Adts)
        .unwrap_or(false);

    // Cache ADTS parameters if needed
    let adts_params = if use_adts {
      let sample_rate_index = get_aac_sample_rate_index(sample_rate as u32);
      let channel_config = std::cmp::min(number_of_channels, 7) as u8;
      Some((sample_rate_index, channel_config))
    } else {
      None
    };

    // Update state
    guard.context = Some(context);
    guard.sample_buffer = Some(sample_buffer);
    guard.target_format = target_format;
    guard.resampler = None;
    guard.use_adts = use_adts;
    guard.adts_params = adts_params;
    guard.active_config = Some(config.clone());
    guard.cached_flac_decoder_config = None;
  }

  /// Report an error via callback and close the encoder
  fn report_error(inner: &mut AudioEncoderInner, error_msg: &str) {
    Self::report_error_from(inner, error_msg, ErrorOrigin::Work);
  }

  /// Report an error configure() found on the JS thread
  fn report_configure_error(inner: &mut AudioEncoderInner, error_msg: &str) {
    Self::report_error_from(inner, error_msg, ErrorOrigin::of_configure_error(error_msg));
  }

  /// Report an error; `origin` decides whether reset() drops it
  fn report_error_from(inner: &mut AudioEncoderInner, error_msg: &str, origin: ErrorOrigin) {
    // A worker cancelled by close()/reset() leaves the codec to them.
    if inner.is_stale() {
      return;
    }

    // Log the error at warn level for debugging (visible even if JS callback fails)
    tracing::warn!(target: "webcodecs", codec = "AudioEncoder", error = error_msg, "Codec error reported");

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

  /// Worker: an error closes the encoder and drops every queued encode. As in
  /// Chrome, encodeQueueSize is already 0 when the error callback runs, and one
  /// dequeue event follows for all the dropped encodes.
  fn close_with_error(
    inner: &mut AudioEncoderInner,
    event_state: &Arc<RwLock<CodecEventState>>,
    message: &str,
  ) {
    Self::report_error(inner, message);
    inner.dequeue_all(event_state);
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

  /// Configure the encoder
  #[napi]
  pub fn configure(&mut self, env: Env, config: AudioEncoderConfig) -> Result<()> {
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
    if let Some(bitrate) = config.bitrate
      && bitrate <= 0.0
    {
      return throw_type_error_unit(&env, "bitrate must be greater than 0");
    }

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: throw InvalidStateError if closed
    if inner.state == CodecState::Closed {
      return throw_invalid_state_error(&env, "Encoder is closed");
    }

    // If already configured, queue reconfigure via microtask for W3C spec FIFO ordering
    // This ensures pending encode commands are processed before reconfiguration
    if inner.state == CodecState::Configured {
      // Validate codec synchronously before queueing
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

      // Prepare subsequent inputs for the requested configuration immediately;
      // the worker keeps its old active_config until this FIFO reconfigure runs.
      inner.config = Some(config.clone());
      inner.target_format = get_encoder_sample_format(codec_id);
      inner.resampler = None;

      // Queue reconfigure via microtask (runs AFTER pending encode microtasks)
      // Use Weak reference to allow close() to immediately close channel without deadlock
      drop(inner); // Release lock before microtask
      if let Some(ref sender) = self.command_sender {
        let weak_sender = Arc::downgrade(sender);
        PromiseRaw::resolve(&env, ())?.then(move |_| {
          // Only send if encoder hasn't been closed (weak reference can still upgrade)
          if let Some(sender) = weak_sender.upgrade() {
            let _ = sender.send(EncoderCommand::Reconfigure(config));
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
        Self::report_configure_error(&mut inner, &format!("Failed to create encoder: {}", e));
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
      Self::report_configure_error(&mut inner, &format!("Failed to configure encoder: {}", e));
      return Ok(());
    }

    // Open the encoder
    if let Err(e) = context.open() {
      Self::report_configure_error(&mut inner, &format!("Failed to open encoder: {}", e));
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
    inner.sample_buffer = Some(sample_buffer);
    inner.target_format = target_format;
    inner.state = CodecState::Configured;
    inner.extradata_sent = false;
    inner.frame_count = 0;
    inner.resampler = None;
    let cleared_queue = inner.encode_queue_size as usize;
    inner.encode_queue_size = 0;
    if cleared_queue > 0
      && let Ok(mut es) = self.event_state.write()
    {
      es.note_queue_cleared(&env, cleared_queue);
    }

    // Check if AAC ADTS format is requested
    let is_aac = codec.to_lowercase().starts_with("mp4a.40") || codec.to_lowercase() == "aac";
    inner.use_adts = is_aac
      && config
        .aac
        .as_ref()
        .and_then(|aac| aac.format)
        .map(|f| f == AacBitstreamFormat::Adts)
        .unwrap_or(false);

    // Cache ADTS parameters if needed
    if inner.use_adts {
      let sample_rate_index = get_aac_sample_rate_index(sample_rate as u32);
      let channel_config = std::cmp::min(number_of_channels, 7) as u8;
      inner.adts_params = Some((sample_rate_index, channel_config));
    } else {
      inner.adts_params = None;
    }

    inner.active_config = Some(config.clone());
    inner.config = Some(config);

    // Create new channel and worker if needed (after reconfiguration)
    if self.command_sender.is_none() {
      let (sender, receiver) = channel::unbounded();
      self.command_sender = Some(Arc::new(sender));
      let worker_inner = self.inner.clone();
      let worker_event_state = self.event_state.clone();
      let worker_cancel = self.cancel.clone();
      drop(inner); // Release lock before spawning thread
      self.worker_handle = Some(std::thread::spawn(move || {
        Self::worker_loop(worker_inner, worker_event_state, receiver, worker_cancel);
      }));
    }

    Ok(())
  }

  /// Encode audio data
  #[napi]
  pub fn encode(&self, env: Env, data: &AudioData) -> Result<()> {
    // W3C spec: throw TypeError if AudioData is closed
    if data.closed()? {
      return throw_type_error_unit(&env, "Cannot encode a closed AudioData");
    }

    // Clone frame, resample if needed, and get timestamp on main thread
    let (frame_to_send, timestamp) = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      // W3C spec: throw InvalidStateError if not configured or closed
      if inner.state == CodecState::Closed {
        return throw_invalid_state_error(&env, "Cannot encode with a closed codec");
      }
      if inner.state != CodecState::Configured {
        return throw_invalid_state_error(&env, "Cannot encode with an unconfigured codec");
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

      // A resampler is valid only for the exact source and destination tuple.
      // AudioEncoder accepts legal mixed-format input, so never reuse a
      // converter created for an earlier AudioData object.
      let resampler_matches = inner.resampler.as_ref().is_some_and(|resampler| {
        resampler.src_channels() == src_channels
          && resampler.src_sample_rate() == src_sample_rate_u32
          && resampler.src_format() == src_format.to_av_format()
          && resampler.dst_channels() == target_channels
          && resampler.dst_sample_rate() == target_sample_rate_u32
          && resampler.dst_format() == inner.target_format
      });

      if !needs_resampling {
        inner.resampler = None;
      } else if !resampler_matches {
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

      // Get frame from AudioData using shallow_clone to share audio buffers via FFmpeg's
      // atomic reference counting. This avoids copying audio sample data (can be 100s of KB).
      //
      // This is safe because:
      // - Resampling: convert_alloc() takes &Frame (read-only) and creates a new output frame
      // - No resampling: avcodec_send_frame() takes const AVFrame* (read-only)
      // - FFmpeg's AVBufferRef handles concurrent access safely with atomic refcounting
      let frame = match data.with_frame(|f| f.shallow_clone()) {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => {
          Self::report_error(&mut inner, &format!("Failed to reference frame: {}", e));
          return Ok(());
        }
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get frame: {}", e));
          return Ok(());
        }
      };

      // Resample if needed (creates new frame) or pass through (shared via refcount)
      let frame_to_send = if needs_resampling {
        let resampler = inner
          .resampler
          .as_mut()
          .expect("resampler is created when conversion is required");
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
      if let Ok(mut es) = self.event_state.write() {
        es.note_enqueue(&env);
      }

      (frame_to_send, timestamp)
    };

    // Send encode command via microtask for W3C spec FIFO ordering
    // This ensures configure() microtasks are processed in correct order with encode()
    // Use Weak reference to allow close() to immediately close channel without deadlock
    if let Some(ref sender) = self.command_sender {
      let weak_sender = Arc::downgrade(sender);
      let cancel = self.cancel.clone();
      PromiseRaw::resolve(&env, ())?.then(move |_| {
        // Skip if reset()/close() ran since, then check the channel is still open
        if !cancel.is_cancelled()
          && let Some(sender) = weak_sender.upgrade()
        {
          let _ = sender.send(EncoderCommand::Encode {
            frame: frame_to_send,
            timestamp,
          });
        }
        Ok(())
      })?;
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
          Some(name) if !self.epoch.error_delivered() => (name, "Encode error occurred"),
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

      inner
        .flushes
        .register(response_sender.clone(), flush_abort.clone())
    };

    // Send flush command through the channel (deferred to microtask for W3C spec compliance)
    // This ensures flush is processed after all pending encode microtasks complete (FIFO order)
    // Use Weak reference to allow close() to immediately close channel without deadlock
    if let Some(ref sender) = self.command_sender {
      let weak_sender = Arc::downgrade(sender);
      let cancel = self.cancel.clone();
      PromiseRaw::resolve(env, ())?.then(move |_| {
        // Skip if reset()/close() ran since, then check the channel is still open
        // (flush Promise is already rejected with AbortError by reset())
        if !cancel.is_cancelled()
          && let Some(sender) = weak_sender.upgrade()
        {
          let _ = sender.send(EncoderCommand::Flush(response_sender));
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
        // Drain pending chunks and call output callback SYNCHRONOUSLY
        // This runs on the main thread with Env access
        let chunks = {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          guard.flushes.begin_output_delivery(flush_id);
          std::mem::take(&mut guard.pending_chunks)
        };

        // Call output callback for each chunk synchronously
        // If callback calls reset(), abort will be set before next iteration
        let callback_result = (|| -> Result<()> {
          let callback = output_callback_ref.borrow_back(env)?;
          for (chunk, metadata) in chunks {
            // Check abort flag before each callback - exit early if reset() was called
            if abort.is_aborted() {
              break;
            }
            callback.call((chunk, metadata).into())?;
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

        // Check abort flag after draining all chunks
        // close()/reset() abort the flush; an error that closed the codec fails it
        if abort.is_aborted() {
          let (name, message) = abort.rejection();
          return Err(native_dom_exception_error(env, name, message)?);
        }

        // Worker errors carry their DOMException name as a message prefix
        match result {
          Ok(()) => Ok(()),
          Err(error) => Err(Error::from(error_as_dom_exception(env, error)?)),
        }
      },
    )
  }

  /// Reset the encoder
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

    // Drop sender to signal worker to stop
    // Note: Don't join - microtasks might still hold cloned senders. After reset()
    // returns, microtasks run, see the cancelled token, skip sending, and drop senders.
    // Channel then disconnects and worker exits naturally.
    drop(self.command_sender.take());
    drop(self.worker_handle.take());

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    inner.flushes.abort_all();

    // Drop existing context
    inner.context = None;
    inner.resampler = None;
    inner.sample_buffer = None;
    inner.config = None;
    inner.active_config = None;
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    inner.extradata_sent = false;
    inner.cached_flac_decoder_config = None;
    inner.failure_name = None;
    let cleared_queue = inner.encode_queue_size as usize;
    inner.encode_queue_size = 0;
    CodecEventState::clear_queue(&self.event_state, &env, cleared_queue);
    inner.timestamp_queue.clear();
    inner.base_timestamp = None;
    // Clear flush-related state
    inner.flushes.clear();
    inner.pending_chunks.clear();

    // Serve the new generation. The detached pre-reset worker keeps its old,
    // now cancelled token, so it cannot resume against the new state.
    inner.generation = generation;
    self.cancel = self.epoch.token();

    // Create new channel and worker for future encode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(Arc::new(sender));
    let worker_inner = self.inner.clone();
    let worker_event_state = self.event_state.clone();
    let worker_cancel = self.cancel.clone();
    drop(inner); // Release lock before spawning thread
    self.worker_handle = Some(std::thread::spawn(move || {
      Self::worker_loop(worker_inner, worker_event_state, receiver, worker_cancel);
    }));

    Ok(())
  }

  /// Close the encoder
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

    // Drop sender to stop accepting new commands and close channel.
    // With Weak references in microtasks, dropping Arc<Sender> immediately closes the channel
    // even if there are pending microtasks (they use Weak which can't keep the channel alive).
    self.command_sender = None;

    // Now safe to join worker - channel is closed and the worker is cancelled, so it
    // exits after the operation in progress. This prevents resource contention where
    // old worker is still holding FFmpeg resources while new encoder is being created.
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
    match config.sample_rate {
      Some(sr) if sr > 0.0 => {}
      Some(_) => return reject_with_type_error(env, "sampleRate must be greater than 0"),
      None => return reject_with_type_error(env, "sampleRate is required"),
    }

    // Validate number of channels - must be present and greater than 0
    match config.number_of_channels {
      Some(nc) if nc > 0 => {}
      Some(_) => return reject_with_type_error(env, "numberOfChannels must be greater than 0"),
      None => return reject_with_type_error(env, "numberOfChannels is required"),
    }

    // Validate bitrate if specified
    if let Some(bitrate) = config.bitrate
      && bitrate <= 0.0
    {
      return reject_with_type_error(env, "bitrate must be greater than 0");
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

  // ============================================================================
  // EventTarget interface (W3C DOM spec)
  // ============================================================================

  /// Add an event listener for the specified event type
  /// Uses separate RwLock to avoid blocking on encode operations
  #[napi(
    ts_args_type = "eventType: string, callback: (event: Event) => unknown, options?: AudioEncoderAddEventListenerOptions | undefined | null"
  )]
  pub fn add_event_listener(
    &self,
    env: Env,
    this: This,
    event_type: String,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    options: Option<AudioEncoderAddEventListenerOptions>,
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
    ts_args_type = "eventType: string, callback: (event: Event) => unknown, options?: AudioEncoderEventListenerOptions | undefined | null"
  )]
  pub fn remove_event_listener(
    &self,
    env: Env,
    event_type: String,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    options: Option<AudioEncoderEventListenerOptions>,
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

/// Get AAC sample rate index for ADTS header
/// Returns the index corresponding to the sample rate in the ADTS header's
/// sampling_frequency_index field (4 bits, values 0-12)
fn get_aac_sample_rate_index(sample_rate: u32) -> u8 {
  match sample_rate {
    96000 => 0,
    88200 => 1,
    64000 => 2,
    48000 => 3,
    44100 => 4,
    32000 => 5,
    24000 => 6,
    22050 => 7,
    16000 => 8,
    12000 => 9,
    11025 => 10,
    8000 => 11,
    7350 => 12,
    _ => 15, // 15 means frequency is explicitly specified (not used in simple ADTS)
  }
}

/// Prepend FLAC stream header to raw STREAMINFO block from FFmpeg.
/// FFmpeg returns just the 34-byte STREAMINFO block, but the W3C spec expects
/// the full FLAC header including "fLaC" magic and metadata block header.
///
/// FLAC stream format:
/// - "fLaC" magic (4 bytes)
/// - METADATA_BLOCK_HEADER: last-block-flag(1) + type(7) + length(24) (4 bytes)
/// - METADATA_BLOCK_DATA (STREAMINFO = 34 bytes)
fn prepend_flac_header(streaminfo: &[u8]) -> Vec<u8> {
  // Only prepend if this looks like raw STREAMINFO (34 bytes, doesn't start with "fLaC")
  if streaminfo.len() != 34 || streaminfo.starts_with(b"fLaC") {
    return streaminfo.to_vec();
  }

  let mut result = Vec::with_capacity(4 + 4 + 34);
  // "fLaC" magic
  result.extend_from_slice(b"fLaC");
  // METADATA_BLOCK_HEADER: 0x80 = last-metadata-block flag + type 0 (STREAMINFO)
  // Length = 34 (0x000022)
  result.push(0x80); // last-metadata-block = 1, type = 0 (STREAMINFO)
  result.push(0x00);
  result.push(0x00);
  result.push(0x22); // length = 34
  // STREAMINFO data
  result.extend_from_slice(streaminfo);
  result
}
