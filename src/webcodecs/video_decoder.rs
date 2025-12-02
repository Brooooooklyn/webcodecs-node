//! VideoDecoder - WebCodecs API implementation
//!
//! Provides video decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videodecoder-interface

use crate::codec::{CodecContext, DecoderConfig, Frame, Packet};
use crate::ffi::{AVCodecID, AVHWDeviceType};
use crate::webcodecs::{CodecState, EncodedVideoChunk, VideoDecoderConfig, VideoFrame};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// Type alias for output callback (takes VideoFrame)
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback = ThreadsafeFunction<VideoFrame, UnknownReturnValue, VideoFrame, Status, false>;

/// Type alias for error callback (takes error message)
/// Still using default CalleeHandled: true for error-first convention
type ErrorCallback = ThreadsafeFunction<String>;

// Note: For ondequeue, we use FunctionRef instead of ThreadsafeFunction
// to support both getter and setter per WebCodecs spec

/// VideoDecoder init dictionary per WebCodecs spec
pub struct VideoDecoderInit {
  /// Output callback - called when decoded frame is available
  pub output: OutputCallback,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for VideoDecoderInit {
  unsafe fn from_napi_value(env: napi::sys::napi_env, value: napi::sys::napi_value) -> Result<Self> {
    let obj = Object::from_napi_value(env, value)?;

    let output: OutputCallback = obj
      .get_named_property("output")
      .map_err(|_| Error::new(Status::InvalidArg, "Missing required 'output' callback"))?;

    let error: ErrorCallback = obj
      .get_named_property("error")
      .map_err(|_| Error::new(Status::InvalidArg, "Missing required 'error' callback"))?;

    Ok(VideoDecoderInit { output, error })
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

/// Internal decoder state
struct VideoDecoderInner {
  state: CodecState,
  config: Option<DecoderConfig>,
  context: Option<CodecContext>,
  codec_string: String,
  frame_count: u64,
  /// Number of pending decode operations (for decodeQueueSize)
  decode_queue_size: u32,
  /// Output callback (required per spec)
  output_callback: OutputCallback,
  /// Error callback (required per spec)
  error_callback: ErrorCallback,
  /// Optional dequeue event callback
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
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
      dequeue_callback: None,
    };

    Ok(Self {
      inner: Arc::new(Mutex::new(inner)),
    })
  }

  /// Report an error via callback and close the decoder
  fn report_error(inner: &mut VideoDecoderInner, error_msg: &str) {
    inner.error_callback.call(
      Ok(error_msg.to_string()),
      ThreadsafeFunctionCallMode::NonBlocking,
    );
    inner.state = CodecState::Closed;
  }

  /// Fire dequeue event if callback is set
  fn fire_dequeue_event(env: &Env, inner: &VideoDecoderInner) -> Result<()> {
    if let Some(ref callback) = inner.dequeue_callback {
      let cb = callback.borrow_back(env)?;
      cb.call(())?;
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

  /// Configure the decoder
  #[napi]
  pub fn configure(&self, config: VideoDecoderConfig) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state == CodecState::Closed {
      Self::report_error(&mut inner, "Decoder is closed");
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
      .and_then(|ha| parse_hw_acceleration(ha));

    // Create decoder context with optional hardware acceleration
    let mut context = match CodecContext::new_decoder_with_hw(codec_id, hw_type) {
      Ok(ctx) => ctx,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Failed to create decoder: {}", e));
        return Ok(());
      }
    };

    // Configure decoder
    let decoder_config = DecoderConfig {
      codec_id,
      thread_count: 0, // Auto
      extradata: config.description.as_ref().map(|d| d.to_vec()),
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
    inner.codec_string = config.codec;
    inner.state = CodecState::Configured;
    inner.frame_count = 0;
    inner.decode_queue_size = 0;

    Ok(())
  }

  /// Decode an encoded video chunk
  #[napi]
  pub fn decode(&self, env: &Env, chunk: &EncodedVideoChunk) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state != CodecState::Configured {
      Self::report_error(&mut inner, "Decoder not configured");
      return Ok(());
    }

    // Increment queue size (pending operation)
    inner.decode_queue_size += 1;

    // Get chunk data
    let data = match chunk.get_data_vec() {
      Ok(d) => d,
      Err(e) => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Failed to get chunk data: {}", e));
        return Ok(());
      }
    };
    let timestamp = match chunk.timestamp() {
      Ok(ts) => ts,
      Err(e) => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Failed to get timestamp: {}", e));
        return Ok(());
      }
    };
    let duration = match chunk.duration() {
      Ok(dur) => dur,
      Err(e) => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Failed to get duration: {}", e));
        return Ok(());
      }
    };

