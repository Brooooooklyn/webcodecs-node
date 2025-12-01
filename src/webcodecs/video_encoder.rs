//! VideoEncoder - WebCodecs API implementation
//!
//! Provides video encoding functionality using FFmpeg.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/VideoEncoder

use crate::codec::{CodecContext, EncoderConfig, Scaler};
use crate::ffi::{AVCodecID, AVHWDeviceType, AVPixelFormat};
use crate::webcodecs::{EncodedVideoChunk, VideoEncoderConfig, VideoFrame};
use napi::bindgen_prelude::*;
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
    /// Create a new VideoEncoder
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
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
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

        // Configure encoder
        let encoder_config = EncoderConfig {
            width: config.width,
            height: config.height,
            pixel_format: AVPixelFormat::Yuv420p, // Most encoders need YUV420p
            bitrate: config.bitrate.unwrap_or(5_000_000.0) as u64,
            framerate_num: config.framerate.unwrap_or(30.0) as u32,
            framerate_den: 1,
            gop_size: 60, // 2 seconds at 30fps
            max_b_frames: 2,
            thread_count: 0, // Auto
            profile: None,
            level: None,
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
            return Err(Error::new(
                Status::GenericFailure,
                "Encoder not configured",
            ));
        }

        // Get config info first (clone to avoid borrow issues)
        let (width, height, codec_string) = {
            let config = inner.config.as_ref().ok_or_else(|| {
                Error::new(Status::GenericFailure, "No encoder config")
            })?;
            (config.width, config.height, config.codec.clone())
        };

        // Get frame data from VideoFrame
        let (internal_frame, needs_conversion) = frame.with_frame(|f| {
            let frame_format = f.format();
            let needs_conv = frame_format != AVPixelFormat::Yuv420p
                || f.width() != width
                || f.height() != height;
            (f.try_clone(), needs_conv)
        })?;

        let internal_frame = internal_frame.map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to clone frame: {}", e))
        })?;

        // Convert frame if needed
        let mut frame_to_encode = if needs_conversion {
            // Create scaler if needed
            if inner.scaler.is_none() {
                let src_format = internal_frame.format();
                let scaler = Scaler::new(
                    internal_frame.width(),
                    internal_frame.height(),
                    src_format,
                    width,
                    height,
                    AVPixelFormat::Yuv420p,
                    crate::codec::scaler::ScaleAlgorithm::Bilinear,
                ).map_err(|e| {
                    Error::new(Status::GenericFailure, format!("Failed to create scaler: {}", e))
                })?;
                inner.scaler = Some(scaler);
            }

            let scaler = inner.scaler.as_ref().unwrap();
            scaler.scale_alloc(&internal_frame).map_err(|e| {
                Error::new(Status::GenericFailure, format!("Failed to scale frame: {}", e))
            })?
        } else {
            internal_frame
        };

        // Set frame PTS based on timestamp
        let pts = frame.timestamp()?;
        frame_to_encode.set_pts(pts);

        // Get extradata before encoding
        let extradata_sent = inner.extradata_sent;
        let extradata = if !extradata_sent {
            inner.context.as_ref().and_then(|ctx| ctx.extradata().map(|d| d.to_vec()))
        } else {
            None
        };

        // Encode the frame
        let context = inner.context.as_mut().ok_or_else(|| {
            Error::new(Status::GenericFailure, "No encoder context")
        })?;

        let packets = context.encode(Some(&frame_to_encode)).map_err(|e| {
            Error::new(Status::GenericFailure, format!("Encode failed: {}", e))
        })?;

        inner.frame_count += 1;

        // Process output packets and queue them
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

            inner.output_queue.push((chunk, metadata));
        }

        Ok(())
    }

    /// Flush the encoder and return all remaining chunks
    #[napi]
    pub fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            return Err(Error::new(
                Status::GenericFailure,
                "Encoder not configured",
            ));
        }

        let context = inner.context.as_mut().ok_or_else(|| {
            Error::new(Status::GenericFailure, "No encoder context")
        })?;

        // Flush encoder
        let packets = context.flush_encoder().map_err(|e| {
            Error::new(Status::GenericFailure, format!("Flush failed: {}", e))
        })?;

        // Process remaining packets
        for packet in packets {
            let chunk = EncodedVideoChunk::from_packet(&packet);
            let metadata = EncodedVideoChunkMetadata {
                decoder_config: None,
            };
            inner.output_queue.push((chunk, metadata));
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
    #[napi]
    pub fn is_config_supported(config: VideoEncoderConfig) -> Result<VideoEncoderSupport> {
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
