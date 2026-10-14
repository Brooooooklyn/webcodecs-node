//! ImageDecoder - WebCodecs API implementation
//!
//! Provides image decoding functionality using FFmpeg.
//! See: <https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder>

use crate::codec::demuxer::{DemuxerContext, MediaType};
use crate::codec::{CodecContext, DecoderConfig, Frame, Packet};
use crate::ffi::avutil::av_rescale_q;
use crate::ffi::{AV_NOPTS_VALUE, AVCodecID, AVRational};
use crate::webcodecs::buffer_source::{self, Uint8ArrayCopy};
use crate::webcodecs::decoded_frame::{DecodedSource, decoded_frame_format};
use crate::webcodecs::error::{
  DOMExceptionName, invalid_state_error, native_dom_exception_error, native_range_error,
  throw_invalid_state_error,
};
use crate::webcodecs::{VideoFrame, VideoPixelFormat};
use futures::stream::StreamExt;
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
  /// Encoded bytes owned by the decoder: a copy of the BufferSource passed to
  /// the constructor, or the stream chunks collected so far.
  Vec(Vec<u8>),
  /// Empty (data has been consumed or not yet available)
  Empty,
}

/// ImageDecoder init options
/// Per W3C spec, `data` can be either a BufferSource or a ReadableStream
pub struct ImageDecoderInit<'env> {
  /// The image data (encoded bytes or stream) - BufferSource | ReadableStream per spec.
  /// Only a handle: a BufferSource is copied by the constructor, after every
  /// init getter has run.
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

    // Get data - a BufferSource or a ReadableStream
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
  /// Cached decoded frames wrapped in Arc for efficient sharing, with the
  /// WebCodecs format that describes each one
  cached_frames: Option<Vec<(Arc<ParkingLotRwLock<Frame>>, VideoPixelFormat)>>,
  /// Incremented when reset/close invalidates in-flight decode work.
  cache_epoch: u64,
  /// Encoded byte length represented by the current cache. Prevents an older
  /// streamed probe from overwriting a newer, more complete decode.
  cached_data_len: usize,
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
  /// Supports a BufferSource (copied here) or a ReadableStream as data source per W3C spec.
  /// ReadableStream input is buffered without a fixed size limit.
  #[napi(constructor)]
  pub fn new<'env>(env: &'env Env, mut this: This, init: ImageDecoderInit<'env>) -> Result<Self> {
    // Parse MIME type to codec ID - accept invalid types (will fail at decode time per W3C spec)
    let codec_id = parse_mime_type(&init.mime_type).ok();

    // Determine if this is an animated format
    let animated = matches!(
      codec_id,
      Some(AVCodecID::Gif)
        | Some(AVCodecID::Webp)
        | Some(AVCodecID::Jpegxl)
        | Some(AVCodecID::JpegxlAnim)
    );

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
      cache_epoch: 0,
      cached_data_len: 0,
      color_space_conversion: init.color_space_conversion,
      desired_width: init.desired_width,
      desired_height: init.desired_height,
      prefer_animation: init.prefer_animation,
    }));

    if let Some(raw) = unsafe { buffer_source::inspect(env.raw(), init.data.raw())? } {
      // BufferSource: copy the bytes now. Every init getter has already run;
      // keeping a pointer into the caller's buffer would let JS detach, shrink
      // or rewrite it while decode work reads it on another thread.
      let bytes = raw.to_vec()?;
      // Like Chrome, empty (or detached) data is a TypeError, except for an
      // unsupported type: decode() rejects with NotSupportedError instead.
      if codec_id.is_some() && bytes.is_empty() {
        env.throw_type_error("No image data provided", None)?;
        return Err(Error::new(Status::InvalidArg, "No image data provided"));
      }
      {
        let mut inner_guard = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
        inner_guard.data = ImageDecoderData::Vec(bytes);
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
    } else if let Ok(stream) = unsafe { init.data.cast::<ReadableStream<Uint8ArrayCopy>>() } {
      // Each chunk is copied on the JS thread when its read() settles, before
      // the bytes cross to the decode task: the producer may detach or reuse
      // a chunk's buffer right after enqueueing it.
      // Stream data: append chunks incrementally and probe at geometrically
      // increasing byte thresholds. This avoids O(n^2) reparsing while letting
      // `tracks.ready` and decode() resolve before a delayed stream close.
      let mut reader = stream.read()?;
      let inner_clone = inner.clone();
      if let Ok(mut inner_guard) = inner_clone.lock() {
        inner_guard.data = ImageDecoderData::Vec(Vec::new());
      }

      let collection_promise = env.spawn_future(async move {
        let mut next_probe_size = 1usize;

        while let Some(item) = reader.next().await {
          let chunk = match item {
            Ok(chunk) => chunk,
            Err(error) => {
              // A stream error is terminal. Mark the input complete before
              // waking decode waiters so they reject instead of waiting for an
              // update that can never arrive.
              if let Ok(inner_guard) = inner_clone.lock() {
                inner_guard.complete.store(true, Ordering::Release);
              }
              signal_tracks_ready(&inner_clone);
              return Err(error);
            }
          };
          let buffered_len = {
            let mut inner_guard = inner_clone
              .lock()
              .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
            match &mut inner_guard.data {
              ImageDecoderData::Vec(data) => data.extend_from_slice(&chunk),
              _ => inner_guard.data = ImageDecoderData::Vec(chunk.into_vec()),
            }
            match &inner_guard.data {
              ImageDecoderData::Vec(data) => data.len(),
              _ => 0,
            }
          };

          if buffered_len >= next_probe_size {
            let probe_inner = inner_clone.clone();
            if matches!(
              spawn_blocking(move || cache_decoded_frames(&probe_inner)).await,
              Ok(Ok(()))
            ) {
              signal_tracks_ready(&inner_clone);
            }
            next_probe_size = buffered_len.saturating_mul(2).max(next_probe_size * 2);
          }
        }

        if let Ok(inner_guard) = inner_clone.lock() {
          inner_guard.complete.store(true, Ordering::Release);
        }

        // Refresh once at EOF so animated images include frames that arrived
        // after an early successful probe.
        let final_inner = inner_clone.clone();
        let _ = spawn_blocking(move || cache_decoded_frames(&final_inner)).await;
        signal_tracks_ready(&inner_clone);
        Ok(())
      })?;

      let ready_flag = ready.clone();
      let ready_notifier = ready_notify.clone();
      let ready_promise = env.spawn_future(async move {
        if ready_flag.load(Ordering::Acquire) {
          return Ok(());
        }
        let notified = ready_notifier.notified();
        if ready_flag.load(Ordering::Acquire) {
          return Ok(());
        }
        notified.await;
        Ok(())
      })?;

      // completed tracks stream EOF; ready tracks the first decodable frame.
      this.define_properties(&[
        Property::new()
          .with_utf8_name(COMPLETED_PROMISE)?
          .with_value(&collection_promise)
          .with_property_attributes(PropertyAttributes::default()),
        Property::new()
          .with_utf8_name(READY_PROMISE)?
          .with_value(&ready_promise)
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

    // Keep the Rust error as the resolved future value so the completion
    // callback can construct the precise native JavaScript exception class.
    let decode = async move {
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

      let frame_index = options.as_ref().and_then(|o| o.frame_index).unwrap_or(0) as usize;
      let update_notify = {
        let guard = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
        guard.tracks.ready_notify.clone()
      };

      // A streamed animated image can establish its tracks before the requested
      // frame has arrived. Keep the decode pending until a later incremental
      // probe finds that frame or EOF proves that the index is infeasible.
      loop {
        // Register before inspecting the cache so a probe cannot notify in the
        // narrow interval between the state check and awaiting the signal.
        let mut notified = std::pin::pin!(update_notify.notified());
        // Constructing Notified does not register it with Notify. Register it
        // before the blocking cache check so notify_waiters() reaches every
        // concurrent decode, rather than relying on a single stored permit.
        notified.as_mut().enable();
        let decode_inner = inner.clone();
        let result = spawn_blocking(move || decode_cached_frame(&decode_inner, frame_index))
          .await
          .map_err(|join_error| {
            Error::new(
              Status::GenericFailure,
              format!("Decode task failed: {}", join_error),
            )
          })??;

        if let Some(result) = result {
          return Ok(result);
        }

        notified.await;
      }
    };

    env.spawn_future_with_callback(
      async move { Ok(decode.await) },
      |env, result| match result {
        Ok(result) => Ok(result),
        Err(error) => Err(native_image_decode_error(env, error)?),
      },
    )
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
    inner.cache_epoch = inner.cache_epoch.wrapping_add(1);
    inner.cached_data_len = 0;

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
    inner.cache_epoch = inner.cache_epoch.wrapping_add(1);
    inner.cached_data_len = 0;
    inner.closed = true;

    // Wake any waiters so they can check closed state
    inner.tracks.ready.store(true, Ordering::Release);
    inner.tracks.ready_notify.notify_waiters();
    inner.tracks.ready_notify.notify_one();

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

fn native_image_decode_error(env: &Env, error: Error) -> Result<Error> {
  if let Some(message) = error.reason.strip_prefix("RangeError:") {
    return native_range_error(env, message.trim());
  }
  if let Some(message) = error.reason.strip_prefix("InvalidStateError:") {
    return native_dom_exception_error(env, DOMExceptionName::InvalidStateError, message.trim());
  }
  if error.reason.starts_with("Unsupported image type:") {
    return native_dom_exception_error(env, DOMExceptionName::NotSupportedError, &error.reason);
  }

  native_dom_exception_error(env, DOMExceptionName::EncodingError, &error.reason)
}

/// Return a requested cached frame, or None while a streamed image may still
/// grow to contain that frame. This implementation only emits complete frames;
/// completeFramesOnly=false therefore retains the allowed complete-frame
/// behavior rather than exposing partial progressive generations.
fn decode_cached_frame(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  frame_index: usize,
) -> Result<Option<ImageDecodeResult>> {
  let (needs_cache, data_complete) = {
    let guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if guard.closed {
      return Err(invalid_state_error("ImageDecoder is closed"));
    }
    (
      guard.cached_frames.is_none(),
      guard.complete.load(Ordering::Acquire),
    )
  };

  // A reset can clear the cache. For fully buffered input, reconstruct it now;
  // for a live stream, wait for the collector's next successful probe.
  if needs_cache {
    if !data_complete {
      return Ok(None);
    }
    cache_decoded_frames(inner)?;
  }

  let guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if guard.closed {
    return Err(invalid_state_error("ImageDecoder is closed"));
  }

  let frames = guard.cached_frames.as_ref().ok_or_else(|| {
    Error::new(
      Status::GenericFailure,
      "No frames available - decoding may have failed",
    )
  })?;
  let data_complete = guard.complete.load(Ordering::Acquire);

  if frame_index >= frames.len() {
    if !data_complete {
      return Ok(None);
    }
    return Err(Error::new(
      Status::InvalidArg,
      format!(
        "RangeError: Frame index {} out of bounds (image has {} frames)",
        frame_index,
        frames.len()
      ),
    ));
  }

  // Clone the Arc to share the frame data (no pixel copy needed).
  let (frame_arc, format) = frames[frame_index].clone();
  let frame_guard = frame_arc.read();
  let pts = frame_guard.pts();
  let frame_duration = frame_guard.duration();
  let duration = (frames.len() > 1 && frame_duration > 0).then_some(frame_duration);
  drop(frame_guard);

  // Per Chromium behavior: "default" extracts color space, "none" ignores it.
  let extract_color_space = guard.color_space_conversion == ColorSpaceConversion::Default;
  let video_frame = VideoFrame::from_internal_arc_with_color_space(
    frame_arc,
    format,
    pts,
    duration,
    extract_color_space,
  );

  Ok(Some(ImageDecodeResult {
    image: video_frame,
    complete: true,
  }))
}

/// Pre-parse image and cache decoded frames
fn pre_parse_and_cache_frames(inner: &Arc<Mutex<ImageDecoderInner>>) -> Result<()> {
  let result = cache_decoded_frames(inner);
  signal_tracks_ready(inner);
  result
}

fn signal_tracks_ready(inner: &Arc<Mutex<ImageDecoderInner>>) {
  if let Ok(inner_guard) = inner.lock() {
    inner_guard.tracks.ready.store(true, Ordering::Release);
    // notify_waiters wakes listeners that are already registered, while
    // notify_one stores a permit for the narrow interval between the atomic
    // recheck and the first poll of a Notified future.
    inner_guard.tracks.ready_notify.notify_waiters();
    inner_guard.tracks.ready_notify.notify_one();
  }
}

/// Attempt to cache every frame currently present in the buffered bytes.
/// Unlike `pre_parse_and_cache_frames`, failures do not resolve `tracks.ready`,
/// which lets streamed images retry as more bytes arrive.
fn cache_decoded_frames(inner: &Arc<Mutex<ImageDecoderInner>>) -> Result<()> {
  // Snapshot everything needed for FFmpeg work under the mutex, then release
  // it. Large animated images can take a long time to decode; close() and
  // reset() must remain responsive while that work is in flight.
  let (
    data_bytes,
    codec_id,
    mime_type,
    prefer_animation,
    desired_width,
    desired_height,
    cache_epoch,
  ) = {
    let guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if guard.closed {
      return Err(invalid_state_error("ImageDecoder is closed"));
    }

    let data_bytes = match &guard.data {
      ImageDecoderData::Vec(vec) => vec.clone(),
      ImageDecoderData::Empty => {
        return Err(Error::new(Status::GenericFailure, "No data available"));
      }
    };

    (
      data_bytes,
      guard.codec_id,
      guard.mime_type.clone(),
      guard.prefer_animation,
      guard.desired_width,
      guard.desired_height,
      guard.cache_epoch,
    )
  };

  // Invalid MIME types are intentionally accepted by the constructor and
  // reject decode(), as required by the WebCodecs ImageDecoder algorithm.
  let codec_id = codec_id.ok_or_else(|| {
    Error::new(
      Status::GenericFailure,
      format!("Unsupported image type: {}", mime_type),
    )
  })?;

  // Demux and decode all frames.
  let (context, mut frames, video_streams) = decode_image_data(codec_id, &data_bytes)?;
  // One video stream with one frame: no animation, alpha or other auxiliary image
  let still = video_streams == 1 && frames.len() == 1;

  // Apply preferAnimation: if false and format supports animation, only keep first frame
  if prefer_animation == Some(false) && !frames.is_empty() {
    frames.truncate(1);
  }

  // Give every frame a WebCodecs format that describes its bytes, converting the
  // FFmpeg formats WebCodecs has no name for (rgb24, pal8, gray, argb, ...), and
  // apply desiredWidth/desiredHeight scaling if both are specified.
  let source = DecodedSource::Image {
    codec: codec_id,
    alpha: match codec_id {
      AVCodecID::Webp => webp_alpha_hint(&data_bytes),
      AVCodecID::Png => png_alpha_hint(&data_bytes),
      _ => None,
    },
    yuv: match codec_id {
      AVCodecID::Mjpeg => jpeg_yuv_hint(&data_bytes),
      // Chrome keeps a still AVIF without alpha as YUV planes
      AVCodecID::Av1 => Some(still),
      _ => None,
    },
    size: desired_width.zip(desired_height),
  };
  let frames = frames
    .into_iter()
    .map(|frame| decoded_frame_format(frame, source))
    .collect::<std::result::Result<Vec<_>, _>>()
    .map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to convert decoded image: {}", e),
      )
    })?;

  let repetition_count = if codec_id == AVCodecID::Gif {
    gif_repetition_count(&data_bytes)
  } else {
    None
  };

  let mut guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if guard.closed {
    return Err(invalid_state_error("ImageDecoder is closed"));
  }
  // reset()/close() invalidated the work, or a newer streamed probe already
  // committed more input. Never overwrite either state with this snapshot.
  if guard.cache_epoch != cache_epoch || data_bytes.len() < guard.cached_data_len {
    return Ok(());
  }

  if let Ok(mut track_inner) = guard.tracks.inner.lock()
    && let Some(track) = track_inner.tracks.get_mut(0)
  {
    if !frames.is_empty() {
      track.frame_count = frames.len() as u32;
    }
    if prefer_animation == Some(false) {
      track.animated = false;
      track.repetition_count = 0.0;
    } else if let Some(repetition_count) = repetition_count {
      track.repetition_count = repetition_count;
    }
  }

  // Wrap frames in Arc for efficient sharing from cache
  let shared_frames: Vec<(Arc<ParkingLotRwLock<Frame>>, VideoPixelFormat)> = frames
    .into_iter()
    .map(|(f, format)| (f.into_shared(), format))
    .collect();
  guard.cached_frames = Some(shared_frames);
  guard.context = Some(context);
  guard.cached_data_len = data_bytes.len();

  Ok(())
}

/// The alpha hint in a WebP header: the VP8X alpha flag, or the alpha_is_used bit of
/// a simple lossless (VP8L) file. Chrome labels the decoded image BGRA or BGRX by it.
fn webp_alpha_hint(data: &[u8]) -> Option<bool> {
  if data.len() < 25 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
    return None;
  }
  match &data[12..16] {
    b"VP8X" => Some(data[20] & 0x10 != 0),
    // 0x2f signature, then 14 bits width - 1, 14 bits height - 1, 1 bit alpha_is_used
    b"VP8L" if data[20] == 0x2f => Some(data[24] & 0x10 != 0),
    _ => None,
  }
}

