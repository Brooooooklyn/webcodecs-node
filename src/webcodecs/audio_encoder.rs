//! AudioEncoder - WebCodecs API implementation
//!
//! Provides audio encoding functionality using FFmpeg.
//! See: https://w3c.github.io/webcodecs/#audioencoder-interface

use crate::codec::{
  context::get_audio_encoder_name, AudioEncoderConfig as InternalAudioEncoderConfig,
  AudioSampleBuffer, CodecContext, Frame, Resampler,
};
use crate::ffi::{AVCodecID, AVSampleFormat};
use crate::webcodecs::{AudioData, AudioEncoderConfig, AudioEncoderSupport, EncodedAudioChunk};
use crossbeam::channel::{self, Receiver, Sender};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
  ThreadsafeFunction, ThreadsafeFunctionCallMode, UnknownReturnValue,
};
use napi_derive::napi;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::video_encoder::CodecState;

/// Type alias for output callback (takes chunk and metadata as separate args)
/// Using FnArgs to spread tuple as separate callback arguments per WebCodecs spec
/// Using CalleeHandled: false for direct callbacks without error-first convention
type OutputCallback = ThreadsafeFunction<
  FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
  UnknownReturnValue,
  FnArgs<(EncodedAudioChunk, EncodedAudioChunkMetadata)>,
  Status,
  false,
  true,
>;

/// Type alias for error callback (takes error message)
/// Still using default CalleeHandled: true for error-first convention
type ErrorCallback = ThreadsafeFunction<String, UnknownReturnValue, String, Status, true, true>;

/// AudioEncoder init dictionary per WebCodecs spec
pub struct AudioEncoderInit {
  /// Output callback - called when encoded chunk is available
  pub output: OutputCallback,
  /// Error callback - called when an error occurs
  pub error: ErrorCallback,
}

impl FromNapiValue for AudioEncoderInit {
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

    Ok(AudioEncoderInit { output, error })
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
  /// Sample rate
  pub sample_rate: Option<u32>,
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
}

/// Internal encoder state
struct AudioEncoderInner {
  state: CodecState,
  config: Option<AudioEncoderConfig>,
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
  /// Optional dequeue event callback
  dequeue_callback: Option<ThreadsafeFunction<(), UnknownReturnValue, (), Status, false, true>>,
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
  dequeue_callback: Option<FunctionRef<(), UnknownReturnValue>>,
  /// Channel sender for worker commands
  command_sender: Option<Sender<EncoderCommand>>,
  /// Worker thread handle
  worker_handle: Option<JoinHandle<()>>,
}

