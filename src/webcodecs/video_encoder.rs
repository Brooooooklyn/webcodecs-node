//! VideoEncoder - WebCodecs API implementation
//!
//! Provides video encoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videoencoder-interface

use crate::codec::{
  BitrateMode as CodecBitrateMode, CodecContext, EncoderConfig, EncoderCreationResult, Frame,
  HwDeviceContext, HwFrameConfig, HwFrameContext, Scaler,
};
use crate::ffi::{AVCodecID, AVHWDeviceType, AVPictureType, AVPixelFormat};
use crate::webcodecs::error::DOMExceptionName;
use crate::webcodecs::error::{throw_invalid_state_error, throw_type_error_unit};
use crate::webcodecs::hw_fallback::{
  is_hw_encoding_disabled, record_hw_encoding_failure, record_hw_encoding_success,
};
use crate::webcodecs::promise_reject::{reject_with_dom_exception_async, reject_with_type_error};
use crate::webcodecs::{
  AvcBitstreamFormat, EncodedVideoChunk, HardwareAcceleration, HevcBitstreamFormat, LatencyMode,
  VideoColorSpaceInit, VideoEncoderBitrateMode, VideoEncoderConfig, VideoFrame,
  convert_annexb_extradata_to_avcc, convert_annexb_extradata_to_hvcc,
  extract_avcc_from_avcc_packet, extract_hvcc_from_hvcc_packet,
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

/// Encoder state per WebCodecs spec
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecState {
  /// Encoder not configured
  #[default]
  #[napi(value = "unconfigured")]
  Unconfigured,
  /// Encoder configured and ready
  #[napi(value = "configured")]
  Configured,
  /// Encoder closed
  #[napi(value = "closed")]
  Closed,
}

/// SVC (Scalable Video Coding) output metadata (W3C WebCodecs spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct SvcOutputMetadata {
  /// Temporal layer ID for this frame
  pub temporal_layer_id: Option<u32>,
}

/// Output callback metadata per WebCodecs spec
#[napi(object)]
pub struct EncodedVideoChunkMetadata {
  /// Decoder configuration for this chunk (only present for keyframes)
  pub decoder_config: Option<VideoDecoderConfigOutput>,
  /// SVC metadata (temporal layer info)
  pub svc: Option<SvcOutputMetadata>,
  /// Alpha channel side data (when alpha option is "keep")
  pub alpha_side_data: Option<Uint8Array>,
}

/// Decoder configuration output (for passing to decoder)
#[napi(object)]
pub struct VideoDecoderConfigOutput {
  /// Codec string
  pub codec: String,
  /// Coded width
  pub coded_width: Option<u32>,
  /// Coded height
  pub coded_height: Option<u32>,
  /// Codec description (e.g., avcC for H.264) - Uint8Array per spec
  pub description: Option<Uint8Array>,
  /// Color space information for the video content
  pub color_space: Option<VideoColorSpaceInit>,
  /// Display aspect width (for non-square pixels)
  pub display_aspect_width: Option<u32>,
  /// Display aspect height (for non-square pixels)
  pub display_aspect_height: Option<u32>,
  /// Rotation in degrees clockwise (0, 90, 180, 270) per W3C spec
  pub rotation: Option<f64>,
  /// Horizontal flip per W3C spec
  pub flip: Option<bool>,
}

// ============================================================================
// Codec-Specific Encode Options (W3C WebCodecs Codec Registry)
// ============================================================================

/// AVC (H.264) encode options (W3C WebCodecs AVC Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoEncoderEncodeOptionsForAvc {
  /// Per-frame quantizer (0-51, lower = higher quality)
  pub quantizer: Option<u16>,
}

/// HEVC (H.265) encode options (W3C WebCodecs HEVC Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoEncoderEncodeOptionsForHevc {
  /// Per-frame quantizer (0-51, lower = higher quality)
  pub quantizer: Option<u16>,
}

/// VP9 encode options (W3C WebCodecs VP9 Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoEncoderEncodeOptionsForVp9 {
  /// Per-frame quantizer (0-63, lower = higher quality)
  pub quantizer: Option<u16>,
}

/// AV1 encode options (W3C WebCodecs AV1 Registration)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoEncoderEncodeOptionsForAv1 {
  /// Per-frame quantizer (0-63, lower = higher quality)
  pub quantizer: Option<u16>,
}

/// Encode options per WebCodecs spec
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoEncoderEncodeOptions {
  /// Force this frame to be a keyframe
  pub key_frame: Option<bool>,
  /// AVC (H.264) codec-specific options
  pub avc: Option<VideoEncoderEncodeOptionsForAvc>,
  /// HEVC (H.265) codec-specific options
  pub hevc: Option<VideoEncoderEncodeOptionsForHevc>,
  /// VP9 codec-specific options
  pub vp9: Option<VideoEncoderEncodeOptionsForVp9>,
  /// AV1 codec-specific options
  pub av1: Option<VideoEncoderEncodeOptionsForAv1>,
}

/// Result of isConfigSupported per WebCodecs spec
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoEncoderSupport {
  /// Whether the configuration is supported
  pub supported: bool,
  /// The configuration that was checked
  pub config: VideoEncoderConfig,
}

/// Output callback type - uses FnArgs to spread tuple members as separate callback arguments
/// This matches the WebCodecs spec: output(chunk, metadata) instead of output([chunk, metadata])
type OutputCallback = ThreadsafeFunction<
  FnArgs<(EncodedVideoChunk, EncodedVideoChunkMetadata)>,
  UnknownReturnValue,
  FnArgs<(EncodedVideoChunk, EncodedVideoChunkMetadata)>,
  Status,
  false,
  true,
>;

/// Type alias for error callback (takes Error object)
/// Using CalleeHandled: false because WebCodecs error callback receives Error directly,
/// not error-first (err, result) style
type ErrorCallback = ThreadsafeFunction<Error, UnknownReturnValue, Error, Status, false, true>;

// Note: For ondequeue, we use FunctionRef instead of ThreadsafeFunction
// to support both getter and setter per WebCodecs spec

/// Commands sent to the worker thread
enum EncoderCommand {
  /// Encode a video frame
  Encode {
    frame: Frame,
    timestamp: i64,
    options: Option<VideoEncoderEncodeOptions>,
    /// Rotation from input VideoFrame (for metadata output)
    rotation: f64,
    /// Flip from input VideoFrame (for metadata output)
    flip: bool,
  },
  /// Flush the encoder and send result back via response channel
  Flush(Sender<Result<()>>),
  /// Reconfigure the encoder with new config (W3C spec: control message)
  Reconfigure(VideoEncoderConfig),
}

/// VideoEncoder init dictionary per WebCodecs spec
pub struct VideoEncoderInit {
  /// Output callback - called when encoded chunk is available (ThreadsafeFunction for worker)
  pub output: OutputCallback,
  /// Output callback reference - stored for synchronous calls from main thread
  pub output_ref:
    FunctionRef<FnArgs<(EncodedVideoChunk, EncodedVideoChunkMetadata)>, UnknownReturnValue>,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for VideoEncoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // W3C spec: throw TypeError if required callbacks are missing
    // Get output callback as Function first, then create both FunctionRef and ThreadsafeFunction
    let output_func: Function<
      FnArgs<(EncodedVideoChunk, EncodedVideoChunkMetadata)>,
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

    Ok(VideoEncoderInit {
      output,
      output_ref,
      error,
    })
  }
}

/// Threshold for detecting silent encoder failure (no output after N frames)
const SILENT_FAILURE_THRESHOLD: u32 = 3;

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

/// State for EventTarget interface, separate from main encoder state
/// to avoid lock contention during encoding operations.
/// Uses RwLock so addEventListener doesn't block on encode operations.
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
pub struct AddEventListenerOptions {
  pub capture: Option<bool>,
  pub once: Option<bool>,
  pub passive: Option<bool>,
}

/// Options for removeEventListener (W3C DOM spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct EventListenerOptions {
  pub capture: Option<bool>,
}

/// Internal encoder state
struct VideoEncoderInner {
  state: CodecState,
  config: Option<VideoEncoderConfig>,
  context: Option<CodecContext>,
  scaler: Option<Scaler>,
  frame_count: u64,
  extradata_sent: bool,
  /// Number of pending encode operations (for encodeQueueSize)
  encode_queue_size: u32,
  /// Output callback (required per spec)
  output_callback: OutputCallback,
  /// Error callback (required per spec)
  error_callback: ErrorCallback,
  /// Pending flush response senders (for AbortError on reset)
  pending_flush_senders: Vec<Sender<Result<()>>>,
  /// Queue of input timestamps for correlation with output packets
  /// (needed because FFmpeg may buffer frames internally and reorder)
  timestamp_queue: std::collections::VecDeque<i64>,

  // ========================================================================
  // Hardware acceleration tracking (for Chromium-aligned fallback behavior)
  // ========================================================================
  /// Whether the encoder is using hardware acceleration
  is_hardware: bool,
  /// Name of the encoder (e.g., "h264_videotoolbox", "libx264")
  encoder_name: String,
  /// Hardware acceleration preference from config
  hw_preference: HardwareAcceleration,
  /// Count of consecutive encodes with no output (for silent failure detection)
  silent_encode_count: u32,
  /// Whether first output has been produced (disables silent failure detection after)
  first_output_produced: bool,
  /// Buffered frames during silent failure detection period (for re-encoding on fallback)
  /// Tuple: (Frame, timestamp, options, rotation, flip)
  pending_frames: Vec<(Frame, i64, Option<VideoEncoderEncodeOptions>, f64, bool)>,
  /// Atomic flag for flush abort - set by reset() to signal pending flush to abort
  flush_abort_flag: Option<Arc<AtomicBool>>,
  /// Queue of encoded chunks waiting to be delivered via output callback
  /// Worker pushes chunks here during flush; flush() drains them synchronously via FunctionRef
  pending_chunks: Vec<(EncodedVideoChunk, EncodedVideoChunkMetadata)>,
  /// Flag indicating whether a flush operation is in progress
  /// When true, worker queues chunks to pending_chunks instead of calling NonBlocking callback
  inside_flush: bool,

