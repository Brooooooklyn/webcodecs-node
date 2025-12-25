//! ImageDecoder - WebCodecs API implementation
//!
//! Provides image decoding functionality using FFmpeg.
//! See: <https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder>

use crate::codec::{CodecContext, DecoderConfig, Frame, Packet, ScaleAlgorithm, Scaler};
use crate::ffi::AVCodecID;
use crate::webcodecs::VideoFrame;
use crate::webcodecs::error::{invalid_state_error, throw_invalid_state_error};
use futures::stream::{StreamExt, TryStreamExt};
use napi::bindgen_prelude::*;
use napi::tokio::sync::Notify;
use napi_derive::napi;
use parking_lot::RwLock as ParkingLotRwLock;
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};

const COMPLETED_PROMISE: &str = "[[completed]]";
const READY_PROMISE: &str = "[[ready]]";

/// ColorSpaceConversion for ImageDecoder (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorSpaceConversion {
  /// Apply default color space conversion (spec default)
  #[default]
  #[napi(value = "default")]
  Default,
  /// No color space conversion
  #[napi(value = "none")]
  None,
}

/// Data source for ImageDecoder - stores the encoded image data
pub enum ImageDecoderData {
  /// Buffered data (Uint8Array from constructor)
  Buffer(Uint8Array),
  /// Collected stream data (Vec<u8> after stream is fully read)
  Vec(Vec<u8>),
  /// Empty (data has been consumed or not yet available)
  Empty,
}

/// ImageDecoder init options
/// Per W3C spec, `data` can be either a BufferSource or a ReadableStream
pub struct ImageDecoderInit<'env> {
  /// The image data (encoded bytes or stream) - BufferSource | ReadableStream per spec
  pub data: Unknown<'env>,
  /// MIME type of the image (e.g., "image/png", "image/jpeg")
  pub mime_type: String,
  /// Color space conversion mode (default: "default")
  pub color_space_conversion: ColorSpaceConversion,
  /// Desired width (optional, for scaling) - must be paired with desired_height
  pub desired_width: Option<u32>,
  /// Desired height (optional, for scaling) - must be paired with desired_width
  pub desired_height: Option<u32>,
  /// Whether to prefer animation (for animated formats)
  pub prefer_animation: Option<bool>,
}

impl<'env> FromNapiValue for ImageDecoderInit<'env> {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // Get MIME type (required) - throw native TypeError if missing
    let mime_type: String = match obj.get_named_property("type") {
      Ok(t) => t,
      Err(_) => {
        env_wrapper.throw_type_error("Missing required 'type' property", None)?;
        return Err(Error::new(
          Status::InvalidArg,
          "Missing required 'type' property",
        ));
      }
    };

    // Get optional properties
    let color_space_conversion_str: Option<String> = obj.get("colorSpaceConversion").ok().flatten();
    let color_space_conversion = match color_space_conversion_str.as_deref() {
      Some("none") => ColorSpaceConversion::None,
      Some("default") | None => ColorSpaceConversion::Default,
      Some(invalid) => {
        env_wrapper.throw_type_error(
          &format!(
            "Invalid colorSpaceConversion value '{}'. Expected 'none' or 'default'",
            invalid
          ),
          None,
        )?;
        return Err(Error::new(
          Status::InvalidArg,
          format!(
            "Invalid colorSpaceConversion value '{}'. Expected 'none' or 'default'",
            invalid
          ),
        ));
      }
    };

    let desired_width: Option<u32> = obj.get("desiredWidth").ok().flatten();
    let desired_height: Option<u32> = obj.get("desiredHeight").ok().flatten();
    let prefer_animation: Option<bool> = obj.get("preferAnimation").ok().flatten();

    // W3C spec validation: desiredWidth and desiredHeight must both exist or both be omitted
    if desired_width.is_some() != desired_height.is_some() {
      env_wrapper.throw_type_error(
        "Both desiredWidth and desiredHeight must be specified, or neither",
        None,
      )?;
      return Err(Error::new(
        Status::InvalidArg,
        "Both desiredWidth and desiredHeight must be specified, or neither",
      ));
    }

