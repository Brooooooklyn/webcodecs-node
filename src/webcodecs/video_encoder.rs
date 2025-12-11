//! VideoEncoder - WebCodecs API implementation
//!
//! Provides video encoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videoencoder-interface

use crate::codec::{
  BitrateMode as CodecBitrateMode, CodecContext, EncoderConfig, EncoderCreationResult, Frame,
  HwDeviceContext, HwFrameConfig, HwFrameContext, Scaler,
};
use crate::ffi::{AVCodecID, AVHWDeviceType, AVPixelFormat};
use crate::webcodecs::error::{invalid_state_error, throw_type_error_unit};
use crate::webcodecs::hw_fallback::{
  is_hw_encoding_disabled, record_hw_encoding_failure, record_hw_encoding_success,
};
use crate::webcodecs::promise_reject::reject_with_type_error;
use crate::webcodecs::{
  EncodedVideoChunk, HardwareAcceleration, LatencyMode, VideoEncoderBitrateMode,
  VideoEncoderConfig, VideoFrame,
};
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
  /// Optional dequeue event callback (ThreadsafeFunction for multi-thread support)
  dequeue_callback: Option<ThreadsafeFunction<(), UnknownReturnValue, (), Status, false, true>>,
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
      dequeue_callback: None,
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
    };

    let inner = Arc::new(Mutex::new(inner));

    // Create channel for worker commands
    let (sender, receiver) = channel::unbounded();

    // Create reset abort flag
    let reset_flag = Arc::new(AtomicBool::new(false));

    // Spawn worker thread
    let worker_inner = inner.clone();
    let worker_reset_flag = reset_flag.clone();
    let worker_handle = std::thread::spawn(move || {
      Self::worker_loop(worker_inner, receiver, worker_reset_flag);
    });

    Ok(Self {
      inner,
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
              let _ = Self::fire_dequeue_event(&guard);
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
          Self::process_encode(&inner, frame, timestamp, options, rotation, flip);
        }
        EncoderCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner);
          let _ = response_sender.send(result);
        }
      }
    }
  }

  /// Process an encode command on the worker thread
  fn process_encode(
    inner: &Arc<Mutex<VideoEncoderInner>>,
    frame: Frame,
    timestamp: i64,
    _options: Option<VideoEncoderEncodeOptions>,
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
        let _ = Self::fire_dequeue_event(&guard);
      }
      Self::report_error(&mut guard, "Encoder not configured");
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
          let _ = Self::fire_dequeue_event(&guard);
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
              let _ = Self::fire_dequeue_event(&guard);
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
            let _ = Self::fire_dequeue_event(&guard);
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

    // Get extradata before encoding
    let extradata_sent = guard.extradata_sent;
    let extradata = if !extradata_sent {
      guard
        .context
        .as_ref()
        .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
    } else {
      None
    };

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
        // For hardware encoder with no-preference, try fallback to software
        if guard.is_hardware
          && !guard.first_output_produced
          && guard.hw_preference == HardwareAcceleration::NoPreference
        {
          // Buffer current frame for re-encoding
          if let Ok(cloned) = frame_to_encode.try_clone() {
            guard
              .pending_frames
              .push((cloned, timestamp, _options.clone(), rotation, flip));
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
                  let chunk = EncodedVideoChunk::from_packet(&packet, Some(buffered_ts));

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
                      ThreadsafeFunctionCallMode::NonBlocking,
                    );
                  }
                  guard.first_output_produced = true;
                }
              }
            }
            let old_size = guard.encode_queue_size;
            guard.encode_queue_size = old_size.saturating_sub(1);
            if old_size > 0 {
              let _ = Self::fire_dequeue_event(&guard);
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
          let _ = Self::fire_dequeue_event(&guard);
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
          .push((cloned_frame, timestamp, _options.clone(), rotation, flip));
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
              let _ = Self::fire_dequeue_event(&guard);
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
                    let chunk = EncodedVideoChunk::from_packet(&packet, Some(buffered_ts));

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
                        ThreadsafeFunctionCallMode::NonBlocking,
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
                let _ = Self::fire_dequeue_event(&guard);
              }
              return;
            } else {
              // Fallback failed, report error
              // Record failure for global tracking
              record_hw_encoding_failure();
              let old_size = guard.encode_queue_size;
              guard.encode_queue_size = old_size.saturating_sub(1);
              if old_size > 0 {
                let _ = Self::fire_dequeue_event(&guard);
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
      let _ = Self::fire_dequeue_event(&guard);
    }

    // Process output packets - call callback for each
    for packet in packets {
      // Pop timestamp from queue to preserve original input timestamp
      // (FFmpeg may modify PTS internally during encoding)
      let output_timestamp = guard.timestamp_queue.pop_front();
      let chunk = EncodedVideoChunk::from_packet(&packet, output_timestamp);

      // Create SVC metadata if temporal layers are configured
      let svc = create_svc_metadata(guard.temporal_layer_count, guard.output_frame_count);
      guard.output_frame_count += 1;

      // Create metadata
      let metadata = if !guard.extradata_sent && packet.is_key() {
        guard.extradata_sent = true;

        EncodedVideoChunkMetadata {
          decoder_config: Some(VideoDecoderConfigOutput {
            codec: codec_string.clone(),
            coded_width: Some(width),
            coded_height: Some(height),
            description: extradata.clone().map(Uint8Array::from),
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
          ThreadsafeFunctionCallMode::NonBlocking,
        );
      }
    }
  }

  /// Process a flush command on the worker thread
  fn process_flush(inner: &Arc<Mutex<VideoEncoderInner>>) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // W3C spec: flush() should reject if encoder is not in configured state
    if guard.state != CodecState::Configured {
      return Err(Error::new(
        Status::GenericFailure,
        "InvalidStateError: Encoder is not configured",
      ));
    }

    // If no frames have been encoded, skip flushing to avoid putting encoder in EOF state
    // (calling flush_encoder on empty encoder triggers EOF mode in FFmpeg which prevents
    // further encoding without context recreation)
    if guard.frame_count == 0 {
      return Ok(());
    }

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

    // Queue remaining packets for synchronous delivery in resolver
    for packet in packets {
      // Pop timestamp from queue to preserve original input timestamp
      let output_timestamp = guard.timestamp_queue.pop_front();
      let chunk = EncodedVideoChunk::from_packet(&packet, output_timestamp);

      // Create SVC metadata if temporal layers are configured
      let svc = create_svc_metadata(guard.temporal_layer_count, guard.output_frame_count);
      guard.output_frame_count += 1;

      let metadata = EncodedVideoChunkMetadata {
        decoder_config: None,
        svc,
        alpha_side_data: None,
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

  /// Report an error via callback and close the encoder
  fn report_error(inner: &mut VideoEncoderInner, error_msg: &str) {
    // Create an Error object that will be passed directly to the JS callback
    let error = Error::new(Status::GenericFailure, error_msg);
    inner
      .error_callback
      .call(error, ThreadsafeFunctionCallMode::NonBlocking);
    inner.state = CodecState::Closed;
  }

  /// Fire dequeue event if callback is set
  fn fire_dequeue_event(inner: &VideoEncoderInner) -> Result<()> {
    if let Some(ref callback) = inner.dequeue_callback {
      callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
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

  /// Configure the encoder
  #[napi]
  pub fn configure(&self, env: Env, config: VideoEncoderConfig) -> Result<()> {
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
      return Err(invalid_state_error("Encoder is closed"));
    }

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
        match Self::create_software_encoder(codec_id, &encoder_config) {
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

    // Open the encoder
    if let Err(e) = context.open() {
      // For no-preference, try software fallback if hardware open fails
      if hw_preference == HardwareAcceleration::NoPreference && is_hardware {
        match Self::create_software_encoder(codec_id, &encoder_config) {
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
        return Err(invalid_state_error("Cannot encode with a closed codec"));
      }
      if inner.state != CodecState::Configured {
        return Err(invalid_state_error(
          "Cannot encode with an unconfigured codec",
        ));
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

      // Increment queue size (pending operation)
      inner.encode_queue_size += 1;

      (internal_frame, timestamp, rotation, flip)
    };

    // Send encode command to worker thread
    if let Some(ref sender) = self.command_sender {
      sender
        .send(EncoderCommand::Encode {
          frame: internal_frame,
          timestamp,
          options,
          rotation,
          flip,
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

    // Clear flush-related state
    inner.inside_flush = false;
    inner.pending_chunks.clear();

    // Reset the abort flag for new worker
    self.reset_flag.store(false, Ordering::SeqCst);

    // Create new channel and worker for future encode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(sender);
    let worker_inner = self.inner.clone();
    let worker_reset_flag = self.reset_flag.clone();
    drop(inner); // Release lock before spawning thread
    self.worker_handle = Some(std::thread::spawn(move || {
      Self::worker_loop(worker_inner, receiver, worker_reset_flag);
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
    // W3C WebCodecs spec: Validate config, reject with TypeError for invalid
    // https://w3c.github.io/webcodecs/#dom-videoencoder-isconfigsupported

    // Validate codec - must be present and not empty
    let codec = match &config.codec {
      Some(c) if !c.is_empty() => c.clone(),
      Some(_) => return reject_with_type_error(env, "codec is required"),
      None => return reject_with_type_error(env, "codec is required"),
    };

    // Validate width - must be present and greater than 0
    match config.width {
      Some(w) if w > 0 => {}
      Some(_) => return reject_with_type_error(env, "width must be greater than 0"),
      None => return reject_with_type_error(env, "width is required"),
    };

    // Validate height - must be present and greater than 0
    match config.height {
      Some(h) if h > 0 => {}
      Some(_) => return reject_with_type_error(env, "height must be greater than 0"),
      None => return reject_with_type_error(env, "height is required"),
    };

    // Validate display dimensions if specified
    if let Some(dw) = config.display_width
      && dw == 0
    {
      return reject_with_type_error(env, "displayWidth must be greater than 0");
    }
    if let Some(dh) = config.display_height
      && dh == 0
    {
      return reject_with_type_error(env, "displayHeight must be greater than 0");
    }

    // Validate bitrate if specified
    if let Some(bitrate) = config.bitrate
      && bitrate <= 0.0
    {
      return reject_with_type_error(env, "bitrate must be greater than 0");
    }

    // Validate framerate if specified
    if let Some(framerate) = config.framerate
      && framerate <= 0.0
    {
      return reject_with_type_error(env, "framerate must be greater than 0");
    }

    // Validate dimensions if specified
    let width = config.width.unwrap_or(0);
    let height = config.height.unwrap_or(0);
    if !are_dimensions_valid(width, height) {
      return env.spawn_future(async move {
        Ok(VideoEncoderSupport {
          supported: false,
          config,
        })
      });
    }

    // Validate scalability mode if specified
    if let Some(ref mode) = config.scalability_mode
      && !is_valid_scalability_mode(mode)
    {
      return env.spawn_future(async move {
        Ok(VideoEncoderSupport {
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
  if codec == "av1" {
    return true; // Short form is valid
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

  // AV1
  if codec.starts_with("av01") || codec == "av1" {
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
