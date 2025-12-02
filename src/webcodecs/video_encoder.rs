//! VideoEncoder - WebCodecs API implementation
//!
//! Provides video encoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videoencoder-interface

use crate::codec::{BitrateMode, CodecContext, EncoderConfig, Scaler};
use crate::ffi::{AVCodecID, AVHWDeviceType, AVPixelFormat};
use crate::webcodecs::{EncodedVideoChunk, VideoEncoderConfig, VideoFrame};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::sync::{Arc, Mutex};

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

/// Output callback metadata per WebCodecs spec
#[napi(object)]
pub struct EncodedVideoChunkMetadata {
  /// Decoder configuration for this chunk (only present for keyframes)
  pub decoder_config: Option<VideoDecoderConfigOutput>,
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

/// Encode options per WebCodecs spec
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoEncoderEncodeOptions {
  /// Force this frame to be a keyframe
  pub key_frame: Option<bool>,
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
>;

/// Type alias for error callback (takes error string)
/// Still using default CalleeHandled: true for error-first convention on error callback
type ErrorCallback = ThreadsafeFunction<String>;

// Note: For ondequeue, we use FunctionRef instead of ThreadsafeFunction
// to support both getter and setter per WebCodecs spec

/// VideoEncoder init dictionary per WebCodecs spec
pub struct VideoEncoderInit {
  /// Output callback - called when encoded chunk is available
  pub output: OutputCallback,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for VideoEncoderInit {
  unsafe fn from_napi_value(env: napi::sys::napi_env, value: napi::sys::napi_value) -> Result<Self> {
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
  /// Optional dequeue event callback
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
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
    };

    Ok(Self {
      inner: Arc::new(Mutex::new(inner)),
    })
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
  fn fire_dequeue_event(env: &Env, inner: &VideoEncoderInner) -> Result<()> {
    if let Some(ref callback) = inner.dequeue_callback {
      let cb = callback.borrow_back(env)?;
      cb.call(())?;
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
  pub fn set_ondequeue(&self, callback: Option<FunctionRef<(), UnknownReturnValue>>) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    inner.dequeue_callback = callback;

    Ok(())
  }

  /// Get the dequeue event handler (per WebCodecs spec)
  #[napi(getter)]
  pub fn get_ondequeue<'env>(
    &self,
    env: &'env Env,
  ) -> Result<Option<Function<'env, (), UnknownReturnValue>>> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if let Some(ref callback) = inner.dequeue_callback {
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

    // Determine hardware acceleration
    let hw_type = config
      .hardware_acceleration
      .as_ref()
      .and_then(|ha| match ha.as_str() {
        "prefer-hardware" | "require-hardware" => {
          #[cfg(target_os = "macos")]
          {
            Some(AVHWDeviceType::Videotoolbox)
          }
          #[cfg(not(target_os = "macos"))]
          {
            Some(AVHWDeviceType::Cuda)
          }
        }
        _ => None,
      });

    // Create encoder context
    let mut context = match CodecContext::new_encoder_with_hw(codec_id, hw_type) {
      Ok(ctx) => ctx,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Failed to create encoder: {}", e));
        return Ok(());
      }
    };

    // Parse bitrate mode from config
    let bitrate_mode = match config.bitrate_mode.as_deref() {
      Some("constant") => BitrateMode::Constant,
      Some("variable") => BitrateMode::Variable,
      Some("quantizer") => BitrateMode::Quantizer,
      _ => BitrateMode::Constant, // Default to CBR
    };

    // Parse latency mode: "realtime" = low latency, "quality" = default quality mode
    let (gop_size, max_b_frames) = match config.latency_mode.as_deref() {
      Some("realtime") => (10, 0), // Low latency: small GOP, no B-frames
      _ => (60, 2),                // Quality mode: larger GOP with B-frames
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
      Self::report_error(&mut inner, &format!("Failed to configure encoder: {}", e));
      return Ok(());
    }

    // Open the encoder
    if let Err(e) = context.open() {
      Self::report_error(&mut inner, &format!("Failed to open encoder: {}", e));
      return Ok(());
    }

    inner.context = Some(context);
    inner.config = Some(config);
    inner.state = CodecState::Configured;
    inner.extradata_sent = false;
    inner.frame_count = 0;
    inner.encode_queue_size = 0;

    Ok(())
  }

  /// Encode a frame
  #[napi]
  pub fn encode(
    &self,
    env: &Env,
    frame: &VideoFrame,
    _options: Option<VideoEncoderEncodeOptions>,
  ) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state != CodecState::Configured {
      Self::report_error(&mut inner, "Encoder not configured");
      return Ok(());
    }

    // Increment queue size (pending operation)
    inner.encode_queue_size += 1;

    // Get config info first (clone to avoid borrow issues)
    let (width, height, codec_string) = match inner.config.as_ref() {
      Some(config) => (config.width, config.height, config.codec.clone()),
      None => {
        inner.encode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, "No encoder config");
        return Ok(());
      }
    };

