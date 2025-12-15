//! VideoDecoder - WebCodecs API implementation
//!
//! Provides video decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videodecoder-interface

use crate::codec::{CodecContext, DecoderConfig, Frame, Packet};
use crate::ffi::{AVCodecID, AVHWDeviceType};
use crate::webcodecs::error::{
  DOMExceptionName, throw_data_error, throw_invalid_state_error, throw_type_error_unit,
};
use crate::webcodecs::promise_reject::{reject_with_dom_exception_async, reject_with_type_error};
use crate::webcodecs::video_frame::VideoColorSpaceInit;
use crate::webcodecs::{
  CodecState, EncodedVideoChunk, EncodedVideoChunkInner, HardwareAcceleration, VideoDecoderConfig,
  VideoFrame, convert_avcc_extradata_to_annexb, convert_avcc_to_annexb,
  convert_hvcc_extradata_to_annexb, is_avcc_extradata, is_avcc_format, is_hvcc_extradata,
};
use crossbeam::channel::{self, Receiver, Sender};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

/// Type alias for output callback (takes VideoFrame)
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback =
  ThreadsafeFunction<VideoFrame, UnknownReturnValue, VideoFrame, Status, false, true>;

/// Type alias for error callback (takes Error object)
/// Using CalleeHandled: false because WebCodecs error callback receives Error directly,
/// not error-first (err, result) style
type ErrorCallback = ThreadsafeFunction<Error, UnknownReturnValue, Error, Status, false, true>;

// Note: For ondequeue, we use FunctionRef instead of ThreadsafeFunction
// to support both getter and setter per WebCodecs spec

/// Type alias for event listener callback (weak to allow Node.js process to exit)
type EventListenerCallback = ThreadsafeFunction<(), UnknownReturnValue, (), Status, false, true>;

/// Entry for tracking event listeners
/// Stores both the weak ThreadsafeFunction and a FunctionRef to prevent GC from collecting the callback
struct EventListenerEntry {
  id: u64,
  callback: Arc<EventListenerCallback>,
  once: bool,
  /// Prevents GC from collecting the JS callback while listener is registered
  /// The weak TSF alone doesn't prevent GC, so we need this to keep the function alive
  _prevent_gc: FunctionRef<(), UnknownReturnValue>,
}

/// State for EventTarget interface, separate from main decoder state
/// to avoid lock contention during decoding operations.
/// Uses RwLock so addEventListener doesn't block on decode operations.
#[derive(Default)]
struct EventListenerState {
  /// Event listeners registry (event type -> list of listeners)
  event_listeners: HashMap<String, Vec<EventListenerEntry>>,
  /// Counter for generating unique listener IDs
  next_listener_id: u64,
  /// Optional dequeue event callback (set via ondequeue property)
  dequeue_callback: Option<Arc<EventListenerCallback>>,
}

/// Options for addEventListener (W3C DOM spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoDecoderAddEventListenerOptions {
  pub capture: Option<bool>,
  pub once: Option<bool>,
  pub passive: Option<bool>,
}

/// Options for removeEventListener (W3C DOM spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoDecoderEventListenerOptions {
  pub capture: Option<bool>,
}

/// Commands sent to the worker thread
enum WorkerCommand {
  /// Decode a video chunk
  Decode(Arc<RwLock<Option<EncodedVideoChunkInner>>>),
  /// Flush the decoder and send result back via response channel
  Flush(Sender<Result<()>>),
  /// Reconfigure the decoder with new config (W3C spec: control message)
  Reconfigure(VideoDecoderConfig),
}

/// VideoDecoder init dictionary per WebCodecs spec
pub struct VideoDecoderInit {
  /// Output callback - called when decoded frame is available (ThreadsafeFunction for worker)
  pub output: OutputCallback,
  /// Output callback reference - stored for synchronous calls from main thread
  pub output_ref: FunctionRef<VideoFrame, UnknownReturnValue>,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for VideoDecoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // W3C spec: throw TypeError if required callbacks are missing
    // Get output callback as Function first, then create both FunctionRef and ThreadsafeFunction
    let output_func: Function<VideoFrame, UnknownReturnValue> =
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

    Ok(VideoDecoderInit {
      output,
      output_ref,
      error,
    })
  }
}

/// Result of isConfigSupported per WebCodecs spec
#[napi(object)]
pub struct VideoDecoderSupport {
  /// Whether the configuration is supported
  pub supported: bool,
  /// The configuration that was checked
  pub config: VideoDecoderConfig,
}

/// Threshold for detecting silent decoder failure (no output after N chunks)
const SILENT_FAILURE_THRESHOLD: u32 = 3;

/// Internal decoder state
struct VideoDecoderInner {
  state: CodecState,
  config: Option<DecoderConfig>,
  context: Option<CodecContext>,
  codec_string: String,
  frame_count: u64,
  /// Number of pending decode operations (for decodeQueueSize)
  decode_queue_size: u32,
  /// Output callback (required per spec) - used by worker thread for error cases
  output_callback: OutputCallback,
  /// Error callback (required per spec)
  error_callback: ErrorCallback,
  /// Whether a keyframe has been received (for delta frame validation)
  keyframe_received: bool,
  /// Whether an error has occurred during decoding (for flush error propagation)
  had_error: bool,
  /// Pending flush response senders (for AbortError on reset)
  pending_flush_senders: Vec<crossbeam::channel::Sender<Result<()>>>,
  /// Queue of input timestamps for correlation with output frames
  /// (needed because FFmpeg may buffer frames internally and modify PTS)
  timestamp_queue: std::collections::VecDeque<(i64, Option<i64>)>,
  /// Atomic flag for flush abort - set by reset() to signal pending flush to abort
  flush_abort_flag: Option<Arc<AtomicBool>>,
  /// Queue of decoded frames waiting to be delivered via output callback
  /// Worker pushes frames here during flush; flush() drains them synchronously via FunctionRef
  pending_frames: Vec<VideoFrame>,
  /// Flag indicating whether a flush operation is in progress
  /// When true, worker queues frames to pending_frames instead of calling NonBlocking callback
  inside_flush: bool,

  // ========================================================================
  // Hardware acceleration tracking (for Chromium-aligned fallback behavior)
  // ========================================================================
  /// Whether the decoder is using hardware acceleration
  is_hardware: bool,
  /// Hardware acceleration preference from config
  hw_preference: HardwareAcceleration,
  /// Count of consecutive decodes with no output (for silent failure detection)
  silent_decode_count: u32,
  /// Whether first output has been produced (disables silent failure detection after)
  first_output_produced: bool,
  /// Buffered chunks during silent failure detection period (for re-decoding on fallback)
  pending_chunks: Vec<Arc<RwLock<Option<EncodedVideoChunkInner>>>>,

  // ========================================================================
  // Orientation metadata (W3C WebCodecs VideoFrame orientation)
  // ========================================================================
  /// Rotation in degrees from config (0, 90, 180, 270)
  config_rotation: f64,
  /// Horizontal flip from config
  config_flip: bool,

  // ========================================================================
  // Color space metadata (W3C WebCodecs VideoFrame colorSpace)
  // ========================================================================
  /// Color space from decoder config - applied to decoded frames
  config_color_space: Option<VideoColorSpaceInit>,
}

/// Get the preferred hardware device type for the current platform
fn get_platform_hw_type() -> AVHWDeviceType {
  #[cfg(target_os = "macos")]
  {
    AVHWDeviceType::Videotoolbox
  }
  #[cfg(target_os = "linux")]
  {
    AVHWDeviceType::Vaapi
  }
  #[cfg(target_os = "windows")]
  {
    AVHWDeviceType::D3d11va
  }
  #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
  {
    AVHWDeviceType::Cuda
  }
}

