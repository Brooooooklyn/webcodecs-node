//! VideoDecoder - WebCodecs API implementation
//!
//! Provides video decoding functionality using FFmpeg.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/VideoDecoder

use crate::codec::{CodecContext, DecoderConfig, Frame, Packet};
use crate::ffi::{AVCodecID, AVHWDeviceType};
use crate::webcodecs::{CodecState, EncodedVideoChunk, VideoDecoderConfig, VideoFrame};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// Result of isConfigSupported
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoDecoderSupport {
    /// Whether the configuration is supported
    pub supported: bool,
    /// The configuration that was checked (codec only for simplicity)
    pub codec: String,
}

/// Internal decoder state
struct VideoDecoderInner {
    state: CodecState,
    config: Option<DecoderConfig>,
    context: Option<CodecContext>,
    codec_string: String,
    frame_count: u64,
    /// Queued output frames (for synchronous retrieval)
    output_queue: Vec<VideoFrame>,
}

/// VideoDecoder - WebCodecs-compliant video decoder
///
/// Decodes EncodedVideoChunk objects into VideoFrame objects using FFmpeg.
///
/// Note: This implementation uses a synchronous output queue model instead of
/// callbacks for simpler integration. Use `takeDecodedFrames()` to retrieve
/// decoded output after calling `decode()` or `flush()`.
#[napi]
pub struct VideoDecoder {
    inner: Arc<Mutex<VideoDecoderInner>>,
}

#[napi]
impl VideoDecoder {
    /// Create a new VideoDecoder
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        let inner = VideoDecoderInner {
            state: CodecState::Unconfigured,
            config: None,
            context: None,
            codec_string: String::new(),
            frame_count: 0,
            output_queue: Vec::new(),
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Get decoder state
    #[napi(getter)]
    pub fn state(&self) -> Result<CodecState> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.state)
    }

    /// Get number of pending output frames
    #[napi(getter)]
    pub fn decode_queue_size(&self) -> Result<u32> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.output_queue.len() as u32)
    }

    /// Configure the decoder
    #[napi]
    pub fn configure(&self, config: VideoDecoderConfig) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Decoder is closed"));
        }

        // Parse codec string to determine codec ID
        let codec_id = parse_codec_string(&config.codec)?;

        // Determine hardware acceleration
        let hw_type = config.hardware_acceleration.as_ref().and_then(|ha| {
            parse_hw_acceleration(ha)
        });

        // Create decoder context with optional hardware acceleration
        let mut context = CodecContext::new_decoder_with_hw(codec_id, hw_type).map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to create decoder: {}", e))
        })?;

        // Configure decoder
        let decoder_config = DecoderConfig {
            codec_id,
            thread_count: 0, // Auto
            extradata: config.description.as_ref().map(|d| d.to_vec()),
        };

        context.configure_decoder(&decoder_config).map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to configure decoder: {}", e))
        })?;

        // Open the decoder
        context.open().map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to open decoder: {}", e))
        })?;

        inner.context = Some(context);
        inner.config = Some(decoder_config);
        inner.codec_string = config.codec;
        inner.state = CodecState::Configured;
        inner.frame_count = 0;
        inner.output_queue.clear();

        Ok(())
    }

    /// Decode an encoded video chunk
    #[napi]
    pub fn decode(&self, chunk: &EncodedVideoChunk) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            return Err(Error::new(
                Status::GenericFailure,
                "Decoder not configured",
            ));
        }

        // Get chunk data
        let data = chunk.get_data_vec()?;
        let timestamp = chunk.timestamp()?;
        let duration = chunk.duration()?;

        // Get context
        let context = inner.context.as_mut().ok_or_else(|| {
            Error::new(Status::GenericFailure, "No decoder context")
        })?;

        // Create a temporary packet with the chunk data
        // This is a workaround since we can't easily set packet data
        // In practice, we'd need proper packet allocation

        // For now, send the packet directly using raw FFmpeg API
        // This is simplified - a production implementation would properly
        // allocate packet buffers

        // Decode using the internal frame
        let frames = decode_chunk_data(context, &data, timestamp, duration)?;

        inner.frame_count += 1;

        // Convert internal frames to VideoFrames and queue them
        for frame in frames {
            let video_frame = VideoFrame::from_internal(
                frame,
                timestamp,
                duration,
            );
            inner.output_queue.push(video_frame);
        }

        Ok(())
    }

    /// Flush the decoder and return all remaining frames
    #[napi]
    pub fn flush(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state != CodecState::Configured {
            return Err(Error::new(
                Status::GenericFailure,
                "Decoder not configured",
            ));
        }

        let context = inner.context.as_mut().ok_or_else(|| {
            Error::new(Status::GenericFailure, "No decoder context")
        })?;

        // Flush decoder
        let frames = context.flush_decoder().map_err(|e| {
            Error::new(Status::GenericFailure, format!("Flush failed: {}", e))
        })?;

        // Convert and queue remaining frames
        for frame in frames {
            let pts = frame.pts();
            let duration = if frame.duration() > 0 { Some(frame.duration()) } else { None };
            let video_frame = VideoFrame::from_internal(frame, pts, duration);
            inner.output_queue.push(video_frame);
        }

        Ok(())
    }

    /// Take all decoded frames from the output queue
    #[napi]
    pub fn take_decoded_frames(&self) -> Result<Vec<VideoFrame>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        let frames: Vec<VideoFrame> = inner.output_queue.drain(..).collect();
        Ok(frames)
    }

    /// Check if there are any pending decoded frames
    #[napi]
    pub fn has_output(&self) -> Result<bool> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(!inner.output_queue.is_empty())
    }

    /// Take the next decoded frame from the output queue (if any)
    #[napi]
    pub fn take_next_frame(&self) -> Result<Option<VideoFrame>> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.output_queue.is_empty() {
            Ok(None)
        } else {
            let frame = inner.output_queue.remove(0);
            Ok(Some(frame))
        }
    }

    /// Reset the decoder
    #[napi]
    pub fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.state == CodecState::Closed {
            return Err(Error::new(Status::GenericFailure, "Decoder is closed"));
        }

        // Drop existing context
        inner.context = None;
        inner.config = None;
        inner.codec_string.clear();
        inner.state = CodecState::Unconfigured;
        inner.frame_count = 0;
        inner.output_queue.clear();

        Ok(())
    }

    /// Close the decoder
    #[napi]
    pub fn close(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        inner.context = None;
        inner.config = None;
        inner.codec_string.clear();
        inner.state = CodecState::Closed;
        inner.output_queue.clear();

        Ok(())
    }

    /// Check if a configuration is supported
    #[napi]
    pub fn is_config_supported(config: VideoDecoderConfig) -> Result<VideoDecoderSupport> {
        // Parse codec string
        let codec_id = match parse_codec_string(&config.codec) {
            Ok(id) => id,
            Err(_) => {
                return Ok(VideoDecoderSupport {
                    supported: false,
                    codec: config.codec,
                });
            }
        };

        // Try to create decoder
        let result = CodecContext::new_decoder(codec_id);

        Ok(VideoDecoderSupport {
            supported: result.is_ok(),
            codec: config.codec,
        })
    }
}

