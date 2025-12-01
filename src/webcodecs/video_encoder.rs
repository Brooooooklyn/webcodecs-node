//! VideoEncoder - WebCodecs API implementation
//!
//! Provides video encoding functionality using FFmpeg.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/VideoEncoder

use crate::codec::{BitrateMode, CodecContext, EncoderConfig, Scaler};
use crate::ffi::{AVCodecID, AVHWDeviceType, AVPixelFormat};
use crate::webcodecs::{EncodedVideoChunk, VideoEncoderConfig, VideoFrame};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// Encoder state
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecState {
    /// Encoder not configured
    #[default]
    Unconfigured,
    /// Encoder configured and ready
    Configured,
    /// Encoder closed
    Closed,
}

/// Output callback metadata
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
    /// Codec description (e.g., avcC for H.264)
    pub description: Option<Buffer>,
}

/// Encode options
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoEncoderEncodeOptions {
    /// Force this frame to be a keyframe
    pub key_frame: Option<bool>,
}

/// Result of isConfigSupported
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoEncoderSupport {
    /// Whether the configuration is supported
    pub supported: bool,
    /// The configuration that was checked
    pub config: VideoEncoderConfig,
}

/// Type alias for output callback (takes chunk and metadata)
type OutputCallback = ThreadsafeFunction<(EncodedVideoChunk, EncodedVideoChunkMetadata)>;

/// Type alias for error callback (takes error message)
type ErrorCallback = ThreadsafeFunction<String>;

/// Internal encoder state
struct VideoEncoderInner {
    state: CodecState,
    config: Option<VideoEncoderConfig>,
    context: Option<CodecContext>,
    scaler: Option<Scaler>,
    frame_count: u64,
    extradata_sent: bool,
    /// Queued output chunks (for synchronous retrieval)
    output_queue: Vec<(EncodedVideoChunk, EncodedVideoChunkMetadata)>,
    /// Optional output callback (WebCodecs spec compliant mode)
    output_callback: Option<OutputCallback>,
    /// Optional error callback (WebCodecs spec compliant mode)
    error_callback: Option<ErrorCallback>,
}

/// VideoEncoder - WebCodecs-compliant video encoder
///
/// Encodes VideoFrame objects into EncodedVideoChunk objects using FFmpeg.
///
/// Note: This implementation uses a synchronous output queue model instead of
/// callbacks for simpler integration. Use `takeEncodedChunks()` to retrieve
/// encoded output after calling `encode()` or `flush()`.
#[napi]
pub struct VideoEncoder {
    inner: Arc<Mutex<VideoEncoderInner>>,
}