    // Get data - try Uint8Array first, then ReadableStream
    let data_napi_value: napi::sys::napi_value = {
      let mut result = std::ptr::null_mut();
      napi::check_status!(
        unsafe {
          napi::sys::napi_get_named_property(env, value, c"data".as_ptr().cast(), &mut result)
        },
        "Failed to get 'data' property"
      )?;
      result
    };

    let data = unsafe { Unknown::from_raw_unchecked(env, data_napi_value) };

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
/// Note: W3C spec defines this as a dictionary, but NAPI-RS doesn't support
/// class instances in objects, so we use a class with the same properties.
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
  pub fn image(&self, env: Env) -> Result<VideoFrame> {
    self.image.clone_frame(env)
  }

  /// Get whether the decode is complete
  #[napi(getter)]
  pub fn complete(&self) -> bool {
    self.complete
  }
}

/// Internal track data (shared between ImageTrack instances)
#[derive(Debug, Clone)]
struct ImageTrackData {
  /// Whether this track is animated
  animated: bool,
  /// Number of frames in this track
  frame_count: u32,
  /// Number of times the animation repeats (Infinity for infinite)
  repetition_count: f64,
}

/// Internal state for ImageTrackList (shared with ImageTrack instances)
#[derive(Debug)]
struct ImageTrackListInner {
  tracks: Vec<ImageTrackData>,
  selected_index: Option<usize>,
}

/// Image track information (W3C spec - class with writable selected property)
#[napi]
pub struct ImageTrack {
  /// Reference to the parent track list's inner state
  track_list_inner: Arc<Mutex<ImageTrackListInner>>,
  /// Index of this track in the parent list
  index: usize,
}

#[napi]
impl ImageTrack {
  /// Whether this track is animated
  #[napi(getter)]
  pub fn animated(&self) -> Result<bool> {
    let inner = self
      .track_list_inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(
      inner
        .tracks
        .get(self.index)
        .map(|t| t.animated)
        .unwrap_or(false),
    )
  }

  /// Number of frames in this track
  #[napi(getter)]
  pub fn frame_count(&self) -> Result<u32> {
    let inner = self
      .track_list_inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(
      inner
        .tracks
        .get(self.index)
        .map(|t| t.frame_count)
        .unwrap_or(0),
    )
  }

  /// Number of times the animation repeats (Infinity for infinite)
  #[napi(getter)]
  pub fn repetition_count(&self) -> Result<f64> {
    let inner = self
      .track_list_inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(
      inner
        .tracks
        .get(self.index)
        .map(|t| t.repetition_count)
        .unwrap_or(0.0),
    )
  }

  /// Whether this track is currently selected (W3C spec - writable)
  #[napi(getter)]
  pub fn selected(&self) -> Result<bool> {
    let inner = self
      .track_list_inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.selected_index == Some(self.index))
  }

  /// Set whether this track is selected (W3C spec - writable)
  /// Setting to true deselects all other tracks
  #[napi(setter)]
  pub fn set_selected(&self, value: bool) -> Result<()> {
    let mut inner = self
      .track_list_inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if value {
      // Select this track (deselects others implicitly)
      inner.selected_index = Some(self.index);
    } else {
      // Only deselect if this track is currently selected
      if inner.selected_index == Some(self.index) {
        inner.selected_index = None;
      }
    }
    Ok(())
  }
}

/// Image track list (W3C spec)
#[napi]
pub struct ImageTrackList {
  inner: Arc<Mutex<ImageTrackListInner>>,
  /// Whether track metadata is ready (established)
  ready: Arc<AtomicBool>,
  /// Notifier for when ready becomes true
  ready_notify: Arc<Notify>,
}

impl Clone for ImageTrackList {
  fn clone(&self) -> Self {
    ImageTrackList {
      inner: self.inner.clone(),
      ready: self.ready.clone(),
      ready_notify: self.ready_notify.clone(),
    }
  }
}