/// Parse WebCodecs codec string to FFmpeg codec ID
fn parse_codec_string(codec: &str) -> Result<AVCodecID> {
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

/// Parse hardware acceleration preference string to device type
fn parse_hw_acceleration(ha: &str) -> Option<AVHWDeviceType> {
    match ha {
        "prefer-hardware" | "require-hardware" => {
            // Return platform-preferred hardware type
            #[cfg(target_os = "macos")]
            { Some(AVHWDeviceType::Videotoolbox) }
            #[cfg(target_os = "linux")]
            { Some(AVHWDeviceType::Vaapi) }
            #[cfg(target_os = "windows")]
            { Some(AVHWDeviceType::D3d11va) }
            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            { None }
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
        Error::new(Status::GenericFailure, format!("Failed to create packet: {}", e))
    })?;

    // Set packet timestamps
    packet.set_pts(timestamp);
    packet.set_dts(timestamp);
    if let Some(dur) = duration {
        packet.set_duration(dur);
    }

    // We need to use FFmpeg's packet allocation to properly set the data
    // For now, we'll use a workaround by sending raw data
    // This requires proper av_packet_from_data or similar

    // Simplified approach: use the packet directly
    // Note: In production, this would need proper packet buffer management

    // For this implementation, we'll use av_new_packet to allocate data
    unsafe {
        use crate::ffi::avcodec::av_new_packet;

        let ret = av_new_packet(packet.as_mut_ptr(), data.len() as i32);
        if ret < 0 {
            return Err(Error::new(
                Status::GenericFailure,
                format!("Failed to allocate packet data: {}", ret),
            ));
        }

        // Copy data to packet
        let pkt_data = packet.data() as *mut u8;
        std::ptr::copy_nonoverlapping(data.as_ptr(), pkt_data, data.len());
    }

    // Decode
    let frames = context.decode(Some(&packet)).map_err(|e| {
        Error::new(Status::GenericFailure, format!("Decode failed: {}", e))
    })?;

    Ok(frames)
}