#[napi]
impl VideoEncoder {
    /// Create a new VideoEncoder (queue-based mode)
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        let inner = VideoEncoderInner {
            state: CodecState::Unconfigured,
            config: None,
            context: None,
            scaler: None,
            frame_count: 0,
            extradata_sent: false,
            output_queue: Vec::new(),
            output_callback: None,
            error_callback: None,
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Create a VideoEncoder with callbacks (WebCodecs spec compliant mode)
    ///
    /// In this mode, encoded chunks are delivered via the output callback
    /// instead of being queued for retrieval. Errors are reported via the
    /// error callback and the encoder transitions to the Closed state.
    ///
    /// Example:
    /// ```javascript
    /// const encoder = VideoEncoder.withCallbacks(
    ///   (chunk, metadata) => { /* handle output */ },
    ///   (error) => { /* handle error */ }
    /// );
    /// ```
    #[napi(factory)]
    pub fn with_callbacks(
        output: ThreadsafeFunction<(EncodedVideoChunk, EncodedVideoChunkMetadata)>,
        error: ThreadsafeFunction<String>,
    ) -> Result<Self> {
        let inner = VideoEncoderInner {
            state: CodecState::Unconfigured,
            config: None,
            context: None,
            scaler: None,
            frame_count: 0,
            extradata_sent: false,
            output_queue: Vec::new(),
            output_callback: Some(output),
            error_callback: Some(error),
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Report an error via callback (if in callback mode) and close the encoder
    /// Returns true if error was reported via callback, false if should return error
    fn report_error(inner: &mut VideoEncoderInner, error_msg: &str) -> bool {
        if let Some(ref callback) = inner.error_callback {
            callback.call(Ok(error_msg.to_string()), ThreadsafeFunctionCallMode::NonBlocking);
            inner.state = CodecState::Closed;
            true
        } else {
            false
        }
    }

    /// Get encoder state
    #[napi(getter)]
    pub fn state(&self) -> Result<CodecState> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.state)
    }

    /// Get number of pending output chunks
    #[napi(getter)]
    pub fn encode_queue_size(&self) -> Result<u32> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.output_queue.len() as u32)
    }

    /// Configure the encoder
    #[napi]
    pub fn configure(&self, config: VideoEncoderConfig) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Encoder is closed"));
        }

        // Parse codec string to determine codec ID
        let codec_id = parse_codec_string(&config.codec)?;

        // Determine hardware acceleration
        let hw_type = config.hardware_acceleration.as_ref().and_then(|ha| {
            match ha.as_str() {
                "prefer-hardware" | "require-hardware" => {
                    #[cfg(target_os = "macos")]
                    { Some(AVHWDeviceType::Videotoolbox) }
                    #[cfg(not(target_os = "macos"))]
                    { Some(AVHWDeviceType::Cuda) }
                }
                _ => None,
            }
        });

        // Create encoder context
        let mut context = CodecContext::new_encoder_with_hw(codec_id, hw_type).map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to create encoder: {}", e))
        })?;

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

        // Parse scalability mode (e.g., "L1T1", "L1T2", "L1T3")
        // Note: Temporal SVC support varies by codec and FFmpeg build
        let _scalability = config
            .scalability_mode
            .as_ref()
            .and_then(|mode| parse_scalability_mode(mode));
        // TODO: Apply temporal layer settings when supported by the codec
        // VP9: Use "ts-layering" option
        // AV1: Use "temporal-layering" option

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
            rc_max_rate: None, // Could be exposed via config later
            rc_buffer_size: None,
            crf: None, // Will use codec-specific defaults
        };

        context.configure_encoder(&encoder_config).map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to configure encoder: {}", e))
        })?;

        // Open the encoder
        context.open().map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to open encoder: {}", e))
        })?;

        inner.context = Some(context);
        inner.config = Some(config);
        inner.state = CodecState::Configured;
        inner.extradata_sent = false;
        inner.frame_count = 0;
        inner.output_queue.clear();

        Ok(())
    }

    /// Encode a frame
    #[napi]
    pub fn encode(&self, frame: &VideoFrame, _options: Option<VideoEncoderEncodeOptions>) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            let msg = "Encoder not configured";
            if Self::report_error(&mut inner, msg) {
                return Ok(());
            }
            return Err(Error::new(Status::GenericFailure, msg));
        }

        // Get config info first (clone to avoid borrow issues)
        let (width, height, codec_string) = match inner.config.as_ref() {
            Some(config) => (config.width, config.height, config.codec.clone()),
            None => {
                let msg = "No encoder config";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Get frame data from VideoFrame
        let (internal_frame, needs_conversion) = match frame.with_frame(|f| {
            let frame_format = f.format();
            let needs_conv = frame_format != AVPixelFormat::Yuv420p
                || f.width() != width
                || f.height() != height;
            (f.try_clone(), needs_conv)
        }) {
            Ok(result) => result,
            Err(e) => {
                let msg = format!("Failed to access frame: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };

        let internal_frame = match internal_frame {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("Failed to clone frame: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
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
                        let msg = format!("Failed to create scaler: {}", e);
                        if Self::report_error(&mut inner, &msg) {
                            return Ok(());
                        }
                        return Err(Error::new(Status::GenericFailure, msg));
                    }
                }
            }

            let scaler = inner.scaler.as_ref().unwrap();
            match scaler.scale_alloc(&internal_frame) {
                Ok(scaled) => scaled,
                Err(e) => {
                    let msg = format!("Failed to scale frame: {}", e);
                    if Self::report_error(&mut inner, &msg) {
                        return Ok(());
                    }
                    return Err(Error::new(Status::GenericFailure, msg));
                }
            }
        } else {
            internal_frame
        };

        // Set frame PTS based on timestamp
        let pts = match frame.timestamp() {
            Ok(ts) => ts,
            Err(e) => {
                let msg = format!("Failed to get frame timestamp: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(e);
            }
        };
        frame_to_encode.set_pts(pts);

        // Get extradata before encoding
        let extradata_sent = inner.extradata_sent;
        let extradata = if !extradata_sent {
            inner.context.as_ref().and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
        } else {
            None
        };

        // Encode the frame
        let context = match inner.context.as_mut() {
            Some(ctx) => ctx,
            None => {
                let msg = "No encoder context";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        let packets = match context.encode(Some(&frame_to_encode)) {
            Ok(pkts) => pkts,
            Err(e) => {
                let msg = format!("Encode failed: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        inner.frame_count += 1;

        // Process output packets - either call callback or queue them
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
                        description: extradata.clone().map(Buffer::from),
                    }),
                }
            } else {
                EncodedVideoChunkMetadata {
                    decoder_config: None,
                }
            };

            // Dispatch output via callback or queue
            if let Some(ref callback) = inner.output_callback {
                callback.call(Ok((chunk, metadata)), ThreadsafeFunctionCallMode::NonBlocking);
            } else {
                inner.output_queue.push((chunk, metadata));
            }
        }

        Ok(())
    }

    /// Flush the encoder and return all remaining chunks
    /// Returns a Promise that resolves when flushing is complete
    #[napi]
    pub async fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            let msg = "Encoder not configured";
            if Self::report_error(&mut inner, msg) {
                return Ok(());
            }
            return Err(Error::new(Status::GenericFailure, msg));
        }

        let context = match inner.context.as_mut() {
            Some(ctx) => ctx,
            None => {
                let msg = "No encoder context";
                if Self::report_error(&mut inner, msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Flush encoder
        let packets = match context.flush_encoder() {
            Ok(pkts) => pkts,
            Err(e) => {
                let msg = format!("Flush failed: {}", e);
                if Self::report_error(&mut inner, &msg) {
                    return Ok(());
                }
                return Err(Error::new(Status::GenericFailure, msg));
            }
        };

        // Process remaining packets - either call callback or queue them
        for packet in packets {
            let chunk = EncodedVideoChunk::from_packet(&packet);
            let metadata = EncodedVideoChunkMetadata {
                decoder_config: None,
            };

            // Dispatch output via callback or queue
            if let Some(ref callback) = inner.output_callback {
                callback.call(Ok((chunk, metadata)), ThreadsafeFunctionCallMode::NonBlocking);
            } else {
                inner.output_queue.push((chunk, metadata));
            }
        }

        Ok(())
    }

    /// Take all encoded chunks from the output queue
    ///
    /// Returns an array of [chunk, metadata] pairs
    #[napi]
    pub fn take_encoded_chunks(&self) -> Result<Vec<EncodedVideoChunk>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        let chunks: Vec<EncodedVideoChunk> = inner.output_queue
            .drain(..)
            .map(|(chunk, _)| chunk)
            .collect();

        Ok(chunks)
    }

    /// Check if there are any pending encoded chunks
    #[napi]
    pub fn has_output(&self) -> Result<bool> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(!inner.output_queue.is_empty())
    }

    /// Take the next encoded chunk from the output queue (if any)
    #[napi]
    pub fn take_next_chunk(&self) -> Result<Option<EncodedVideoChunk>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.output_queue.is_empty() {
            Ok(None)
        } else {
            let (chunk, _) = inner.output_queue.remove(0);
            Ok(Some(chunk))
        }
    }

    /// Reset the encoder
    #[napi]
    pub fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Encoder is closed"));
        }

        // Drop existing context
        inner.context = None;
        inner.scaler = None;
        inner.config = None;
        inner.state = CodecState::Unconfigured;
        inner.frame_count = 0;
        inner.extradata_sent = false;
        inner.output_queue.clear();

        Ok(())
    }

    /// Close the encoder
    #[napi]
    pub fn close(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        inner.context = None;
        inner.scaler = None;
        inner.config = None;
        inner.state = CodecState::Closed;
        inner.output_queue.clear();

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
    } else if codec_lower.starts_with("hev1") || codec_lower.starts_with("hvc1") || codec_lower == "h265" || codec_lower == "hevc" {
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

/// Parse scalability mode string (e.g., "L1T1", "L1T2", "L1T3")
/// Returns (spatial_layers, temporal_layers)
fn parse_scalability_mode(mode: &str) -> Option<(u32, u32)> {
    let mode_upper = mode.to_uppercase();

    // Parse LxTy format (e.g., L1T1, L1T2, L1T3, L2T1, etc.)
    if mode_upper.starts_with('L') && mode_upper.contains('T') {
        let parts: Vec<&str> = mode_upper.split('T').collect();
        if parts.len() == 2 {
            let spatial = parts[0].trim_start_matches('L').parse::<u32>().ok()?;
            let temporal = parts[1].chars().next()?.to_digit(10)?;
            return Some((spatial, temporal));
        }
    }

    None
}