#[napi]
impl ImageTrackList {
  /// Get the number of tracks
  #[napi(getter)]
  pub fn length(&self) -> Result<u32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.tracks.len() as u32)
  }

  /// Get the currently selected track (if any)
  #[napi(getter)]
  pub fn selected_track(&self) -> Result<Option<ImageTrack>> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match inner.selected_index {
      Some(idx) if idx < inner.tracks.len() => Ok(Some(ImageTrack {
        track_list_inner: self.inner.clone(),
        index: idx,
      })),
      _ => Ok(None),
    }
  }

  /// Get the selected track index (W3C spec: returns -1 if no track selected)
  #[napi(getter)]
  pub fn selected_index(&self) -> Result<i32> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.selected_index.map(|i| i as i32).unwrap_or(-1))
  }

  /// Promise that resolves when track metadata is available (W3C spec)
  #[napi(getter)]
  pub async fn ready(&self) -> Result<()> {
    // Fast path: already ready
    if self.ready.load(Ordering::Acquire) {
      return Ok(());
    }

    // IMPORTANT: Create the listener FIRST, before checking the flag again.
    // This prevents a race condition where notify_waiters() is called
    // after our initial check but before we register as a waiter.
    let notified = self.ready_notify.notified();

    // Check again - if became ready while creating listener, return immediately
    if self.ready.load(Ordering::Acquire) {
      return Ok(());
    }

    // Now safe to wait - the listener is registered, so any subsequent
    // notify_waiters() call will wake us up
    notified.await;
    Ok(())
  }

  /// Get track at specified index (W3C spec)
  #[napi]
  pub fn item(&self, index: u32) -> Result<Option<ImageTrack>> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if (index as usize) < inner.tracks.len() {
      Ok(Some(ImageTrack {
        track_list_inner: self.inner.clone(),
        index: index as usize,
      }))
    } else {
      Ok(None)
    }
  }
}

/// Internal decoder state
struct ImageDecoderInner {
  /// Original encoded data
  data: ImageDecoderData,
  /// MIME type
  mime_type: String,
  /// Codec ID for decoding (None if MIME type is unsupported)
  codec_id: Option<AVCodecID>,
  /// Decoder context (created on first decode)
  context: Option<CodecContext>,
  /// Whether data is fully buffered (true for Buffer, becomes true for Stream when finished)
  complete: Arc<AtomicBool>,
  /// Track list
  tracks: ImageTrackList,
  /// Whether decoder is closed
  closed: bool,
  /// Cached decoded frames (for animated images, populated on first decode)
  /// Cached decoded frames wrapped in Arc for efficient sharing
  cached_frames: Option<Vec<Arc<ParkingLotRwLock<Frame>>>>,
  /// Color space conversion mode (W3C spec)
  color_space_conversion: ColorSpaceConversion,
  /// Desired width for scaling (W3C spec - must be paired with desired_height)
  desired_width: Option<u32>,
  /// Desired height for scaling (W3C spec - must be paired with desired_width)
  desired_height: Option<u32>,
  /// Whether to prefer animation for animated formats (W3C spec)
  prefer_animation: Option<bool>,
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
  pub fn new<'env>(env: &'env Env, mut this: This, init: ImageDecoderInit<'env>) -> Result<Self> {
    // Parse MIME type to codec ID - accept invalid types (will fail at decode time per W3C spec)
    let codec_id = parse_mime_type(&init.mime_type).ok();

    // Determine if this is an animated format
    let animated = matches!(codec_id, Some(AVCodecID::Gif) | Some(AVCodecID::Webp));

    // For static images, there's one frame; for animated, we'll detect later
    let frame_count = if animated { 0 } else { 1 };