/// The alpha a PNG header declares: color type 4 or 6, or a tRNS chunk (which comes
/// before IDAT). Chrome labels the decoded image BGRA or BGRX by it, even when every
/// tRNS entry is 255.
fn png_alpha_hint(data: &[u8]) -> Option<bool> {
  const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
  if data.len() < 33 || &data[..8] != SIGNATURE || &data[12..16] != b"IHDR" {
    return None;
  }
  if data[25] & 4 != 0 {
    return Some(true);
  }
  let mut pos = 8usize;
  while let Some(header) = data.get(pos..pos.checked_add(8)?) {
    match &header[4..8] {
      b"tRNS" => return Some(true),
      b"IDAT" | b"IEND" => return Some(false),
      _ => {}
    }
    // length, type, data, CRC
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    pos = pos.checked_add(12)?.checked_add(length)?;
  }
  None
}

/// Whether Chrome decodes a JPEG to YUV planes, from its SOF sampling factors: three
/// components, both chroma components 1x1 and luma 1x1, 2x1 or 2x2 (4:4:4, 4:2:2,
/// 4:2:0). Other factors decode to BGRX, also ones FFmpeg reads as the same chroma
/// layout (FFmpeg writes 4:2:2 as 2x2/1x2/1x2 and 4:4:4 as 1x2/1x2/1x2).
fn jpeg_yuv_hint(data: &[u8]) -> Option<bool> {
  if data.get(..2)? != [0xFF, 0xD8] {
    return None;
  }
  let mut pos = 2usize;
  loop {
    if *data.get(pos)? != 0xFF {
      return None;
    }
    let marker = *data.get(pos + 1)?;
    match marker {
      // fill byte before a marker
      0xFF => pos += 1,
      // markers without a length: TEM, RSTn, SOI
      0x01 | 0xD0..=0xD8 => pos += 2,
      // scan data or end of image before any frame header
      0xD9 | 0xDA => return None,
      _ => {
        let length = u16::from_be_bytes([*data.get(pos + 2)?, *data.get(pos + 3)?]) as usize;
        // SOFn (C4 DHT, C8 JPG and CC DAC are not frame headers): precision, height,
        // width, component count, then (id, HxV, table) for each component
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
          let header = data.get(pos + 4..pos + 2 + length)?;
          let count = *header.get(5)? as usize;
          let factors = (0..count)
            .map(|i| header.get(7 + 3 * i).copied())
            .collect::<Option<Vec<u8>>>()?;
          return Some(
            count == 3
              && matches!(factors[0], 0x11 | 0x21 | 0x22)
              && factors[1] == 0x11
              && factors[2] == 0x11,
          );
        }
        pos += 2 + length;
      }
    }
  }
}

