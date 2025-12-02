//! ImageDecoder - WebCodecs API implementation
//!
//! Provides image decoding functionality using FFmpeg.
//! See: <https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder>

use crate::codec::{CodecContext, DecoderConfig, Packet};
use crate::ffi::AVCodecID;
use crate::webcodecs::VideoFrame;
use napi::bindgen_prelude::*;
use napi::tokio_stream::StreamExt;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// Data source for ImageDecoder - can be either a Uint8Array or ReadableStream
pub enum ImageDecoderData {
  /// Buffered data (Uint8Array)
  Buffer(Vec<u8>),
  /// Streaming data (ReadableStream) - data will be read incrementally
  Stream(Vec<u8>), // Stores collected stream data
}

/// ImageDecoder init options
/// Per W3C spec, `data` can be either a BufferSource or a ReadableStream
pub struct ImageDecoderInit {
  /// The image data (encoded bytes or stream) - BufferSource | ReadableStream per spec
  pub data: ImageDecoderData,
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

impl FromNapiValue for ImageDecoderInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let obj = Object::from_napi_value(env, value)?;

    // Get MIME type (required)
    let mime_type: String = obj
      .get_named_property("type")
      .map_err(|_| Error::new(Status::InvalidArg, "Missing required 'type' property"))?;

    // Get optional properties
    let color_space_conversion: Option<String> = obj.get("colorSpaceConversion").ok().flatten();
    let desired_width: Option<u32> = obj.get("desiredWidth").ok().flatten();
    let desired_height: Option<u32> = obj.get("desiredHeight").ok().flatten();
    let prefer_animation: Option<bool> = obj.get("preferAnimation").ok().flatten();

    // Get data - try Uint8Array first, then ReadableStream
    let data_napi_value: napi::sys::napi_value = {
      let mut result = std::ptr::null_mut();
      napi::check_status!(
        napi::sys::napi_get_named_property(env, value, c"data".as_ptr().cast(), &mut result),
        "Failed to get 'data' property"
      )?;
      result
    };

    // Check if it's a Uint8Array (Buffer)
    let data = if let Ok(buffer) = Uint8Array::from_napi_value(env, data_napi_value) {
      ImageDecoderData::Buffer(buffer.to_vec())
    } else if let Ok(stream) = ReadableStream::<BufferSlice>::from_napi_value(env, data_napi_value)
    {
      // It's a ReadableStream - read all data from it
      // Note: We need to collect the stream data synchronously during construction
      // This is a limitation - we can't do async in FromNapiValue
      // For true streaming support, we would need a different API design
      let reader = stream.read().map_err(|e| {
        Error::new(
          Status::GenericFailure,
          format!("Failed to get stream reader: {}", e),
        )
      })?;

      // Collect stream data using a blocking approach
      // This is not ideal but necessary for the current API design
      let mut collected = Vec::new();
      let rt = tokio::runtime::Handle::try_current().map_err(|_| {
        Error::new(
          Status::GenericFailure,
          "ReadableStream requires async runtime",
        )
      })?;

      rt.block_on(async {
        let mut reader = reader;
        while let Some(chunk) = reader.next().await {
          match chunk {
            Ok(data) => collected.extend_from_slice(data.as_ref()),
            Err(e) => {
              return Err(Error::new(
                Status::GenericFailure,
                format!("Stream read error: {}", e),
              ))
            }
          }
        }
        Ok(())
      })?;

      ImageDecoderData::Stream(collected)
    } else {
      return Err(Error::new(
        Status::InvalidArg,
        "data must be a Uint8Array or ReadableStream",
      ));
    };