  // ========================================================================
  // Hardware frame context for zero-copy GPU encoding
  // ========================================================================
  /// Hardware device context (needed for creating frame context)
  hw_device_ctx: Option<HwDeviceContext>,
  /// Hardware frame context for GPU frame pool
  hw_frame_ctx: Option<HwFrameContext>,
  /// Whether to use hardware frame upload path
  use_hw_frames: bool,
  /// NV12 scaler for converting I420 to NV12 (required by most hardware encoders)
  nv12_scaler: Option<Scaler>,

  // ========================================================================
  // Temporal SVC (Scalable Video Coding) tracking
  // ========================================================================
  /// Number of temporal layers parsed from scalabilityMode (L1T2=2, L1T3=3)
  /// None for L1T1 (single temporal layer) or no SVC configured
  temporal_layer_count: Option<u32>,
  /// Counter for output frames used to compute temporal layer ID
  /// Reset on configure() and reset()
  output_frame_count: u64,

  // ========================================================================
  // Bitstream format conversion
  // ========================================================================
  /// Whether to convert H.264/H.265 output from Annex B to AVCC/HVCC format
  /// True for H.264 when avc.format is "avc" (default) or not specified
  /// True for H.265 when hevc.format is "hevc" (default) or not specified
  use_avcc_format: bool,