/// VideoDecoder - WebCodecs-compliant video decoder
///
/// Decodes EncodedVideoChunk objects into VideoFrame objects using FFmpeg.
///
/// Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
///
/// Example:
/// ```javascript
/// const decoder = new VideoDecoder({
///   output: (frame) => { console.log('decoded frame', frame); },
///   error: (e) => { console.error('error', e); }
/// });
///
/// decoder.configure({
///   codec: 'avc1.42001E'
/// });
///
/// decoder.decode(chunk);
/// await decoder.flush();
/// ```
#[napi]
pub struct VideoDecoder {
  inner: Arc<Mutex<VideoDecoderInner>>,
  /// Separate lock for EventTarget state to avoid lock contention with decode operations.
  /// This allows addEventListener to complete immediately even when worker holds inner lock.
  event_state: Arc<RwLock<EventListenerState>>,
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
  /// Output callback reference - stored for synchronous calls from main thread (in flush resolver)
  /// Wrapped in Rc to allow sharing with spawn_future_with_callback closure
  /// (Rc is !Send but that's OK - the callback runs on the main thread)
  output_callback_ref: Rc<FunctionRef<VideoFrame, UnknownReturnValue>>,
  /// Channel sender for worker commands
  command_sender: Option<Sender<WorkerCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
  /// Reset abort flag - set by reset() to signal worker to skip pending decodes
  reset_flag: Arc<AtomicBool>,
}

impl Drop for VideoDecoder {
  fn drop(&mut self) {
    // Signal worker to stop
    self.command_sender = None;

    // Wait for worker to finish (brief block, necessary for safety)
    if let Some(handle) = self.worker_handle.take() {
      let _ = handle.join();
    }

    // Drain decoder to ensure libaom/AV1 threads finish before context drops.
    // This prevents SIGSEGV when avcodec_free_context is called while libaom
    // still has internal threads running.
    if let Ok(mut inner) = self.inner.lock()
      && let Some(ctx) = inner.context.as_mut()
    {
      // Flush internal buffers first - this synchronizes libaom's thread pool
      ctx.flush();
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
    }
  }
}

#[napi]
impl VideoDecoder {
  /// Create a new VideoDecoder with init dictionary (per WebCodecs spec)
  ///
  /// @param init - Init dictionary containing output and error callbacks
  #[napi(constructor)]
  pub fn new(
    #[napi(ts_arg_type = "{ output: (frame: VideoFrame) => void, error: (error: Error) => void }")]
    init: VideoDecoderInit,
  ) -> Result<Self> {
    let inner = VideoDecoderInner {
      state: CodecState::Unconfigured,
      config: None,
      context: None,
      codec_string: String::new(),
      frame_count: 0,
      decode_queue_size: 0,
      output_callback: init.output,
      error_callback: init.error,
      keyframe_received: false,
      had_error: false,
      pending_flush_senders: Vec::new(),
      timestamp_queue: std::collections::VecDeque::new(),
      flush_abort_flag: None,
      pending_frames: Vec::new(),
      inside_flush: false,
      // Hardware acceleration tracking (Chromium-aligned)
      is_hardware: false,
      hw_preference: HardwareAcceleration::NoPreference,
      silent_decode_count: 0,
      first_output_produced: false,
      pending_chunks: Vec::new(),
      // Orientation metadata (default: no rotation/flip)
      config_rotation: 0.0,
      config_flip: false,
      // Color space from config (None = extract from FFmpeg frame)
      config_color_space: None,
    };

    let inner = Arc::new(Mutex::new(inner));

    // Create separate lock for event listener state (avoids contention with decode operations)
    let event_state = Arc::new(RwLock::new(EventListenerState::default()));

    // Create channel for worker commands
    let (sender, receiver) = channel::unbounded();

    // Create reset abort flag
    let reset_flag = Arc::new(AtomicBool::new(false));

    // Spawn worker thread
    let worker_inner = inner.clone();
    let worker_event_state = event_state.clone();
    let worker_reset_flag = reset_flag.clone();
    let worker_handle = std::thread::spawn(move || {
      Self::worker_loop(
        worker_inner,
        worker_event_state,
        receiver,
        worker_reset_flag,
      );
    });

    Ok(Self {
      inner,
      event_state,
      dequeue_callback: None,
      output_callback_ref: Rc::new(init.output_ref),
      command_sender: Some(sender),
      worker_handle: Some(worker_handle),
      reset_flag,
    })
  }

