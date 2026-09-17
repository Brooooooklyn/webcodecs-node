//! VideoDecoder - WebCodecs API implementation
//!
//! Provides video decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videodecoder-interface

use crate::codec::{CodecContext, DecoderConfig, Frame, Packet, download_hw_frame};
use crate::ffi::{AV_NOPTS_VALUE, AVCodecID, AVHWDeviceType, accessors::ffctx_set_hw_get_format};
use crate::webcodecs::encoded_video_chunk::InternalSlice;
use crate::webcodecs::error::{
  DOMExceptionName, error_as_dom_exception, native_dom_exception_error, throw_data_error,
  throw_invalid_state_error, throw_type_error_unit,
};
use crate::webcodecs::event_target::CodecEventState;
use crate::webcodecs::flush_tracker::FlushTracker;
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
use std::borrow::Cow;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

/// Type alias for output callback (takes VideoFrame)
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback =
  ThreadsafeFunction<VideoFrame, UnknownReturnValue, VideoFrame, Status, false, true>;

/// Type alias for error callback (takes Error object)
/// Using CalleeHandled: false because WebCodecs error callback receives the
/// error directly, not error-first (err, result) style. The Error payload is
/// converted to a native DOMException in the JS-side callback.
type ErrorCallback =
  ThreadsafeFunction<Error, UnknownReturnValue, Unknown<'static>, Status, false, true>;

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
  /// Error callback reference - prevents GC from collecting the error callback
  pub error_ref: FunctionRef<Error, UnknownReturnValue>,
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
    let error: ErrorCallback = error_func
      .build_threadsafe_function()
      .callee_handled::<false>()
      .weak::<true>()
      .build_callback(|ctx| error_as_dom_exception(&ctx.env, ctx.value))?;

    Ok(VideoDecoderInit {
      output,
      output_ref,
      error,
      error_ref,
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
/// Set to 10 to accommodate H.264/HEVC B-frame buffering (typically 4-8 frames)
/// while still detecting genuinely failing decoders within ~333ms at 30fps.
const SILENT_FAILURE_THRESHOLD: u32 = 10;

/// Metadata for one in-flight chunk, keyed by its opaque sequence tag.
/// The tag travels through FFmpeg as the packet/frame opaque pointer, giving
/// exact chunk identity through B-frame reordering, even for duplicate
/// timestamps. All software decoders propagate it via ff_get_buffer
/// (including VideoToolbox, which is a hwaccel inside them); decoders that
/// set frame props themselves (cuvid, mediacodec) may not and fall back to
/// matching by frame PTS.
#[derive(Clone, Copy)]
struct ChunkMeta {
  timestamp: i64,
  duration: Option<i64>,
  /// Display events the chunk will still produce. VP9 superframes emit one
  /// output per visible constituent, all carrying the same opaque tag, so
  /// the entry is consumed only after the last expected output.
  remaining: u32,
}

/// Consume one display event from a chunk's entry, removing it once no
/// further output is expected. Returns the chunk's metadata.
fn consume_chunk_meta(map: &mut ChunkMetaMap, seq: u64) -> Option<ChunkMeta> {
  let meta = *map.get(&seq)?;
  if meta.remaining <= 1 {
    map.remove(&seq);
  } else if let Some(entry) = map.get_mut(&seq) {
    entry.remaining -= 1;
  }
  Some(meta)
}

type ChunkMetaMap = std::collections::BTreeMap<u64, ChunkMeta>;

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
  /// Pending flush operations, tracked independently for overlapping calls.
  flushes: FlushTracker,
  /// Next chunk sequence number for opaque identity tagging (1-based)
  next_chunk_seq: u64,
  /// Sequence number -> (timestamp, duration) for chunks in flight
  chunk_meta: ChunkMetaMap,
  /// Queue of decoded frames waiting to be delivered via output callback
  /// Worker pushes frames here during flush; flush() drains them synchronously via FunctionRef
  pending_frames: Vec<VideoFrame>,

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
  event_state: Arc<RwLock<CodecEventState>>,
  /// Output callback reference - stored for synchronous calls from main thread (in flush resolver)
  /// Wrapped in Rc to allow sharing with spawn_future_with_callback closure
  /// (Rc is !Send but that's OK - the callback runs on the main thread)
  output_callback_ref: Rc<FunctionRef<VideoFrame, UnknownReturnValue>>,
  /// Error callback reference - prevents GC from collecting the error callback
  /// (weak ThreadsafeFunction alone can be collected on slow platforms like armv7 QEMU)
  #[allow(dead_code)]
  error_callback_ref: Rc<FunctionRef<Error, UnknownReturnValue>>,
  /// Channel sender for worker commands (wrapped in Arc for Weak references in microtasks)
  command_sender: Option<Arc<Sender<WorkerCommand>>>,
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
  }
}