    // Create async signaling primitives
    let complete = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicBool::new(false));
    let ready_notify = Arc::new(Notify::new());

    let track_list_inner = Arc::new(Mutex::new(ImageTrackListInner {
      tracks: vec![ImageTrackData {
        animated,
        frame_count,
        repetition_count: if animated { f64::INFINITY } else { 0.0 },
      }],
      selected_index: Some(0),
    }));

    let tracks = ImageTrackList {
      inner: track_list_inner,
      ready: ready.clone(),
      ready_notify: ready_notify.clone(),
    };

    // Create inner state first so we can share it with async tasks
    let inner = Arc::new(Mutex::new(ImageDecoderInner {
      data: ImageDecoderData::Empty,
      mime_type: init.mime_type.clone(),
      codec_id,
      context: None,
      complete: complete.clone(),
      tracks: tracks.clone(),
      closed: false,
      cached_frames: None,
      color_space_conversion: init.color_space_conversion,
      desired_width: init.desired_width,
      desired_height: init.desired_height,
      prefer_animation: init.prefer_animation,
    }));

    if let Ok(buf) = unsafe { init.data.cast::<Uint8Array>() } {
      // Buffer data: store immediately and mark complete
      {
        let mut inner_guard = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
        inner_guard.data = ImageDecoderData::Buffer(buf);
      }
      complete.store(true, Ordering::Release);

      // For buffer data: completed resolves immediately (data is already available)
      let completed_promise = env.spawn_future(async { Ok(()) })?;

      // Spawn pre-parse task for ready promise
      let inner_clone = inner.clone();
      let ready_promise = env.spawn_future(async move {
        let result = spawn_blocking(move || pre_parse_and_cache_frames(&inner_clone)).await;

        match result {
          Ok(Ok(())) | Ok(Err(_)) | Err(_) => {
            // Always signal ready, even on error (per W3C spec behavior)
          }
        }

        Ok(())
      })?;

      // Store both promises as non-enumerable properties
      this.define_properties(&[
        Property::new()
          .with_utf8_name(COMPLETED_PROMISE)?
          .with_value(&completed_promise)
          .with_property_attributes(PropertyAttributes::default()),
        Property::new()
          .with_utf8_name(READY_PROMISE)?
          .with_value(&ready_promise)
          .with_property_attributes(PropertyAttributes::default()),
      ])?;
    } else if let Ok(stream) = unsafe { init.data.cast::<ReadableStream<Uint8Array>>() } {
      // Stream data: start collecting asynchronously
      let reader = stream.read()?;
      let inner_clone = inner.clone();

      // For stream data: combined promise that does collection + pre-parse
      // Both completed and ready resolve when this completes (Option A: simpler)
      let combined_promise = env.spawn_future(async move {
        // Collect all stream data
        let stream_data = reader.map(|s| s.map(|c| c.to_vec())).try_concat().await;

        match stream_data {
          Ok(collected_data) => {
            // Store collected data in inner
            {
              if let Ok(mut inner_guard) = inner_clone.lock() {
                inner_guard.data = ImageDecoderData::Vec(collected_data);
                inner_guard.complete.store(true, Ordering::Release);
              }
            }

            // Pre-parse metadata and cache frames
            let result = spawn_blocking(move || pre_parse_and_cache_frames(&inner_clone)).await;

            match result {
              Ok(Ok(())) | Ok(Err(_)) | Err(_) => {
                // Always signal ready (done in pre_parse_and_cache_frames)
              }
            }
          }
          Err(_) => {
            // Stream error - signal ready_notify to unblock waiters
            if let Ok(inner_guard) = inner_clone.lock() {
              inner_guard.tracks.ready_notify.notify_waiters();
            }
          }
        }

        Ok(())
      })?;

      // Store the combined promise for both completed and ready
      this.define_properties(&[
        Property::new()
          .with_utf8_name(COMPLETED_PROMISE)?
          .with_value(&combined_promise)
          .with_property_attributes(PropertyAttributes::default()),
        Property::new()
          .with_utf8_name(READY_PROMISE)?
          .with_value(&combined_promise)
          .with_property_attributes(PropertyAttributes::default()),
      ])?;
    } else {
      return Err(Error::new(Status::InvalidArg, "Invalid data type"));
    }

    Ok(Self { inner })
  }

  /// Whether the data is fully buffered
  #[napi(getter)]
  pub fn complete(&self) -> Result<bool> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.complete.load(Ordering::Relaxed))
  }

  /// Promise that resolves when data is fully loaded (per WebCodecs spec)
  /// Returns a new promise chained from the stored promise (allows multiple accesses)
  #[napi(getter)]
  pub fn completed(&self, env: Env, this: This) -> Result<PromiseRaw<'_, ()>> {
    // Check if closed
    {
      let inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      if inner.closed {
        return throw_invalid_state_error(&env, "ImageDecoder is closed");
      }
    }

    // Get stored promise from this and chain a new promise for this access
    let promise: PromiseRaw<()> = this.get_named_property(COMPLETED_PROMISE)?;
    promise.then(|_| Ok(()))
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
  pub fn decode<'env>(
    &self,
    env: &'env Env,
    this: This,
    options: Option<ImageDecodeOptions>,
  ) -> Result<PromiseRaw<'env, ImageDecodeResult>> {
    // Get ready promise synchronously (before entering async context)
    let ready_promise: Promise<()> = {
      let promise_raw: PromiseRaw<()> = this.get_named_property(READY_PROMISE)?;
      promise_raw.into_sendable_promise()?
    };

    let inner = self.inner.clone();

    // Spawn async task that first waits for ready, then decodes
    env.spawn_future(async move {
      // Check if closed before waiting
      {
        let inner_guard = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
        if inner_guard.closed {
          return Err(invalid_state_error("ImageDecoder is closed"));
        }
      }

      // Wait for ready promise (ensures initial pre-parsing is complete)
      ready_promise.await?;

      // Now do the actual decode in a blocking task
      spawn_blocking(move || {
        let mut inner = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

        if inner.closed {
          return Err(invalid_state_error("ImageDecoder is closed"));
        }

        let frame_index = options.as_ref().and_then(|o| o.frame_index).unwrap_or(0) as usize;

        // If frames were cleared (e.g., by reset), re-decode them
        if inner.cached_frames.is_none() {
          // Get the data bytes
          let data_bytes: Vec<u8> = match &inner.data {
            ImageDecoderData::Buffer(buf) => buf.as_ref().to_vec(),
            ImageDecoderData::Vec(vec) => vec.clone(),
            ImageDecoderData::Empty => {
              return Err(Error::new(Status::GenericFailure, "No data available"));
            }
          };

          // Check if codec_id is valid (invalid MIME type at construction is deferred to here)
          let codec_id = match inner.codec_id {
            Some(id) => id,
            None => {
              return Err(Error::new(
                Status::GenericFailure,
                format!("Unsupported image type: {}", inner.mime_type),
              ));
            }
          };

          // Create decoder context if needed
          if inner.context.is_none() {
            let mut context = CodecContext::new_decoder(codec_id).map_err(|e| {
              Error::new(
                Status::GenericFailure,
                format!("Failed to create decoder: {}", e),
              )
            })?;

            let decoder_config = DecoderConfig {
              codec_id,
              thread_count: 0,
              extradata: None,
              low_latency: false,
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

          // Decode all frames
          let context = inner.context.as_mut().unwrap();
          let mut frames = decode_image_data(context, &data_bytes)?;

          // Apply preferAnimation: if false and format supports animation, only keep first frame
          if inner.prefer_animation == Some(false) && !frames.is_empty() {
            frames.truncate(1);
            // Mark track as non-animated since user explicitly prefers static
            if let Ok(mut track_inner) = inner.tracks.inner.lock()
              && let Some(track) = track_inner.tracks.get_mut(0)
            {
              track.animated = false;
            }
          }

          // Apply desiredWidth/desiredHeight scaling if both are specified
          let frames = if let (Some(dw), Some(dh)) = (inner.desired_width, inner.desired_height) {
            let mut scaled_frames = Vec::with_capacity(frames.len());
            for frame in frames {
              let scaler = Scaler::new(
                frame.width(),
                frame.height(),
                frame.format(),
                dw,
                dh,
                frame.format(),
                ScaleAlgorithm::Lanczos,
              )
              .map_err(|e| {
                Error::new(
                  Status::GenericFailure,
                  format!("Failed to create scaler: {}", e),
                )
              })?;

              let scaled = scaler.scale_alloc(&frame).map_err(|e| {
                Error::new(
                  Status::GenericFailure,
                  format!("Failed to scale frame: {}", e),
                )
              })?;
              scaled_frames.push(scaled);
            }
            scaled_frames
          } else {
            frames
          };

          // Update frame_count
          if !frames.is_empty()
            && let Ok(mut track_inner) = inner.tracks.inner.lock()
            && let Some(track) = track_inner.tracks.get_mut(0)
          {
            track.frame_count = frames.len() as u32;
          }

          // Wrap frames in Arc for efficient sharing from cache
          let shared_frames: Vec<Arc<ParkingLotRwLock<Frame>>> =
            frames.into_iter().map(|f| f.into_shared()).collect();
          inner.cached_frames = Some(shared_frames);
        }

        // Get reference to cached frames
        let frames = inner.cached_frames.as_ref().ok_or_else(|| {
          Error::new(
            Status::GenericFailure,
            "No frames available - decoding may have failed",
          )
        })?;

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
              "RangeError: Frame index {} out of bounds (image has {} frames)",
              frame_index,
              frames.len()
            ),
          ));
        }

        // Clone the Arc to share the frame data (no pixel copy needed)
        let frame_arc = frames[frame_index].clone();
        let pts = frame_arc.read().pts();

        // Per Chromium behavior: "default" extracts color space, "none" ignores it
        let extract_color_space = inner.color_space_conversion == ColorSpaceConversion::Default;
        let video_frame =
          VideoFrame::from_internal_arc_with_color_space(frame_arc, pts, None, extract_color_space);

        Ok(ImageDecodeResult {
          image: video_frame,
          complete: true,
        })
      })
      .await
      .map_err(|join_error| {
        Error::new(
          Status::GenericFailure,
          format!("Decode task failed: {}", join_error),
        )
      })?
    })
  }

  /// Reset the decoder
  /// Clears cached frames - next decode() will re-decode from stored data
  #[napi]
  pub fn reset(&self, env: Env) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if inner.closed {
      return throw_invalid_state_error(&env, "ImageDecoder is closed");
    }

    inner.context = None;
    inner.cached_frames = None;

    // Reset frame_count for animated formats (will be re-detected on next decode)
    if let Ok(mut track_inner) = inner.tracks.inner.lock()
      && let Some(track) = track_inner.tracks.get_mut(0)
      && track.animated
    {
      track.frame_count = 0;
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

    // Wake any waiters so they can check closed state
    inner.tracks.ready_notify.notify_waiters();

    Ok(())
  }

  /// Whether this ImageDecoder has been closed (W3C WebCodecs spec)
  #[napi(getter)]
  pub fn closed(&self) -> Result<bool> {
    let inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(inner.closed)
  }

  /// Check if a MIME type is supported
  #[napi]
  pub async fn is_type_supported(mime_type: String) -> bool {
    parse_mime_type(&mime_type).is_ok()
  }
}

