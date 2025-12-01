//! ImageDecoder - WebCodecs API implementation
//!
//! Provides image decoding functionality using FFmpeg.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder

use crate::codec::{CodecContext, DecoderConfig, Packet};
use crate::ffi::AVCodecID;
use crate::webcodecs::VideoFrame;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// ImageDecoder init options
#[napi(object)]
pub struct ImageDecoderInit {
    /// The image data (encoded bytes)
    pub data: Buffer,
    /// MIME type of the image (e.g., "image/png", "image/jpeg")
    pub mime_type: String,
    /// Color space conversion hint (optional)
    pub color_space_conversion: Option<String>,
    /// Desired width (optional, for scaling)
    pub desired_width: Option<u32>,
    /// Desired height (optional, for scaling)
    pub desired_height: Option<u32>,
    /// Whether to prefer animation (for animated formats)
    pub prefer_animation: Option<bool>,
}

/// Image decode options
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct ImageDecodeOptions {
    /// Frame index to decode (for animated images)
    pub frame_index: Option<u32>,
    /// Whether to only decode complete frames
    pub complete_frames_only: Option<bool>,
}

/// Image decode result
#[napi]
pub struct ImageDecodeResult {
    /// The decoded image as a VideoFrame
    image: VideoFrame,
    /// Whether the image is fully decoded
    complete: bool,
}

#[napi]
impl ImageDecodeResult {
    /// Get the decoded image
    #[napi(getter)]
    pub fn image(&self) -> Result<VideoFrame> {
        self.image.clone_frame()
    }

    /// Get whether the decode is complete
    #[napi(getter)]
    pub fn complete(&self) -> bool {
        self.complete
    }
}

/// Image track information
#[napi(object)]
#[derive(Debug, Clone)]
pub struct ImageTrack {
    /// Whether this track is animated
    pub animated: bool,
    /// Number of frames in this track
    pub frame_count: u32,
    /// Number of times the animation repeats (Infinity for infinite)
    pub repetition_count: f64,
    /// Whether this track is currently selected
    pub selected: bool,
}

/// Image track list
#[napi]
#[derive(Debug, Clone)]
pub struct ImageTrackList {
    tracks: Vec<ImageTrack>,
    selected_index: Option<usize>,
}

#[napi]
impl ImageTrackList {
    /// Get the number of tracks
    #[napi(getter)]
    pub fn length(&self) -> u32 {
        self.tracks.len() as u32
    }

    /// Get the currently selected track (if any)
    #[napi(getter)]
    pub fn selected_track(&self) -> Option<ImageTrack> {
        self.selected_index.and_then(|i| self.tracks.get(i).cloned())
    }

    /// Get the selected track index
    #[napi(getter)]
    pub fn selected_index(&self) -> Option<u32> {
        self.selected_index.map(|i| i as u32)
    }
}

/// Internal decoder state
struct ImageDecoderInner {
    /// Original encoded data
    data: Vec<u8>,
    /// MIME type
    mime_type: String,
    /// Codec ID for decoding
    codec_id: AVCodecID,
    /// Decoder context (created on first decode)
    context: Option<CodecContext>,
    /// Whether data is fully buffered
    complete: bool,
    /// Track list
    tracks: ImageTrackList,
    /// Whether decoder is closed
    closed: bool,
}

/// ImageDecoder - WebCodecs-compliant image decoder
///
/// Decodes image data (JPEG, PNG, WebP, GIF, BMP) into VideoFrame objects.
///
/// Example:
/// ```javascript
/// const decoder = new ImageDecoder({
///   data: imageBytes,
///   type: 'image/png'
/// });
///
/// const result = await decoder.decode();
/// const frame = result.image;
/// ```
#[napi]
pub struct ImageDecoder {
    inner: Arc<Mutex<ImageDecoderInner>>,
}

#[napi]
impl ImageDecoder {
    /// Create a new ImageDecoder
    #[napi(constructor)]
    pub fn new(init: ImageDecoderInit) -> Result<Self> {
        // Parse MIME type to codec ID
        let codec_id = parse_mime_type(&init.mime_type)?;

        // Determine if this is an animated format
        let animated = matches!(codec_id, AVCodecID::Gif | AVCodecID::Webp);

        // For static images, there's one frame; for animated, we'll detect later
        let frame_count = if animated { 0 } else { 1 };

        let tracks = ImageTrackList {
            tracks: vec![ImageTrack {
                animated,
                frame_count,
                repetition_count: if animated { f64::INFINITY } else { 0.0 },
                selected: true,
            }],
            selected_index: Some(0),
        };

        let inner = ImageDecoderInner {
            data: init.data.to_vec(),
            mime_type: init.mime_type,
            codec_id,
            context: None,
            complete: true, // Data is complete (we have Buffer, not stream)
            tracks,
            closed: false,
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Whether the data is fully buffered
    #[napi(getter)]
    pub fn complete(&self) -> Result<bool> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.complete)
    }