    Ok(ImageDecoderInit {
      data,
      mime_type,
      color_space_conversion,
      desired_width,
      desired_height,
      prefer_animation,
    })
  }
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
    self
      .selected_index
      .and_then(|i| self.tracks.get(i).cloned())
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
  /// Whether data is fully buffered (true for Buffer, becomes true for Stream when finished)
  complete: bool,
  /// Track list
  tracks: ImageTrackList,
  /// Whether decoder is closed
  closed: bool,
  /// Cached decoded frames (for animated images, populated on first decode)
  cached_frames: Option<Vec<crate::codec::Frame>>,
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
  /// Supports both Uint8Array and ReadableStream as data source per W3C spec
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

    // Extract data and determine source type
    let data = match init.data {
      ImageDecoderData::Buffer(buf) => buf,
      ImageDecoderData::Stream(buf) => buf,
    };

    let inner = ImageDecoderInner {
      data,
      mime_type: init.mime_type,
      codec_id,
      context: None,
      complete: true, // Data is complete (collected from stream or buffer)
      tracks,
      closed: false,
      cached_frames: None,
    };

    Ok(Self {
      inner: Arc::new(Mutex::new(inner)),
    })
  }

  /// Whether the data is fully buffered
  #[napi(getter)]
  pub fn complete(&self) -> Result<bool> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.complete)
  }

  /// Promise that resolves when data is fully loaded (per WebCodecs spec)
  /// Since we use buffered data, this resolves immediately
  #[napi(getter)]
  pub async fn completed(&self) -> Result<()> {
    // For buffered data, we're always complete immediately
    Ok(())
  }

  /// Get the MIME type
  #[napi(getter, js_name = "type")]
  pub fn get_type(&self) -> Result<String> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.mime_type.clone())
  }

  /// Get the track list
  #[napi(getter)]
  pub fn tracks(&self) -> Result<ImageTrackList> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.tracks.clone())
  }

  /// Decode the image (or a specific frame)
  #[napi]
  pub async fn decode(&self, options: Option<ImageDecodeOptions>) -> Result<ImageDecodeResult> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.closed {
      return Err(Error::new(Status::GenericFailure, "ImageDecoder is closed"));
    }

    let frame_index = options.as_ref().and_then(|o| o.frame_index).unwrap_or(0) as usize;

    // Use cached frames if available, otherwise decode and cache
    if inner.cached_frames.is_none() {
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

      // Decode all frames from the image data
      let context = inner.context.as_mut().unwrap();
      let decoded_frames = decode_image_data(context, &data)?;

      // Update frame_count in track info
      if !decoded_frames.is_empty() {
        inner.tracks.tracks[0].frame_count = decoded_frames.len() as u32;
      }

      // Cache the decoded frames
      inner.cached_frames = Some(decoded_frames);
    }

    // Get reference to cached frames
    let frames = inner.cached_frames.as_ref().unwrap();

    // Validate frame_index
    if frames.is_empty() {
      return Err(Error::new(
        Status::GenericFailure,
        "No frames decoded from image",
      ));
    }

    if frame_index >= frames.len() {
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "Frame index {} out of bounds (image has {} frames)",
          frame_index,
          frames.len()
        ),
      ));
    }

    // Clone the requested frame
    let frame = frames[frame_index].try_clone().map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to clone frame: {}", e),
      )
    })?;

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
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.closed {
      return Err(Error::new(Status::GenericFailure, "ImageDecoder is closed"));
    }

    inner.context = None;
    inner.cached_frames = None;

    // Reset frame_count for animated formats (will be re-detected on next decode)
    if inner.tracks.tracks[0].animated {
      inner.tracks.tracks[0].frame_count = 0;
    }

    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    inner.context = None;
    inner.cached_frames = None;
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
fn decode_image_data(context: &mut CodecContext, data: &[u8]) -> Result<Vec<crate::codec::Frame>> {
  // Create a packet with the image data
  let mut packet = Packet::new().map_err(|e| {
    Error::new(
      Status::GenericFailure,
      format!("Failed to create packet: {}", e),
    )
  })?;

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

  // Flush to get any remaining frames
  let mut all_frames = frames;
  if let Ok(flushed) = context.flush_decoder() {
    all_frames.extend(flushed);
  }

  Ok(all_frames)
}