/// Pre-parse image and cache decoded frames
fn pre_parse_and_cache_frames(inner: &Arc<Mutex<ImageDecoderInner>>) -> Result<()> {
  let mut inner_guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

  // Check if already closed
  if inner_guard.closed {
    inner_guard.tracks.ready.store(true, Ordering::Release);
    inner_guard.tracks.ready_notify.notify_waiters();
    return Err(invalid_state_error("ImageDecoder is closed"));
  }

  // Get the data bytes
  let data_bytes: Vec<u8> = match &inner_guard.data {
    ImageDecoderData::Buffer(buf) => buf.as_ref().to_vec(),
    ImageDecoderData::Vec(vec) => vec.clone(),
    ImageDecoderData::Empty => {
      inner_guard.tracks.ready.store(true, Ordering::Release);
      inner_guard.tracks.ready_notify.notify_waiters();
      return Err(Error::new(Status::GenericFailure, "No data available"));
    }
  };

  // Check if codec_id is valid (invalid MIME type at construction is deferred to here)
  let codec_id = match inner_guard.codec_id {
    Some(id) => id,
    None => {
      inner_guard.tracks.ready.store(true, Ordering::Release);
      inner_guard.tracks.ready_notify.notify_waiters();
      return Err(Error::new(
        Status::GenericFailure,
        format!("Unsupported image type: {}", inner_guard.mime_type),
      ));
    }
  };

  // Create decoder context
  let mut context = CodecContext::new_decoder(codec_id).map_err(|e| {
    inner_guard.tracks.ready.store(true, Ordering::Release);
    inner_guard.tracks.ready_notify.notify_waiters();
    Error::new(
      Status::GenericFailure,
      format!("Failed to create decoder: {}", e),
    )
  })?;

  let decoder_config = DecoderConfig {
    codec_id,
    thread_count: 0,
    extradata: None,
    low_latency: false,
  };

  context.configure_decoder(&decoder_config).map_err(|e| {
    inner_guard.tracks.ready.store(true, Ordering::Release);
    inner_guard.tracks.ready_notify.notify_waiters();
    Error::new(
      Status::GenericFailure,
      format!("Failed to configure decoder: {}", e),
    )
  })?;

  context.open().map_err(|e| {
    inner_guard.tracks.ready.store(true, Ordering::Release);
    inner_guard.tracks.ready_notify.notify_waiters();
    Error::new(
      Status::GenericFailure,
      format!("Failed to open decoder: {}", e),
    )
  })?;

  // Decode all frames
  let mut frames = decode_image_data(&mut context, &data_bytes).inspect_err(|_e| {
    inner_guard.tracks.ready.store(true, Ordering::Release);
    inner_guard.tracks.ready_notify.notify_waiters();
  })?;

  // Apply preferAnimation: if false and format supports animation, only keep first frame
  let prefer_animation = inner_guard.prefer_animation;
  if prefer_animation == Some(false) && !frames.is_empty() {
    frames.truncate(1);
    // Mark track as non-animated since user explicitly prefers static
    if let Ok(mut track_inner) = inner_guard.tracks.inner.lock()
      && let Some(track) = track_inner.tracks.get_mut(0)
    {
      track.animated = false;
    }
  }

  // Apply desiredWidth/desiredHeight scaling if both are specified
  let desired_width = inner_guard.desired_width;
  let desired_height = inner_guard.desired_height;
  let frames = if let (Some(dw), Some(dh)) = (desired_width, desired_height) {
    let mut scaled_frames = Vec::with_capacity(frames.len());
    for frame in frames {
      // Create scaler for this frame's dimensions and format
      let scaler = Scaler::new(
        frame.width(),
        frame.height(),
        frame.format(),
        dw,
        dh,
        frame.format(),
        ScaleAlgorithm::Lanczos, // High quality for images
      )
      .map_err(|e| {
        inner_guard.tracks.ready.store(true, Ordering::Release);
        inner_guard.tracks.ready_notify.notify_waiters();
        Error::new(
          Status::GenericFailure,
          format!("Failed to create scaler: {}", e),
        )
      })?;

      let scaled = scaler.scale_alloc(&frame).map_err(|e| {
        inner_guard.tracks.ready.store(true, Ordering::Release);
        inner_guard.tracks.ready_notify.notify_waiters();
        Error::new(
          Status::GenericFailure,
          format!("Failed to scale frame: {}", e),
        )
      })?;
      scaled_frames.push(scaled);
    }
    scaled_frames
  } else {
    frames
  };

  // Update frame_count in track info
  if !frames.is_empty()
    && let Ok(mut track_inner) = inner_guard.tracks.inner.lock()
    && let Some(track) = track_inner.tracks.get_mut(0)
  {
    track.frame_count = frames.len() as u32;
  }

  // Wrap frames in Arc for efficient sharing from cache
  let shared_frames: Vec<Arc<ParkingLotRwLock<Frame>>> =
    frames.into_iter().map(|f| f.into_shared()).collect();
  inner_guard.cached_frames = Some(shared_frames);
  inner_guard.context = Some(context);

  // Signal ready
  inner_guard.tracks.ready.store(true, Ordering::Release);
  inner_guard.tracks.ready_notify.notify_waiters();

  Ok(())
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