    /// Get the MIME type
    #[napi(getter, js_name = "type")]
    pub fn get_type(&self) -> Result<String> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.mime_type.clone())
    }

    /// Get the track list
    #[napi(getter)]
    pub fn tracks(&self) -> Result<ImageTrackList> {
        let inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;
        Ok(inner.tracks.clone())
    }

    /// Decode the image (or a specific frame)
    #[napi]
    pub async fn decode(&self, options: Option<ImageDecodeOptions>) -> Result<ImageDecodeResult> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.closed {
            return Err(Error::new(Status::GenericFailure, "ImageDecoder is closed"));
        }

        let _frame_index = options
            .as_ref()
            .and_then(|o| o.frame_index)
            .unwrap_or(0);

        // Create decoder context if not already created
        if inner.context.is_none() {
            let mut context = CodecContext::new_decoder(inner.codec_id).map_err(|e| {
                Error::new(
                    Status::GenericFailure,
                    format!("Failed to create decoder: {}", e),
                )
            })?;

            let decoder_config = DecoderConfig {
                codec_id: inner.codec_id,
                thread_count: 0,
                extradata: None,
            };

            context.configure_decoder(&decoder_config).map_err(|e| {
                Error::new(
                    Status::GenericFailure,
                    format!("Failed to configure decoder: {}", e),
                )
            })?;

            context.open().map_err(|e| {
                Error::new(
                    Status::GenericFailure,
                    format!("Failed to open decoder: {}", e),
                )
            })?;

            inner.context = Some(context);
        }

        // Clone data before mutable borrow
        let data = inner.data.clone();

        // Decode the image data
        let context = inner.context.as_mut().unwrap();
        let frames = decode_image_data(context, &data)?;

        if frames.is_empty() {
            return Err(Error::new(
                Status::GenericFailure,
                "No frames decoded from image",
            ));
        }

        // Take the first frame (or requested frame for animated images)
        let frame = frames.into_iter().next().unwrap();
        let pts = frame.pts();
        let video_frame = VideoFrame::from_internal(frame, pts, None);

        Ok(ImageDecodeResult {
            image: video_frame,
            complete: true,
        })
    }

    /// Reset the decoder
    #[napi]
    pub fn reset(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        if inner.closed {
            return Err(Error::new(Status::GenericFailure, "ImageDecoder is closed"));
        }

        inner.context = None;
        Ok(())
    }

    /// Close the decoder
    #[napi]
    pub fn close(&self) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        inner.context = None;
        inner.closed = true;
        Ok(())
    }

    /// Check if a MIME type is supported
    #[napi]
    pub async fn is_type_supported(mime_type: String) -> bool {
        parse_mime_type(&mime_type).is_ok()
    }
}

/// Parse MIME type to FFmpeg codec ID
fn parse_mime_type(mime_type: &str) -> Result<AVCodecID> {
    let mime_lower = mime_type.to_lowercase();

    if mime_lower == "image/jpeg" || mime_lower == "image/jpg" {
        return Ok(AVCodecID::Mjpeg);
    }
    if mime_lower == "image/png" {
        return Ok(AVCodecID::Png);
    }
    if mime_lower == "image/webp" {
        return Ok(AVCodecID::Webp);
    }
    if mime_lower == "image/gif" {
        return Ok(AVCodecID::Gif);
    }
    if mime_lower == "image/bmp" || mime_lower == "image/x-bmp" {
        return Ok(AVCodecID::Bmp);
    }
    if mime_lower == "image/avif" {
        // AVIF uses AV1 codec
        return Ok(AVCodecID::Av1);
    }

    Err(Error::new(
        Status::GenericFailure,
        format!("Unsupported image type: {}", mime_type),
    ))
}

/// Decode image data using FFmpeg
fn decode_image_data(
    context: &mut CodecContext,
    data: &[u8],
) -> Result<Vec<crate::codec::Frame>> {
    // Create a packet with the image data
    let mut packet = Packet::new().map_err(|e| {
        Error::new(
            Status::GenericFailure,
            format!("Failed to create packet: {}", e),
        )
    })?;

    // Set packet data
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

    // Flush to get any remaining frames
    let mut all_frames = frames;
    if let Ok(flushed) = context.flush_decoder() {
        all_frames.extend(flushed);
    }

    Ok(all_frames)
}