    // Get context
    let context = match inner.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, "No decoder context");
        return Ok(());
      }
    };

    // Decode using the internal frame
    let frames = match decode_chunk_data(context, &data, timestamp, duration) {
      Ok(f) => f,
      Err(e) => {
        inner.decode_queue_size -= 1;
        Self::fire_dequeue_event(env, &inner)?;
        Self::report_error(&mut inner, &format!("Decode failed: {}", e));
        return Ok(());
      }
    };

    inner.frame_count += 1;

    // Decrement queue size and fire dequeue event
    inner.decode_queue_size -= 1;
    Self::fire_dequeue_event(env, &inner)?;

    // Convert internal frames to VideoFrames and call output callback
    for frame in frames {
      let video_frame = VideoFrame::from_internal(frame, timestamp, duration);

      // Call output callback (CalleeHandled: false means direct value, not Result)
      inner
        .output_callback
        .call(video_frame, ThreadsafeFunctionCallMode::NonBlocking);
    }

    Ok(())
  }

  /// Flush the decoder
  /// Returns a Promise that resolves when flushing is complete
  #[napi]
  pub async fn flush(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state != CodecState::Configured {
      Self::report_error(&mut inner, "Decoder not configured");
      return Ok(());
    }

    let context = match inner.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut inner, "No decoder context");
        return Ok(());
      }
    };

    // Flush decoder
    let frames = match context.flush_decoder() {
      Ok(f) => f,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Flush failed: {}", e));
        return Ok(());
      }
    };

    // Convert and call output callback for remaining frames
    for frame in frames {
      let pts = frame.pts();
      let duration = if frame.duration() > 0 {
        Some(frame.duration())
      } else {
        None
      };
      let video_frame = VideoFrame::from_internal(frame, pts, duration);

      // Call output callback (CalleeHandled: false means direct value, not Result)
      inner
        .output_callback
        .call(video_frame, ThreadsafeFunctionCallMode::NonBlocking);
    }

    Ok(())
  }

  /// Reset the decoder
  #[napi]
  pub fn reset(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state == CodecState::Closed {
      Self::report_error(&mut inner, "Decoder is closed");
      return Ok(());
    }

    // Drain decoder before dropping to ensure libaom/AV1 threads finish
    if let Some(ctx) = inner.context.as_mut() {
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

    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Drain decoder before dropping to ensure libaom/AV1 threads finish
    if let Some(ctx) = inner.context.as_mut() {
      let _ = ctx.send_packet(None);
      while ctx.receive_frame().ok().flatten().is_some() {}
    }

    inner.context = None;
    inner.config = None;
    inner.codec_string.clear();
    inner.state = CodecState::Closed;
    inner.decode_queue_size = 0;

    Ok(())
  }

  /// Check if a configuration is supported
  /// Returns a Promise that resolves with support information
  #[napi]
  pub async fn is_config_supported(config: VideoDecoderConfig) -> Result<VideoDecoderSupport> {
    // Parse codec string
    let codec_id = match parse_codec_string(&config.codec) {
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
  }
}

/// Parse WebCodecs codec string to FFmpeg codec ID
fn parse_codec_string(codec: &str) -> Result<AVCodecID> {
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

/// Parse hardware acceleration preference string to device type
fn parse_hw_acceleration(ha: &str) -> Option<AVHWDeviceType> {
  match ha {
    "prefer-hardware" | "require-hardware" => {
      // Return platform-preferred hardware type
      #[cfg(target_os = "macos")]
      {
        Some(AVHWDeviceType::Videotoolbox)
      }
      #[cfg(target_os = "linux")]
      {
        Some(AVHWDeviceType::Vaapi)
      }
      #[cfg(target_os = "windows")]
      {
        Some(AVHWDeviceType::D3d11va)
      }
      #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
      {
        None
      }
    }
    _ => None,
  }
}

/// Decode chunk data using FFmpeg
fn decode_chunk_data(
  context: &mut CodecContext,
  data: &[u8],
  timestamp: i64,
  duration: Option<i64>,
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
