//! ImageDecoder - WebCodecs API implementation
//!
//! Provides image decoding functionality using FFmpeg.
//! See: <https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder>

use crate::codec::demuxer::{DemuxerContext, MediaType};
use crate::codec::jpeg::{decode_jpeg, jpeg_info};
use crate::codec::{CodecContext, DecoderConfig, Frame, Packet};
use crate::ffi::avutil::{av_opt_set_int, av_rescale_q, opt_flag};
use crate::ffi::{AV_NOPTS_VALUE, AVCodecID, AVPixelFormat, AVRational, check_error};
use crate::webcodecs::buffer_source::{self, Uint8ArrayCopy};
use crate::webcodecs::decoded_frame::{DecodedSource, decoded_frame_format};
use crate::webcodecs::error::{
  DOMExceptionName, invalid_state_error, native_dom_exception_error, native_range_error,
  throw_invalid_state_error,
};
use crate::webcodecs::image_hints::{PixelHint, bmp_header, gif_frame_alpha, png_header};
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
  /// Desired width (optional). Like Chrome, only a JPEG uses it, and only with desired_height.
  pub desired_width: Option<u32>,
  /// Desired height (optional). Like Chrome, only a JPEG uses it, and only with desired_width.
  pub desired_height: Option<u32>,
  /// Whether to prefer animation (for animated formats)
  pub prefer_animation: Option<bool>,
}

/// Read an optional `[EnforceRange] unsigned long` member of ImageDecoderInit the way Chrome
/// does: undefined is absent; any other value goes through ToNumber (null is 0, strings
/// convert) and is truncated; NaN, an infinity or a value outside 0..=2^32-1 is a TypeError.
fn enforce_range_unsigned_long(env: &Env, init: &Object, name: &str) -> Result<Option<u32>> {
  let value: Unknown = init.get_named_property(name)?;
  if value.get_type()? == ValueType::Undefined {
    return Ok(None);
  }
  let number = value.coerce_to_number()?.get_double()?;
  let problem = if number.is_nan() {
    "Value is not of type 'unsigned long'."
  } else if number.is_infinite() {
    "Value is infinite and not of type 'unsigned long'."
  } else if !(0.0..=f64::from(u32::MAX)).contains(&number.trunc()) {
    "Value is outside the 'unsigned long' value range."
  } else {
    return Ok(Some(number.trunc() as u32));
  };
  let message = format!(
    "Failed to construct 'ImageDecoder': Failed to read the '{name}' property from 'ImageDecoderInit': {problem}"
  );
  env.throw_type_error(&message, None)?;
  Err(Error::new(Status::InvalidArg, message))
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

    // WebIDL reads dictionary members in name order. One of the two alone is no error:
    // Chrome ignores it.
    let desired_height = enforce_range_unsigned_long(&env_wrapper, &obj, "desiredHeight")?;
    let desired_width = enforce_range_unsigned_long(&env_wrapper, &obj, "desiredWidth")?;
    let prefer_animation: Option<bool> = obj.get("preferAnimation").ok().flatten();

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
  /// Whether the metadata was rejected: `ready` then rejects (set before `ready`)
  failed: Arc<AtomicBool>,
  /// Notifier for when ready becomes true
  ready_notify: Arc<Notify>,
}