/// Parse the GIF application extension used for animation loop control.
/// NETSCAPE/ANIMEXTS stores a little-endian repeat count; zero means infinite.
fn gif_repetition_count(data: &[u8]) -> Option<f64> {
  const NETSCAPE: &[u8; 11] = b"NETSCAPE2.0";
  const ANIMEXTS: &[u8; 11] = b"ANIMEXTS1.0";

  for window in data.windows(18) {
    if window[0] != 0x21 || window[1] != 0xff || window[2] != 0x0b {
      continue;
    }
    let application = &window[3..14];
    if application != NETSCAPE && application != ANIMEXTS {
      continue;
    }
    if window[14] != 0x03 || window[15] != 0x01 {
      continue;
    }

    let count = u16::from_le_bytes([window[16], window[17]]);
    return Some(if count == 0 {
      f64::INFINITY
    } else {
      count as f64
    });
  }

  // No loop extension means the animation plays once and repeats zero times.
  Some(0.0)
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
  if mime_lower == "image/jxl" {
    return Ok(AVCodecID::Jpegxl);
  }

  Err(Error::new(
    Status::GenericFailure,
    format!("Unsupported image type: {}", mime_type),
  ))
}

fn create_image_decoder(codec_id: AVCodecID, extradata: Option<Vec<u8>>) -> Result<CodecContext> {
  let mut context = CodecContext::new_decoder(codec_id).map_err(|e| {
    Error::new(
      Status::GenericFailure,
      format!("Failed to create decoder: {}", e),
    )
  })?;

  context
    .configure_decoder(&DecoderConfig {
      codec_id,
      thread_count: 0,
      extradata,
      low_latency: false,
      width: None,
      height: None,
    })
    .map_err(|e| {
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
  Ok(context)
}

/// Decode image data using FFmpeg. Image formats are containers too: animated
/// GIF/WebP files must be demuxed into per-frame packets before decoding.
/// Also returns the number of video streams in the container (an AVIF alpha plane
/// is a stream of its own); a raw bitstream counts as one.
fn decode_image_data(
  codec_id: AVCodecID,
  data: &[u8],
) -> Result<(CodecContext, Vec<crate::codec::Frame>, usize)> {
  if let Ok(mut demuxer) = DemuxerContext::open_buffer(data.to_vec())
    && let Some(stream) = demuxer.find_best_stream(MediaType::Video).cloned()
  {
    let codec_matches = stream.codec_id == codec_id
      || matches!(
        (codec_id, stream.codec_id),
        (AVCodecID::Jpegxl, AVCodecID::JpegxlAnim) | (AVCodecID::JpegxlAnim, AVCodecID::Jpegxl)
      );
    if !codec_matches {
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "Image data codec {:?} does not match the declared MIME type codec {:?}",
          stream.codec_id, codec_id
        ),
      ));
    }
    let mut context = create_image_decoder(stream.codec_id, stream.extradata)?;
    let mut frames = Vec::new();

    while let Some((packet, stream_index)) = demuxer.read_packet().map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to demux image: {}", e),
      )
    })? {
      if stream_index == stream.index {
        frames.extend(
          context
            .decode(Some(&packet))
            .map_err(|e| Error::new(Status::GenericFailure, format!("Decode failed: {}", e)))?,
        );
      }
    }

    frames.extend(context.flush_decoder().map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to flush image decoder: {}", e),
      )
    })?);

    if !frames.is_empty() {
      normalize_frame_timing(&mut frames, stream.time_base);
      let video_streams = demuxer
        .streams()
        .iter()
        .filter(|s| s.media_type == MediaType::Video)
        .count();
      return Ok((context, frames, video_streams));
    }
  }

  // Fallback for raw elementary image bitstreams that libavformat cannot
  // probe. Static formats still decode from a single packet as before.
  let mut context = create_image_decoder(codec_id, None)?;
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
  // Raw elementary image packets have no container timestamp. FFmpeg commonly
  // leaves AVFrame.pts at AV_NOPTS_VALUE; WebCodecs timestamps must never
  // expose that sentinel to JavaScript.
  for frame in &mut all_frames {
    if frame.pts() == AV_NOPTS_VALUE {
      frame.set_pts(0);
    }
  }

  Ok((context, all_frames, 1))
}

