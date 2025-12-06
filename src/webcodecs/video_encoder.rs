//! VideoEncoder - WebCodecs API implementation
//!
//! Provides video encoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videoencoder-interface

use crate::codec::{
  BitrateMode as CodecBitrateMode, CodecContext, EncoderConfig, EncoderCreationResult, Frame,
  Scaler,
};
use crate::ffi::{AVCodecID, AVHWDeviceType, AVPixelFormat};
use crate::webcodecs::hw_fallback::{
  is_hw_encoding_disabled, record_hw_encoding_failure, record_hw_encoding_success,
};
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

/// Type alias for error callback (takes error string)
/// Still using default CalleeHandled: true for error-first convention on error callback
type ErrorCallback = ThreadsafeFunction<String, UnknownReturnValue, String, Status, true, true>;

// Note: For ondequeue, we use FunctionRef instead of ThreadsafeFunction
// to support both getter and setter per WebCodecs spec

/// Commands sent to the worker thread
enum EncoderCommand {
  /// Encode a video frame
  Encode {
    frame: Frame,
    timestamp: i64,
    options: Option<VideoEncoderEncodeOptions>,
  },
  /// Flush the encoder and send result back via response channel
  Flush(Sender<Result<()>>),
}

/// VideoEncoder init dictionary per WebCodecs spec
pub struct VideoEncoderInit {
  /// Output callback - called when encoded chunk is available
  pub output: OutputCallback,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for VideoEncoderInit {
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

    Ok(VideoEncoderInit { output, error })
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
  pending_frames: Vec<(Frame, i64, Option<VideoEncoderEncodeOptions>)>,
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
  /// Channel sender for worker commands
  command_sender: Option<Sender<EncoderCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
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
    if let Ok(mut inner) = self.inner.lock() {
      if let Some(ctx) = inner.context.as_mut() {
        let _ = ctx.send_frame(None);
        while ctx.receive_packet().ok().flatten().is_some() {}
      }
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
      // Hardware acceleration tracking
      is_hardware: false,
      encoder_name: String::new(),
      hw_preference: HardwareAcceleration::NoPreference,
      silent_encode_count: 0,
      first_output_produced: false,
      pending_frames: Vec::new(),
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
  fn worker_loop(inner: Arc<Mutex<VideoEncoderInner>>, receiver: Receiver<EncoderCommand>) {
    while let Ok(command) = receiver.recv() {
      match command {
        EncoderCommand::Encode {
          frame,
          timestamp,
          options,
        } => {
          Self::process_encode(&inner, frame, timestamp, options);
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
  ) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Check if encoder is still configured
    if guard.state != CodecState::Configured {
      guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
      let _ = Self::fire_dequeue_event(&guard);
      Self::report_error(&mut guard, "Encoder not configured");
      return;
    }

    // Get config info
    let (width, height, codec_string) = match guard.config.as_ref() {
      Some(config) => (config.width, config.height, config.codec.clone()),
      None => {
        guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
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
            guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
            let _ = Self::fire_dequeue_event(&guard);
            Self::report_error(&mut guard, &format!("Failed to create scaler: {}", e));
            return;
          }
        }
      }

      let scaler = guard.scaler.as_ref().unwrap();
      match scaler.scale_alloc(&frame) {
        Ok(scaled) => scaled,
        Err(e) => {
          guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
          let _ = Self::fire_dequeue_event(&guard);
          Self::report_error(&mut guard, &format!("Failed to scale frame: {}", e));
          return;
        }
      }
    } else {
      frame
    };

    // Set frame PTS
    frame_to_encode.set_pts(timestamp);

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
        guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
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
              .push((cloned, timestamp, _options.clone()));
          }
          let pending_frames = std::mem::take(&mut guard.pending_frames);

          if Self::fallback_to_software(&mut guard) {
            // Re-encode all buffered frames with software encoder
            for (buffered_frame, buffered_ts, _buffered_opts) in pending_frames {
              let mut frame_to_reencode = buffered_frame;
              frame_to_reencode.set_pts(buffered_ts);

              if let Some(ctx) = guard.context.as_mut() {
                if let Ok(pkts) = ctx.encode(Some(&frame_to_reencode)) {
                  for packet in pkts {
                    let chunk = EncodedVideoChunk::from_packet(&packet);
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
                        }),
                        svc: None,
                        alpha_side_data: None,
                      }
                    } else {
                      EncodedVideoChunkMetadata {
                        decoder_config: None,
                        svc: None,
                        alpha_side_data: None,
                      }
                    };
                    guard.output_callback.call(
                      (chunk, metadata).into(),
                      ThreadsafeFunctionCallMode::NonBlocking,
                    );
                    guard.first_output_produced = true;
                  }
                }
              }
            }
            guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
            let _ = Self::fire_dequeue_event(&guard);
            return;
          }
          // Fallback failed - report error with context
          let codec = guard
            .config
            .as_ref()
            .map(|c| c.codec.clone())
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
            .map(|c| c.codec.clone())
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
        guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
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
          .push((cloned_frame, timestamp, _options.clone()));
      }
      guard.silent_encode_count += 1;

      if guard.silent_encode_count >= SILENT_FAILURE_THRESHOLD {
        match guard.hw_preference {
          HardwareAcceleration::PreferHardware => {
            // prefer-hardware: Report error, no fallback
            // Record failure for global tracking
            record_hw_encoding_failure();
            guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
            let _ = Self::fire_dequeue_event(&guard);
            let codec = guard
              .config
              .as_ref()
              .map(|c| c.codec.clone())
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
              for (buffered_frame, buffered_ts, _buffered_opts) in pending_frames {
                let mut frame_to_reencode = buffered_frame;
                frame_to_reencode.set_pts(buffered_ts);

                if let Some(ctx) = guard.context.as_mut() {
                  if let Ok(pkts) = ctx.encode(Some(&frame_to_reencode)) {
                    // Process any output packets from re-encoding
                    for packet in pkts {
                      let chunk = EncodedVideoChunk::from_packet(&packet);
                      let metadata = if !guard.extradata_sent && packet.is_key() {
                        guard.extradata_sent = true;
                        EncodedVideoChunkMetadata {
                          decoder_config: Some(VideoDecoderConfigOutput {
                            codec: codec_string.clone(),
                            coded_width: Some(width),
                            coded_height: Some(height),
                            description: guard.context.as_ref().and_then(|ctx| {
                              ctx.extradata().map(|d| Uint8Array::from(d.to_vec()))
                            }),
                          }),
                          svc: None,
                          alpha_side_data: None,
                        }
                      } else {
                        EncodedVideoChunkMetadata {
                          decoder_config: None,
                          svc: None,
                          alpha_side_data: None,
                        }
                      };
                      guard.output_callback.call(
                        (chunk, metadata).into(),
                        ThreadsafeFunctionCallMode::NonBlocking,
                      );
                      guard.first_output_produced = true;
                    }
                  }
                }
              }

              // Decrement queue size and continue
              guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
              let _ = Self::fire_dequeue_event(&guard);
              return;
            } else {
              // Fallback failed, report error
              // Record failure for global tracking
              record_hw_encoding_failure();
              guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
              let _ = Self::fire_dequeue_event(&guard);
              let codec = guard
                .config
                .as_ref()
                .map(|c| c.codec.clone())
                .unwrap_or_else(|| "unknown".to_string());
              let encoder_name = guard.encoder_name.clone();
              Self::report_error(
                &mut guard,
                &format!(
                  "OperationError: {} encoder ({}) failed (silent failure) and software fallback unavailable",
                  codec,
                  encoder_name
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

    // Decrement queue size and fire dequeue event
    guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
    let _ = Self::fire_dequeue_event(&guard);

    // Process output packets - call callback for each
    for packet in packets {
      let chunk = EncodedVideoChunk::from_packet(&packet);

      // Create metadata
      let metadata = if !guard.extradata_sent && packet.is_key() {
        guard.extradata_sent = true;

        EncodedVideoChunkMetadata {
          decoder_config: Some(VideoDecoderConfigOutput {
            codec: codec_string.clone(),
            coded_width: Some(width),
            coded_height: Some(height),
            description: extradata.clone().map(Uint8Array::from),
          }),
          svc: None,
          alpha_side_data: None,
        }
      } else {
        EncodedVideoChunkMetadata {
          decoder_config: None,
          svc: None,
          alpha_side_data: None,
        }
      };

      // Call callback with EncodedVideoChunk directly (per W3C WebCodecs spec)
      guard.output_callback.call(
        (chunk, metadata).into(),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }
  }

  /// Process a flush command on the worker thread
  fn process_flush(inner: &Arc<Mutex<VideoEncoderInner>>) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if guard.state != CodecState::Configured {
      Self::report_error(&mut guard, "Encoder not configured");
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

    // Process remaining packets - call callback for each
    for packet in packets {
      let chunk = EncodedVideoChunk::from_packet(&packet);
      let metadata = EncodedVideoChunkMetadata {
        decoder_config: None,
        svc: None,
        alpha_side_data: None,
      };

      guard.output_callback.call(
        (chunk, metadata).into(),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }

    Ok(())
  }

  /// Report an error via callback and close the encoder
  fn report_error(inner: &mut VideoEncoderInner, error_msg: &str) {
    inner.error_callback.call(
      Ok(error_msg.to_string()),
      ThreadsafeFunctionCallMode::NonBlocking,
    );
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

    let codec_id = match parse_codec_string(&config.codec) {
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
      width: config.width,
      height: config.height,
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
  pub fn configure(&self, config: VideoEncoderConfig) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state == CodecState::Closed {
      Self::report_error(&mut inner, "Encoder is closed");
      return Ok(());
    }

    // Parse codec string to determine codec ID
    let codec_id = match parse_codec_string(&config.codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Invalid codec: {}", e));
        return Ok(());
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
      width: config.width,
      height: config.height,
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

    Ok(())
  }

  /// Encode a frame
  #[napi]
  pub fn encode(
    &self,
    frame: &VideoFrame,
    options: Option<VideoEncoderEncodeOptions>,
  ) -> Result<()> {
    // Clone frame and get timestamp on main thread (brief lock)
    let (internal_frame, timestamp) = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state != CodecState::Configured {
        Self::report_error(&mut inner, "Encoder not configured");
        return Ok(());
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

      // Increment queue size (pending operation)
      inner.encode_queue_size += 1;

      (internal_frame, timestamp)
    };

    // Send encode command to worker thread
    if let Some(ref sender) = self.command_sender {
      sender
        .send(EncoderCommand::Encode {
          frame: internal_frame,
          timestamp,
          options,
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
  #[napi]
  pub async fn flush(&self) -> Result<()> {
    // Create a response channel
    let (response_sender, response_receiver) = channel::bounded::<Result<()>>(1);

    // Send flush command through the channel to ensure it's processed after all pending encodes
    if let Some(ref sender) = self.command_sender {
      sender
        .send(EncoderCommand::Flush(response_sender))
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?;
    } else {
      return Err(Error::new(
        Status::GenericFailure,
        "Encoder has been closed",
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

  /// Reset the encoder
  #[napi]
  pub fn reset(&mut self) -> Result<()> {
    // Check state first before touching the worker
    {
      let inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        return Err(Error::new(Status::GenericFailure, "Encoder is closed"));
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
  #[napi]
  pub async fn is_config_supported(config: VideoEncoderConfig) -> Result<VideoEncoderSupport> {
    // Parse codec string
    let codec_id = match parse_codec_string(&config.codec) {
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
  }
}

/// Parse WebCodecs codec string to FFmpeg codec ID
fn parse_codec_string(codec: &str) -> Result<AVCodecID> {
  // Handle common codec strings
  // https://www.w3.org/TR/webcodecs-codec-registry/

  let codec_lower = codec.to_lowercase();

  if codec_lower.starts_with("avc1") || codec_lower.starts_with("avc3") || codec_lower == "h264" {
    Ok(AVCodecID::H264)
  } else if codec_lower.starts_with("hev1")
    || codec_lower.starts_with("hvc1")
    || codec_lower == "h265"
    || codec_lower == "hevc"
  {
    Ok(AVCodecID::Hevc)
  } else if codec_lower == "vp8" {
    Ok(AVCodecID::Vp8)
  } else if codec_lower.starts_with("vp09") || codec_lower == "vp9" {
    Ok(AVCodecID::Vp9)
  } else if codec_lower.starts_with("av01") || codec_lower == "av1" {
    Ok(AVCodecID::Av1)
  } else {
    Err(Error::new(
      Status::GenericFailure,
      format!("Unsupported codec: {}", codec),
    ))
  }
}