  /// Worker loop that processes commands from the channel
  fn worker_loop(
    inner: Arc<Mutex<VideoDecoderInner>>,
    event_state: Arc<RwLock<EventListenerState>>,
    receiver: Receiver<WorkerCommand>,
    reset_flag: Arc<AtomicBool>,
  ) {
    while let Ok(command) = receiver.recv() {
      // Check reset flag before processing each command
      // If reset() was called, skip remaining decode commands
      if reset_flag.load(Ordering::SeqCst) {
        // Still process flush commands to send responses, but skip decodes
        if let WorkerCommand::Flush(response_sender) = command {
          let _ = response_sender.send(Err(Error::new(
            Status::GenericFailure,
            "AbortError: The operation was aborted",
          )));
        } else {
          // For decode commands, just decrement queue and fire dequeue
          if let Ok(mut guard) = inner.lock() {
            let old_size = guard.decode_queue_size;
            guard.decode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(&event_state);
            }
          }
        }
        continue;
      }

      match command {
        WorkerCommand::Decode(chunk) => {
          Self::process_decode(&inner, &event_state, chunk);
        }
        WorkerCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner, &event_state);
          let _ = response_sender.send(result);
        }
        WorkerCommand::Reconfigure(config) => {
          Self::process_reconfigure(&inner, config);
        }
      }
    }
  }

  /// Process a decode command
  ///
  /// Implements Chromium-aligned silent failure detection:
  /// - If hardware decoder produces no output after SILENT_FAILURE_THRESHOLD chunks,
  ///   either report error (prefer-hardware) or fall back to software (no-preference)
  fn process_decode(
    inner: &Arc<Mutex<VideoDecoderInner>>,
    event_state: &Arc<RwLock<EventListenerState>>,
    chunk: Arc<RwLock<Option<EncodedVideoChunkInner>>>,
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Check if decoder is still configured
    if guard.state != CodecState::Configured {
      let old_size = guard.decode_queue_size;
      guard.decode_queue_size = old_size.saturating_sub(1);
      if old_size > 0 {
        let _ = Self::fire_dequeue_event(event_state);
      }
      // Per W3C spec: "cease producing output" - silently discard pending work
      // State could be Unconfigured (reset called) or Closed (close called)
      // Don't call report_error() - that would set state to Closed and invoke error callback
      return;
    }

    // Get chunk data
    let chunk_read_guard = chunk.read();
    let encoded_chunk = match chunk_read_guard
      .as_ref()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))
      .and_then(|d| {
        d.as_ref()
          .ok_or_else(|| Error::new(Status::GenericFailure, "Chunk is closed"))
      }) {
      Ok(c) => c,
      Err(e) => {
        let old_size = guard.decode_queue_size;
        guard.decode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(event_state);
        }
        Self::report_error(&mut guard, &e.reason);
        return;
      }
    };

    let timestamp = encoded_chunk.timestamp_us;
    let duration = encoded_chunk.duration_us;
    let raw_data = encoded_chunk.data.clone();
    let is_keyframe = encoded_chunk.chunk_type == crate::webcodecs::EncodedVideoChunkType::Key;

    // Drop the chunk read guard before decoding
    drop(chunk_read_guard);

    // Convert AVCC/HVCC format to Annex B if needed for H.264/H.265
    // FFmpeg's decoder expects Annex B format (start code prefixed NALUs)
    // Also prepend SPS/PPS from extradata to keyframes (FFmpeg may not use extradata properly)
    let data = {
      let codec = &guard.codec_string;
      let is_avc_codec = codec.starts_with("avc1")
        || codec.starts_with("avc3")
        || codec.starts_with("hvc1")
        || codec.starts_with("hev1");

      if is_avc_codec && is_avcc_format(&raw_data) {
        let mut converted = convert_avcc_to_annexb(&raw_data);

        // Prepend SPS/PPS/VPS from extradata to keyframes
        // This is needed because FFmpeg may not properly use extradata for H.264/H.265
        if is_keyframe
          && let Some(config) = guard.config.as_ref()
          && let Some(extradata) = &config.extradata
        {
          // Extradata should already be in Annex B format (converted in configure)
          // Prepend it to the keyframe data
          let mut with_extradata = extradata.clone();
          with_extradata.append(&mut converted);
          converted = with_extradata;
        }
        converted
      } else {
        raw_data
      }
    };

    // Push timestamp to queue for correlation with output frames
    // (FFmpeg may buffer frames internally and modify PTS)
    guard.timestamp_queue.push_back((timestamp, duration));

    // Buffer chunk during silent failure detection period (for re-decoding on fallback)
    if guard.is_hardware && !guard.first_output_produced {
      guard.pending_chunks.push(chunk.clone());
    }

    // Get context
    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        let old_size = guard.decode_queue_size;
        guard.decode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(event_state);
        }
        Self::report_error(&mut guard, "No decoder context");
        return;
      }
    };

    // Decode
    let frames = match decode_chunk_data(context, &data, timestamp, duration) {
      Ok(f) => f,
      Err(e) => {
        // Handle decode error - may trigger fallback for hardware decoder
        if guard.is_hardware && !guard.first_output_produced {
          match &guard.hw_preference {
            HardwareAcceleration::PreferHardware => {
              // prefer-hardware: Report error, don't fall back
              let old_size = guard.decode_queue_size;
              guard.decode_queue_size = old_size.saturating_sub(1);
              if old_size > 0 {
                let _ = Self::fire_dequeue_event(event_state);
              }
              Self::report_error(
                &mut guard,
                &format!("OperationError: Hardware decoding failed: {}", e),
              );
              return;
            }
            HardwareAcceleration::NoPreference => {
              // no-preference: Try to fall back to software
              let pending = std::mem::take(&mut guard.pending_chunks);
              if Self::fallback_to_software(&mut guard).is_ok() {
                // Re-decode buffered chunks with software decoder
                let old_size = guard.decode_queue_size;
                guard.decode_queue_size = old_size.saturating_sub(1);
                if old_size > 0 {
                  let _ = Self::fire_dequeue_event(event_state);
                }
                drop(guard);
                Self::redecode_pending_chunks(inner, pending);
                return;
              }
              // Fallback failed, report original error
            }
            _ => {}
          }
        }
        let old_size = guard.decode_queue_size;
        guard.decode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(event_state);
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
      let _ = Self::fire_dequeue_event(event_state);
    }

    // Check for silent failure (hardware decoder, no frames produced)
    if frames.is_empty() {
      if guard.is_hardware && !guard.first_output_produced {
        guard.silent_decode_count += 1;

        if guard.silent_decode_count >= SILENT_FAILURE_THRESHOLD {
          // Silent failure detected - hardware decoder not producing output
          match &guard.hw_preference {
            HardwareAcceleration::PreferHardware => {
              // prefer-hardware: Report error, don't fall back
              Self::report_error(
                &mut guard,
                "OperationError: Hardware decoder not producing output (silent failure)",
              );
              return;
            }
            HardwareAcceleration::NoPreference => {
              // no-preference: Silently fall back to software and re-decode buffered chunks
              let pending = std::mem::take(&mut guard.pending_chunks);
              if Self::fallback_to_software(&mut guard).is_ok() {
                // Re-decode all buffered chunks with software decoder
                drop(guard);
                Self::redecode_pending_chunks(inner, pending);
                return;
              }
              // Fallback failed - continue with hardware (may never produce output)
            }
            _ => {}
          }
        }
      }
      // No frames this decode - normal for B-frames, etc.
      return;
    }

    // Successfully produced output
    if guard.is_hardware && !guard.first_output_produced {
      guard.first_output_produced = true;
      guard.silent_decode_count = 0;
      guard.pending_chunks.clear(); // No longer need the buffer
    }

    // Convert internal frames to VideoFrames and deliver
    for frame in frames {
      // Pop timestamp from queue to preserve original input timestamp
      // (FFmpeg may modify PTS internally during decoding)
      let (output_timestamp, output_duration) = guard
        .timestamp_queue
        .pop_front()
        .unwrap_or((timestamp, duration));
      let video_frame = VideoFrame::from_internal_with_orientation(
        frame,
        output_timestamp,
        output_duration,
        guard.config_rotation,
        guard.config_flip,
        guard.config_color_space.as_ref(),
      );

      // During flush, queue frames for synchronous delivery in resolver
      // Otherwise, use NonBlocking callback for immediate delivery
      if guard.inside_flush {
        guard.pending_frames.push(video_frame);
      } else {
        guard
          .output_callback
          .call(video_frame, ThreadsafeFunctionCallMode::Blocking);
      }
    }
  }

  /// Fall back to software decoder (for no-preference mode)
  fn fallback_to_software(inner: &mut VideoDecoderInner) -> Result<()> {
    // Get the codec ID from existing config
    let decoder_config = inner
      .config
      .as_ref()
      .ok_or_else(|| Error::new(Status::GenericFailure, "No decoder config"))?
      .clone();

    // Drain existing decoder before dropping (AV1 safety)
    if let Some(ctx) = inner.context.as_mut() {
      ctx.flush();
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
    }

    // Create software decoder
    let mut context = CodecContext::new_decoder(decoder_config.codec_id).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create software decoder: {}", e),
      )
    })?;

    // Configure decoder with same settings
    context.configure_decoder(&decoder_config).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to configure software decoder: {}", e),
      )
    })?;

    // Open the decoder
    context.open().map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to open software decoder: {}", e),
      )
    })?;

    // Replace context and update state
    inner.context = Some(context);
    inner.is_hardware = false;
    inner.silent_decode_count = 0;
    inner.first_output_produced = false;

    Ok(())
  }

  /// Re-decode buffered chunks after fallback to software
  fn redecode_pending_chunks(
    inner: &Arc<Mutex<VideoDecoderInner>>,
    chunks: Vec<Arc<RwLock<Option<EncodedVideoChunkInner>>>>,
  ) {
    for chunk in chunks {
      let mut guard = match inner.lock() {
        Ok(g) => g,
        Err(_) => return,
      };

      // Check state
      if guard.state != CodecState::Configured {
        return;
      }

      // Get chunk data
      let chunk_read_guard = chunk.read();
      let (timestamp, duration, raw_data) = match chunk_read_guard
        .as_ref()
        .ok()
        .and_then(|d| d.as_ref())
        .map(|c| (c.timestamp_us, c.duration_us, c.data.clone()))
      {
        Some(d) => d,
        None => continue, // Skip closed chunks
      };
      drop(chunk_read_guard);

      // Convert AVCC/HVCC format to Annex B if needed for H.264/H.265
      let data = {
        let codec = &guard.codec_string;
        let is_avc_codec = codec.starts_with("avc1")
          || codec.starts_with("avc3")
          || codec.starts_with("hvc1")
          || codec.starts_with("hev1");

        if is_avc_codec && is_avcc_format(&raw_data) {
          convert_avcc_to_annexb(&raw_data)
        } else {
          raw_data
        }
      };

      // Decode with software decoder
      let context = match guard.context.as_mut() {
        Some(ctx) => ctx,
        None => return,
      };

      let frames = match decode_chunk_data(context, &data, timestamp, duration) {
        Ok(f) => f,
        Err(_) => continue, // Skip failed chunks during re-decode
      };

      // Mark first output produced on success
      if !frames.is_empty() && !guard.first_output_produced {
        guard.first_output_produced = true;
      }

      // Deliver frames (queue during flush, NonBlocking otherwise)
      for frame in frames {
        let video_frame = VideoFrame::from_internal_with_orientation(
          frame,
          timestamp,
          duration,
          guard.config_rotation,
          guard.config_flip,
          guard.config_color_space.as_ref(),
        );
        if guard.inside_flush {
          guard.pending_frames.push(video_frame);
        } else {
          guard
            .output_callback
            .call(video_frame, ThreadsafeFunctionCallMode::Blocking);
        }
      }
    }
  }

  /// Process a flush command
  fn process_flush(
    inner: &Arc<Mutex<VideoDecoderInner>>,
    _event_state: &Arc<RwLock<EventListenerState>>,
  ) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: If an error occurred during decoding, flush should reject FIRST
    // Check this before state check to return EncodingError instead of InvalidStateError
    if guard.had_error {
      return Err(Error::new(
        Status::GenericFailure,
        "EncodingError: Decode error occurred",
      ));
    }

    // W3C spec: flush() should reject if decoder is not in configured state
    if guard.state != CodecState::Configured {
      return Err(Error::new(
        Status::GenericFailure,
        "InvalidStateError: Decoder is not configured",
      ));
    }

    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut guard, "No decoder context");
        return Err(Error::new(
          Status::GenericFailure,
          "EncodingError: No decoder context",
        ));
      }
    };

    // Flush decoder
    let frames = match context.flush_decoder() {
      Ok(f) => f,
      Err(e) => {
        let msg = format!("Flush failed: {}", e);
        Self::report_error(&mut guard, &msg);
        return Err(Error::new(
          Status::GenericFailure,
          format!("EncodingError: {}", msg),
        ));
      }
    };

    // Queue remaining frames for delivery (always queue during flush for synchronous delivery)
    for frame in frames {
      // Pop timestamp from queue to preserve original input timestamp
      // (FFmpeg may modify PTS internally during decoding)
      let (output_timestamp, output_duration) =
        guard.timestamp_queue.pop_front().unwrap_or_else(|| {
          // Fallback to FFmpeg's PTS if queue is empty
          let pts = frame.pts();
          let dur = if frame.duration() > 0 {
            Some(frame.duration())
          } else {
            None
          };
          (pts, dur)
        });
      let video_frame = VideoFrame::from_internal_with_orientation(
        frame,
        output_timestamp,
        output_duration,
        guard.config_rotation,
        guard.config_flip,
        guard.config_color_space.as_ref(),
      );
      // Always queue during flush for synchronous delivery in resolver
      guard.pending_frames.push(video_frame);
    }

    // Clear any remaining timestamps in queue after flush
    guard.timestamp_queue.clear();

    // Reset decoder state so it can accept more data (per W3C spec, flush should leave
    // decoder in configured state, ready for more decode() calls)
    if let Some(ref mut context) = guard.context {
      context.flush();
    }

    Ok(())
  }

  /// Process a reconfigure command on the worker thread
  /// Drains old context and creates new one with updated config
  fn process_reconfigure(inner: &Arc<Mutex<VideoDecoderInner>>, config: VideoDecoderConfig) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Don't reconfigure if decoder is closed
    if guard.state == CodecState::Closed {
      return;
    }

    // Drain old context (AV1/libaom thread safety)
    if let Some(ctx) = guard.context.as_mut() {
      ctx.flush();
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
    }

    // Clear work-related state
    guard.decode_queue_size = 0;
    guard.timestamp_queue.clear();
    guard.keyframe_received = false;
    guard.silent_decode_count = 0;
    guard.first_output_produced = false;

    // Parse codec to get codec_id
    let codec = match config.codec.as_ref() {
      Some(c) => c.clone(),
      None => {
        Self::report_error(&mut guard, "NotSupportedError: codec is required");
        return;
      }
    };

    let codec_id = match parse_codec_string(&codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_error(
          &mut guard,
          &format!("NotSupportedError: Invalid codec: {}", e),
        );
        return;
      }
    };

    // Determine hardware type based on preference
    // For decoding, only use hardware for PreferHardware (software is more reliable)
    let hw_preference = config
      .hardware_acceleration
      .unwrap_or(HardwareAcceleration::NoPreference);

    let hw_type = match &hw_preference {
      HardwareAcceleration::PreferHardware => Some(get_platform_hw_type()),
      HardwareAcceleration::NoPreference | HardwareAcceleration::PreferSoftware => None,
    };

    // Create decoder context
    let (mut context, is_hardware) = if let Some(hw) = hw_type {
      match CodecContext::new_decoder_with_hw_info(codec_id, Some(hw)) {
        Ok(result) => (result.context, result.is_hardware),
        Err(e) => {
          Self::report_error(
            &mut guard,
            &format!("NotSupportedError: Failed to create decoder: {}", e),
          );
          return;
        }
      }
    } else {
      match CodecContext::new_decoder(codec_id) {
        Ok(ctx) => (ctx, false),
        Err(e) => {
          Self::report_error(
            &mut guard,
            &format!("NotSupportedError: Failed to create decoder: {}", e),
          );
          return;
        }
      }
    };

    // Convert avcC/hvcC extradata to Annex B if needed
    let extradata = config.description.as_ref().and_then(|d| {
      let data = d.to_vec();
      let is_h264 = codec.starts_with("avc1") || codec.starts_with("avc3");
      let is_h265 = codec.starts_with("hvc1") || codec.starts_with("hev1");

      if is_h264 && is_avcc_extradata(&data) {
        convert_avcc_extradata_to_annexb(&data).or(Some(data))
      } else if is_h265 && is_hvcc_extradata(&data) {
        convert_hvcc_extradata_to_annexb(&data).or(Some(data))
      } else {
        Some(data)
      }
    });

    // Configure decoder
    let decoder_config = DecoderConfig {
      codec_id,
      thread_count: 0,
      extradata,
      low_latency: config.optimize_for_latency.unwrap_or(false),
    };

    if let Err(e) = context.configure_decoder(&decoder_config) {
      Self::report_error(
        &mut guard,
        &format!("NotSupportedError: Failed to configure decoder: {}", e),
      );
      return;
    }

    if let Err(e) = context.open() {
      Self::report_error(
        &mut guard,
        &format!("NotSupportedError: Failed to open decoder: {}", e),
      );
      return;
    }

    // Update inner state
    guard.context = Some(context);
    guard.config = Some(decoder_config);
    guard.codec_string = codec;
    guard.is_hardware = is_hardware;
    guard.hw_preference = hw_preference;

    // Store orientation from config
    guard.config_rotation = config.rotation.unwrap_or(0.0);
    guard.config_flip = config.flip.unwrap_or(false);

    // Store colorSpace from config
    guard.config_color_space = config.color_space;
  }

  /// Report an error via callback and close the decoder
  fn report_error(inner: &mut VideoDecoderInner, error_msg: &str) {
    // Create an Error object that will be passed directly to the JS callback
    let error = Error::new(Status::GenericFailure, error_msg);
    inner
      .error_callback
      .call(error, ThreadsafeFunctionCallMode::NonBlocking);
    inner.had_error = true;
    inner.state = CodecState::Closed;
  }

  /// Fire dequeue event - uses separate RwLock to avoid blocking addEventListener
  /// Also dispatches to EventTarget listeners registered via addEventListener
  fn fire_dequeue_event(event_state: &Arc<RwLock<EventListenerState>>) -> Result<()> {
    // Use write lock to fire callbacks and remove once listeners atomically
    // NonBlocking mode ensures callbacks are queued without blocking the worker thread
    let mut state = match event_state.write() {
      Ok(s) => s,
      Err(_) => return Err(Error::new(Status::GenericFailure, "Lock poisoned")),
    };

    // 1. Fire ondequeue callback
    if let Some(ref callback) = state.dequeue_callback {
      callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
    }

    // 2. Fire EventTarget listeners
    // For "once" listeners, use call_with_return_value to ensure _prevent_gc cleanup
    // For regular listeners, use simple call()
    if let Some(listeners) = state.event_listeners.get_mut("dequeue") {
      // Partition into once and regular listeners
      let (once_listeners, regular_listeners): (Vec<_>, Vec<_>) =
        std::mem::take(listeners).into_iter().partition(|e| e.once);

      // Fire regular listeners with simple call()
      for entry in &regular_listeners {
        entry
          .callback
          .call((), ThreadsafeFunctionCallMode::NonBlocking);
      }

      // Fire "once" listeners with call_with_return_value for proper _prevent_gc cleanup
      // The cleanup closure drops _prevent_gc only after the JS callback has executed
      // IMPORTANT: Clone the Arc to keep TSF alive until callback executes (NonBlocking mode
      // queues the callback but returns immediately - without this clone, the Arc drops at loop
      // end and the weak TSF may abort the pending callback)
      for entry in once_listeners {
        let prevent_gc = entry._prevent_gc;
        let callback_clone = entry.callback.clone();
        entry.callback.call_with_return_value(
          (),
          ThreadsafeFunctionCallMode::NonBlocking,
          move |_: Result<UnknownReturnValue>, _env: Env| {
            // Drop callback_clone and prevent_gc after the callback has executed
            drop(callback_clone);
            drop(prevent_gc);
            Ok(())
          },
        );
      }

      // Put back regular listeners (once listeners are already consumed/removed)
      *listeners = regular_listeners;
      if listeners.is_empty() {
        state.event_listeners.remove("dequeue");
      }
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
    // Update event_state with ThreadsafeFunction for worker thread
    let mut state = self
      .event_state
      .write()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    state.dequeue_callback = match callback {
      Some(ref cb) => Some(Arc::new(
        cb.borrow_back(env)?
          .build_threadsafe_function()
          .callee_handled::<false>()
          .weak::<true>() // Weak to allow Node.js process to exit
          .build()?,
      )),
      None => None,
    };
    drop(state); // Release lock before storing FunctionRef

    // Store FunctionRef for getter (main thread only)
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
  ///
  /// Implements Chromium-aligned hardware acceleration behavior:
  /// - `prefer-hardware`: Try hardware only, report error if fails
  /// - `no-preference`: Try hardware first, silently fall back to software
  /// - `prefer-software`: Use software only
  #[napi]
  pub fn configure(&mut self, env: Env, config: VideoDecoderConfig) -> Result<()> {
    // W3C WebCodecs spec: Validate config synchronously, throw TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-videodecoder-configure

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      _ => return throw_type_error_unit(&env, "codec is required"),
    };

    // Validate coded dimensions if specified
    if let Some(w) = config.coded_width
      && w == 0
    {
      return throw_type_error_unit(&env, "codedWidth must be greater than 0");
    }
    if let Some(h) = config.coded_height
      && h == 0
    {
      return throw_type_error_unit(&env, "codedHeight must be greater than 0");
    }

    // Validate display aspect dimensions if specified
    if let Some(dw) = config.display_aspect_width
      && dw == 0
    {
      return throw_type_error_unit(&env, "displayAspectWidth must be greater than 0");
    }
    if let Some(dh) = config.display_aspect_height
      && dh == 0
    {
      return throw_type_error_unit(&env, "displayAspectHeight must be greater than 0");
    }

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: throw InvalidStateError if closed
    if inner.state == CodecState::Closed {
      return throw_invalid_state_error(&env, "Decoder is closed");
    }

    // W3C spec: If already configured, queue reconfigure via microtask
    // This ensures FIFO ordering with pending decode commands
    if inner.state == CodecState::Configured {
      // Validate codec synchronously before queueing
      if parse_codec_string(&codec).is_err() {
        Self::report_error(
          &mut inner,
          &format!("NotSupportedError: Invalid codec: {}", codec),
        );
        return Ok(());
      }

      // Queue reconfigure via microtask (runs AFTER pending decode microtasks)
      // Don't update inner.config here - worker will do it after processing pending decodes
      drop(inner); // Release lock before scheduling microtask
      if let Some(ref sender) = self.command_sender {
        let sender = sender.clone();
        PromiseRaw::resolve(&env, ())?.then(move |_| {
          let _ = sender.send(WorkerCommand::Reconfigure(config));
          Ok(())
        })?;
      }
      return Ok(());
    }

    // First-time configure: create context and worker synchronously
    // Parse codec string to determine codec ID
    let codec_id = match parse_codec_string(&codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_error(
          &mut inner,
          &format!("NotSupportedError: Invalid codec: {}", e),
        );
        return Ok(());
      }
    };

    // Parse hardware preference (default to no-preference per spec)
    let hw_preference = config
      .hardware_acceleration
      .unwrap_or(HardwareAcceleration::NoPreference);

    // Determine hardware type based on preference and global state
    //
    // NOTE: Unlike encoding, hardware DECODING via FFmpeg often produces incorrect
    // output (null format, garbage data) on many systems. Therefore, we only use
    // hardware decoding when explicitly requested via prefer-hardware.
    //
    // Behavior:
    // - prefer-hardware: Try hardware only (may produce errors if HW unavailable)
    // - no-preference: Use software (safest default)
    // - prefer-software: Use software
    let hw_type = match &hw_preference {
      HardwareAcceleration::PreferHardware => Some(get_platform_hw_type()),
      // For no-preference and prefer-software, use software decoding
      // Hardware decoding via FFmpeg often produces incorrect output
      HardwareAcceleration::NoPreference | HardwareAcceleration::PreferSoftware => None,
    };

    // Create decoder context with optional hardware acceleration
    let (mut context, is_hardware) = if let Some(hw) = hw_type {
      // Hardware decoder requested (prefer-hardware only)
      match CodecContext::new_decoder_with_hw_info(codec_id, Some(hw)) {
        Ok(result) => (result.context, result.is_hardware),
        Err(e) => {
          // Hardware decoder creation failed - report error (no fallback for prefer-hardware)
          Self::report_error(
            &mut inner,
            &format!("OperationError: Hardware decoder creation failed: {}", e),
          );
          return Ok(());
        }
      }
    } else {
      // Software decoder (no-preference or prefer-software)
      match CodecContext::new_decoder(codec_id) {
        Ok(ctx) => (ctx, false),
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to create decoder: {}", e));
          return Ok(());
        }
      }
    };

    // Convert avcC/hvcC extradata to Annex B if needed for H.264/H.265
    // FFmpeg's decoder expects Annex B format (SPS/PPS/VPS with start codes)
    let extradata = config.description.as_ref().and_then(|d| {
      let data = d.to_vec();
      let is_h264 = codec.starts_with("avc1") || codec.starts_with("avc3");
      let is_h265 = codec.starts_with("hvc1") || codec.starts_with("hev1");

      if is_h264 && is_avcc_extradata(&data) {
        convert_avcc_extradata_to_annexb(&data).or(Some(data))
      } else if is_h265 && is_hvcc_extradata(&data) {
        convert_hvcc_extradata_to_annexb(&data).or(Some(data))
      } else {
        Some(data)
      }
    });

    // Configure decoder
    let decoder_config = DecoderConfig {
      codec_id,
      thread_count: 0, // Auto
      extradata,
      low_latency: config.optimize_for_latency.unwrap_or(false),
    };

    if let Err(e) = context.configure_decoder(&decoder_config) {
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
    inner.keyframe_received = false;

    // Store hardware acceleration tracking state
    inner.is_hardware = is_hardware;
    inner.hw_preference = hw_preference;
    inner.silent_decode_count = 0;
    inner.first_output_produced = false;
    inner.pending_chunks.clear();

    // Store orientation metadata from config (W3C WebCodecs spec)
    inner.config_rotation = config.rotation.unwrap_or(0.0);
    inner.config_flip = config.flip.unwrap_or(false);

    // Store colorSpace from config (W3C WebCodecs spec)
    // If provided, this colorSpace will be applied to all decoded frames
    inner.config_color_space = config.color_space;

    // Create new channel and worker if needed (after reconfiguration)
    if self.command_sender.is_none() {
      let (sender, receiver) = channel::unbounded();
      self.command_sender = Some(sender);
      let worker_inner = self.inner.clone();
      let worker_event_state = self.event_state.clone();
      let worker_reset_flag = self.reset_flag.clone();
      drop(inner); // Release lock before spawning thread
      self.worker_handle = Some(std::thread::spawn(move || {
        Self::worker_loop(
          worker_inner,
          worker_event_state,
          receiver,
          worker_reset_flag,
        );
      }));
    }

    Ok(())
  }

  /// Decode an encoded video chunk
  #[napi]
  pub fn decode(&self, env: Env, chunk: &EncodedVideoChunk) -> Result<()> {
    // Increment queue size first (under lock)
    {
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

      // W3C spec: throw DataError if first chunk is not a keyframe
      let is_key = chunk.is_key();
      if !inner.keyframe_received {
        if is_key {
          inner.keyframe_received = true;
        } else {
          // Trying to decode a delta frame before any keyframe
          return throw_data_error(&env, "First chunk must be a keyframe");
        }
      }

      inner.decode_queue_size += 1;
    }

    // Send decode command to worker thread via microtask for W3C spec FIFO ordering
    // This ensures all commands (decode, configure, flush) are ordered correctly
    if let Some(ref sender) = self.command_sender {
      let sender = sender.clone();
      let reset_flag = self.reset_flag.clone();
      let chunk_inner = chunk.inner.clone();
      PromiseRaw::resolve(&env, ())?.then(move |_| {
        // Check reset flag - if reset() was called, skip sending
        if !reset_flag.load(Ordering::SeqCst) {
          let _ = sender.send(WorkerCommand::Decode(chunk_inner));
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
        let (error_name, error_msg) = if inner.had_error {
          (DOMExceptionName::EncodingError, "Decode error occurred")
        } else {
          (
            DOMExceptionName::InvalidStateError,
            "Cannot flush a closed codec",
          )
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

      // Store abort flag for reset() to access
      inner.flush_abort_flag = Some(flush_abort_flag.clone());
      // Set inside_flush flag so worker queues frames instead of calling NonBlocking callback
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

    // Send flush command through the channel (deferred to microtask for W3C spec compliance)
    // This ensures flush is processed after all pending decode microtasks complete (FIFO order)
    if let Some(ref sender) = self.command_sender {
      let sender = sender.clone();
      let reset_flag = self.reset_flag.clone();
      PromiseRaw::resolve(env, ())?.then(move |_| {
        // Check reset flag - if reset() was called, skip sending
        // (flush Promise is already rejected with AbortError by reset())
        if !reset_flag.load(Ordering::SeqCst) {
          let _ = sender.send(WorkerCommand::Flush(response_sender));
        }
        Ok(())
      })?;
    } else {
      return throw_invalid_state_error(env, "Cannot flush a closed codec");
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
        // Drain pending frames and call output callback SYNCHRONOUSLY
        // This runs on the main thread with Env access
        let frames = {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          std::mem::take(&mut guard.pending_frames)
        };

        // Call output callback for each frame synchronously
        // If callback calls reset(), abort_flag will be set before next iteration
        let callback = output_callback_ref.borrow_back(env)?;
        for frame in frames {
          // Check abort flag before each callback - exit early if reset() was called
          if abort_flag.load(Ordering::SeqCst) {
            break;
          }
          callback.call(frame)?;
        }

        // Clean up flags
        {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          guard.flush_abort_flag = None;
          guard.inside_flush = false;
        }

        // Check abort flag after draining all frames
        if abort_flag.load(Ordering::SeqCst) {
          return Err(Error::new(
            Status::GenericFailure,
            "AbortError: The operation was aborted",
          ));
        }

        // Return worker result (errors keep DOMException-style message for now)
        result
      },
    )
  }

  /// Reset the decoder
  #[napi]
  pub fn reset(&mut self, env: Env) -> Result<()> {
    // Check state first before touching the worker
    {
      let inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      // W3C spec: throw InvalidStateError if closed
      if inner.state == CodecState::Closed {
        return throw_invalid_state_error(&env, "Cannot reset a closed codec");
      }

      // Set abort flag FIRST (synchronously, before any other reset logic)
      // This signals any pending flush() that is yielding to return AbortError
      if let Some(ref flag) = inner.flush_abort_flag {
        flag.store(true, Ordering::SeqCst);
      }
    }

    // Set reset flag to signal worker to skip remaining pending decodes
    // This must be done BEFORE dropping the command sender
    self.reset_flag.store(true, Ordering::SeqCst);

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

    // Drop sender to signal worker to stop
    // Note: Don't join worker here - with microtask pattern, worker might not exit
    // immediately if microtasks still hold cloned senders. Worker will exit on
    // next timeout check when it sees reset_flag is set.
    drop(self.command_sender.take());
    drop(self.worker_handle.take()); // Detach old worker thread

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Drain decoder before dropping to ensure libaom/AV1 threads finish
    if let Some(ctx) = inner.context.as_mut() {
      ctx.flush();
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
    }

    // Drop existing context
    inner.context = None;
    inner.config = None;
    inner.codec_string.clear();
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    inner.decode_queue_size = 0;
    inner.keyframe_received = false;
    inner.had_error = false;

    // Reset hardware tracking state
    inner.is_hardware = false;
    inner.hw_preference = HardwareAcceleration::NoPreference;
    inner.silent_decode_count = 0;
    inner.first_output_produced = false;
    inner.pending_chunks.clear();
    inner.timestamp_queue.clear();

    // Clear flush-related state
    inner.inside_flush = false;
    inner.pending_frames.clear();

    // Reset the abort flag for new worker
    self.reset_flag.store(false, Ordering::SeqCst);

    // Create new channel and worker for future decode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(sender);
    let worker_inner = self.inner.clone();
    let worker_event_state = self.event_state.clone();
    let worker_reset_flag = self.reset_flag.clone();

    // Create synchronization channel to wait for worker to be ready
    let (ready_sender, ready_receiver) = channel::bounded::<()>(1);

    drop(inner); // Release lock before spawning thread
    self.worker_handle = Some(std::thread::spawn(move || {
      // Signal that worker is ready before entering the loop
      let _ = ready_sender.send(());
      Self::worker_loop(
        worker_inner,
        worker_event_state,
        receiver,
        worker_reset_flag,
      );
    }));

    // Wait for worker to be ready (prevents race condition)
    let _ = ready_receiver.recv();

    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&mut self, env: Env) -> Result<()> {
    // Check state first - W3C spec: throw InvalidStateError if already closed
    {
      let inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        return throw_invalid_state_error(&env, "Cannot close an already closed codec");
      }
    }

    // Drop sender to stop accepting new commands
    self.command_sender = None;

    // Let worker detach - will exit when channel closes (all senders dropped)
    self.worker_handle = None;

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Drain decoder before dropping to ensure libaom/AV1 threads finish
    if let Some(ctx) = inner.context.as_mut() {
      // Flush internal buffers first - this synchronizes libaom's thread pool
      ctx.flush();
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
    }

    inner.context = None;
    inner.config = None;
    inner.codec_string.clear();
    inner.state = CodecState::Closed;
    inner.decode_queue_size = 0;

    // Reset hardware tracking state
    inner.is_hardware = false;
    inner.silent_decode_count = 0;
    inner.first_output_produced = false;
    inner.pending_chunks.clear();

    Ok(())
  }

  /// Check if a configuration is supported
  /// Returns a Promise that resolves with support information
  ///
  /// W3C WebCodecs spec: Throws TypeError for invalid configs,
  /// returns { supported: false } for valid but unsupported configs.
  #[napi]
  pub fn is_config_supported<'env>(
    env: &'env Env,
    config: VideoDecoderConfig,
  ) -> Result<PromiseRaw<'env, VideoDecoderSupport>> {
    // W3C WebCodecs spec: Validate config, throw TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-videodecoder-isconfigsupported

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      Some(_) => return reject_with_type_error(env, "codec is required"),
      None => return reject_with_type_error(env, "codec is required"),
    };

    // Validate coded dimensions if specified
    if let Some(w) = config.coded_width
      && w == 0
    {
      return reject_with_type_error(env, "codedWidth must be greater than 0");
    }
    if let Some(h) = config.coded_height
      && h == 0
    {
      return reject_with_type_error(env, "codedHeight must be greater than 0");
    }

    // Validate display aspect dimensions if specified
    if let Some(dw) = config.display_aspect_width
      && dw == 0
    {
      return reject_with_type_error(env, "displayAspectWidth must be greater than 0");
    }
    if let Some(dh) = config.display_aspect_height
      && dh == 0
    {
      return reject_with_type_error(env, "displayAspectHeight must be greater than 0");
    }

    // Validate dimensions if specified
    let width = config.coded_width.unwrap_or(0);
    let height = config.coded_height.unwrap_or(0);
    if width > 0 && height > 0 && !are_dimensions_valid(width, height) {
      return env.spawn_future(async move {
        Ok(VideoDecoderSupport {
          supported: false,
          config,
        })
      });
    }

    env.spawn_future(async move {
      // Parse codec string
      let codec_id = match parse_codec_string(&codec) {
        Ok(id) => id,
        Err(_) => {
          return Ok(VideoDecoderSupport {
            supported: false,
            config,
          });
        }
      };

      // Try to create decoder
      let result = CodecContext::new_decoder(codec_id);

      Ok(VideoDecoderSupport {
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
  #[napi]
  pub fn add_event_listener(
    &self,
    env: Env,
    event_type: String,
    callback: FunctionRef<(), UnknownReturnValue>,
    options: Option<VideoDecoderAddEventListenerOptions>,
  ) -> Result<()> {
    let mut state = self
      .event_state
      .write()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let id = state.next_listener_id;
    state.next_listener_id += 1;

    // Get the function and create both a Ref (to prevent GC) and a weak TSF
    let func = callback.borrow_back(&env)?;
    let prevent_gc = func.create_ref()?;
    let tsf = Arc::new(
      func
        .build_threadsafe_function()
        .callee_handled::<false>()
        .weak::<true>() // Weak to allow Node.js process to exit
        .build()?,
    );

    let entry = EventListenerEntry {
      id,
      callback: tsf,
      once: options.as_ref().and_then(|o| o.once).unwrap_or(false),
      _prevent_gc: prevent_gc, // Prevents GC while listener is registered
    };

    state
      .event_listeners
      .entry(event_type)
      .or_default()
      .push(entry);
    Ok(())
  }

  /// Remove an event listener for the specified event type
  #[napi]
  pub fn remove_event_listener(
    &self,
    event_type: String,
    _callback: FunctionRef<(), UnknownReturnValue>,
    _options: Option<VideoDecoderEventListenerOptions>,
  ) -> Result<()> {
    let mut state = self
      .event_state
      .write()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Note: We can't compare function references directly, so we remove the last added listener
    // for simplicity. A more complete implementation would need to track callback identity.
    if let Some(listeners) = state.event_listeners.get_mut(&event_type) {
      listeners.pop();
      if listeners.is_empty() {
        state.event_listeners.remove(&event_type);
      }
    }
    Ok(())
  }

  /// Dispatch an event to all registered listeners
  #[napi]
  pub fn dispatch_event(&self, event_type: String) -> Result<bool> {
    let mut state = self
      .event_state
      .write()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let mut ids_to_remove = Vec::new();

    if let Some(listeners) = state.event_listeners.get(&event_type) {
      for entry in listeners {
        // Call the listener with no arguments (like dequeue callback)
        entry
          .callback
          .call((), ThreadsafeFunctionCallMode::Blocking);
        if entry.once {
          ids_to_remove.push(entry.id);
        }
      }
    }

    // Remove "once" listeners
    if !ids_to_remove.is_empty()
      && let Some(listeners) = state.event_listeners.get_mut(&event_type)
    {
      listeners.retain(|e| !ids_to_remove.contains(&e.id));
      if listeners.is_empty() {
        state.event_listeners.remove(&event_type);
      }
    }

    Ok(true) // Event was not cancelled
  }
}

/// Valid H.264/AVC profiles (decimal values)
const VALID_AVC_PROFILES: &[u8] = &[
  66,  // Baseline
  77,  // Main
  88,  // Extended
  100, // High
  110, // High 10
  122, // High 4:2:2
  244, // High 4:4:4 Predictive
];

/// Valid H.264/AVC levels (decimal values)
const VALID_AVC_LEVELS: &[u8] = &[
  10, 11, 12, 13, // 1, 1.1, 1.2, 1.3
  20, 21, 22, // 2, 2.1, 2.2
  30, 31, 32, // 3, 3.1, 3.2
  40, 41, 42, // 4, 4.1, 4.2
  50, 51, 52, // 5, 5.1, 5.2
  60, 61, 62, // 6, 6.1, 6.2
];

/// Valid VP9 profiles (0-3)
const MAX_VP9_PROFILE: u8 = 3;

/// Valid VP9 levels
const VALID_VP9_LEVELS: &[u8] = &[10, 11, 20, 21, 30, 31, 40, 41, 50, 51, 52, 60, 61, 62];

/// Valid AV1 profiles (0-2)
const MAX_AV1_PROFILE: u8 = 2;

/// Valid AV1 levels (0-23)
const MAX_AV1_LEVEL: u8 = 23;

/// Valid HEVC profiles
const VALID_HEVC_PROFILES: &[u8] = &[1, 2, 3, 4];

/// Maximum dimension (width/height) for decoder
const MAX_DIMENSION: u32 = 16384;

/// Validate H.264/AVC codec string format and parameters
/// Format: avc1.PPCCLL or avc3.PPCCLL where PP=profile, CC=constraint, LL=level (hex)
fn validate_avc_codec(codec: &str) -> bool {
  // Must start with exactly "avc1." or "avc3."
  if !codec.starts_with("avc1.") && !codec.starts_with("avc3.") {
    return codec == "avc1" || codec == "avc3" || codec == "h264";
  }

  let params = &codec[5..]; // Skip "avc1." or "avc3."
  if params.len() != 6 {
    return false;
  }

  // Parse profile (first 2 hex digits)
  let profile = match u8::from_str_radix(&params[0..2], 16) {
    Ok(p) => p,
    Err(_) => return false,
  };

  // Parse level (last 2 hex digits)
  let level = match u8::from_str_radix(&params[4..6], 16) {
    Ok(l) => l,
    Err(_) => return false,
  };

  // Validate profile and level
  VALID_AVC_PROFILES.contains(&profile) && VALID_AVC_LEVELS.contains(&level)
}

/// Validate VP9 codec string format and parameters
/// Format: vp09.PP.LL.BB[.CC.CP.TC.FR.CS] where PP=profile, LL=level, BB=bit depth
/// Short form "vp9" is accepted and defaults to profile 0, level 1.0, 8-bit
fn validate_vp9_codec(codec: &str) -> bool {
  // Short form "vp9" is valid - defaults to profile 0, level 1, 8-bit
  if codec == "vp9" {
    return true;
  }

  if !codec.starts_with("vp09.") {
    return false;
  }

  let params = &codec[5..]; // Skip "vp09."
  let parts: Vec<&str> = params.split('.').collect();
  if parts.len() < 3 {
    return false;
  }

  // Parse profile (2 decimal digits)
  let profile: u8 = match parts[0].parse() {
    Ok(p) => p,
    Err(_) => return false,
  };

  // Parse level (2 decimal digits)
  let level: u8 = match parts[1].parse() {
    Ok(l) => l,
    Err(_) => return false,
  };

  // Validate profile and level
  profile <= MAX_VP9_PROFILE && VALID_VP9_LEVELS.contains(&level)
}

/// Validate AV1 codec string format and parameters
/// Format: av01.P.LLM.BB[.M.CCC.CP.TC.FR.CS] where P=profile, LL=level, M=tier
fn validate_av1_codec(codec: &str) -> bool {
  // Short forms "av1" and "av01" are valid - default to main profile, level 4.0, 8-bit
  if codec == "av1" || codec == "av01" {
    return true;
  }

  if !codec.starts_with("av01.") {
    return false;
  }

  let params = &codec[5..]; // Skip "av01."
  let parts: Vec<&str> = params.split('.').collect();
  if parts.len() < 3 {
    return false;
  }

  // Parse profile (single digit)
  let profile: u8 = match parts[0].parse() {
    Ok(p) => p,
    Err(_) => return false,
  };

  // Parse level (2 digits + tier letter, e.g., "04M" or "10H")
  let level_str = parts[1];
  if level_str.len() < 2 {
    return false;
  }
  let level: u8 = match level_str[..2].parse() {
    Ok(l) => l,
    Err(_) => return false,
  };

  // Validate profile and level
  profile <= MAX_AV1_PROFILE && level <= MAX_AV1_LEVEL
}

/// Validate HEVC codec string format and parameters
/// Format: hvc1.P.CCCCCC.Lxx or hev1.P.CCCCCC.Lxx
fn validate_hevc_codec(codec: &str) -> bool {
  if codec == "hevc" || codec == "h265" {
    return true;
  }

  if !codec.starts_with("hvc1.") && !codec.starts_with("hev1.") {
    return codec == "hvc1" || codec == "hev1";
  }

  let params = &codec[5..]; // Skip "hvc1." or "hev1."
  let parts: Vec<&str> = params.split('.').collect();
  if parts.is_empty() {
    return false;
  }

  // Parse profile indicator (first part after codec prefix)
  // Can be "1", "2", "A1", "B1", "C99" etc.
  let profile_part = parts[0];

  // Extract numeric profile from formats like "1", "A1", "B1", "C99"
  let profile_num: u8 = if profile_part
    .chars()
    .next()
    .is_some_and(|c| c.is_ascii_digit())
  {
    // Starts with digit - parse the whole part as profile
    match profile_part.parse() {
      Ok(p) => p,
      Err(_) => return false,
    }
  } else if profile_part.len() >= 2 {
    // Starts with letter - parse digits after the letter
    match profile_part[1..].parse() {
      Ok(p) => p,
      Err(_) => return false,
    }
  } else {
    return false;
  };

  // Validate profile (1-4 for standard profiles)
  // Note: Profile indicators like C99 should fail
  VALID_HEVC_PROFILES.contains(&profile_num)
}

/// Check if codec string has valid casing (case-sensitive per W3C spec)
fn has_valid_codec_casing(codec: &str) -> bool {
  // Check for leading/trailing whitespace
  if codec != codec.trim() {
    return false;
  }

  // VP8 must be exact lowercase
  if codec.eq_ignore_ascii_case("vp8") && codec != "vp8" {
    return false;
  }

  // VP9 short form must be exact lowercase
  if codec.eq_ignore_ascii_case("vp9") && codec != "vp9" {
    return false;
  }

  // vp09.* must start with lowercase vp09
  if codec.to_lowercase().starts_with("vp09.") && !codec.starts_with("vp09.") {
    return false;
  }

  // av01.* must start with lowercase av01
  if codec.to_lowercase().starts_with("av01.") && !codec.starts_with("av01.") {
    return false;
  }

  // avc1/avc3 must be lowercase
  if codec.to_lowercase().starts_with("avc1") && !codec.starts_with("avc1") {
    return false;
  }
  if codec.to_lowercase().starts_with("avc3") && !codec.starts_with("avc3") {
    return false;
  }

  // hvc1/hev1 must be lowercase
  if codec.to_lowercase().starts_with("hvc1") && !codec.starts_with("hvc1") {
    return false;
  }
  if codec.to_lowercase().starts_with("hev1") && !codec.starts_with("hev1") {
    return false;
  }

  true
}

/// Check if dimensions are within valid range
fn are_dimensions_valid(width: u32, height: u32) -> bool {
  width <= MAX_DIMENSION && height <= MAX_DIMENSION
}

/// Parse WebCodecs codec string to FFmpeg codec ID
/// Returns error for unsupported or invalid codec strings
fn parse_codec_string(codec: &str) -> Result<AVCodecID> {
  // Handle common codec strings
  // https://www.w3.org/TR/webcodecs-codec-registry/

  // Check case sensitivity first
  if !has_valid_codec_casing(codec) {
    return Err(Error::new(
      Status::GenericFailure,
      format!("Unsupported codec: {}", codec),
    ));
  }

  // H.264/AVC
  if codec.starts_with("avc1") || codec.starts_with("avc3") || codec == "h264" {
    if validate_avc_codec(codec) {
      return Ok(AVCodecID::H264);
    }
    return Err(Error::new(
      Status::GenericFailure,
      format!("Unsupported codec: {}", codec),
    ));
  }

  // HEVC
  if codec.starts_with("hev1") || codec.starts_with("hvc1") || codec == "h265" || codec == "hevc" {
    if validate_hevc_codec(codec) {
      return Ok(AVCodecID::Hevc);
    }
    return Err(Error::new(
      Status::GenericFailure,
      format!("Unsupported codec: {}", codec),
    ));
  }

  // VP8
  if codec == "vp8" {
    return Ok(AVCodecID::Vp8);
  }

  // VP9 - note: short form "vp9" is ambiguous for decoders
  if codec.starts_with("vp09") {
    if validate_vp9_codec(codec) {
      return Ok(AVCodecID::Vp9);
    }
    return Err(Error::new(
      Status::GenericFailure,
      format!("Unsupported codec: {}", codec),
    ));
  }

  // VP9 short form - accept and default to profile 0
  if codec == "vp9" {
    return Ok(AVCodecID::Vp9);
  }

  // AV1 - accept both "av1" and "av01" short forms
  if codec.starts_with("av01") || codec == "av1" || codec == "av01" {
    if validate_av1_codec(codec) {
      return Ok(AVCodecID::Av1);
    }
    return Err(Error::new(
      Status::GenericFailure,
      format!("Unsupported codec: {}", codec),
    ));
  }

  Err(Error::new(
    Status::GenericFailure,
    format!("Unsupported codec: {}", codec),
  ))
}

/// Decode chunk data using FFmpeg
fn decode_chunk_data(
  context: &mut CodecContext,
  data: &[u8],
  timestamp: i64,
  duration: Option<i64>,
) -> Result<Vec<Frame>> {
  // W3C spec: Empty data should trigger EncodingError
  if data.is_empty() {
    return Err(Error::new(
      Status::GenericFailure,
      "EncodingError: Cannot decode empty frame data",
    ));
  }

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
  if let Some(dur) = duration {
    packet.set_duration(dur);
  }

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