  // ========================================================================
  // Input colorSpace tracking (for decoder config output)
  // ========================================================================
  /// Color space from the first input frame (used in decoderConfig metadata)
  input_color_space: Option<VideoColorSpaceInit>,
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

/// VideoEncoder - WebCodecs-compliant video encoder
///
/// Encodes VideoFrame objects into EncodedVideoChunk objects using FFmpeg.
///
/// Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
///
/// Example:
/// ```javascript
/// const encoder = new VideoEncoder({
///   output: (chunk, metadata) => { console.log('encoded chunk', chunk); },
///   error: (e) => { console.error('error', e); }
/// });
///
/// encoder.configure({
///   codec: 'avc1.42001E',
///   width: 1920,
///   height: 1080,
///   bitrate: 5_000_000
/// });
///
/// encoder.encode(frame);
/// await encoder.flush();
/// ```
#[napi]
pub struct VideoEncoder {
  inner: Arc<Mutex<VideoEncoderInner>>,
  /// Separate lock for EventTarget state to avoid lock contention with encode operations.
  /// This allows addEventListener to complete immediately even when worker holds inner lock.
  event_state: Arc<RwLock<EventListenerState>>,
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
  /// Output callback reference - stored for synchronous calls from main thread (in flush resolver)
  /// Wrapped in Rc to allow sharing with spawn_future_with_callback closure
  /// (Rc is !Send but that's OK - the callback runs on the main thread)
  output_callback_ref:
    Rc<FunctionRef<FnArgs<(EncodedVideoChunk, EncodedVideoChunkMetadata)>, UnknownReturnValue>>,
  /// Channel sender for worker commands
  command_sender: Option<Sender<EncoderCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
  /// Reset abort flag - set by reset() to signal worker to skip pending encodes
  reset_flag: Arc<AtomicBool>,
}

impl Drop for VideoEncoder {
  fn drop(&mut self) {
    // Signal worker to stop
    self.command_sender = None;

    // Wait for worker to finish (brief block, necessary for safety)
    if let Some(handle) = self.worker_handle.take() {
      let _ = handle.join();
    }

    // Drain encoder to ensure libaom/AV1 threads finish before context drops.
    // This prevents SIGSEGV when avcodec_free_context is called while libaom
    // still has internal threads running.
    if let Ok(mut inner) = self.inner.lock()
      && let Some(ctx) = inner.context.as_mut()
    {
      // Flush internal buffers first - this synchronizes libaom's thread pool
      ctx.flush();
      let _ = ctx.send_frame(None);
      while ctx.receive_packet().ok().flatten().is_some() {}
    }
  }
}

#[napi]
impl VideoEncoder {
  /// Create a new VideoEncoder with init dictionary (per WebCodecs spec)
  ///
  /// @param init - Init dictionary containing output and error callbacks
  #[napi(constructor)]
  pub fn new(
    #[napi(
      ts_arg_type = "{ output: (chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata) => void, error: (error: Error) => void }"
    )]
    init: VideoEncoderInit,
  ) -> Result<Self> {
    let inner = VideoEncoderInner {
      state: CodecState::Unconfigured,
      config: None,
      context: None,
      scaler: None,
      frame_count: 0,
      extradata_sent: false,
      encode_queue_size: 0,
      output_callback: init.output,
      error_callback: init.error,
      pending_flush_senders: Vec::new(),
      timestamp_queue: std::collections::VecDeque::new(),
      // Hardware acceleration tracking
      is_hardware: false,
      encoder_name: String::new(),
      hw_preference: HardwareAcceleration::NoPreference,
      silent_encode_count: 0,
      first_output_produced: false,
      pending_frames: Vec::new(),
      flush_abort_flag: None,
      pending_chunks: Vec::new(),
      inside_flush: false,
      // Hardware frame context fields
      hw_device_ctx: None,
      hw_frame_ctx: None,
      use_hw_frames: false,
      nv12_scaler: None,
      // Temporal SVC tracking
      temporal_layer_count: None,
      output_frame_count: 0,
      // Bitstream format conversion (set during configure)
      use_avcc_format: false,
      // Input colorSpace tracking
      input_color_space: None,
    };

    let inner = Arc::new(Mutex::new(inner));

    // Create separate lock for event listener state (avoids contention with encode operations)
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
    inner: Arc<Mutex<VideoEncoderInner>>,
    event_state: Arc<RwLock<EventListenerState>>,
    receiver: Receiver<EncoderCommand>,
    reset_flag: Arc<AtomicBool>,
  ) {
    while let Ok(command) = receiver.recv() {
      // Check reset flag before processing each command
      // If reset() was called, skip remaining encode commands
      if reset_flag.load(Ordering::SeqCst) {
        // Still process flush commands to send responses, but skip encodes
        if let EncoderCommand::Flush(response_sender) = command {
          let _ = response_sender.send(Err(Error::new(
            Status::GenericFailure,
            "AbortError: The operation was aborted",
          )));
        } else {
          // For encode commands, just decrement queue and fire dequeue
          if let Ok(mut guard) = inner.lock() {
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(&event_state);
            }
          }
        }
        continue;
      }

      match command {
        EncoderCommand::Encode {
          frame,
          timestamp,
          options,
          rotation,
          flip,
        } => {
          Self::process_encode(
            &inner,
            &event_state,
            frame,
            timestamp,
            options,
            rotation,
            flip,
          );
        }
        EncoderCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner, &event_state);
          let _ = response_sender.send(result);
        }
        EncoderCommand::Reconfigure(config) => {
          Self::process_reconfigure(&inner, config);
        }
      }
    }
  }

  /// Process an encode command on the worker thread
  fn process_encode(
    inner: &Arc<Mutex<VideoEncoderInner>>,
    event_state: &Arc<RwLock<EventListenerState>>,
    frame: Frame,
    timestamp: i64,
    options: Option<VideoEncoderEncodeOptions>,
    rotation: f64,
    flip: bool,
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Check if encoder is still configured
    if guard.state != CodecState::Configured {
      let old_size = guard.encode_queue_size;
      guard.encode_queue_size = old_size.saturating_sub(1);
      if old_size > 0 {
        let _ = Self::fire_dequeue_event(event_state);
      }
      // Per W3C spec: "cease producing output" - silently discard pending work
      // State could be Unconfigured (reset called) or Closed (close called)
      // Don't call report_error() - that would set state to Closed and invoke error callback
      return;
    }

    // Get config info (unwrap validated config values)
    let (width, height, codec_string, display_width, display_height) = match guard.config.as_ref() {
      Some(config) => (
        config.width.unwrap_or(0),
        config.height.unwrap_or(0),
        config.codec.clone().unwrap_or_default(),
        config.display_width,
        config.display_height,
      ),
      None => {
        let old_size = guard.encode_queue_size;
        guard.encode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(event_state);
        }
        Self::report_error(&mut guard, "No encoder config");
        return;
      }
    };

    // Check if frame needs conversion
    let frame_format = frame.format();
    let needs_conversion =
      frame_format != AVPixelFormat::Yuv420p || frame.width() != width || frame.height() != height;

    // Convert frame if needed
    let mut frame_to_encode = if needs_conversion {
      // Create scaler if needed
      if guard.scaler.is_none() {
        match Scaler::new(
          frame.width(),
          frame.height(),
          frame_format,
          width,
          height,
          AVPixelFormat::Yuv420p,
          crate::codec::scaler::ScaleAlgorithm::Bilinear,
        ) {
          Ok(scaler) => guard.scaler = Some(scaler),
          Err(e) => {
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(event_state);
            }
            Self::report_error(&mut guard, &format!("Failed to create scaler: {}", e));
            return;
          }
        }
      }

      let scaler = guard.scaler.as_ref().unwrap();
      match scaler.scale_alloc(&frame) {
        Ok(scaled) => scaled,
        Err(e) => {
          let old_size = guard.encode_queue_size;
          guard.encode_queue_size = old_size.saturating_sub(1);
          if old_size > 0 {
            let _ = Self::fire_dequeue_event(event_state);
          }
          Self::report_error(&mut guard, &format!("Failed to scale frame: {}", e));
          return;
        }
      }
    } else {
      frame
    };

    // Set frame PTS
    frame_to_encode.set_pts(timestamp);

    // Force keyframe if requested via encode options (W3C WebCodecs spec)
    if options.as_ref().is_some_and(|o| o.key_frame == Some(true)) {
      frame_to_encode.set_pict_type(AVPictureType::I);
    }

    // Upload frame to GPU if hardware frame context is available
    // This provides zero-copy encoding for hardware encoders
    if guard.use_hw_frames && guard.hw_frame_ctx.is_some() {
      let hw_upload_result = Self::try_upload_to_gpu(&mut guard, &frame_to_encode);
      if let Some(hw_frame) = hw_upload_result {
        frame_to_encode = hw_frame;
      }
      // If upload failed, use_hw_frames is set to false and we continue with CPU frame
    }

    // Push timestamp to queue for correlation with output packets
    // (FFmpeg may modify PTS internally, so we track input timestamps separately)
    guard.timestamp_queue.push_back(timestamp);

    // Encode the frame
    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        let old_size = guard.encode_queue_size;
        guard.encode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(event_state);
        }
        Self::report_error(&mut guard, "No encoder context");
        return;
      }
    };

    let packets = match context.encode(Some(&frame_to_encode)) {
      Ok(pkts) => pkts,
      Err(e) => {
        // For hardware encoder with no-preference, try fallback to software
        if guard.is_hardware
          && !guard.first_output_produced
          && guard.hw_preference == HardwareAcceleration::NoPreference
        {
          // Buffer current frame for re-encoding
          if let Ok(cloned) = frame_to_encode.try_clone() {
            guard
              .pending_frames
              .push((cloned, timestamp, options.clone(), rotation, flip));
          }
          let pending_frames = std::mem::take(&mut guard.pending_frames);

          if Self::fallback_to_software(&mut guard) {
            // Re-encode all buffered frames with software encoder
            for (buffered_frame, buffered_ts, _buffered_opts, buffered_rotation, buffered_flip) in
              pending_frames
            {
              let mut frame_to_reencode = buffered_frame;
              frame_to_reencode.set_pts(buffered_ts);

              if let Some(ctx) = guard.context.as_mut()
                && let Ok(pkts) = ctx.encode(Some(&frame_to_reencode))
              {
                for packet in pkts {
                  // Use buffered_ts (the original input timestamp) instead of packet.pts()
                  let chunk = EncodedVideoChunk::from_packet_with_format(
                    &packet,
                    Some(buffered_ts),
                    guard.use_avcc_format,
                  );

                  // Create SVC metadata if temporal layers are configured
                  let svc =
                    create_svc_metadata(guard.temporal_layer_count, guard.output_frame_count);
                  guard.output_frame_count += 1;

                  let metadata = if !guard.extradata_sent && packet.is_key() {
                    guard.extradata_sent = true;
                    EncodedVideoChunkMetadata {
                      decoder_config: Some(VideoDecoderConfigOutput {
                        codec: codec_string.clone(),
                        coded_width: Some(width),
                        coded_height: Some(height),
                        description: guard
                          .context
                          .as_ref()
                          .and_then(|ctx| ctx.extradata().map(|d| Uint8Array::from(d.to_vec()))),
                        color_space: guard.input_color_space.clone(),
                        display_aspect_width: display_width,
                        display_aspect_height: display_height,
                        rotation: if buffered_rotation != 0.0 {
                          Some(buffered_rotation)
                        } else {
                          None
                        },
                        flip: if buffered_flip { Some(true) } else { None },
                      }),
                      svc,
                      alpha_side_data: None,
                    }
                  } else {
                    EncodedVideoChunkMetadata {
                      decoder_config: None,
                      svc,
                      alpha_side_data: None,
                    }
                  };
                  // During flush, queue chunks for synchronous delivery in resolver
                  // Otherwise, use Blocking callback for immediate delivery
                  if guard.inside_flush {
                    guard.pending_chunks.push((chunk, metadata));
                  } else {
                    guard.output_callback.call(
                      (chunk, metadata).into(),
                      ThreadsafeFunctionCallMode::Blocking,
                    );
                  }
                  guard.first_output_produced = true;
                }
              }
            }
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(event_state);
            }
            return;
          }
          // Fallback failed - report error with context
          let codec = guard
            .config
            .as_ref()
            .and_then(|c| c.codec.clone())
            .unwrap_or_else(|| "unknown".to_string());
          let encoder_name = guard.encoder_name.clone();
          Self::report_error(
            &mut guard,
            &format!(
              "OperationError: {} encoder ({}) failed: {} (software fallback also failed)",
              codec, encoder_name, e
            ),
          );
        } else {
          let codec = guard
            .config
            .as_ref()
            .and_then(|c| c.codec.clone())
            .unwrap_or_else(|| "unknown".to_string());
          let encoder_name = guard.encoder_name.clone();
          Self::report_error(
            &mut guard,
            &format!(
              "OperationError: {} encoder ({}) failed: {}",
              codec, encoder_name, e
            ),
          );
        }
        let old_size = guard.encode_queue_size;
        guard.encode_queue_size = old_size.saturating_sub(1);
        if old_size > 0 {
          let _ = Self::fire_dequeue_event(event_state);
        }
        return;
      }
    };

    guard.frame_count += 1;

    // ========================================================================
    // Silent failure detection (Chromium-aligned behavior)
    // ========================================================================
    // Some hardware encoders may be created successfully but fail to produce
    // output (e.g., in CI VMs where VideoToolbox is detected but doesn't work).
    // We detect this by tracking consecutive encodes with no output.
    //
    // IMPORTANT: This only applies to hardware encoders. Software encoders
    // naturally buffer frames (e.g., for B-frame reordering) and may not
    // produce output for the first few frames - this is expected behavior.

    if packets.is_empty() && guard.is_hardware && !guard.first_output_produced {
      // Buffer the frame for potential re-encoding on fallback
      // Use try_clone since Frame doesn't implement Clone
      if let Ok(cloned_frame) = frame_to_encode.try_clone() {
        guard
          .pending_frames
          .push((cloned_frame, timestamp, options.clone(), rotation, flip));
      }
      guard.silent_encode_count += 1;

      if guard.silent_encode_count >= SILENT_FAILURE_THRESHOLD {
        match guard.hw_preference {
          HardwareAcceleration::PreferHardware => {
            // prefer-hardware: Report error, no fallback
            // Record failure for global tracking
            record_hw_encoding_failure();
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(event_state);
            }
            let codec = guard
              .config
              .as_ref()
              .and_then(|c| c.codec.clone())
              .unwrap_or_else(|| "unknown".to_string());
            let encoder_name = guard.encoder_name.clone();
            Self::report_error(
              &mut guard,
              &format!(
                "OperationError: {} encoder ({}) failed to produce output (silent failure after {} frames)",
                codec, encoder_name, SILENT_FAILURE_THRESHOLD
              ),
            );
            return;
          }
          HardwareAcceleration::NoPreference => {
            // no-preference with hardware: Try to fall back to software
            let pending_frames = std::mem::take(&mut guard.pending_frames);

            if Self::fallback_to_software(&mut guard) {
              // Re-encode all buffered frames with software encoder
              for (buffered_frame, buffered_ts, _buffered_opts, buffered_rotation, buffered_flip) in
                pending_frames
              {
                let mut frame_to_reencode = buffered_frame;
                frame_to_reencode.set_pts(buffered_ts);

                if let Some(ctx) = guard.context.as_mut()
                  && let Ok(pkts) = ctx.encode(Some(&frame_to_reencode))
                {
                  // Process any output packets from re-encoding
                  for packet in pkts {
                    // Use buffered_ts (the original input timestamp) instead of packet.pts()
                    let chunk = EncodedVideoChunk::from_packet_with_format(
                      &packet,
                      Some(buffered_ts),
                      guard.use_avcc_format,
                    );

                    // Create SVC metadata if temporal layers are configured
                    let svc =
                      create_svc_metadata(guard.temporal_layer_count, guard.output_frame_count);
                    guard.output_frame_count += 1;

                    let metadata = if !guard.extradata_sent && packet.is_key() {
                      guard.extradata_sent = true;
                      EncodedVideoChunkMetadata {
                        decoder_config: Some(VideoDecoderConfigOutput {
                          codec: codec_string.clone(),
                          coded_width: Some(width),
                          coded_height: Some(height),
                          description: guard
                            .context
                            .as_ref()
                            .and_then(|ctx| ctx.extradata().map(|d| Uint8Array::from(d.to_vec()))),
                          color_space: guard.input_color_space.clone(),
                          display_aspect_width: display_width,
                          display_aspect_height: display_height,
                          rotation: if buffered_rotation != 0.0 {
                            Some(buffered_rotation)
                          } else {
                            None
                          },
                          flip: if buffered_flip { Some(true) } else { None },
                        }),
                        svc,
                        alpha_side_data: None,
                      }
                    } else {
                      EncodedVideoChunkMetadata {
                        decoder_config: None,
                        svc,
                        alpha_side_data: None,
                      }
                    };
                    // During flush, queue chunks for synchronous delivery in resolver
                    // Otherwise, use NonBlocking callback for immediate delivery
                    if guard.inside_flush {
                      guard.pending_chunks.push((chunk, metadata));
                    } else {
                      guard.output_callback.call(
                        (chunk, metadata).into(),
                        ThreadsafeFunctionCallMode::Blocking,
                      );
                    }
                    guard.first_output_produced = true;
                  }
                }
              }

              // Decrement queue size and continue
              let old_size = guard.encode_queue_size;
              guard.encode_queue_size = old_size.saturating_sub(1);
              if old_size > 0 {
                let _ = Self::fire_dequeue_event(event_state);
              }
              return;
            } else {
              // Fallback failed, report error
              // Record failure for global tracking
              record_hw_encoding_failure();
              let old_size = guard.encode_queue_size;
              guard.encode_queue_size = old_size.saturating_sub(1);
              if old_size > 0 {
                let _ = Self::fire_dequeue_event(event_state);
              }
              let codec = guard
                .config
                .as_ref()
                .and_then(|c| c.codec.clone())
                .unwrap_or_else(|| "unknown".to_string());
              let encoder_name = guard.encoder_name.clone();
              Self::report_error(
                &mut guard,
                &format!(
                  "OperationError: {} encoder ({}) failed (silent failure) and software fallback unavailable",
                  codec, encoder_name
                ),
              );
              return;
            }
          }
          HardwareAcceleration::PreferSoftware => {
            // This shouldn't happen (is_hardware should be false), but handle it anyway
          }
        }
      }
    } else if !packets.is_empty() {
      // Successfully produced output - mark first output and clear buffer
      guard.first_output_produced = true;
      guard.silent_encode_count = 0;
      guard.pending_frames.clear();

      // Record success for global tracking (resets failure count)
      if guard.is_hardware {
        record_hw_encoding_success();
      }
    }

    // Decrement queue size and fire dequeue event (only if queue was not empty)
    let old_size = guard.encode_queue_size;
    guard.encode_queue_size = old_size.saturating_sub(1);
    if old_size > 0 {
      let _ = Self::fire_dequeue_event(event_state);
    }

    // Process output packets - call callback for each
    for packet in packets {
      // Pop timestamp from queue to preserve original input timestamp
      // (FFmpeg may modify PTS internally during encoding)
      let output_timestamp = guard.timestamp_queue.pop_front();
      let chunk = EncodedVideoChunk::from_packet_with_format(
        &packet,
        output_timestamp,
        guard.use_avcc_format,
      );

      // Create SVC metadata if temporal layers are configured
      let svc = create_svc_metadata(guard.temporal_layer_count, guard.output_frame_count);
      guard.output_frame_count += 1;

      // Create metadata
      // Note: extradata must be fetched AFTER encoding, as FFmpeg only sets it after first encode
      // Only include decoder_config if we actually have extradata (for codecs that need it)
      let metadata = if !guard.extradata_sent && packet.is_key() {
        // Check codec type for format conversion
        let is_h264 = codec_string.starts_with("avc1")
          || codec_string.starts_with("avc3")
          || codec_string == "h264";
        let is_h265 = codec_string.starts_with("hvc1")
          || codec_string.starts_with("hev1")
          || codec_string == "h265";

        // Get extradata and optionally convert to avcC/hvcC format for proper AVCC/HVCC mode
        let description = guard.context.as_ref().and_then(|ctx| {
          ctx.extradata().and_then(|extradata| {
            if guard.use_avcc_format {
              // Convert Annex B extradata to avcC/hvcC box format
              if is_h264 {
                // Check if extradata is already in avcC format (starts with 0x01 = config version)
                // VideoToolbox produces avcC directly, libx264 produces Annex B
                if !extradata.is_empty() && extradata[0] == 0x01 {
                  Some(Uint8Array::from(extradata.to_vec()))
                } else {
                  convert_annexb_extradata_to_avcc(extradata).map(Uint8Array::from)
                }
              } else if is_h265 {
                // Check if extradata is already in hvcC format (starts with 0x01 = config version)
                // VideoToolbox produces hvcC directly, libx265 produces Annex B
                if !extradata.is_empty() && extradata[0] == 0x01 {
                  Some(Uint8Array::from(extradata.to_vec()))
                } else {
                  convert_annexb_extradata_to_hvcc(extradata).map(Uint8Array::from)
                }
              } else {
                Some(Uint8Array::from(extradata.to_vec()))
              }
            } else {
              // Annex B mode - use extradata as-is
              Some(Uint8Array::from(extradata.to_vec()))
            }
          })
        });

        // Fallback: If extradata is not available but we're in AVCC/HVCC mode,
        // try to extract SPS/PPS (and VPS for HEVC) from the packet data itself.
        // VideoToolbox embeds parameter sets inline in the first key frame packet
        // instead of populating the codec context's extradata field.
        let description = if description.is_none() && guard.use_avcc_format {
          if is_h264 {
            chunk
              .get_data()
              .and_then(|data| extract_avcc_from_avcc_packet(&data).map(Uint8Array::from))
          } else if is_h265 {
            chunk
              .get_data()
              .and_then(|data| extract_hvcc_from_hvcc_packet(&data).map(Uint8Array::from))
          } else {
            description
          }
        } else {
          description
        };

        // Determine if this codec requires description (H.264/H.265 in AVCC mode)
        // For codecs that require description, only send decoderConfig when description is available
        // For other codecs (VP8, VP9, AV1), description is optional - send decoderConfig immediately
        let requires_description = guard.use_avcc_format && (is_h264 || is_h265);

        if requires_description && description.is_none() {
          // H.264/H.265 needs description - don't send decoderConfig yet, try again on next key frame
          EncodedVideoChunkMetadata {
            decoder_config: None,
            svc,
            alpha_side_data: None,
          }
        } else {
          // Either we have description, or this codec doesn't require it
          guard.extradata_sent = true;
          EncodedVideoChunkMetadata {
            decoder_config: Some(VideoDecoderConfigOutput {
              codec: codec_string.clone(),
              coded_width: Some(width),
              coded_height: Some(height),
              description,
              color_space: guard.input_color_space.clone(),
              display_aspect_width: display_width,
              display_aspect_height: display_height,
              rotation: if rotation != 0.0 {
                Some(rotation)
              } else {
                None
              },
              flip: if flip { Some(true) } else { None },
            }),
            svc,
            alpha_side_data: None,
          }
        }
      } else {
        EncodedVideoChunkMetadata {
          decoder_config: None,
          svc,
          alpha_side_data: None,
        }
      };

      // During flush, queue chunks for synchronous delivery in resolver
      // Otherwise, use Blocking callback for immediate delivery
      if guard.inside_flush {
        guard.pending_chunks.push((chunk, metadata));
      } else {
        guard.output_callback.call(
          (chunk, metadata).into(),
          ThreadsafeFunctionCallMode::Blocking,
        );
      }
    }
  }

  /// Process a flush command on the worker thread
  fn process_flush(
    inner: &Arc<Mutex<VideoEncoderInner>>,
    _event_state: &Arc<RwLock<EventListenerState>>,
  ) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Per W3C spec: state check happens on main thread (in flush() method).
    // If state changed after that check (e.g., reconfigure failed), silently succeed.
    // The error callback has already been invoked by the failing operation.
    // This is consistent with process_encode() which silently discards when state is wrong.
    if guard.state != CodecState::Configured {
      return Ok(());
    }

    // If no frames have been encoded, skip flushing to avoid putting encoder in EOF state
    // (calling flush_encoder on empty encoder triggers EOF mode in FFmpeg which prevents
    // further encoding without context recreation)
    if guard.frame_count == 0 {
      return Ok(());
    }

    // Capture extradata BEFORE flush, as FFmpeg may clear it during drain mode
    // This is critical for when all output comes during flush (e.g., B-frame encoding)
    let cached_extradata = if !guard.extradata_sent {
      guard
        .context
        .as_ref()
        .and_then(|ctx| ctx.extradata().map(|e| e.to_vec()))
    } else {
      None
    };

    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut guard, "No encoder context");
        return Ok(());
      }
    };

    // Flush encoder
    let packets = match context.flush_encoder() {
      Ok(pkts) => pkts,
      Err(e) => {
        Self::report_error(&mut guard, &format!("Flush failed: {}", e));
        return Ok(());
      }
    };

    // Try to capture extradata again after flush - VideoToolbox may populate it now
    let cached_extradata = if cached_extradata.is_none() && !guard.extradata_sent {
      guard
        .context
        .as_ref()
        .and_then(|ctx| ctx.extradata().map(|e| e.to_vec()))
    } else {
      cached_extradata
    };

    // Queue remaining packets for synchronous delivery in resolver
    for packet in packets {
      // Pop timestamp from queue to preserve original input timestamp
      let output_timestamp = guard.timestamp_queue.pop_front();
      let chunk = EncodedVideoChunk::from_packet_with_format(
        &packet,
        output_timestamp,
        guard.use_avcc_format,
      );

      // Create SVC metadata if temporal layers are configured
      let svc = create_svc_metadata(guard.temporal_layer_count, guard.output_frame_count);
      guard.output_frame_count += 1;

      // Create metadata (include decoder_config if not sent yet and this is a key frame)
      let metadata = if !guard.extradata_sent && packet.is_key() {
        // Get config values for metadata
        let (codec_string, width, height, display_width, display_height) = guard
          .config
          .as_ref()
          .map_or((String::new(), 0, 0, None, None), |c| {
            (
              c.codec.clone().unwrap_or_default(),
              c.width.unwrap_or(0),
              c.height.unwrap_or(0),
              c.display_width,
              c.display_height,
            )
          });

        // Get extradata - first try cached (captured before flush), then try context
        // This handles the case where FFmpeg clears extradata during drain mode
        let extradata_source = cached_extradata
          .as_deref()
          .or_else(|| guard.context.as_ref().and_then(|ctx| ctx.extradata()));

        // Check codec type for format conversion
        let is_h264 = codec_string.starts_with("avc1")
          || codec_string.starts_with("avc3")
          || codec_string == "h264";
        let is_h265 = codec_string.starts_with("hvc1")
          || codec_string.starts_with("hev1")
          || codec_string == "h265";

        // Optionally convert to avcC/hvcC format for proper AVCC/HVCC mode
        let description = extradata_source.and_then(|extradata| {
          if guard.use_avcc_format {
            // Convert Annex B extradata to avcC/hvcC box format
            if is_h264 {
              // Check if extradata is already in avcC format (starts with 0x01 = config version)
              // VideoToolbox produces avcC directly, libx264 produces Annex B
              if !extradata.is_empty() && extradata[0] == 0x01 {
                Some(Uint8Array::from(extradata.to_vec()))
              } else {
                convert_annexb_extradata_to_avcc(extradata).map(Uint8Array::from)
              }
            } else if is_h265 {
              // Check if extradata is already in hvcC format (starts with 0x01 = config version)
              // VideoToolbox produces hvcC directly, libx265 produces Annex B
              if !extradata.is_empty() && extradata[0] == 0x01 {
                Some(Uint8Array::from(extradata.to_vec()))
              } else {
                convert_annexb_extradata_to_hvcc(extradata).map(Uint8Array::from)
              }
            } else {
              Some(Uint8Array::from(extradata.to_vec()))
            }
          } else {
            // Annex B mode - use extradata as-is
            Some(Uint8Array::from(extradata.to_vec()))
          }
        });

        // Fallback: If extradata is not available but we're in AVCC/HVCC mode,
        // try to extract SPS/PPS (and VPS for HEVC) from the packet data itself.
        // VideoToolbox embeds parameter sets inline in the first key frame packet
        // instead of populating the codec context's extradata field.
        let description = if description.is_none() && guard.use_avcc_format {
          if is_h264 {
            chunk
              .get_data()
              .and_then(|data| extract_avcc_from_avcc_packet(&data).map(Uint8Array::from))
          } else if is_h265 {
            chunk
              .get_data()
              .and_then(|data| extract_hvcc_from_hvcc_packet(&data).map(Uint8Array::from))
          } else {
            description
          }
        } else {
          description
        };

        // Determine if this codec requires description (H.264/H.265 in AVCC mode)
        // For codecs that require description, only send decoderConfig when description is available
        // For other codecs (VP8, VP9, AV1), description is optional - send decoderConfig immediately
        let requires_description = guard.use_avcc_format && (is_h264 || is_h265);

        if requires_description && description.is_none() {
          // H.264/H.265 needs description - don't send decoderConfig yet, try again on next key frame
          EncodedVideoChunkMetadata {
            decoder_config: None,
            svc,
            alpha_side_data: None,
          }
        } else {
          // Either we have description, or this codec doesn't require it
          guard.extradata_sent = true;
          EncodedVideoChunkMetadata {
            decoder_config: Some(VideoDecoderConfigOutput {
              codec: codec_string,
              coded_width: Some(width),
              coded_height: Some(height),
              description,
              color_space: guard.input_color_space.clone(),
              display_aspect_width: display_width,
              display_aspect_height: display_height,
              rotation: None,
              flip: None,
            }),
            svc,
            alpha_side_data: None,
          }
        }
      } else {
        EncodedVideoChunkMetadata {
          decoder_config: None,
          svc,
          alpha_side_data: None,
        }
      };

      // Always queue during flush for synchronous delivery
      guard.pending_chunks.push((chunk, metadata));
    }

    // Clear any remaining timestamps in queue after flush
    guard.timestamp_queue.clear();

    // Reset encoder state so it can accept more frames
    // Some encoders (like libvpx) don't properly support reuse after flush_encoder().
    // The encoder enters "EOF" state and avcodec_flush_buffers() doesn't always reset it.
    // Per W3C spec, flush() should leave encoder in configured state ready for new encodes.
    // We recreate the encoder context to ensure clean state.
    if let Some(ref config) = guard.config.clone() {
      // Get codec info from current config
      let codec_string = config.codec.clone().unwrap_or_default();
      if let Ok(codec_id) = parse_codec_string(&codec_string) {
        // Determine hardware type based on stored preference
        let hw_type = match guard.hw_preference {
          HardwareAcceleration::PreferHardware => Some(get_platform_hw_type()),
          HardwareAcceleration::NoPreference => {
            if is_hw_encoding_disabled() || !guard.is_hardware {
              None
            } else {
              Some(get_platform_hw_type())
            }
          }
          HardwareAcceleration::PreferSoftware => None,
        };

        // Recreate encoder context
        if let Ok(result) = CodecContext::new_encoder_with_hw_info(codec_id, hw_type) {
          let mut new_context = result.context;

          // Configure encoder with same settings
          let bitrate_mode = match config.bitrate_mode {
            Some(VideoEncoderBitrateMode::Constant) => CodecBitrateMode::Constant,
            Some(VideoEncoderBitrateMode::Variable) => CodecBitrateMode::Variable,
            Some(VideoEncoderBitrateMode::Quantizer) => CodecBitrateMode::Quantizer,
            None => CodecBitrateMode::Constant,
          };

          let (gop_size, max_b_frames) = match config.latency_mode {
            Some(LatencyMode::Realtime) => (10, 0),
            _ => (60, 2),
          };

          let encoder_config = EncoderConfig {
            width: config.width.unwrap_or(0),
            height: config.height.unwrap_or(0),
            pixel_format: AVPixelFormat::Yuv420p,
            bitrate: config.bitrate.unwrap_or(5_000_000.0) as u64,
            framerate_num: config.framerate.unwrap_or(30.0) as u32,
            framerate_den: 1,
            gop_size,
            max_b_frames,
            thread_count: 0,
            profile: None,
            level: None,
            bitrate_mode,
            rc_max_rate: None,
            rc_buffer_size: None,
            crf: None,
          };

          if new_context.configure_encoder(&encoder_config).is_ok() && new_context.open().is_ok() {
            // Drop old context and replace with new one
            guard.context = Some(new_context);
            guard.extradata_sent = false;
            guard.frame_count = 0;
          }
        }
      }
    }

    Ok(())
  }

  /// Process a reconfigure command on the worker thread
  /// Drains old context and creates new one with updated config
  fn process_reconfigure(inner: &Arc<Mutex<VideoEncoderInner>>, config: VideoEncoderConfig) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Don't reconfigure if encoder is closed
    if guard.state == CodecState::Closed {
      return;
    }

    // Drain old context (libaom/AV1 thread safety)
    if let Some(ctx) = guard.context.as_mut() {
      ctx.flush();
      let _ = ctx.send_frame(None);
      while ctx.receive_packet().ok().flatten().is_some() {}
    }

    // Clear work-related state
    guard.encode_queue_size = 0;
    guard.timestamp_queue.clear();
    guard.frame_count = 0;
    guard.extradata_sent = false;
    guard.output_frame_count = 0;
    guard.pending_frames.clear();

    // Parse codec to get codec_id
    let codec_string = match config.codec.as_ref() {
      Some(c) => c.clone(),
      None => {
        Self::report_error(&mut guard, "NotSupportedError: codec is required");
        return;
      }
    };

    let codec_id = match parse_codec_string(&codec_string) {
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
    let hw_type = match guard.hw_preference {
      HardwareAcceleration::PreferHardware => Some(get_platform_hw_type()),
      HardwareAcceleration::NoPreference => {
        if is_hw_encoding_disabled() {
          None
        } else {
          Some(get_platform_hw_type())
        }
      }
      HardwareAcceleration::PreferSoftware => None,
    };

    // Create encoder context
    let result = match CodecContext::new_encoder_with_hw_info(codec_id, hw_type) {
      Ok(r) => r,
      Err(e) => {
        Self::report_error(
          &mut guard,
          &format!("NotSupportedError: Failed to create encoder: {}", e),
        );
        return;
      }
    };

    let mut context = result.context;

    // Configure encoder
    let bitrate_mode = match config.bitrate_mode {
      Some(VideoEncoderBitrateMode::Constant) => CodecBitrateMode::Constant,
      Some(VideoEncoderBitrateMode::Variable) => CodecBitrateMode::Variable,
      Some(VideoEncoderBitrateMode::Quantizer) => CodecBitrateMode::Quantizer,
      None => CodecBitrateMode::Constant,
    };

    let (gop_size, max_b_frames) = match config.latency_mode {
      Some(LatencyMode::Realtime) => (10, 0),
      _ => (60, 2),
    };

    let encoder_config = EncoderConfig {
      width: config.width.unwrap_or(0),
      height: config.height.unwrap_or(0),
      pixel_format: AVPixelFormat::Yuv420p,
      bitrate: config.bitrate.unwrap_or(5_000_000.0) as u64,
      framerate_num: config.framerate.unwrap_or(30.0) as u32,
      framerate_den: 1,
      gop_size,
      max_b_frames,
      thread_count: 0,
      profile: None,
      level: None,
      bitrate_mode,
      rc_max_rate: None,
      rc_buffer_size: None,
      crf: None,
    };

    if let Err(e) = context.configure_encoder(&encoder_config) {
      Self::report_error(
        &mut guard,
        &format!("NotSupportedError: Failed to configure encoder: {}", e),
      );
      return;
    }

    // Determine if AVCC/HVCC format is needed (set GLOBAL_HEADER)
    let is_h264 = codec_string.starts_with("avc1")
      || codec_string.starts_with("avc3")
      || codec_string == "h264";
    let is_h265 = codec_string.starts_with("hvc1")
      || codec_string.starts_with("hev1")
      || codec_string == "h265";

    let use_avcc_format = if is_h264 {
      !matches!(
        config.avc.as_ref().and_then(|avc| avc.format),
        Some(AvcBitstreamFormat::Annexb)
      )
    } else if is_h265 {
      !matches!(
        config.hevc.as_ref().and_then(|hevc| hevc.format),
        Some(HevcBitstreamFormat::Annexb)
      )
    } else {
      false
    };

    if use_avcc_format {
      context.set_global_header();
    }

    if let Err(e) = context.open() {
      Self::report_error(
        &mut guard,
        &format!("NotSupportedError: Failed to open encoder: {}", e),
      );
      return;
    }

    // Update inner state
    guard.context = Some(context);
    guard.config = Some(config.clone());
    guard.is_hardware = result.is_hardware;
    guard.encoder_name = result.encoder_name;
    guard.use_avcc_format = use_avcc_format;

    // Parse temporal layer count from scalabilityMode
    guard.temporal_layer_count = config
      .scalability_mode
      .as_ref()
      .and_then(|mode| parse_temporal_layer_count(mode));

    // Clear hardware frame context (will be recreated if needed)
    guard.hw_device_ctx = None;
    guard.hw_frame_ctx = None;
    guard.use_hw_frames = false;
    guard.nv12_scaler = None;
  }

  /// Report an error via callback and close the encoder
  fn report_error(inner: &mut VideoEncoderInner, error_msg: &str) {
    // Create an Error object that will be passed directly to the JS callback
    let error = Error::new(Status::GenericFailure, error_msg);
    inner
      .error_callback
      .call(error, ThreadsafeFunctionCallMode::Blocking);
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

  /// Attempt to fall back to software encoder (for no-preference mode)
  ///
  /// This is called when hardware encoder silently fails (produces no output).
  /// Returns true if fallback succeeded, false otherwise.
  fn fallback_to_software(inner: &mut VideoEncoderInner) -> bool {
    let config = match inner.config.as_ref() {
      Some(c) => c,
      None => return false,
    };

    let codec_string = config.codec.clone().unwrap_or_default();
    let codec_id = match parse_codec_string(&codec_string) {
      Ok(id) => id,
      Err(_) => return false,
    };

    // Create software encoder (hw_type = None forces software)
    let result = match CodecContext::new_encoder_with_hw_info(codec_id, None) {
      Ok(r) => r,
      Err(_) => return false,
    };

    // Configure the new encoder with same settings
    let bitrate_mode = match config.bitrate_mode {
      Some(VideoEncoderBitrateMode::Constant) => CodecBitrateMode::Constant,
      Some(VideoEncoderBitrateMode::Variable) => CodecBitrateMode::Variable,
      Some(VideoEncoderBitrateMode::Quantizer) => CodecBitrateMode::Quantizer,
      None => CodecBitrateMode::Constant,
    };

    let (gop_size, max_b_frames) = match config.latency_mode {
      Some(LatencyMode::Realtime) => (10, 0),
      _ => (60, 2),
    };

    let encoder_config = EncoderConfig {
      width: config.width.unwrap_or(0),
      height: config.height.unwrap_or(0),
      pixel_format: AVPixelFormat::Yuv420p,
      bitrate: config.bitrate.unwrap_or(5_000_000.0) as u64,
      framerate_num: config.framerate.unwrap_or(30.0) as u32,
      framerate_den: 1,
      gop_size,
      max_b_frames,
      thread_count: 0,
      profile: None,
      level: None,
      bitrate_mode,
      rc_max_rate: None,
      rc_buffer_size: None,
      crf: None,
    };

    let mut context = result.context;
    if context.configure_encoder(&encoder_config).is_err() {
      return false;
    }

    if context.open().is_err() {
      return false;
    }

    // Replace the hardware context with software
    inner.context = Some(context);
    inner.is_hardware = false;
    inner.encoder_name = result.encoder_name;
    inner.silent_encode_count = 0;
    inner.first_output_produced = false;
    inner.extradata_sent = false;

    true
  }

  /// Create a software encoder with the given configuration
  ///
  /// Used for fallback when hardware encoder fails during configure/open.
  fn create_software_encoder(
    codec_id: AVCodecID,
    encoder_config: &EncoderConfig,
    needs_global_header: bool,
  ) -> Result<(CodecContext, String)> {
    let result = CodecContext::new_encoder_with_hw_info(codec_id, None).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create software encoder: {}", e),
      )
    })?;

    let mut context = result.context;

    context.configure_encoder(encoder_config).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to configure software encoder: {}", e),
      )
    })?;

    // Set GLOBAL_HEADER for AVCC/HVCC format output
    if needs_global_header {
      context.set_global_header();
    }

    context.open().map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to open software encoder: {}", e),
      )
    })?;

    Ok((context, result.encoder_name))
  }

  /// Try to create hardware frame context for zero-copy GPU encoding
  ///
  /// This creates a GPU frame pool that allows uploading CPU frames to GPU memory
  /// before encoding, which can improve performance for hardware encoders.
  ///
  /// Returns (HwDeviceContext, HwFrameContext) on success, or error if creation fails.
  /// Failure is not fatal - we can fall back to letting the encoder handle CPU frames.
  fn try_create_hw_frame_context(
    hw_type: AVHWDeviceType,
    width: u32,
    height: u32,
  ) -> Result<(HwDeviceContext, HwFrameContext)> {
    // Create hardware device context
    let device = HwDeviceContext::new(hw_type).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create hardware device context: {}", e),
      )
    })?;

    // Create hardware frames context with NV12 software format
    // Most hardware encoders prefer NV12 for input
    let config = HwFrameConfig {
      width,
      height,
      sw_format: AVPixelFormat::Nv12,
      hw_format: None, // Auto-detect based on device type
      pool_size: 20,   // Pre-allocate frames for smooth encoding
    };

    let frames = HwFrameContext::new(&device, config).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create hardware frames context: {}", e),
      )
    })?;

    Ok((device, frames))
  }

  /// Try to upload a CPU frame to GPU for hardware encoding
  ///
  /// Converts the frame to NV12 if needed (most hardware encoders prefer NV12),
  /// then uploads to GPU memory using the hardware frame context.
  ///
  /// Returns Some(hw_frame) on success, None on failure.
  /// On failure, sets guard.use_hw_frames to false for fallback.
  fn try_upload_to_gpu(guard: &mut VideoEncoderInner, frame: &Frame) -> Option<Frame> {
    // Convert to NV12 if needed
    let nv12_frame = if frame.format() != AVPixelFormat::Nv12 {
      // Create NV12 scaler if needed
      if guard.nv12_scaler.is_none() {
        match Scaler::new(
          frame.width(),
          frame.height(),
          frame.format(),
          frame.width(),
          frame.height(),
          AVPixelFormat::Nv12,
          crate::codec::scaler::ScaleAlgorithm::Bilinear,
        ) {
          Ok(scaler) => {
            guard.nv12_scaler = Some(scaler);
          }
          Err(e) => {
            guard.use_hw_frames = false;
            eprintln!(
              "Warning: Failed to create NV12 scaler, falling back to CPU frames: {}",
              e
            );
            return None;
          }
        }
      }

      // Scale to NV12
      let scaler = guard.nv12_scaler.as_ref()?;
      match scaler.scale_alloc(frame) {
        Ok(nv12) => nv12,
        Err(e) => {
          guard.use_hw_frames = false;
          eprintln!(
            "Warning: NV12 conversion failed, falling back to CPU frames: {}",
            e
          );
          return None;
        }
      }
    } else {
      // Already NV12, clone for upload
      match frame.try_clone() {
        Ok(cloned) => cloned,
        Err(e) => {
          guard.use_hw_frames = false;
          eprintln!(
            "Warning: Frame clone failed, falling back to CPU frames: {}",
            e
          );
          return None;
        }
      }
    };

    // Upload to GPU
    let hw_frame_ctx = guard.hw_frame_ctx.as_ref()?;
    match hw_frame_ctx.upload_frame(&nv12_frame) {
      Ok(hw_frame) => Some(hw_frame),
      Err(e) => {
        guard.use_hw_frames = false;
        eprintln!(
          "Warning: GPU upload failed, falling back to CPU frames: {}",
          e
        );
        None
      }
    }
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

  /// Configure the encoder
  #[napi]
  pub fn configure(&mut self, env: Env, config: VideoEncoderConfig) -> Result<()> {
    // W3C WebCodecs spec: Validate config synchronously, throw TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-videoencoder-configure

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      _ => return throw_type_error_unit(&env, "codec is required"),
    };

    // Validate width - must be present and greater than 0
    let width = match config.width {
      Some(w) if w > 0 => w,
      Some(_) => return throw_type_error_unit(&env, "width must be greater than 0"),
      None => return throw_type_error_unit(&env, "width is required"),
    };

    // Validate height - must be present and greater than 0
    let height = match config.height {
      Some(h) if h > 0 => h,
      Some(_) => return throw_type_error_unit(&env, "height must be greater than 0"),
      None => return throw_type_error_unit(&env, "height is required"),
    };

    // Validate display dimensions if specified
    if let Some(dw) = config.display_width
      && dw == 0
    {
      return throw_type_error_unit(&env, "displayWidth must be greater than 0");
    }
    if let Some(dh) = config.display_height
      && dh == 0
    {
      return throw_type_error_unit(&env, "displayHeight must be greater than 0");
    }

    // Validate bitrate if specified
    if let Some(bitrate) = config.bitrate
      && bitrate <= 0.0
    {
      return throw_type_error_unit(&env, "bitrate must be greater than 0");
    }

    // Validate framerate if specified
    if let Some(framerate) = config.framerate
      && framerate <= 0.0
    {
      return throw_type_error_unit(&env, "framerate must be greater than 0");
    }

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: throw InvalidStateError if closed
    if inner.state == CodecState::Closed {
      return throw_invalid_state_error(&env, "Encoder is closed");
    }

    // W3C spec: If already configured, queue reconfigure via microtask
    // This ensures FIFO ordering with pending encode commands
    if inner.state == CodecState::Configured {
      // Validate codec synchronously before queueing
      let _codec_id = match parse_codec_string(&codec) {
        Ok(id) => id,
        Err(e) => {
          Self::report_error(
            &mut inner,
            &format!("NotSupportedError: Invalid codec: {}", e),
          );
          return Ok(());
        }
      };

      // Validate scalability mode if specified
      if let Some(ref mode) = config.scalability_mode
        && !is_valid_scalability_mode(mode)
      {
        Self::report_error(
          &mut inner,
          &format!("NotSupportedError: Unsupported scalability mode: {}", mode),
        );
        return Ok(());
      }

      // Store config for immediate property reads and new encode validation
      inner.config = Some(config.clone());

      // Queue reconfigure via microtask (runs AFTER pending encode microtasks)
      drop(inner); // Release lock before scheduling microtask
      if let Some(ref sender) = self.command_sender {
        let sender = sender.clone();
        PromiseRaw::resolve(&env, ())?.then(move |_| {
          let _ = sender.send(EncoderCommand::Reconfigure(config));
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

    // Validate scalability mode if specified
    if let Some(ref mode) = config.scalability_mode
      && !is_valid_scalability_mode(mode)
    {
      Self::report_error(
        &mut inner,
        &format!("NotSupportedError: Unsupported scalability mode: {}", mode),
      );
      return Ok(());
    }

    // Validate dimensions are within reasonable limits
    if !are_dimensions_valid(width, height) {
      Self::report_error(
        &mut inner,
        "NotSupportedError: Dimensions exceed maximum supported size",
      );
      return Ok(());
    }

    // Calculate if GLOBAL_HEADER flag is needed for AVCC/HVCC format
    // This needs to be determined early since it's used in fallback paths
    // AVCC/HVCC is the W3C default - only disable when Annex B is explicitly requested
    let needs_global_header = {
      let is_h264 = codec.starts_with("avc1") || codec.starts_with("avc3") || codec == "h264";
      let is_h265 = codec.starts_with("hvc1") || codec.starts_with("hev1") || codec == "h265";

      if is_h264 {
        // For H.264: need global header unless Annex B is explicitly requested (W3C default is AVCC)
        !matches!(
          config.avc.as_ref().and_then(|avc| avc.format),
          Some(AvcBitstreamFormat::Annexb)
        )
      } else if is_h265 {
        // For H.265: need global header unless Annex B is explicitly requested (W3C default is HVCC)
        !matches!(
          config.hevc.as_ref().and_then(|hevc| hevc.format),
          Some(HevcBitstreamFormat::Annexb)
        )
      } else {
        false
      }
    };

    // Determine hardware acceleration preference (Chromium-aligned behavior)
    let hw_preference = config
      .hardware_acceleration
      .unwrap_or(HardwareAcceleration::NoPreference);

    // Determine hardware type based on preference:
    // - prefer-hardware: Try hardware only, error if fails
    // - no-preference: Try hardware first, fallback to software at runtime
    // - prefer-software: Use software only
    //
    // Also check if hardware encoding is globally disabled due to repeated failures
    let hw_type = match &hw_preference {
      HardwareAcceleration::PreferHardware => {
        // prefer-hardware: Always try hardware (even if globally disabled, per user request)
        Some(get_platform_hw_type())
      }
      HardwareAcceleration::NoPreference => {
        // no-preference: Use hardware unless globally disabled
        if is_hw_encoding_disabled() {
          None // Skip hardware, use software
        } else {
          Some(get_platform_hw_type())
        }
      }
      HardwareAcceleration::PreferSoftware => None,
    };

    // Create encoder context with hardware acceleration info
    let EncoderCreationResult {
      mut context,
      mut is_hardware,
      mut encoder_name,
    } = match CodecContext::new_encoder_with_hw_info(codec_id, hw_type) {
      Ok(result) => result,
      Err(e) => {
        // For no-preference, try again with software only if HW failed at creation
        if hw_preference == HardwareAcceleration::NoPreference {
          match CodecContext::new_encoder_with_hw_info(codec_id, None) {
            Ok(result) => result,
            Err(e2) => {
              Self::report_error(&mut inner, &format!("Failed to create encoder: {}", e2));
              return Ok(());
            }
          }
        } else {
          Self::report_error(&mut inner, &format!("Failed to create encoder: {}", e));
          return Ok(());
        }
      }
    };

    // Convert WebCodecs bitrate mode to internal codec bitrate mode
    let bitrate_mode = match config.bitrate_mode {
      Some(VideoEncoderBitrateMode::Constant) => CodecBitrateMode::Constant,
      Some(VideoEncoderBitrateMode::Variable) => CodecBitrateMode::Variable,
      Some(VideoEncoderBitrateMode::Quantizer) => CodecBitrateMode::Quantizer,
      None => CodecBitrateMode::Constant, // Default to CBR
    };

    // Parse latency mode: "realtime" = low latency, "quality" = default quality mode
    let (gop_size, max_b_frames) = match config.latency_mode {
      Some(LatencyMode::Realtime) => (10, 0), // Low latency: small GOP, no B-frames
      _ => (60, 2),                           // Quality mode: larger GOP with B-frames
    };

    // Configure encoder
    let encoder_config = EncoderConfig {
      width,
      height,
      pixel_format: AVPixelFormat::Yuv420p, // Most encoders need YUV420p
      bitrate: config.bitrate.unwrap_or(5_000_000.0) as u64,
      framerate_num: config.framerate.unwrap_or(30.0) as u32,
      framerate_den: 1,
      gop_size,
      max_b_frames,
      thread_count: 0, // Auto
      profile: None,
      level: None,
      bitrate_mode,
      rc_max_rate: None,
      rc_buffer_size: None,
      crf: None,
    };

    if let Err(e) = context.configure_encoder(&encoder_config) {
      // For no-preference, try software fallback if hardware configure fails
      if hw_preference == HardwareAcceleration::NoPreference && is_hardware {
        match Self::create_software_encoder(codec_id, &encoder_config, needs_global_header) {
          Ok((sw_ctx, sw_name)) => {
            context = sw_ctx;
            is_hardware = false;
            encoder_name = sw_name;
          }
          Err(e2) => {
            Self::report_error(
              &mut inner,
              &format!(
                "Failed to configure encoder: {} (software fallback also failed: {})",
                e, e2
              ),
            );
            return Ok(());
          }
        }
      } else {
        Self::report_error(&mut inner, &format!("Failed to configure encoder: {}", e));
        return Ok(());
      }
    }

    // Apply hardware encoder-specific options based on latency mode
    // This sets sensible defaults for each hardware encoder (VideoToolbox, NVENC, VAAPI, QSV)
    // based on whether the user requested realtime (low-latency) or quality mode
    if is_hardware {
      let realtime = matches!(config.latency_mode, Some(LatencyMode::Realtime));
      context.apply_hw_encoder_options(&encoder_name, realtime);
    }

    // Set GLOBAL_HEADER flag for AVCC/HVCC format output
    // This puts SPS/PPS into extradata instead of embedding in keyframes
    if needs_global_header {
      context.set_global_header();
    }

    // Open the encoder
    if let Err(e) = context.open() {
      // For no-preference, try software fallback if hardware open fails
      if hw_preference == HardwareAcceleration::NoPreference && is_hardware {
        match Self::create_software_encoder(codec_id, &encoder_config, needs_global_header) {
          Ok((sw_ctx, sw_name)) => {
            context = sw_ctx;
            is_hardware = false;
            encoder_name = sw_name;
          }
          Err(e2) => {
            Self::report_error(
              &mut inner,
              &format!(
                "Failed to open encoder: {} (software fallback also failed: {})",
                e, e2
              ),
            );
            return Ok(());
          }
        }
      } else {
        Self::report_error(&mut inner, &format!("Failed to open encoder: {}", e));
        return Ok(());
      }
    }

    // Try to create hardware frame context for zero-copy GPU encoding
    // This is optional - if it fails, we fall back to CPU frames (current behavior)
    let (hw_device_ctx, hw_frame_ctx, use_hw_frames) = if is_hardware {
      if let Some(hw) = hw_type {
        match Self::try_create_hw_frame_context(hw, width, height) {
          Ok((device, frames)) => (Some(device), Some(frames), true),
          Err(_) => {
            // Failed to create hw frame context, fall back to CPU frames
            // This is not an error - hardware encoders can accept CPU frames too
            (None, None, false)
          }
        }
      } else {
        (None, None, false)
      }
    } else {
      (None, None, false)
    };

    inner.context = Some(context);
    inner.config = Some(config);
    inner.state = CodecState::Configured;
    inner.extradata_sent = false;
    inner.frame_count = 0;
    inner.encode_queue_size = 0;

    // Hardware acceleration tracking
    inner.is_hardware = is_hardware;
    inner.encoder_name = encoder_name;
    inner.hw_preference = hw_preference;
    inner.silent_encode_count = 0;
    inner.first_output_produced = false;
    inner.pending_frames.clear();

    // Hardware frame context for zero-copy GPU encoding
    inner.hw_device_ctx = hw_device_ctx;
    inner.hw_frame_ctx = hw_frame_ctx;
    inner.use_hw_frames = use_hw_frames;
    inner.nv12_scaler = None; // Will be created lazily if needed

    // Temporal SVC tracking - parse layer count from scalabilityMode
    inner.temporal_layer_count = inner
      .config
      .as_ref()
      .and_then(|c| c.scalability_mode.as_ref())
      .and_then(|mode| parse_temporal_layer_count(mode));
    inner.output_frame_count = 0;

    // Bitstream format conversion - determine if AVCC/HVCC format is needed
    // W3C spec: Default is AVCC/HVCC format (length-prefixed NAL units)
    // Use Annex B only if explicitly requested via avc.format or hevc.format
    inner.use_avcc_format = inner.config.as_ref().is_some_and(|c| {
      let codec_str = c.codec.as_deref().unwrap_or("");
      let is_h264 =
        codec_str.starts_with("avc1") || codec_str.starts_with("avc3") || codec_str == "h264";
      let is_h265 =
        codec_str.starts_with("hvc1") || codec_str.starts_with("hev1") || codec_str == "h265";

      if is_h264 {
        // For H.264: use AVCC unless explicitly set to Annex B
        !matches!(
          c.avc.as_ref().and_then(|avc| avc.format),
          Some(AvcBitstreamFormat::Annexb)
        )
      } else if is_h265 {
        // For H.265: use HVCC unless explicitly set to Annex B
        !matches!(
          c.hevc.as_ref().and_then(|hevc| hevc.format),
          Some(HevcBitstreamFormat::Annexb)
        )
      } else {
        false // Other codecs don't need conversion
      }
    });

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

  /// Encode a frame
  #[napi]
  pub fn encode(
    &self,
    env: Env,
    frame: &VideoFrame,
    options: Option<VideoEncoderEncodeOptions>,
  ) -> Result<()> {
    // W3C spec: throw TypeError if frame is closed
    if frame.closed()? {
      return throw_type_error_unit(&env, "Cannot encode a closed VideoFrame");
    }

    // Clone frame and get timestamp on main thread (brief lock)
    let (internal_frame, timestamp, rotation, flip) = {
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

      // Clone frame data from VideoFrame
      let internal_frame = match frame.with_frame(|f| f.try_clone()) {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => {
          Self::report_error(&mut inner, &format!("Failed to clone frame: {}", e));
          return Ok(());
        }
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to access frame: {}", e));
          return Ok(());
        }
      };

      // Get timestamp
      let timestamp = match frame.timestamp() {
        Ok(ts) => ts,
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get frame timestamp: {}", e));
          return Ok(());
        }
      };

      // Get rotation and flip for metadata output (W3C WebCodecs spec)
      let rotation = frame.rotation().unwrap_or(0.0);
      let flip = frame.flip().unwrap_or(false);

      // Capture colorSpace from first input frame (for decoderConfig metadata)
      if inner.input_color_space.is_none()
        && let Ok(color_space) = frame.color_space()
      {
        inner.input_color_space = Some(color_space.to_init());
      }

      // Increment queue size (pending operation)
      inner.encode_queue_size += 1;

      (internal_frame, timestamp, rotation, flip)
    };

    // Send encode command to worker thread via microtask for W3C spec FIFO ordering
    // This ensures all commands (encode, configure, flush) are ordered correctly
    if let Some(ref sender) = self.command_sender {
      let sender = sender.clone();
      let reset_flag = self.reset_flag.clone();
      PromiseRaw::resolve(&env, ())?.then(move |_| {
        // Check reset flag - if reset() was called, skip sending
        if !reset_flag.load(Ordering::SeqCst) {
          let _ = sender.send(EncoderCommand::Encode {
            frame: internal_frame,
            timestamp,
            options,
            rotation,
            flip,
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
    let flush_abort_flag = Arc::new(AtomicBool::new(false));

    // W3C spec: Check state upfront and return rejected promise with appropriate error
    // (not throw synchronously - flush() should always return a promise)
    {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        // Return rejected promise with native DOMException (async to allow error callback to run)
        return reject_with_dom_exception_async(
          env,
          DOMExceptionName::InvalidStateError,
          "Cannot flush a closed codec",
        );
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
      // Set inside_flush flag so worker queues chunks instead of calling NonBlocking callback
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
    // This ensures flush is processed after all pending encode microtasks complete (FIFO order)
    if let Some(ref sender) = self.command_sender {
      let sender = sender.clone();
      let reset_flag = self.reset_flag.clone();
      PromiseRaw::resolve(env, ())?.then(move |_| {
        // Check reset flag - if reset() was called, skip sending
        // (flush Promise is already rejected with AbortError by reset())
        if !reset_flag.load(Ordering::SeqCst) {
          let _ = sender.send(EncoderCommand::Flush(response_sender));
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

        // Return worker result (errors keep DOMException-style message for now)
        result
      },
    )
  }

  /// Reset the encoder
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

    // Set reset flag to signal worker to skip remaining pending encodes
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
    // Note: Don't join - microtasks might still hold cloned senders. After reset()
    // returns, microtasks run, see reset_flag=true, skip sending, and drop senders.
    // Channel then disconnects and worker exits naturally.
    drop(self.command_sender.take());
    drop(self.worker_handle.take());

    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Drain encoder before dropping to ensure libaom/AV1 threads finish
    if let Some(ctx) = inner.context.as_mut() {
      ctx.flush();
      let _ = ctx.send_frame(None);
      while ctx.receive_packet().ok().flatten().is_some() {}
    }

    // Drop existing context
    inner.context = None;
    inner.scaler = None;
    inner.config = None;
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    inner.extradata_sent = false;
    inner.encode_queue_size = 0;

    // Reset hardware tracking state
    inner.is_hardware = false;
    inner.encoder_name = String::new();
    inner.hw_preference = HardwareAcceleration::NoPreference;
    inner.silent_encode_count = 0;
    inner.first_output_produced = false;
    inner.pending_frames.clear();
    inner.timestamp_queue.clear();

    // Reset temporal SVC tracking
    inner.temporal_layer_count = None;
    inner.output_frame_count = 0;

    // Reset bitstream format conversion
    inner.use_avcc_format = false;

    // Clear flush-related state
    inner.inside_flush = false;
    inner.pending_chunks.clear();

    // Reset the abort flag for new worker
    self.reset_flag.store(false, Ordering::SeqCst);

    // Create new channel and worker for future encode operations
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

    Ok(())
  }

  /// Close the encoder
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

    // Drain encoder before dropping to ensure libaom/AV1 threads finish
    // This prevents SIGSEGV crashes during cleanup
    if let Some(ctx) = inner.context.as_mut() {
      // Flush internal buffers first - this synchronizes libaom's thread pool
      ctx.flush();
      // Send NULL frame to signal end of stream
      let _ = ctx.send_frame(None);
      // Drain all remaining packets
      while ctx.receive_packet().ok().flatten().is_some() {}
    }

    inner.context = None;
    inner.scaler = None;
    inner.config = None;
    inner.state = CodecState::Closed;
    inner.encode_queue_size = 0;

    Ok(())
  }

  // ============================================================================
  // EventTarget interface (W3C DOM spec)
  // ============================================================================

  /// Add an event listener for the specified event type
  /// Uses separate RwLock to avoid blocking on encode operations
  #[napi]
  pub fn add_event_listener(
    &self,
    env: Env,
    event_type: String,
    callback: FunctionRef<(), UnknownReturnValue>,
    options: Option<AddEventListenerOptions>,
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
    _options: Option<EventListenerOptions>,
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

  /// Check if a configuration is supported
  /// Returns a Promise that resolves with support information
  ///
  /// W3C WebCodecs spec: Throws TypeError for invalid configs,
  /// returns { supported: false } for valid but unsupported configs.
  ///
  /// Note: The config parameter is validated via FromNapiValue which throws
  /// native TypeError for missing required fields.
  #[napi]
  pub fn is_config_supported<'env>(
    env: &'env Env,
    config: VideoEncoderConfig,
  ) -> Result<PromiseRaw<'env, VideoEncoderSupport>> {
    // W3C WebCodecs spec: Validate config
    // - TypeError for missing required fields
    // - Return { supported: false } for invalid values
    // https://w3c.github.io/webcodecs/#dom-videoencoder-isconfigsupported

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      Some(_) => return reject_with_type_error(env, "codec is required"),
      None => return reject_with_type_error(env, "codec is required"),
    };

    // Validate width - must be present (value checked in async block)
    if config.width.is_none() {
      return reject_with_type_error(env, "width is required");
    }

    // Validate height - must be present and non-zero
    if config.height.is_none() {
      return reject_with_type_error(env, "height is required");
    }

    // W3C spec: TypeError for structurally invalid configs (zero values)
    // These must be checked BEFORE async block to throw synchronously
    if let Some(w) = config.width
      && w == 0
    {
      return reject_with_type_error(env, "width must be non-zero");
    }

    if let Some(h) = config.height
      && h == 0
    {
      return reject_with_type_error(env, "height must be non-zero");
    }

    if let Some(dw) = config.display_width
      && dw == 0
    {
      return reject_with_type_error(env, "displayWidth must be non-zero");
    }

    if let Some(dh) = config.display_height
      && dh == 0
    {
      return reject_with_type_error(env, "displayHeight must be non-zero");
    }

    if let Some(bitrate) = config.bitrate
      && bitrate <= 0.0
    {
      return reject_with_type_error(env, "bitrate must be positive");
    }

    env.spawn_future(async move {
      // Validate framerate if specified (return { supported: false } not TypeError)
      if let Some(framerate) = config.framerate
        && framerate <= 0.0
      {
        return Ok(VideoEncoderSupport {
          supported: false,
          config,
        });
      }

      // Validate dimensions range
      let width = config.width.unwrap_or(0);
      let height = config.height.unwrap_or(0);
      if !are_dimensions_valid(width, height) {
        return Ok(VideoEncoderSupport {
          supported: false,
          config,
        });
      }

      // Validate scalability mode if specified
      if let Some(ref mode) = config.scalability_mode
        && !is_valid_scalability_mode(mode)
      {
        return Ok(VideoEncoderSupport {
          supported: false,
          config,
        });
      }

      // Parse codec string
      let codec_id = match parse_codec_string(&codec) {
        Ok(id) => id,
        Err(_) => {
          return Ok(VideoEncoderSupport {
            supported: false,
            config,
          });
        }
      };

      // Try to create encoder
      let result = CodecContext::new_encoder(codec_id);

      Ok(VideoEncoderSupport {
        supported: result.is_ok(),
        config,
      })
    })
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

/// Maximum dimension (width/height) for encoder
const MAX_DIMENSION: u32 = 16384;

/// Valid scalability modes per W3C WebCodecs spec
const VALID_SCALABILITY_MODES: &[&str] = &[
  "L1T1", "L1T2", "L1T3", "L2T1", "L2T2", "L2T3", "L3T1", "L3T2", "L3T3", "L2T1h", "L2T2h",
  "L2T3h", "L3T1h", "L3T2h", "L3T3h", "S2T1", "S2T2", "S2T3", "S2T1h", "S2T2h", "S2T3h", "S3T1",
  "S3T2", "S3T3", "S3T1h", "S3T2h", "S3T3h", "L2T1_KEY", "L2T2_KEY", "L2T3_KEY", "L3T1_KEY",
  "L3T2_KEY", "L3T3_KEY",
];

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
fn validate_vp9_codec(codec: &str) -> bool {
  if codec == "vp9" {
    return true; // Short form is valid
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

/// Validate scalability mode
fn is_valid_scalability_mode(mode: &str) -> bool {
  VALID_SCALABILITY_MODES.contains(&mode)
}

/// Parse temporal layer count from scalability mode string.
/// Returns Some(n) for L1Tx modes where x >= 2, None otherwise.
/// Only L1Tx (single spatial layer) modes are supported for temporal layer metadata.
fn parse_temporal_layer_count(mode: &str) -> Option<u32> {
  // Only support L1Tx modes (single spatial layer with temporal layers)
  // Pattern: L1T<n> where n >= 2
  mode
    .strip_prefix("L1T")
    .and_then(|suffix| suffix.parse::<u32>().ok())
    .filter(|&n| n >= 2)
}

/// Compute temporal layer ID for a given frame index based on temporal layer count.
///
/// Temporal layer patterns (frame index -> layer):
/// - L1T2 (2 layers): [0, 1, 0, 1, 0, 1, ...]
/// - L1T3 (3 layers): [0, 2, 1, 2, 0, 2, 1, 2, ...]
///
/// The pattern for L1Tx follows these rules:
/// - Layer 0 (base): every 2^(T-1) frames starting at 0
/// - Layer T-1 (highest enhancement): odd frames
/// - Layer k (1 <= k < T-1): frames at 2^(T-1-k) + n*2^(T-k)
fn compute_temporal_layer_id(frame_index: u64, temporal_layers: u32) -> u32 {
  if temporal_layers <= 1 {
    return 0;
  }

  // For L1T2: period = 2, pattern [0, 1]
  // For L1T3: period = 4, pattern [0, 2, 1, 2]
  // General: period = 2^(T-1)
  let period = 1u64 << (temporal_layers - 1);
  let pos = frame_index % period;

  if pos == 0 {
    // Base layer (layer 0)
    return 0;
  }

  // Find the highest power of 2 that divides pos
  // This determines which enhancement layer
  let trailing_zeros = pos.trailing_zeros();

  // Map: trailing_zeros 0 -> highest layer (T-1)
  //      trailing_zeros k -> layer (T-1-k)
  (temporal_layers - 1) - trailing_zeros
}

/// Create SvcOutputMetadata if temporal layers are configured
fn create_svc_metadata(layer_count: Option<u32>, frame_idx: u64) -> Option<SvcOutputMetadata> {
  layer_count.map(|layers| SvcOutputMetadata {
    temporal_layer_id: Some(compute_temporal_layer_id(frame_idx, layers)),
  })
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

  // VP9
  if codec.starts_with("vp09") || codec == "vp9" {
    if validate_vp9_codec(codec) {
      return Ok(AVCodecID::Vp9);
    }
    return Err(Error::new(
      Status::GenericFailure,
      format!("Unsupported codec: {}", codec),
    ));
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