/// Convert container time-base ticks into the microseconds required by
/// WebCodecs before frames enter the shared cache.
fn normalize_frame_timing(frames: &mut [Frame], time_base: (i32, i32)) {
  let source = (time_base.0 > 0 && time_base.1 > 0).then_some(AVRational {
    num: time_base.0,
    den: time_base.1,
  });
  for frame in frames {
    let pts = frame.pts();
    if pts == AV_NOPTS_VALUE {
      frame.set_pts(0);
    } else if let Some(source) = source {
      frame.set_pts(unsafe { av_rescale_q(pts, source, AVRational::MICROSECONDS) });
    }
    let duration = frame.duration();
    if duration > 0
      && let Some(source) = source
    {
      frame.set_duration(unsafe { av_rescale_q(duration, source, AVRational::MICROSECONDS) });
    }
  }
}

#[cfg(test)]
mod tests {
  use super::{
    gif_repetition_count, jpeg_yuv_hint, normalize_frame_timing, png_alpha_hint, webp_alpha_hint,
  };
  use crate::codec::Frame;
  use crate::ffi::types::AV_NOPTS_VALUE;

  #[test]
  fn parses_finite_and_infinite_gif_repetition_counts() {
    let mut extension = b"\x21\xff\x0bNETSCAPE2.0\x03\x01\x05\x00\x00".to_vec();
    assert_eq!(gif_repetition_count(&extension), Some(5.0));

    extension[16] = 0;
    assert_eq!(gif_repetition_count(&extension), Some(f64::INFINITY));

    // The 18-byte application block is sufficient to read the loop count;
    // the trailing block terminator is not part of the fields inspected here.
    extension.pop();
    assert_eq!(gif_repetition_count(&extension), Some(f64::INFINITY));
  }