impl Drop for AudioEncoder {
  fn drop(&mut self) {
    // Drop the sender to signal the worker to stop.
    // The worker will see the channel disconnect and exit its loop.
    self.command_sender = None;

    // Don't join the worker thread here - it would block the JS thread during GC.
    // Instead, let the thread become detached and finish on its own.
    // Safety: The Arc<Mutex<AudioEncoderInner>> ensures the inner state (including
    // callbacks and FFmpeg context) stays alive until the worker exits.
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
      ts_arg_type = "{ output: (chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata) => void, error: (error: Error) => void }"
    )]
    init: AudioEncoderInit,
  ) -> Result<Self> {
    let inner = AudioEncoderInner {
      state: CodecState::Unconfigured,
      config: None,
      context: None,
      resampler: None,
      sample_buffer: None,
      frame_count: 0,
      extradata_sent: false,
      target_format: AVSampleFormat::Fltp,
      encode_queue_size: 0,
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
  fn worker_loop(inner: Arc<Mutex<AudioEncoderInner>>, receiver: Receiver<EncoderCommand>) {
    while let Ok(command) = receiver.recv() {
      match command {
        EncoderCommand::Encode { frame, timestamp } => {
          Self::process_encode(&inner, frame, timestamp);
        }
        EncoderCommand::Flush(response_sender) => {
          let result = Self::process_flush(&inner);
          let _ = response_sender.send(result);
        }
      }
    }
  }

  /// Process an encode command on the worker thread
  fn process_encode(inner: &Arc<Mutex<AudioEncoderInner>>, frame: Frame, timestamp: i64) {
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
    let codec_string = match guard.config.as_ref() {
      Some(config) => config.codec.clone(),
      None => {
        guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
        Self::report_error(&mut guard, "No encoder config");
        return;
      }
    };

    // Add frame to sample buffer
    {
      let sample_buffer = match guard.sample_buffer.as_mut() {
        Some(buf) => buf,
        None => {
          guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
          let _ = Self::fire_dequeue_event(&guard);
          Self::report_error(&mut guard, "No sample buffer");
          return;
        }
      };

      if let Err(e) = sample_buffer.add_frame(&frame) {
        guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
        let _ = Self::fire_dequeue_event(&guard);
        Self::report_error(&mut guard, &format!("Failed to add samples: {}", e));
        return;
      }
    }

    // Get extradata before encoding first frame
    let extradata = if !guard.extradata_sent {
      guard
        .context
        .as_ref()
        .and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
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
          guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
          let _ = Self::fire_dequeue_event(&guard);
          Self::report_error(&mut guard, "No sample buffer");
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
            guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
            let _ = Self::fire_dequeue_event(&guard);
            Self::report_error(&mut guard, "No sample buffer");
            return;
          }
        };
        match sample_buffer.take_frame() {
          Ok(Some(f)) => f,
          Ok(None) => {
            guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
            let _ = Self::fire_dequeue_event(&guard);
            Self::report_error(&mut guard, "No frame available");
            return;
          }
          Err(e) => {
            guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
            let _ = Self::fire_dequeue_event(&guard);
            Self::report_error(&mut guard, &format!("Failed to get frame: {}", e));
            return;
          }
        }
      };

      // Set timestamp (approximate based on frame count)
      let frame_timestamp = if guard.frame_count == 0 {
        timestamp
      } else {
        timestamp + (guard.frame_count as i64 * frame_size * 1_000_000) / sample_rate
      };
      frame_to_encode.set_pts(frame_timestamp);

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
          guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
          let _ = Self::fire_dequeue_event(&guard);
          Self::report_error(&mut guard, &format!("Encode failed: {}", e));
          return;
        }
      };

      guard.frame_count += 1;

      // Calculate duration per frame in microseconds
      let duration_us = (frame_size * 1_000_000) / sample_rate;

      // Process output packets - call callback for each
      for packet in packets {
        let chunk = EncodedAudioChunk::from_packet(&packet, Some(duration_us));

        // Create metadata
        let metadata = if !guard.extradata_sent {
          guard.extradata_sent = true;
          let (target_sample_rate, target_channels) = guard
            .config
            .as_ref()
            .map(|c| (c.sample_rate, c.number_of_channels))
            .unwrap_or((48000, 2));

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

        guard.output_callback.call(
          (chunk, metadata).into(),
          ThreadsafeFunctionCallMode::NonBlocking,
        );
      }
    }

    // Decrement queue size and fire dequeue event
    guard.encode_queue_size = guard.encode_queue_size.saturating_sub(1);
    let _ = Self::fire_dequeue_event(&guard);
  }

  /// Process a flush command on the worker thread
  fn process_flush(inner: &Arc<Mutex<AudioEncoderInner>>) -> Result<()> {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if guard.state != CodecState::Configured {
      Self::report_error(&mut guard, "Encoder not configured");
      return Ok(());
    }

    // Flush any remaining samples in buffer
    if let Some(ref mut sample_buffer) = guard.sample_buffer {
      if let Ok(Some(mut frame)) = sample_buffer.flush() {
        // Set timestamp
        let frame_size = sample_buffer.frame_size() as i64;
        let sample_rate = sample_buffer.sample_rate() as i64;
        let frame_timestamp = (guard.frame_count as i64 * frame_size * 1_000_000) / sample_rate;
        frame.set_pts(frame_timestamp);

        let context = match guard.context.as_mut() {
          Some(ctx) => ctx,
          None => {
            Self::report_error(&mut guard, "No encoder context");
            return Ok(());
          }
        };

        if let Ok(packets) = context.encode(Some(&frame)) {
          let duration_us = (frame.nb_samples() as i64 * 1_000_000) / sample_rate;
          for packet in packets {
            let chunk = EncodedAudioChunk::from_packet(&packet, Some(duration_us));
            let metadata = EncodedAudioChunkMetadata {
              decoder_config: None,
            };
            guard.output_callback.call(
              (chunk, metadata).into(),
              ThreadsafeFunctionCallMode::NonBlocking,
            );
          }
        }
      }
    }

    // Flush encoder
    let context = match guard.context.as_mut() {
      Some(ctx) => ctx,
      None => {
        Self::report_error(&mut guard, "No encoder context");
        return Ok(());
      }
    };

    let packets = match context.flush_encoder() {
      Ok(pkts) => pkts,
      Err(e) => {
        Self::report_error(&mut guard, &format!("Flush failed: {}", e));
        return Ok(());
      }
    };

    // Process remaining packets - call callback for each
    for packet in packets {
      let chunk = EncodedAudioChunk::from_packet(&packet, None);
      let metadata = EncodedAudioChunkMetadata {
        decoder_config: None,
      };
      guard.output_callback.call(
        (chunk, metadata).into(),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }

    Ok(())
  }

  /// Report an error via callback and close the encoder
  fn report_error(inner: &mut AudioEncoderInner, error_msg: &str) {
    inner.error_callback.call(
      Ok(error_msg.to_string()),
      ThreadsafeFunctionCallMode::NonBlocking,
    );
    inner.state = CodecState::Closed;
  }

  /// Fire dequeue event if callback is set
  fn fire_dequeue_event(inner: &AudioEncoderInner) -> Result<()> {
    if let Some(ref callback) = inner.dequeue_callback {
      callback.call((), ThreadsafeFunctionCallMode::NonBlocking);
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
  pub fn configure(&self, config: AudioEncoderConfig) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.state == CodecState::Closed {
      Self::report_error(&mut inner, "Encoder is closed");
      return Ok(());
    }

    // Parse codec string to determine codec ID
    let codec_id = match parse_audio_codec_string(&config.codec) {
      Ok(id) => id,
      Err(e) => {
        Self::report_error(&mut inner, &format!("Invalid codec: {}", e));
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
        Self::report_error(&mut inner, &format!("Failed to create encoder: {}", e));
        return Ok(());
      }
    };

    // Determine target sample format based on codec
    let target_format = get_encoder_sample_format(codec_id);

    // Configure encoder
    let sample_rate = config.sample_rate;
    let channels = config.number_of_channels;

    let encoder_config = InternalAudioEncoderConfig {
      sample_rate,
      channels,
      sample_format: target_format,
      bitrate: config.bitrate.unwrap_or(128_000.0) as u64,
      thread_count: 0,
    };

    if let Err(e) = context.configure_audio_encoder(&encoder_config) {
      Self::report_error(&mut inner, &format!("Failed to configure encoder: {}", e));
      return Ok(());
    }

    // Open the encoder
    if let Err(e) = context.open() {
      Self::report_error(&mut inner, &format!("Failed to open encoder: {}", e));
      return Ok(());
    }

    // Get the actual frame size from the encoder
    let frame_size = context.frame_size();
    let frame_size = if frame_size == 0 {
      // Some encoders don't set frame_size, use codec default
      AudioSampleBuffer::frame_size_for_codec(&config.codec)
    } else {
      frame_size as usize
    };

    // Create sample buffer
    let sample_buffer = AudioSampleBuffer::new(frame_size, channels, sample_rate, target_format);

    inner.context = Some(context);
    inner.config = Some(config);
    inner.sample_buffer = Some(sample_buffer);
    inner.target_format = target_format;
    inner.state = CodecState::Configured;
    inner.extradata_sent = false;
    inner.frame_count = 0;
    inner.resampler = None;
    inner.encode_queue_size = 0;

    Ok(())
  }

  /// Encode audio data
  #[napi]
  pub fn encode(&self, data: &AudioData) -> Result<()> {
    // Clone frame, resample if needed, and get timestamp on main thread
    let (frame_to_send, timestamp) = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      if inner.state != CodecState::Configured {
        Self::report_error(&mut inner, "Encoder not configured");
        return Ok(());
      }

      // Get config info
      let (target_sample_rate, target_channels) = match inner.config.as_ref() {
        Some(config) => (config.sample_rate, config.number_of_channels),
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

      // Check if we need resampling
      let needs_resampling = src_sample_rate != target_sample_rate
        || src_channels != target_channels
        || src_format.to_av_format() != inner.target_format;

      // Create resampler if needed and not already created
      if needs_resampling && inner.resampler.is_none() {
        match Resampler::new(
          src_channels,
          src_sample_rate,
          src_format.to_av_format(),
          target_channels,
          target_sample_rate,
          inner.target_format,
        ) {
          Ok(resampler) => inner.resampler = Some(resampler),
          Err(e) => {
            Self::report_error(&mut inner, &format!("Failed to create resampler: {}", e));
            return Ok(());
          }
        }
      }

      // Get frame from AudioData
      let frame = match data.with_frame(|f| f.try_clone()) {
        Ok(Ok(f)) => f,
        Ok(Err(e)) => {
          Self::report_error(&mut inner, &format!("Failed to clone frame: {}", e));
          return Ok(());
        }
        Err(e) => {
          Self::report_error(&mut inner, &format!("Failed to get frame: {}", e));
          return Ok(());
        }
      };

      // Resample if needed
      let frame_to_send = if let Some(ref mut resampler) = inner.resampler {
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

      (frame_to_send, timestamp)
    };

    // Send encode command to worker thread
    if let Some(ref sender) = self.command_sender {
      sender
        .send(EncoderCommand::Encode {
          frame: frame_to_send,
          timestamp,
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

    // Drop existing context
    inner.context = None;
    inner.resampler = None;
    inner.sample_buffer = None;
    inner.config = None;
    inner.state = CodecState::Unconfigured;
    inner.frame_count = 0;
    inner.extradata_sent = false;
    inner.encode_queue_size = 0;

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

    inner.context = None;
    inner.resampler = None;
    inner.sample_buffer = None;
    inner.config = None;
    inner.state = CodecState::Closed;
    inner.encode_queue_size = 0;

    Ok(())
  }

  /// Check if a configuration is supported
  /// Returns a Promise that resolves with support information
  #[napi]
  pub async fn is_config_supported(config: AudioEncoderConfig) -> Result<AudioEncoderSupport> {
    // Parse codec string
    let codec_id = match parse_audio_codec_string(&config.codec) {
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