#[napi]
impl VideoDecoder {
  /// Create a new VideoDecoder with init dictionary (per WebCodecs spec)
  ///
  /// @param init - Init dictionary containing output and error callbacks
  #[napi(constructor)]
  pub fn new(
    #[napi(
      ts_arg_type = "{ output: (frame: VideoFrame) => void, error: (error: DOMException) => void }"
    )]
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
      flushes: FlushTracker::default(),
      next_chunk_seq: 1,
      chunk_meta: ChunkMetaMap::new(),
      pending_frames: Vec::new(),
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
    let event_state = Arc::new(RwLock::new(CodecEventState::default()));

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
      output_callback_ref: Rc::new(init.output_ref),
      error_callback_ref: Rc::new(init.error_ref),
      command_sender: Some(Arc::new(sender)),
      worker_handle: Some(worker_handle),
      reset_flag,
    })
  }

  /// Worker loop that processes commands from the channel
  fn worker_loop(
    inner: Arc<Mutex<VideoDecoderInner>>,
    event_state: Arc<RwLock<CodecEventState>>,
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
        }
        continue;
      }

      match command {
        WorkerCommand::Decode(chunk) => {
          Self::process_decode(&inner, &event_state, chunk, &reset_flag);
        }
        WorkerCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner, &event_state, &reset_flag);
          let _ = response_sender.send(result);
        }
        WorkerCommand::Reconfigure(config) => {
          Self::process_reconfigure(&inner, config, &reset_flag);
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
    event_state: &Arc<RwLock<CodecEventState>>,
    chunk: Arc<RwLock<Option<EncodedVideoChunkInner>>>,
    reset_flag: &AtomicBool,
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    if reset_flag.load(Ordering::SeqCst) {
      return;
    }

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
    let is_keyframe = encoded_chunk.chunk_type == crate::webcodecs::EncodedVideoChunkType::Key;

    // Handle packet data format based on decoder type:
    // - Hardware decoders (VideoToolbox, etc.) expect AVCC/HVCC format (length-prefixed NALUs)
    // - Software decoders expect Annex B format (start code prefixed NALUs)
    let data = {
      let codec = &guard.codec_string;
      let is_avc_codec = codec.starts_with("avc1")
        || codec.starts_with("avc3")
        || codec.starts_with("hvc1")
        || codec.starts_with("hev1");

      // For hardware decoding, keep data in original AVCC/HVCC format
      // VideoToolbox expects length-prefixed NALUs directly
      if guard.is_hardware {
        Cow::Borrowed(encoded_chunk.data.as_slice())
      } else if is_avc_codec && is_avcc_format(encoded_chunk.data.as_slice()) {
        // For software decoding, convert to Annex B format
        let mut converted = convert_avcc_to_annexb(encoded_chunk.data.as_slice());

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
        Cow::Owned(converted)
      } else {
        Cow::Borrowed(encoded_chunk.data.as_slice())
      }
    };

    // Tag the chunk with a sequence number carried through FFmpeg as the
    // packet/frame opaque pointer: exact identity through B-frame reordering,
    // even for duplicate timestamps. Chunks that produce no display event of
    // their own (invisible VP9 reference frames) record nothing: their entry
    // could otherwise win PTS matching against the real display chunk.
    let seq = guard.next_chunk_seq;
    guard.next_chunk_seq += 1;
    let remaining = display_event_count(&guard.codec_string, encoded_chunk.data.as_slice());
    if remaining > 0 {
      guard.chunk_meta.insert(
        seq,
        ChunkMeta {
          timestamp,
          duration,
          remaining,
        },
      );
    }

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
    let frames = match decode_chunk_data(context, &data, timestamp, duration, seq) {
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

    // Drop the chunk read guard now that decoding has completed
    drop(chunk_read_guard);
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
      // Resolve the chunk this frame came from: exact identity via the opaque
      // sequence tag, else frame PTS, else oldest pending chunk.
      let (output_timestamp, output_duration) =
        Self::resolve_frame_metadata(&mut guard, &frame, (timestamp, duration));

      // Download hardware frames to CPU memory if needed
      let output_frame = if frame.format().is_hardware() {
        match download_hw_frame(&frame) {
          Ok(sw_frame) => sw_frame,
          Err(e) => {
            Self::report_error(
              &mut guard,
              &format!("OperationError: Failed to download hardware frame: {}", e),
            );
            return;
          }
        }
      } else {
        frame
      };

      let video_frame = VideoFrame::from_internal_with_orientation(
        output_frame,
        output_timestamp,
        output_duration,
        guard.config_rotation,
        guard.config_flip,
        guard.config_color_space.as_ref(),
      );

      // During flush, queue frames for synchronous delivery in resolver
      // Otherwise, use NonBlocking callback for immediate delivery
      if guard.flushes.should_buffer_outputs() {
        guard.pending_frames.push(video_frame);
      } else {
        guard
          .output_callback
          .call(video_frame, ThreadsafeFunctionCallMode::NonBlocking);
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
        .map(|c| (c.timestamp_us, c.duration_us, &c.data))
      {
        Some(d) => d,
        None => continue, // Skip closed chunks
      };

      // Convert AVCC/HVCC format to Annex B if needed for H.264/H.265
      let data = {
        let codec = &guard.codec_string;
        let is_avc_codec = codec.starts_with("avc1")
          || codec.starts_with("avc3")
          || codec.starts_with("hvc1")
          || codec.starts_with("hev1");

        if is_avc_codec && is_avcc_format(raw_data.as_slice()) {
          Cow::Owned(convert_avcc_to_annexb(raw_data.as_slice()))
        } else {
          Cow::Borrowed(raw_data.as_slice())
        }
      };

      // Re-tag the chunk with a fresh sequence number; the original tag's
      // entry is still pending (hardware produced no frames before fallback).
      let seq = guard.next_chunk_seq;
      guard.next_chunk_seq += 1;
      let remaining = display_event_count(&guard.codec_string, raw_data.as_slice());
      if remaining > 0 {
        guard.chunk_meta.insert(
          seq,
          ChunkMeta {
            timestamp,
            duration,
            remaining,
          },
        );
      }

      // Decode with software decoder
      let context = match guard.context.as_mut() {
        Some(ctx) => ctx,
        None => return,
      };

      let frames = match decode_chunk_data(context, &data, timestamp, duration, seq) {
        Ok(f) => f,
        Err(_) => continue, // Skip failed chunks during re-decode
      };

      // Drop the chunk read guard now that decoding has completed
      drop(chunk_read_guard);

      // Mark first output produced on success
      if !frames.is_empty() && !guard.first_output_produced {
        guard.first_output_produced = true;
      }

      // Deliver frames (queue during flush, NonBlocking otherwise)
      for frame in frames {
        // Resolve the chunk this frame came from: B-frame reordering means
        // output frames do not correspond to the chunk just decoded.
        let (output_timestamp, output_duration) =
          Self::resolve_frame_metadata(&mut guard, &frame, (timestamp, duration));

        // Download hardware frames to CPU memory if needed
        // (shouldn't happen in fallback path but handle for safety)
        let output_frame = if frame.format().is_hardware() {
          match download_hw_frame(&frame) {
            Ok(sw_frame) => sw_frame,
            Err(_) => continue, // Skip failed frame downloads during re-decode
          }
        } else {
          frame
        };

        let video_frame = VideoFrame::from_internal_with_orientation(
          output_frame,
          output_timestamp,
          output_duration,
          guard.config_rotation,
          guard.config_flip,
          guard.config_color_space.as_ref(),
        );
        if guard.flushes.should_buffer_outputs() {
          guard.pending_frames.push(video_frame);
        } else {
          guard
            .output_callback
            .call(video_frame, ThreadsafeFunctionCallMode::NonBlocking);
        }
      }
    }
  }

  /// Process a flush command
  fn process_flush(
    inner: &Arc<Mutex<VideoDecoderInner>>,
    _event_state: &Arc<RwLock<CodecEventState>>,
    reset_flag: &AtomicBool,
  ) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if reset_flag.load(Ordering::SeqCst) {
      return Err(Error::new(
        Status::GenericFailure,
        "AbortError: The operation was aborted",
      ));
    }

    // W3C spec: If an error occurred during decoding, flush should reject with EncodingError.
    // This must be checked first to return the correct error type.
    if guard.had_error {
      return Err(Error::new(
        Status::GenericFailure,
        "EncodingError: Decode error occurred",
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
        // No context means decoder was reset/closed - silently succeed
        return Ok(());
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
    tracing::debug!(target: "webcodecs", "process_flush: processing {} flushed frames", frames.len());
    for frame in frames.into_iter() {
      // Resolve the chunk this frame came from (opaque tag, else PTS match).
      let (output_timestamp, output_duration) =
        Self::resolve_frame_metadata(&mut guard, &frame, (0, None));

      // Download hardware frames to CPU memory if needed
      let output_frame = if frame.format().is_hardware() {
        match download_hw_frame(&frame) {
          Ok(sw_frame) => sw_frame,
          Err(e) => {
            let msg = format!("Failed to download hardware frame: {}", e);
            Self::report_error(&mut guard, &msg);
            return Err(Error::new(
              Status::GenericFailure,
              format!("EncodingError: {}", msg),
            ));
          }
        }
      } else {
        frame
      };

      let video_frame = VideoFrame::from_internal_with_orientation(
        output_frame,
        output_timestamp,
        output_duration,
        guard.config_rotation,
        guard.config_flip,
        guard.config_color_space.as_ref(),
      );
      // Always queue during flush for synchronous delivery in resolver
      guard.pending_frames.push(video_frame);
    }

    // Clear any remaining duration mappings after flush
    guard.chunk_meta.clear();

    // Reset decoder state so it can accept more data (per W3C spec, flush should leave
    // decoder in configured state, ready for more decode() calls)
    if let Some(ref mut context) = guard.context {
      context.flush();
    }

    Ok(())
  }

  /// Process a reconfigure command on the worker thread
  /// Replaces the old context with a new one with updated config
  fn process_reconfigure(
    inner: &Arc<Mutex<VideoDecoderInner>>,
    config: VideoDecoderConfig,
    reset_flag: &AtomicBool,
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    if reset_flag.load(Ordering::SeqCst) {
      return;
    }

    // Don't reconfigure if decoder is closed
    if guard.state == CodecState::Closed {
      return;
    }

    // Clear codec-local work state. Do not reset decode_queue_size here:
    // main-thread decode() calls after this FIFO command are already counted.
    // keyframe_received is intentionally not cleared here: configure() already
    // resets it synchronously on the main thread, and clearing it again on the
    // worker would clobber a key chunk accepted after configure() returned.
    guard.chunk_meta.clear();
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
    let (mut context, is_hardware, hw_pix_fmt_raw) = if let Some(hw) = hw_type {
      match CodecContext::new_decoder_with_hw_info(codec_id, Some(hw)) {
        Ok(result) => (result.context, result.is_hardware, result.hw_pix_fmt_raw),
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
        Ok(ctx) => (ctx, false, None),
        Err(e) => {
          Self::report_error(
            &mut guard,
            &format!("NotSupportedError: Failed to create decoder: {}", e),
          );
          return;
        }
      }
    };

    // Handle extradata format based on decoder type:
    // - Hardware decoders (VideoToolbox, etc.) expect avcC/hvcC format (original container format)
    // - Software decoders expect Annex B format (start code prefixed NALUs)
    let extradata = config.description.as_ref().and_then(|d| {
      let data = d.to_vec();

      // For hardware decoding, keep extradata in original avcC/hvcC format
      // VideoToolbox parses avcC format directly to initialize the decoder session
      if is_hardware {
        return Some(data);
      }

      // For software decoding, convert to Annex B format
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
    // For hardware decoders, use single-threaded mode (thread_count=1) to avoid
    // race conditions during flush that can cause crashes with VideoToolbox and other
    // hardware accelerators. For software decoders, use auto-detect (thread_count=0)
    // for optimal performance.
    let thread_count = if is_hardware { 1 } else { 0 };
    let decoder_config = DecoderConfig {
      codec_id,
      thread_count,
      extradata,
      low_latency: config.optimize_for_latency.unwrap_or(false),
      width: config.coded_width,
      height: config.coded_height,
    };

    if let Err(e) = context.configure_decoder(&decoder_config) {
      Self::report_error(
        &mut guard,
        &format!("NotSupportedError: Failed to configure decoder: {}", e),
      );
      return;
    }

    // Set up get_format callback for hardware decoding
    // This is required for FFmpeg to negotiate the correct pixel format with hardware decoders
    if is_hardware && let Some(pix_fmt_raw) = hw_pix_fmt_raw {
      unsafe {
        ffctx_set_hw_get_format(context.as_mut_ptr(), pix_fmt_raw);
      }
    }

    if let Err(e) = context.open() {
      Self::report_error(
        &mut guard,
        &format!("NotSupportedError: Failed to open decoder: {}", e),
      );
      return;
    }

    // Log context state after opening for debugging
    if is_hardware {
      tracing::debug!(
        "Decoder opened: pix_fmt={:?}, width={}, height={}, is_hardware={}",
        context.pixel_format(),
        context.width(),
        context.height(),
        is_hardware
      );
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

  /// Resolve the (timestamp, duration) pair belonging to a decoded frame.
  ///
  /// Resolution tiers:
  /// 1. Exact chunk identity via the opaque sequence tag (all software
  ///    decoders propagate packet opaque to frame under
  ///    AV_CODEC_FLAG_COPY_OPAQUE, including VideoToolbox's hwaccel path).
  /// 2. Frame PTS matched against pending chunk timestamps (decoders that
  ///    set frame props themselves, e.g. cuvid, mediacodec).
  /// 3. Oldest pending chunk (NOPTS frames), else the given fallback.
  fn resolve_frame_metadata(
    guard: &mut VideoDecoderInner,
    frame: &Frame,
    fallback: (i64, Option<i64>),
  ) -> (i64, Option<i64>) {
    let frame_pts = frame.pts();
    let opaque_seq = frame.opaque() as u64;
    let opaque_meta = if opaque_seq != 0 {
      guard.chunk_meta.get(&opaque_seq).copied()
    } else {
      None
    };

    // VP9/AV1 show_existing_frame packets re-emit a stored reference frame:
    // FFmpeg overrides pts/pkt_dts with the display packet's but the frame
    // keeps the reference's inherited opaque and duration (vp9.c
    // show_existing path). Such a frame carries an opaque tag whose chunk
    // entry is already exhausted (reference was visible) or whose timestamp
    // contradicts the frame PTS (reference was invisible). The inherited
    // frame.duration belongs to the reference chunk and must not be
    // trusted either. A tag with a live entry is never a re-emission: VP9
    // superframe constituents share one tag and entry across one output
    // each (tracked by ChunkMeta::remaining).
    let reemitted = opaque_seq != 0
      && match opaque_meta {
        Some(meta) => frame_pts != AV_NOPTS_VALUE && meta.timestamp != frame_pts,
        None => true,
      };

    if !reemitted && let Some(meta) = consume_chunk_meta(&mut guard.chunk_meta, opaque_seq) {
      return (meta.timestamp, meta.duration);
    }
    if frame_pts != AV_NOPTS_VALUE {
      // Match the oldest pending chunk carrying this PTS. FFmpeg-propagated
      // frame.duration reflects the same chunk, so prefer it when present —
      // except for re-emitted reference frames, where it is inherited from
      // the reference chunk and the matched chunk's duration always wins
      // (including explicit zero and None).
      let seq = guard
        .chunk_meta
        .iter()
        .find(|(_, meta)| meta.timestamp == frame_pts)
        .map(|(seq, _)| *seq);
      let mapped = seq
        .and_then(|s| consume_chunk_meta(&mut guard.chunk_meta, s))
        .and_then(|meta| meta.duration);
      let dur = if reemitted {
        mapped
      } else if frame.duration() > 0 {
        Some(frame.duration())
      } else {
        mapped
      };
      return (frame_pts, dur);
    }

    if let Some(&seq) = guard.chunk_meta.keys().next() {
      let meta = consume_chunk_meta(&mut guard.chunk_meta, seq);
      if let Some(meta) = meta {
        return (meta.timestamp, meta.duration);
      }
    }
    fallback
  }

  /// Report an error via callback and close the decoder
  fn report_error(inner: &mut VideoDecoderInner, error_msg: &str) {
    // Log the error at warn level for debugging (visible even if JS callback fails)
    tracing::warn!(target: "webcodecs", codec = "VideoDecoder", error = error_msg, "Codec error reported");

    // Create an Error object that will be passed directly to the JS callback
    let error = Error::new(Status::GenericFailure, error_msg);
    inner
      .error_callback
      .call(error, ThreadsafeFunctionCallMode::NonBlocking);
    inner.had_error = true;
    inner.state = CodecState::Closed;
  }

  /// Fire the dequeue event: one dispatcher call runs the whole dispatch on
  /// the JS thread so all listeners share a single Event object.
  fn fire_dequeue_event(event_state: &Arc<RwLock<CodecEventState>>) -> Result<()> {
    CodecEventState::fire(event_state);
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
    let mut state = self
      .event_state
      .write()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if callback.is_some() {
      state.ensure_dispatcher(env, this.object)?;
    }
    state.set_ondequeue(env, callback)?;
    Ok(())
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
  ///
  /// Implements Chromium-aligned hardware acceleration behavior:
  /// - `prefer-hardware`: Try hardware only, report error if fails
  /// - `no-preference`: Try hardware first, silently fall back to software
  /// - `prefer-software`: Use software only
  #[napi]
  pub fn configure(&mut self, env: Env, mut config: VideoDecoderConfig) -> Result<()> {
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

    // W3C spec: configure() sets [[key chunk required]] = true synchronously,
    // so a delta chunk decoded right after configure() must throw DataError
    // before the worker's reconfigure command runs.
    inner.keyframe_received = false;

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

      // Snapshot the description before queueing: it borrows the caller's
      // ArrayBuffer and is only copied in process_reconfigure() on the worker
      // thread, so a post-configure() write would race the applied extradata.
      if let Some(desc) = config.description.as_mut() {
        *desc = Uint8Array::new(desc.to_vec());
      }

      // Queue reconfigure via microtask (runs AFTER pending decode microtasks)
      // Don't update inner.config here - worker will do it after processing pending decodes
      // Use Weak reference to allow close() to immediately close channel without deadlock
      drop(inner); // Release lock before scheduling microtask
      if let Some(ref sender) = self.command_sender {
        let weak_sender = Arc::downgrade(sender);
        PromiseRaw::resolve(&env, ())?.then(move |_| {
          // Only send if decoder hasn't been closed (weak reference can still upgrade)
          if let Some(sender) = weak_sender.upgrade() {
            let _ = sender.send(WorkerCommand::Reconfigure(config));
          }
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
    let (mut context, is_hardware, hw_pix_fmt_raw) = if let Some(hw) = hw_type {
      // Hardware decoder requested (prefer-hardware only)
      match CodecContext::new_decoder_with_hw_info(codec_id, Some(hw)) {
        Ok(result) => (result.context, result.is_hardware, result.hw_pix_fmt_raw),
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
        Ok(ctx) => (ctx, false, None),
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to create decoder: {}", e));
          return Ok(());
        }
      }
    };

    // Handle extradata format based on decoder type:
    // - Hardware decoders (VideoToolbox, etc.) expect avcC/hvcC format (original container format)
    // - Software decoders expect Annex B format (start code prefixed NALUs)
    let extradata = config.description.as_ref().and_then(|d| {
      let data = d.to_vec();

      // For hardware decoding, keep extradata in original avcC/hvcC format
      // VideoToolbox parses avcC format directly to initialize the decoder session
      if is_hardware {
        return Some(data);
      }

      // For software decoding, convert to Annex B format
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
    // For hardware decoders, use single-threaded mode (thread_count=1) to avoid
    // race conditions during flush that can cause crashes with VideoToolbox and other
    // hardware accelerators. For software decoders, use auto-detect (thread_count=0)
    // for optimal performance.
    let thread_count = if is_hardware { 1 } else { 0 };
    let decoder_config = DecoderConfig {
      codec_id,
      thread_count,
      extradata,
      low_latency: config.optimize_for_latency.unwrap_or(false),
      width: config.coded_width,
      height: config.coded_height,
    };

    if let Err(e) = context.configure_decoder(&decoder_config) {
      Self::report_error(&mut inner, &format!("Failed to configure decoder: {}", e));
      return Ok(());
    }

    // Set up get_format callback for hardware decoding
    // This is required for FFmpeg to negotiate the correct pixel format with hardware decoders
    tracing::debug!(
      is_hardware = is_hardware,
      hw_type = ?hw_type,
      hw_pix_fmt_raw = ?hw_pix_fmt_raw,
      "Configuring decoder hardware acceleration"
    );
    if is_hardware && let Some(pix_fmt_raw) = hw_pix_fmt_raw {
      tracing::debug!(
        hw_pix_fmt_raw = pix_fmt_raw,
        "Setting get_format callback for hardware pixel format"
      );
      unsafe {
        ffctx_set_hw_get_format(context.as_mut_ptr(), pix_fmt_raw);
      }
    }

    // Open the decoder
    if let Err(e) = context.open() {
      Self::report_error(&mut inner, &format!("Failed to open decoder: {}", e));
      return Ok(());
    }

    // Log context state after opening for debugging
    if is_hardware {
      tracing::debug!(
        "Decoder opened (reconfigure): pix_fmt={:?}, width={}, height={}, is_hardware={}",
        context.pixel_format(),
        context.width(),
        context.height(),
        is_hardware
      );
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
      self.command_sender = Some(Arc::new(sender));
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
      if let Ok(mut es) = self.event_state.write() {
        es.note_enqueue(&env);
      }
    }

    // Send decode command to worker thread via microtask for W3C spec FIFO ordering
    // This ensures all commands (decode, configure, flush) are ordered correctly
    // Use Weak reference to allow close() to immediately close channel without deadlock
    if let Some(ref sender) = self.command_sender {
      let weak_sender = Arc::downgrade(sender);
      let reset_flag = self.reset_flag.clone();
      let chunk_inner = chunk.inner.clone();
      PromiseRaw::resolve(&env, ())?.then(move |_| {
        // Check reset flag first, then check if decoder hasn't been closed
        if !reset_flag.load(Ordering::SeqCst)
          && let Some(sender) = weak_sender.upgrade()
        {
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
    let (response_sender, response_receiver) = channel::bounded::<Result<()>>(1);

    // W3C spec: Check state upfront and return rejected promise with appropriate error
    // (not throw synchronously - flush() should always return a promise)
    let flush_id = {
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

      // W3C spec: flush() sets [[key chunk required]] = true synchronously,
      // so a delta chunk decoded after flush() must throw DataError.
      inner.keyframe_received = false;

      inner
        .flushes
        .register(response_sender.clone(), flush_abort_flag.clone())
    };

    // Send flush command through the channel (deferred to microtask for W3C spec compliance)
    // This ensures flush is processed after all pending decode microtasks complete (FIFO order)
    // Use Weak reference to allow close() to immediately close channel without deadlock
    if let Some(ref sender) = self.command_sender {
      let weak_sender = Arc::downgrade(sender);
      let reset_flag = self.reset_flag.clone();
      PromiseRaw::resolve(env, ())?.then(move |_| {
        // Check reset flag first, then check if decoder hasn't been closed
        // (flush Promise is already rejected with AbortError by reset())
        if !reset_flag.load(Ordering::SeqCst)
          && let Some(sender) = weak_sender.upgrade()
        {
          let _ = sender.send(WorkerCommand::Flush(response_sender));
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

        Ok((result, inner_clone, flush_abort_flag, flush_id))
      },
      move |env, (result, inner, abort_flag, flush_id)| {
        // Drain pending frames and call output callback SYNCHRONOUSLY
        // This runs on the main thread with Env access
        let frames = {
          let mut guard = inner
            .lock()
            .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
          guard.flushes.begin_output_delivery(flush_id);
          std::mem::take(&mut guard.pending_frames)
        };

        // Call output callback for each frame synchronously
        // If callback calls reset(), abort_flag will be set before next iteration
        let callback_result = (|| -> Result<()> {
          let callback = output_callback_ref.borrow_back(env)?;
          for frame in frames {
            // Check abort flag before each callback - exit early if reset() was called
            if abort_flag.load(Ordering::SeqCst) {
              break;
            }
            callback.call(frame)?;
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

        // Check abort flag after draining all frames
        if abort_flag.load(Ordering::SeqCst) {
          return Err(native_dom_exception_error(
            env,
            DOMExceptionName::AbortError,
            "The operation was aborted",
          )?);
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
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      // W3C spec: throw InvalidStateError if closed
      if inner.state == CodecState::Closed {
        return throw_invalid_state_error(&env, "Cannot reset a closed codec");
      }

      inner.flushes.abort_all();
    }

    // Set reset flag to signal worker to skip remaining pending decodes
    // This must be done BEFORE dropping the command sender
    self.reset_flag.store(true, Ordering::SeqCst);

    // Drop sender to signal worker to stop. Pending microtasks hold only Weak
    // references, so the channel disconnects after they observe reset_flag.
    // Do not join here: the old worker may be waiting on a JS callback.
    drop(self.command_sender.take());
    drop(self.worker_handle.take()); // Detach old worker thread

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
    let cleared_queue = inner.decode_queue_size as usize;
    inner.decode_queue_size = 0;
    if cleared_queue > 0
      && let Ok(mut es) = self.event_state.write()
    {
      es.note_queue_cleared(&env, cleared_queue);
    }
    inner.keyframe_received = false;
    inner.had_error = false;

    // Reset hardware tracking state
    inner.is_hardware = false;
    inner.hw_preference = HardwareAcceleration::NoPreference;
    inner.silent_decode_count = 0;
    inner.first_output_produced = false;
    inner.pending_chunks.clear();
    inner.chunk_meta.clear();

    // Clear flush-related state
    inner.flushes.clear();
    inner.pending_frames.clear();

    // Give the new worker a fresh cancellation token. Re-arming the old token
    // would allow a detached pre-reset worker to resume against new state.
    self.reset_flag = Arc::new(AtomicBool::new(false));

    // Create new channel and worker for future decode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(Arc::new(sender));
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
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        return throw_invalid_state_error(&env, "Cannot close an already closed codec");
      }

      inner.flushes.abort_all();
    }

    // Drop sender to stop accepting new commands and close channel.
    // With Weak references in microtasks, dropping Arc<Sender> immediately closes the channel
    // even if there are pending microtasks (they use Weak which can't keep the channel alive).
    self.command_sender = None;

    // Now safe to join worker - channel is closed, worker will see recv() Err and exit.
    // This prevents resource contention where old worker is still holding FFmpeg resources
    // while new decoder is being created.
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
    let cleared_queue = inner.decode_queue_size as usize;
    inner.decode_queue_size = 0;
    if cleared_queue > 0
      && let Ok(mut es) = self.event_state.write()
    {
      es.note_queue_cleared(&env, cleared_queue);
    }

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
  #[napi(
    ts_args_type = "eventType: string, callback: (event: Event) => unknown, options?: VideoDecoderAddEventListenerOptions | undefined | null"
  )]
  pub fn add_event_listener(
    &self,
    env: Env,
    this: This,
    event_type: String,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    options: Option<VideoDecoderAddEventListenerOptions>,
  ) -> Result<()> {
    let mut state = self
      .event_state
      .write()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    state.ensure_dispatcher(&env, this.object)?;
    let once = options.as_ref().and_then(|o| o.once).unwrap_or(false);
    let capture = options.as_ref().and_then(|o| o.capture).unwrap_or(false);
    state.add_listener(&env, &event_type, callback, once, capture)
  }

  /// Remove an event listener for the specified event type
  #[napi(
    ts_args_type = "eventType: string, callback: (event: Event) => unknown, options?: VideoDecoderEventListenerOptions | undefined | null"
  )]
  pub fn remove_event_listener(
    &self,
    env: Env,
    event_type: String,
    callback: FunctionRef<Unknown<'static>, UnknownReturnValue>,
    options: Option<VideoDecoderEventListenerOptions>,
  ) -> Result<()> {
    let mut state = self
      .event_state
      .write()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    let capture = options.as_ref().and_then(|o| o.capture).unwrap_or(false);
    state.remove_listener(&env, &event_type, &callback, capture);
    Ok(())
  }

  /// Dispatch an event to all registered listeners
  #[napi]
  pub fn dispatch_event(&self, env: Env, event_type: String) -> Result<bool> {
    crate::webcodecs::event_target::dispatch(&env, &self.event_state, &event_type);
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
/// VP9 uncompressed-header visibility flags, MSB-first within each byte:
/// frame_marker(2), profile(2, +1 when 3), show_existing_frame(1),
/// frame_type(1), show_frame(1). Returns (show_existing, show_frame), or
/// None for malformed or short headers — callers must treat those as visible.
fn vp9_visibility(data: &[u8]) -> Option<(bool, bool)> {
  let bit = |i: usize| -> Option<bool> { Some(data.get(i / 8)? & (0x80 >> (i % 8)) != 0) };
  if !bit(0)? || bit(1)? {
    return None; // frame marker must be 0b10
  }
  let profile = bit(2)? as u8 | (bit(3)? as u8) << 1;
  let mut pos = 4 + usize::from(profile == 3);
  let show_existing = bit(pos)?;
  pos += 1;
  if show_existing {
    // Re-displays a stored reference: a display event of its own.
    return Some((true, true));
  }
  let _frame_type = bit(pos)?;
  pos += 1;
  let show_frame = bit(pos)?;
  Some((false, show_frame))
}

/// VP9 superframe constituent ranges, per the vp9_superframe_split layout:
/// the packet ends with [marker][LE frame sizes][marker]; the marker holds
/// 0b110 in bits 7-5, (length_size-1) in bits 4-3 and (nb_frames-1) in
/// bits 2-0. None when the packet is not a well-formed superframe.
fn vp9_superframe_constituents(data: &[u8]) -> Option<Vec<(usize, usize)>> {
  let &marker = data.last()?;
  if marker & 0xe0 != 0xc0 {
    return None;
  }
  let length_size = 1 + ((marker as usize >> 3) & 0x3);
  let nb_frames = 1 + (marker as usize & 0x7);
  let idx_size = 2 + nb_frames * length_size;
  if data.len() < idx_size || data[data.len() - idx_size] != marker {
    return None;
  }
  let sizes_start = data.len() - idx_size + 1;
  let mut ranges = Vec::with_capacity(nb_frames);
  let mut offset = 0usize;
  for i in 0..nb_frames {
    let at = sizes_start + i * length_size;
    let mut size = 0usize;
    for (j, b) in data[at..at + length_size].iter().enumerate() {
      size |= (*b as usize) << (j * 8);
    }
    let end = offset.checked_add(size)?;
    if size == 0 || end > data.len() - idx_size {
      return None;
    }
    ranges.push((offset, end));
    offset = end;
  }
  Some(ranges)
}

/// Display events a chunk produces. VP9 chunks whose constituents are all
/// invisible (show_frame=0) produce none: they are stored as references and
/// only re-emitted by show_existing_frame packets. A VP9 superframe
/// produces one per visible or show_existing constituent
/// (vp9_superframe_split emits one packet per constituent, visible ones
/// carrying the chunk's opaque tag). Everything else produces one. Only VP9
/// is parsed: its visibility flags sit in the first byte of each frame, and
/// its decoder (vp9.c) is the one re-emitting stored references with
/// inherited metadata.
fn display_event_count(codec: &str, data: &[u8]) -> u32 {
  if codec != "vp9" && !codec.starts_with("vp09") {
    return 1;
  }
  let is_display = |frame: &[u8]| !matches!(vp9_visibility(frame), Some((false, false)));
  match vp9_superframe_constituents(data) {
    Some(ranges) => ranges
      .iter()
      .filter(|&&(start, end)| is_display(&data[start..end]))
      .count() as u32,
    None => u32::from(is_display(data)),
  }
}

fn decode_chunk_data(
  context: &mut CodecContext,
  data: &[u8],
  timestamp: i64,
  duration: Option<i64>,
  seq: u64,
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
  // Carry chunk identity through decoder reordering (opaque is also reset by
  // the unref in copy_data_from, so set it here too)
  packet.set_opaque(seq as usize);
  if let Some(dur) = duration {
    packet.set_duration(dur);
  }

  // Decode
  let frames = context
    .decode(Some(&packet))
    .map_err(|e| Error::new(Status::GenericFailure, format!("Decode failed: {}", e)))?;

  Ok(frames)
}