  #[test]
  fn reads_webp_alpha_hints() {
    let lossless = |alpha: u8| {
      let mut b = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f\x05\xc0\x00\x00".to_vec();
      b[24] = alpha;
      b
    };
    assert_eq!(webp_alpha_hint(&lossless(0x10)), Some(true));
    assert_eq!(webp_alpha_hint(&lossless(0x00)), Some(false));
    let mut extended = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\x10\0\0\0\0".to_vec();
    assert_eq!(webp_alpha_hint(&extended), Some(true));
    extended[20] = 0x02; // animation flag only
    assert_eq!(webp_alpha_hint(&extended), Some(false));
    assert_eq!(
      webp_alpha_hint(b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\0\0"),
      None
    );
    assert_eq!(webp_alpha_hint(b"GIF89a"), None);
  }

  #[test]
  fn reads_png_alpha_hints() {
    let chunk = |kind: &[u8], data: &[u8]| {
      let mut c = (data.len() as u32).to_be_bytes().to_vec();
      c.extend_from_slice(kind);
      c.extend_from_slice(data);
      c.extend_from_slice(&[0; 4]); // CRC is not checked
      c
    };
    let png = |color_type: u8, chunks: &[Vec<u8>]| {
      let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
      b.extend(chunk(
        b"IHDR",
        &[0, 0, 0, 1, 0, 0, 0, 1, 8, color_type, 0, 0, 0],
      ));
      chunks.iter().for_each(|c| b.extend_from_slice(c));
      b.extend(chunk(b"IDAT", &[]));
      b
    };
    let plte = chunk(b"PLTE", &[0, 0, 0]);
    let trns = chunk(b"tRNS", &[255]);
    let text = chunk(b"tEXt", b"a\0b");
    assert_eq!(
      png_alpha_hint(&png(3, std::slice::from_ref(&plte))),
      Some(false)
    );
    // tRNS declares alpha even when every entry is 255
    assert_eq!(
      png_alpha_hint(&png(3, &[plte, text.clone(), trns.clone()])),
      Some(true)
    );
    assert_eq!(png_alpha_hint(&png(0, &[trns])), Some(true));
    assert_eq!(png_alpha_hint(&png(2, &[text])), Some(false));
    assert_eq!(png_alpha_hint(&png(4, &[])), Some(true));
    assert_eq!(png_alpha_hint(&png(6, &[])), Some(true));
    // no IDAT yet (truncated) or not a PNG
    assert_eq!(png_alpha_hint(&png(3, &[])[..33]), None);
    assert_eq!(png_alpha_hint(b"GIF89a"), None);
  }

  #[test]
  fn reads_jpeg_yuv_hints() {
    // SOI, APP0 (skipped), SOF with the given (HxV) factors per component, SOS
    let jpeg = |sof: u8, factors: &[u8]| {
      let mut b = vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xFF, sof,
      ];
      b.extend_from_slice(&((8 + 3 * factors.len()) as u16).to_be_bytes());
      b.extend_from_slice(&[8, 0, 16, 0, 16, factors.len() as u8]);
      for (i, f) in factors.iter().enumerate() {
        b.extend_from_slice(&[i as u8 + 1, *f, 0]);
      }
      b.extend_from_slice(&[0xFF, 0xDA]);
      b
    };
    for (factors, yuv) in [
      (&[0x22, 0x11, 0x11][..], true),
      (&[0x21, 0x11, 0x11], true),
      (&[0x11, 0x11, 0x11], true),
      (&[0x12, 0x11, 0x11], false), // 4:4:0
      (&[0x41, 0x11, 0x11], false), // 4:1:1
      (&[0x22, 0x12, 0x12], false), // FFmpeg's 4:2:2
      (&[0x12, 0x12, 0x12], false), // FFmpeg's 4:4:4
      (&[0x21, 0x21, 0x21], false),
      (&[0x11], false),                   // grayscale
      (&[0x11, 0x11, 0x11, 0x11], false), // CMYK
    ] {
      assert_eq!(
        jpeg_yuv_hint(&jpeg(0xC0, factors)),
        Some(yuv),
        "{factors:02x?}"
      );
    }
    assert_eq!(jpeg_yuv_hint(&jpeg(0xC2, &[0x22, 0x11, 0x11])), Some(true)); // progressive
    // a DHT segment is not a frame header
    let mut dht_first = jpeg(0xC0, &[0x22, 0x12, 0x12]);
    dht_first.splice(2..2, [0xFF, 0xC4, 0x00, 0x02]);
    assert_eq!(jpeg_yuv_hint(&dht_first), Some(false));
    // truncated, no frame header, not a JPEG
    assert_eq!(jpeg_yuv_hint(&jpeg(0xC0, &[0x22, 0x11, 0x11])[..15]), None);
    assert_eq!(jpeg_yuv_hint(&[0xFF, 0xD8, 0xFF, 0xDA]), None);
    assert_eq!(jpeg_yuv_hint(b"GIF89a"), None);
  }

  #[test]
  fn gif_without_loop_extension_does_not_repeat() {
    assert_eq!(gif_repetition_count(b"GIF89a"), Some(0.0));
  }

  #[test]
  fn container_frame_without_pts_is_normalized_to_zero() {
    let mut frame = Frame::new().expect("allocate frame");
    frame.set_pts(AV_NOPTS_VALUE);
    let mut frames = [frame];

    normalize_frame_timing(&mut frames, (1, 1000));

    assert_eq!(frames[0].pts(), 0);
  }

  #[test]
  fn missing_pts_is_normalized_even_with_invalid_container_time_base() {
    let mut frame = Frame::new().expect("allocate frame");
    frame.set_pts(AV_NOPTS_VALUE);
    let mut frames = [frame];

    normalize_frame_timing(&mut frames, (0, 0));

    assert_eq!(frames[0].pts(), 0);
  }
}
