//! VideoDecoder - WebCodecs API implementation
//!
//! Provides video decoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#videodecoder-interface

use crate::codec::{CodecContext, DecoderConfig, Frame, Packet};
use crate::ffi::{AVCodecID, AVHWDeviceType};
use crate::webcodecs::{
  CodecState, EncodedVideoChunk, EncodedVideoChunkInner, VideoDecoderConfig, VideoFrame,
};
use crossbeam::channel::{self, Receiver, Sender};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

/// Type alias for output callback (takes VideoFrame)
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback =
  ThreadsafeFunction<VideoFrame, UnknownReturnValue, VideoFrame, Status, false, true>;

/// Type alias for error callback (takes error message)
/// Still using default CalleeHandled: true for error-first convention
type ErrorCallback = ThreadsafeFunction<String, UnknownReturnValue, String, Status, true, true>;

// Note: For ondequeue, we use FunctionRef instead of ThreadsafeFunction
// to support both getter and setter per WebCodecs spec

/// Commands sent to the worker thread
enum WorkerCommand {
  /// Decode a video chunk
  Decode(Arc<RwLock<Option<EncodedVideoChunkInner>>>),
  /// Flush the decoder and send result back via response channel
  Flush(Sender<Result<()>>),
}

/// VideoDecoder init dictionary per WebCodecs spec
pub struct VideoDecoderInit {
  /// Output callback - called when decoded frame is available
  pub output: OutputCallback,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for VideoDecoderInit {
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
  /// Optional dequeue event callback (ThreadsafeFunction for multi-thread support)
  dequeue_callback: Option<ThreadsafeFunction<(), UnknownReturnValue, (), Status, false, true>>,
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
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
  /// Channel sender for worker commands
  command_sender: Option<Sender<WorkerCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
}

impl Drop for VideoDecoder {
  fn drop(&mut self) {
    // Drop the sender to signal the worker to stop.
    // The worker will see the channel disconnect and exit its loop.
    self.command_sender = None;

    // Don't join the worker thread here - it would block the JS thread during GC.
    // Instead, let the thread become detached and finish on its own.
    // Safety: The Arc<Mutex<VideoDecoderInner>> ensures the inner state (including
    // callbacks and FFmpeg context) stays alive until the worker exits.
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
      dequeue_callback: None,
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
  fn worker_loop(inner: Arc<Mutex<VideoDecoderInner>>, receiver: Receiver<WorkerCommand>) {
    while let Ok(command) = receiver.recv() {
      match command {
        WorkerCommand::Decode(chunk) => {
          Self::process_decode(&inner, chunk);
        }
        WorkerCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner);
          let _ = response_sender.send(result);
        }
      }
    }
  }

  /// Process a decode command
  fn process_decode(inner: &Arc<Mutex<VideoDecoderInner>>, chunk: Arc<RwLock<Option<EncodedVideoChunkInner>>>) {
    let mut guard = match inner.lock() {
      Ok(g) => g,
      Err(_) => return, // Lock poisoned
    };

    // Check if decoder is still configured
    if guard.state != CodecState::Configured {
      guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
      let _ = Self::fire_dequeue_event(&guard);
      Self::report_error(&mut guard, "Decoder not configured");
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
        guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
        Self::report_error(&mut guard, &e.reason);
        return;
      }
    };

    let timestamp = encoded_chunk.timestamp_us;
    let duration = encoded_chunk.duration_us;
    let data = encoded_chunk.data.clone();

    // Drop the chunk read guard before decoding
    drop(chunk_read_guard);

    // Get context
    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
        Self::report_error(&mut guard, "No decoder context");
        return;
      }
    };

    // Decode
    let frames = match decode_chunk_data(context, &data, timestamp, duration) {
      Ok(f) => f,
      Err(e) => {
        guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
        Self::report_error(&mut guard, &format!("Decode failed: {}", e));
        return;
      }
    };

    guard.frame_count += 1;

    // Decrement queue size and fire dequeue event
    guard.decode_queue_size = guard.decode_queue_size.saturating_sub(1);
    let _ = Self::fire_dequeue_event(&guard);

    // Convert internal frames to VideoFrames and call output callback
    for frame in frames {
      let video_frame = VideoFrame::from_internal(frame, timestamp, duration);
      guard
        .output_callback
        .call(video_frame, ThreadsafeFunctionCallMode::NonBlocking);
    }
  }

  /// Process a flush command
  fn process_flush(inner: &Arc<Mutex<VideoDecoderInner>>) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if guard.state != CodecState::Configured {
      Self::report_error(&mut guard, "Decoder not configured");
      return Ok(());
    }

    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut guard, "No decoder context");
        return Ok(());
      }
    };

    // Flush decoder
    let frames = match context.flush_decoder() {
      Ok(f) => f,
      Err(e) => {
        Self::report_error(&mut guard, &format!("Flush failed: {}", e));
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
      guard
        .output_callback
        .call(video_frame, ThreadsafeFunctionCallMode::NonBlocking);
    }

    Ok(())
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
  fn fire_dequeue_event(inner: &VideoDecoderInner) -> Result<()> {
    if let Some(ref callback) = inner.dequeue_callback {
      callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
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
  pub fn decode(&self, chunk: &EncodedVideoChunk) -> Result<()> {
    // Increment queue size first (under lock)
    {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state != CodecState::Configured {
        Self::report_error(&mut inner, "Decoder not configured");
        return Ok(());
      }

      inner.decode_queue_size += 1;
    }

    // Send decode command to worker thread
    if let Some(ref sender) = self.command_sender {
      sender
        .send(WorkerCommand::Decode(chunk.inner.clone()))
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?;
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
  #[napi]
  pub async fn flush(&self) -> Result<()> {
    // Create a response channel
    let (response_sender, response_receiver) = channel::bounded::<Result<()>>(1);

    // Send flush command through the channel to ensure it's processed after all pending decodes
    if let Some(ref sender) = self.command_sender {
      sender
        .send(WorkerCommand::Flush(response_sender))
        .map_err(|_| Error::new(Status::GenericFailure, "Worker thread terminated"))?;
    } else {
      return Err(Error::new(
        Status::GenericFailure,
        "Decoder has been closed",
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

  /// Reset the decoder
  #[napi]
  pub fn reset(&mut self) -> Result<()> {
    // Check state first before touching the worker
    {
      let inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state == CodecState::Closed {
        return Err(Error::new(
          Status::GenericFailure,
          "Decoder is closed",
        ));
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

    // Create new channel and worker for future decode operations
    let (sender, receiver) = channel::unbounded();
    self.command_sender = Some(sender);
    let worker_inner = self.inner.clone();
    drop(inner); // Release lock before spawning thread
    self.worker_handle = Some(std::thread::spawn(move || {
      Self::worker_loop(worker_inner, receiver);
    }));

    Ok(())
  }

  /// Close the decoder
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