    // Get frame data from VideoFrame
    let (internal_frame, needs_conversion) = match frame.with_frame(|f| {
      let frame_format = f.format();
      let needs_conv =
        frame_format != AVPixelFormat::Yuv420p || f.width() != width || f.height() != height;
      (f.try_clone(), needs_conv)
    }) {
      Ok(result) => result,
      Err(e) => {
        inner.encode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Failed to access frame: {}", e));
        return Ok(());
      }
    };

    let internal_frame = match internal_frame {
      Ok(f) => f,
      Err(e) => {
        inner.encode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Failed to clone frame: {}", e));
        return Ok(());
      }
    };

    // Convert frame if needed
    let mut frame_to_encode = if needs_conversion {
      // Create scaler if needed
      if inner.scaler.is_none() {
        let src_format = internal_frame.format();
        match Scaler::new(
          internal_frame.width(),
          internal_frame.height(),
          src_format,
          width,
          height,
          AVPixelFormat::Yuv420p,
          crate::codec::scaler::ScaleAlgorithm::Bilinear,
        ) {
          Ok(scaler) => inner.scaler = Some(scaler),
          Err(e) => {
            inner.encode_queue_size -= 1;
            Self::fire_dequeue_event(env, &inner)?;
            Self::report_error(&mut inner, &format!("Failed to create scaler: {}", e));
            return Ok(());
          }
        }
      }

      let scaler = inner.scaler.as_ref().unwrap();
      match scaler.scale_alloc(&internal_frame) {
        Ok(scaled) => scaled,
        Err(e) => {
          inner.encode_queue_size -= 1;
          Self::fire_dequeue_event(env, &inner)?;
          Self::report_error(&mut inner, &format!("Failed to scale frame: {}", e));
          return Ok(());
        }
      }
    } else {
      internal_frame
    };

    // Set frame PTS based on timestamp
    let pts = match frame.timestamp() {
      Ok(ts) => ts,
      Err(e) => {
        inner.encode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Failed to get frame timestamp: {}", e));
        return Ok(());
      }
    };
    frame_to_encode.set_pts(pts);

    // Get extradata before encoding
    let extradata_sent = inner.extradata_sent;
    let extradata = if !extradata_sent {
      inner
        .context
        .as_ref()
        .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
    } else {
      None
    };

    // Encode the frame
    let context = match inner.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        inner.encode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, "No encoder context");
        return Ok(());
      }
    };

    let packets = match context.encode(Some(&frame_to_encode)) {
      Ok(pkts) => pkts,
      Err(e) => {
        inner.encode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Encode failed: {}", e));
        return Ok(());
      }
    };

    inner.frame_count += 1;

    // Decrement queue size and fire dequeue event
    inner.encode_queue_size -= 1;
    Self::fire_dequeue_event(env, &inner)?;

    // Process output packets - call callback for each
    for packet in packets {
      let chunk = EncodedVideoChunk::from_packet(&packet);

      // Create metadata
      let metadata = if !inner.extradata_sent && packet.is_key() {
        inner.extradata_sent = true;

        EncodedVideoChunkMetadata {
          decoder_config: Some(VideoDecoderConfigOutput {
            codec: codec_string.clone(),
            coded_width: Some(width),
            coded_height: Some(height),
            description: extradata.clone().map(Uint8Array::from),
          }),
        }
      } else {
        EncodedVideoChunkMetadata {
          decoder_config: None,
        }
      };

      // Call callback with EncodedVideoChunk directly (per W3C WebCodecs spec)
      // Use .into() to convert tuple to FnArgs for spreading as separate arguments
      inner.output_callback.call(
        (chunk, metadata).into(),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }

    Ok(())
  }

  /// Flush the encoder
  /// Returns a Promise that resolves when flushing is complete
  #[napi]
  pub async fn flush(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state != CodecState::Configured {
      Self::report_error(&mut inner, "Encoder not configured");
      return Ok(());
    }

    let context = match inner.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut inner, "No encoder context");
        return Ok(());
      }
    };

    // Flush encoder
    let packets = match context.flush_encoder() {
      Ok(pkts) => pkts,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Flush failed: {}", e));
        return Ok(());
      }
    };

    // Process remaining packets - call callback for each
    for packet in packets {
      let chunk = EncodedVideoChunk::from_packet(&packet);
      let metadata = EncodedVideoChunkMetadata {
        decoder_config: None,
      };

      // Call callback with EncodedVideoChunk directly (per W3C WebCodecs spec)
      // Use .into() to convert tuple to FnArgs for spreading as separate arguments
      inner.output_callback.call(
        (chunk, metadata).into(),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }

    Ok(())
  }

  /// Reset the encoder
  #[napi]
  pub fn reset(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state == CodecState::Closed {
      Self::report_error(&mut inner, "Encoder is closed");
      return Ok(());
    }

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

    Ok(())
  }

  /// Close the encoder
  #[napi]
  pub fn close(&self) -> Result<()> {
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