impl Clone for ImageTrackList {
  fn clone(&self) -> Self {
    ImageTrackList {
      inner: self.inner.clone(),
      ready: self.ready.clone(),
      failed: self.failed.clone(),
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
  #[napi(getter, ts_return_type = "Promise<void>")]
  pub fn ready<'env>(&self, env: &'env Env) -> Result<PromiseRaw<'env, ()>> {
    let tracks = self.clone();
    let wait = async move {
      // Fast path: already ready
      if !tracks.ready.load(Ordering::Acquire) {
        // IMPORTANT: Create the listener FIRST, before checking the flag again.
        // This prevents a race condition where notify_waiters() is called
        // after our initial check but before we register as a waiter.
        let notified = tracks.ready_notify.notified();

        // Check again - if became ready while creating listener, do not wait.
        // Otherwise the listener is registered, so any subsequent
        // notify_waiters() call will wake us up
        if !tracks.ready.load(Ordering::Acquire) {
          notified.await;
        }
      }
      Ok(tracks.failed.load(Ordering::Acquire))
    };
    let ready = env.spawn_future_with_callback(wait, |env, failed| {
      if failed {
        return Err(native_dom_exception_error(
          env,
          DOMExceptionName::InvalidStateError,
          "Failed to retrieve track metadata.",
        )?);
      }
      Ok(())
    })?;
    // Like Chrome, never report a rejection of `ready` as unhandled.
    ready.catch(|_: CallbackContext<Unknown>| Ok(()))?;
    Ok(ready)
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
  /// Whether all the data is here: a buffer, or a stream that ended (`complete` follows once
  /// the data is decoded).
  all_data: bool,
  /// Whether a decode() of the first frame came before all the data. Chrome then decodes a JPEG
  /// to RGB rows as the data arrives, and never to YUV planes after that
  /// (image_decoder_core.cc have_completed_rgb_decode_). Only the JPEG path reads it. The
  /// stream reader sees the end of a stream one task after its last chunk, so a decode() made
  /// before the stream is read counts even when the last chunk and the close came together,
  /// where Chrome sees both at once and decodes YUV.
  rgb_decode: bool,
  /// Whether a frame failed to decode where Chrome's decoder fails (JPEG): the decoder closes,
  /// and the decode() calls still pending reject (see `closed_error`).
  frame_failed: bool,
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
  /// Desired width (a JPEG decodes at M/8 of its size, see `jpeg_scale_numerator`)
  desired_width: Option<u32>,
  /// Desired height (a JPEG decodes at M/8 of its size, see `jpeg_scale_numerator`)
  desired_height: Option<u32>,
  /// Whether to prefer animation for animated formats (W3C spec)
  prefer_animation: Option<bool>,
}

impl ImageDecoderInner {
  fn close(&mut self) {
    self.context = None;
    self.cached_frames = None;
    self.cache_epoch = self.cache_epoch.wrapping_add(1);
    self.cached_data_len = 0;
    self.closed = true;

    // Wake any waiters so they can check closed state
    self.tracks.ready.store(true, Ordering::Release);
    self.tracks.ready_notify.notify_waiters();
    self.tracks.ready_notify.notify_one();
  }
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
      failed: Arc::new(AtomicBool::new(false)),
      ready_notify: ready_notify.clone(),
    };

    // Create inner state first so we can share it with async tasks
    let inner = Arc::new(Mutex::new(ImageDecoderInner {
      data: ImageDecoderData::Empty,
      mime_type: init.mime_type.clone(),
      codec_id,
      context: None,
      complete: complete.clone(),
      all_data: false,
      rgb_decode: false,
      frame_failed: false,
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
        inner_guard.all_data = true;
      }
      complete.store(true, Ordering::Release);

      // For buffer data: completed resolves immediately (data is already available)
      let completed_promise = env.spawn_future(async { Ok(()) })?;

      // Spawn pre-parse task for ready promise. Always signal ready, even on error (per W3C
      // spec behavior). Like Chrome, a rejected metadata closes the decoder in a JS task of its
      // own, so a decode() made before that task is still pending.
      let parse_inner = inner.clone();
      let ready_inner = inner.clone();
      let ready_promise = env.spawn_future_with_callback(
        async move {
          let _ = spawn_blocking(move || cache_decoded_frames(&parse_inner, false)).await;
          Ok(())
        },
        move |_, ()| {
          close_if_failed(&ready_inner);
          signal_tracks_ready(&ready_inner);
          Ok(())
        },
      )?;

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

      // The collector resolves with whether the decoder failed before the data was complete:
      // Chrome then closes it and stops reading, and `completed` rejects.
      let data_complete = complete.clone();
      let collect = async move {
        let mut next_probe_size = 1usize;

        while let Some(item) = reader.next().await {
          let chunk = match item {
            Ok(chunk) => chunk,
            Err(error) => {
              // A stream error is terminal. Mark the input complete before
              // waking decode waiters so they reject instead of waiting for an
              // update that can never arrive.
              if let Ok(mut inner_guard) = inner_clone.lock() {
                inner_guard.all_data = true;
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
              spawn_blocking(move || cache_decoded_frames(&probe_inner, false)).await,
              Ok(Ok(()))
            ) {
              signal_tracks_ready(&inner_clone);
            }
            if close_if_failed(&inner_clone) {
              return Ok(!data_complete.load(Ordering::Acquire));
            }
            next_probe_size = buffered_len.saturating_mul(2).max(next_probe_size * 2);
          }
        }

        // Refresh once at EOF so animated images include frames that arrived
        // after an early successful probe. Like Chrome, the data is complete only
        // once it is decoded: a metadata rejected here still rejects `completed`.
        if let Ok(mut inner_guard) = inner_clone.lock() {
          inner_guard.all_data = true;
        }
        let final_inner = inner_clone.clone();
        let _ = spawn_blocking(move || cache_decoded_frames(&final_inner, false)).await;
        if close_if_failed(&inner_clone) {
          return Ok(!data_complete.load(Ordering::Acquire));
        }
        if let Ok(inner_guard) = inner_clone.lock() {
          inner_guard.complete.store(true, Ordering::Release);
        }
        signal_tracks_ready(&inner_clone);
        Ok(false)
      };
      let collection_promise = env.spawn_future_with_callback(collect, move |env, aborted| {
        if aborted {
          return Err(native_dom_exception_error(
            env,
            DOMExceptionName::AbortError,
            "Aborted by close.",
          )?);
        }
        Ok(())
      })?;
      // Like Chrome, never report a rejection of `completed` as unhandled.
      collection_promise.catch(|_: CallbackContext<Unknown>| Ok(()))?;

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

    // Get stored promise from this and chain a new promise for this access. Like Chrome,
    // never report its rejection as unhandled.
    let promise: PromiseRaw<()> = this.get_named_property(COMPLETED_PROMISE)?;
    let completed = promise.then(|_| Ok(()))?;
    completed.catch(|_: CallbackContext<Unknown>| Ok(()))?;
    Ok(completed)
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
    let frame_index = options.as_ref().and_then(|o| o.frame_index).unwrap_or(0) as usize;
    // Read now: a decode() made before the decoder fails rejects differently from one made
    // after (see `closed_error`).
    let closed = {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      // Chrome decodes the first frame as the data arrives (see `rgb_decode`).
      if frame_index == 0 && !guard.all_data {
        guard.rgb_decode = true;
      }
      guard.closed
    };

    // Keep the Rust error as the resolved future value so the completion
    // callback can construct the precise native JavaScript exception class.
    let decode = async move {
      if closed {
        return Err(invalid_state_error("ImageDecoder is closed"));
      }

      // Wait for ready promise (ensures initial pre-parsing is complete)
      ready_promise.await?;

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
    // A metadata failure counts once `close_if_failed` has closed the decoder. A close() that
    // comes first wins, also when a worker thread already found the failure (`reject_metadata`).
    if !inner.closed {
      inner.tracks.failed.store(false, Ordering::Release);
    }
    inner.close();
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
  let (needs_cache, data_complete, jpeg) = {
    let guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if guard.closed {
      return Err(closed_error(&guard, frame_index));
    }
    (
      guard.cached_frames.is_none(),
      guard.complete.load(Ordering::Acquire),
      guard.codec_id == Some(AVCodecID::Mjpeg),
    )
  };

  // A reset can clear the cache. For fully buffered input, reconstruct it now;
  // for a live stream, wait for the collector's next successful probe. Like Chrome, a
  // JPEG is decoded now with the data so far: it gives a frame only once it is whole.
  if needs_cache && (data_complete || (jpeg && frame_index == 0)) {
    cache_decoded_frames(inner, true).map_err(|error| {
      if !close_if_failed(inner) {
        return error;
      }
      inner
        .lock()
        .map_or(error, |guard| closed_error(&guard, frame_index))
    })?;
  }

  let guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if guard.closed {
    return Err(closed_error(&guard, frame_index));
  }

  let data_complete = guard.complete.load(Ordering::Acquire);
  let Some(frames) = guard.cached_frames.as_ref() else {
    if !data_complete {
      return Ok(None);
    }
    return Err(Error::new(
      Status::GenericFailure,
      "No frames available - decoding may have failed",
    ));
  };

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

/// The error of a decode() that finds the decoder closed. When the decoder closed because its
/// metadata was rejected or a frame failed, Chrome rejects the decodes still pending with
/// EncodingError.
fn closed_error(inner: &ImageDecoderInner, frame_index: usize) -> Error {
  if inner.tracks.failed.load(Ordering::Acquire) || inner.frame_failed {
    return Error::new(
      Status::GenericFailure,
      format!("Failed to decode frame at index {frame_index}"),
    );
  }
  invalid_state_error("ImageDecoder is closed")
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
/// Failures do not resolve `tracks.ready`, which lets streamed images retry as
/// more bytes arrive. A rejected metadata or a failed frame is left to `close_if_failed`.
/// `attempt`: a decode() waits for the frames (see `jpeg_frame`).
fn cache_decoded_frames(inner: &Arc<Mutex<ImageDecoderInner>>, attempt: bool) -> Result<()> {
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
    all_data,
    rgb_decode,
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
      guard.all_data,
      guard.rgb_decode,
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

  if codec_id == AVCodecID::Mjpeg {
    let desired = desired_width.zip(desired_height);
    return match jpeg_frame(inner, &data_bytes, desired, all_data, rgb_decode, attempt)? {
      Some(frame) => store_frames(inner, cache_epoch, &data_bytes, vec![frame], None, None),
      // the metadata, but no frame yet
      None => Ok(()),
    };
  }

  // Chrome fails the metadata of a BMP whose bitfield masks the image crate rejects.
  let bmp = match codec_id {
    AVCodecID::Bmp => bmp_header(&data_bytes).map_err(|()| reject_metadata(inner))?,
    _ => None,
  };

  // Demux and decode all frames.
  let (context, mut frames, video_streams) = decode_image_data(codec_id, &data_bytes)?;
  // One video stream with one frame: no animation, alpha or other auxiliary image. An
  // 'avis' AVIF is an animation even when only its first frame has arrived.
  let still = video_streams == 1 && frames.len() == 1 && !avif_is_sequence(&data_bytes);

  // Chrome labels each GIF frame by the frames it is drawn over. FFmpeg's frames are the first
  // ones of the file, but can be fewer (one for a GIF without graphic control extensions). A
  // frame list shorter than FFmpeg's (damaged data) makes every frame BGRA.
  let gif_alpha = (codec_id == AVCodecID::Gif).then(|| {
    gif_frame_alpha(&data_bytes)
      .filter(|alpha| alpha.len() >= frames.len())
      .unwrap_or_else(|| vec![true; frames.len()])
  });

  // Apply preferAnimation: if false and format supports animation, only keep first frame
  if prefer_animation == Some(false) && !frames.is_empty() {
    frames.truncate(1);
  }

  // Give every frame a WebCodecs format that describes its bytes, converting the
  // FFmpeg formats WebCodecs has no name for (rgb24, pal8, gray, argb, ...).
  let header = match codec_id {
    AVCodecID::Png => png_header(&data_bytes),
    AVCodecID::Bmp => bmp,
    _ => None,
  };
  let alpha = match codec_id {
    AVCodecID::Webp => webp_alpha_hint(&data_bytes),
    _ => header.map(|header| header.alpha),
  };
  let yuv = match codec_id {
    // Chrome keeps a still AVIF without alpha as YUV planes
    AVCodecID::Av1 => Some(still),
    AVCodecID::Webp => Some(webp_is_simple_lossy(&data_bytes)),
    _ => None,
  };
  let pixels = header.map_or(PixelHint::None, |header| header.pixels);
  let frames = frames
    .into_iter()
    .enumerate()
    .map(|(i, frame)| {
      let source = DecodedSource::Image {
        codec: codec_id,
        alpha: gif_alpha.as_ref().map_or(alpha, |gif| Some(gif[i])),
        yuv,
        pixels,
      };
      decoded_frame_format(frame, source)
    })
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
  store_frames(
    inner,
    cache_epoch,
    &data_bytes,
    frames,
    Some(context),
    repetition_count,
  )
}

/// Cache the frames decoded from `data`, unless reset()/close() or a newer probe came first.
fn store_frames(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  cache_epoch: u64,
  data: &[u8],
  frames: Vec<(Frame, VideoPixelFormat)>,
  context: Option<CodecContext>,
  repetition_count: Option<f64>,
) -> Result<()> {
  let mut guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if guard.closed {
    return Err(invalid_state_error("ImageDecoder is closed"));
  }
  // reset()/close() invalidated the work, or a newer streamed probe already
  // committed more input. Never overwrite either state with this snapshot.
  if guard.cache_epoch != cache_epoch || data.len() < guard.cached_data_len {
    return Ok(());
  }
  let prefer_animation = guard.prefer_animation;

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
  guard.context = context;
  guard.cached_data_len = data.len();

  Ok(())
}

/// Chrome's frame for a JPEG: Blink's JPEGImageDecoder on the libjpeg-turbo Chrome ships (see
/// `codec::jpeg`), at M/8 of its size for desiredWidth/desiredHeight (`jpeg_scale_numerator`).
/// - The header is the metadata. Chrome rejects it when it is bad, when the data ends before it
///   does, or when M is 0.
/// - Like Chrome, the frame is decoded only for a decode(): `attempt`, or `rgb_decode` (a
///   decode() came before all the data, so Chrome decodes as the data arrives). It is YUV planes
///   only when all the data was here before the first decode() of it, else BGRX.
/// - None: no frame (yet). The data so far ends before the image does, or nothing decoded it.
fn jpeg_frame(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  data: &[u8],
  desired: Option<(u32, u32)>,
  all_data: bool,
  rgb_decode: bool,
  attempt: bool,
) -> Result<Option<(Frame, VideoPixelFormat)>> {
  let allow_yuv = all_data && !rgb_decode;
  let info = match jpeg_info(data, 8, allow_yuv) {
    Ok(Some(info)) => info,
    Ok(None) if !all_data => {
      return Err(Error::new(
        Status::GenericFailure,
        "The JPEG header has not arrived yet",
      ));
    }
    // A header cut short by the end of the data fails only once the data is complete.
    Ok(None) => {
      if let Ok(guard) = inner.lock() {
        guard.complete.store(true, Ordering::Release);
      }
      return Err(reject_metadata(inner));
    }
    Err(_) => return Err(reject_metadata(inner)),
  };
  let scale = desired.map_or(8, |desired| {
    jpeg_scale_numerator(info.width, info.height, desired)
  });
  // Chrome rejects the metadata and closes the decoder.
  if scale == 0 {
    return Err(reject_metadata(inner));
  }
  if !(attempt || rgb_decode) {
    return Ok(None);
  }
  match decode_jpeg(data, scale, allow_yuv) {
    Ok(Some(mut frame)) => {
      frame.set_pts(0);
      let format = match frame.format() {
        // alpha 255
        AVPixelFormat::Bgra => VideoPixelFormat::BGRX,
        yuv => VideoPixelFormat::from_av_format(yuv).unwrap_or(VideoPixelFormat::I420),
      };
      Ok(Some((frame, format)))
    }
    // Chrome resolves `completed` before it tries the frame with all the data.
    Ok(None)
      if !inner
        .lock()
        .is_ok_and(|guard| guard.complete.load(Ordering::Acquire)) =>
    {
      Ok(None)
    }
    // A frame Chrome fails to decode fails the decoder (see `close_if_failed`) when Chrome tries
    // it: an error as soon as it decodes it, data that ends before the image does at a decode()
    // once the data is complete.
    result => {
      if (attempt || result.is_err())
        && let Ok(mut guard) = inner.lock()
      {
        guard.frame_failed = true;
      }
      Err(Error::new(
        Status::GenericFailure,
        "Failed to decode frame at index 0",
      ))
    }
  }
}

/// Reject the metadata. The caller then closes the decoder with `close_if_failed`.
fn reject_metadata(inner: &Arc<Mutex<ImageDecoderInner>>) -> Error {
  if let Ok(guard) = inner.lock()
    && !guard.closed
  {
    guard.tracks.failed.store(true, Ordering::Release);
  }
  invalid_state_error("Failed to retrieve track metadata.")
}

/// Close the decoder if its metadata was rejected (`tracks.ready` rejects) or a frame failed:
/// the decode() calls still pending reject (see `closed_error`). Returns whether it failed.
fn close_if_failed(inner: &Arc<Mutex<ImageDecoderInner>>) -> bool {
  let Ok(mut guard) = inner.lock() else {
    return false;
  };
  let failed = guard.tracks.failed.load(Ordering::Acquire) || guard.frame_failed;
  if failed && !guard.closed {
    guard.close();
  }
  failed
}

/// Chrome's scale numerator M: libjpeg decodes a JPEG at M/8 of its size so that its 4-byte
/// pixels fit the budget that desiredWidth/desiredHeight give
/// (JPEGImageDecoder::DesiredScaleNumerator, CalculateMaxDecodedBytes). 8 is full size; 0 means
/// that even 1/8 is over the budget, and Chrome then fails. Chrome computes in 32 bits: a side
/// of 2^31 or more is a negative int (no budget), and the byte counts wrap.
fn jpeg_scale_numerator(width: u32, height: u32, desired: (u32, u32)) -> u32 {
  let (desired_width, desired_height) = (desired.0 as i32, desired.1 as i32);
  // An empty SkISize sets no budget.
  if desired_width <= 0 || desired_height <= 0 {
    return 8;
  }
  let max_bytes = (desired_width as u32)
    .wrapping_mul(desired_height as u32)
    .wrapping_mul(4);
  let original_bytes = width.wrapping_mul(height).wrapping_mul(4);
  if original_bytes <= max_bytes {
    return 8;
  }
  ((f64::from(max_bytes) / f64::from(original_bytes)).sqrt() * 8.0).floor() as u32
}

/// Whether an AVIF's major brand is 'avis', an image sequence. Chrome then decodes its
/// track, not the still image, and resolves no frame before the track header ('moov').
/// The ftyp box comes first, so this is known before any frame. The frame count is
/// not: a stream can stop after the first frame, or before a 'moov' that follows the
/// image data, where FFmpeg sees one stream with one frame.
fn avif_is_sequence(data: &[u8]) -> bool {
  data.get(4..12) == Some(&b"ftypavis"[..])
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

/// Whether a WebP is simple lossy: its first chunk is VP8, so it has no VP8X header (Blink's
/// IsSimpleLossyWebPImage). Chrome decodes only these to YUV planes.
fn webp_is_simple_lossy(data: &[u8]) -> bool {
  data.len() >= 20 && data.starts_with(b"RIFF") && &data[8..16] == b"WEBPVP8 "
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
  if codec_id == AVCodecID::Gif {
    // Chrome's transparent GIF pixels are 0, 0, 0, 0; FFmpeg's default is transparent white.
    check_error(unsafe {
      av_opt_set_int(
        context.as_mut_ptr().cast(),
        c"trans_color".as_ptr(),
        0,
        opt_flag::SEARCH_CHILDREN,
      )
    })
    .map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to configure decoder: {}", e),
      )
    })?;
  }
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
    avif_is_sequence, gif_repetition_count, jpeg_scale_numerator, normalize_frame_timing,
    webp_alpha_hint, webp_is_simple_lossy,
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
  fn finds_simple_lossy_webps_like_blink() {
    assert!(webp_is_simple_lossy(b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0"));
    // 20 bytes at least, like Blink's IsSimpleLossyWebPImage
    assert!(!webp_is_simple_lossy(b"RIFF\0\0\0\0WEBPVP8 \0\0\0"));
    assert!(!webp_is_simple_lossy(b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0"));
    assert!(!webp_is_simple_lossy(b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0"));
    assert!(!webp_is_simple_lossy(b"RIFX\0\0\0\0WEBPVP8 \0\0\0\0"));
  }

  #[test]
  fn computes_chromes_jpeg_scale_numerator() {
    // (JPEG size, desired size, M): Chrome 155 decodes at ceil(size * M / 8), and fails for 0
    for ((width, height), desired, m) in [
      ((64, 48), (32, 24), 4),
      ((64, 48), (63, 47), 7),
      ((64, 48), (64, 12), 4),
      ((64, 48), (31, 23), 3),
      ((64, 48), (64, 48), 8),
      ((64, 48), (128, 96), 8),
      ((64, 48), (7, 6), 0),
      ((64, 48), (0, 24), 8),
      ((37, 9), (36, 8), 7),
      ((4000, 3000), (800, 600), 1),
      ((4000, 3000), (999, 749), 1),
      ((4000, 3000), (1000, 750), 2),
      ((4000, 3000), (3999, 2999), 7),
      ((4000, 3000), (499, 374), 0),
      ((4000, 3000), (16000, 1), 0),
      // the budget (4 bytes per desired pixel) wraps at 2^32
      ((4000, 3000), (32768, 32768), 0),
      ((4000, 3000), (32768, 32769), 0),
      ((4000, 3000), (46341, 46341), 0),
      ((4000, 3000), (65535, 65535), 8),
      ((64, 48), (65536, 16384), 0),
      ((64, 48), (65536, 16385), 8),
      ((320, 240), (65536, 16385), 7),
      ((320, 240), (46341, 46341), 1),
      // a side of 2^31 or more is a negative int: no budget
      ((4000, 3000), (1 << 31, 24), 8),
      ((4000, 3000), (1 << 31, 1 << 31), 8),
      ((320, 240), (u32::MAX, 1), 8),
    ] {
      assert_eq!(
        jpeg_scale_numerator(width, height, desired),
        m,
        "{width}x{height} at {desired:?}"
      );
    }
  }

  #[test]
  fn reads_the_avif_sequence_brand() {
    // only the major brand counts: Chrome decodes the still image of an 'avif' file
    // that also lists 'avis'
    assert!(avif_is_sequence(b"\0\0\0\x1cftypavis\0\0\0\0avifavis"));
    assert!(!avif_is_sequence(b"\0\0\0\x1cftypavif\0\0\0\0avifavis"));
    assert!(!avif_is_sequence(b"\0\0\0\x1cftypav"));
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
