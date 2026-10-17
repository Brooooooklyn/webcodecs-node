//! ImageDecoder - WebCodecs API implementation
//!
//! Provides image decoding functionality: GIF and BMP with the decoders that follow Chrome
//! (gif_decoder, bmp_decoder), the other formats with FFmpeg.
//! See: <https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder>

use crate::codec::demuxer::{DemuxerContext, MediaType};
use crate::codec::jpeg::{JpegDecode, decode_jpeg, jpeg_info};
use crate::codec::{CodecContext, CodecError, DecoderConfig, Frame, Packet};
use crate::ffi::avutil::{av_image_check_size, av_rescale_q};
use crate::ffi::{
  AV_NOPTS_VALUE, AVCodecID, AVColorRange, AVColorSpace, AVPixelFormat, AVRational,
};
use crate::webcodecs::avif::{self, Avif, AvifImage, AvifSamples};
use crate::webcodecs::bmp_decoder::{self, BmpDecoder};
use crate::webcodecs::buffer_source::{self, Uint8ArrayCopy};
use crate::webcodecs::decoded_frame::{DecodedSource, avif_frame, decoded_frame_format};
use crate::webcodecs::error::{
  DOMExceptionName, abort_error, invalid_state_error, native_dom_exception_error,
  native_range_error, throw_invalid_state_error,
};
use crate::webcodecs::gif_decoder::{self, GifDecoder};
use crate::webcodecs::image_hints::{PixelHint, png_header};
use crate::webcodecs::{VideoFrame, VideoPixelFormat};
use futures::stream::StreamExt;
use napi::bindgen_prelude::*;
use napi::tokio::sync::Notify;
use napi::tokio::sync::mpsc::{
  UnboundedReceiver, UnboundedSender, WeakUnboundedSender, unbounded_channel,
};
use napi_derive::napi;
use parking_lot::RwLock as ParkingLotRwLock;
use std::collections::HashSet;
use std::sync::{
  Arc, Mutex, Weak,
  atomic::{AtomicBool, AtomicU8, Ordering},
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

impl ImageTrackData {
  /// The track before its metadata is known
  fn initial(codec_id: Option<AVCodecID>) -> Self {
    // Determine if this is an animated format
    let animated = matches!(
      codec_id,
      Some(AVCodecID::Gif)
        | Some(AVCodecID::Webp)
        | Some(AVCodecID::Jpegxl)
        | Some(AVCodecID::JpegxlAnim)
    );
    ImageTrackData {
      animated,
      // For static images, there's one frame; for animated, we'll detect later
      frame_count: if animated { 0 } else { 1 },
      repetition_count: if animated { f64::INFINITY } else { 0.0 },
    }
  }
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
  /// Whether a decode of the first frame of a JPEG ran on part of the data and got to output
  /// rows. Like Chrome, the JPEG is then decoded to RGB rows as the data arrives, and never to
  /// YUV planes after that, also not after reset() (image_decoder_core.cc
  /// have_completed_rgb_decode_). Set by `jpeg_frame`.
  rgb_decode: bool,
  /// What the JS thread has read of a JPEG stream: the bytes, and whether it ended. A decode()
  /// that waits tries the frame again only once more bytes came (`decode_posted_bytes`), like
  /// Chrome (ImageDecoderExternal::MaybeSatisfyPendingDecodes, `bytes_read_index`).
  stream_bytes: usize,
  stream_ended: bool,
  decode_posted_bytes: usize,
  /// How many reads of a JPEG stream the JS thread has handled, and whether the reader's `closed`
  /// promise has settled there (see `queue_jpeg_post`)
  stream_reads: u64,
  stream_closed: bool,
  /// The JPEG decode() calls in `jpeg_requests` whose try is not posted yet (see
  /// `queue_jpeg_post`). A chunk that comes to an idle stream is tried at once only for a decode()
  /// made before it: Chrome reads that chunk inside the enqueue(), so a decode() made later in the
  /// same task was not there yet.
  jpeg_unposted: u32,
  /// `stream_reads` at each of those decode() calls whose try waits for the reads after a chunk
  /// that came before it (see `post_before_end`)
  jpeg_drains: Vec<u64>,
  /// Where the JS thread sends a JPEG stream's results and decode tries, in its own order, to
  /// the collector (see `JpegItem`). Weak: the JS side (`JpegStream`) holds the only strong
  /// sender, so the collector ends when the JS side is gone, also at env teardown.
  jpeg_items: Option<WeakUnboundedSender<JpegItem>>,
  /// The decode() calls of the first frame of a JPEG that still wait, made since the last
  /// reset(), which aborts them (`abort_if_reset`). While there are any that posted their try
  /// (`jpeg_unposted`), a chunk that comes to an idle stream is decoded at once (see
  /// `FrameCall::Arrival`).
  jpeg_requests: u32,
  /// Whether a frame failed to decode where Chrome's decoder fails (JPEG), with the
  /// `cache_epoch` that the work which found it first was made for (see `jpeg_frame`). Only the
  /// JS thread closes the decoder on it, like Chrome, which applies a failed decode in a
  /// main-thread task (OnDecodeReady): a decode() of that epoch, as it settles
  /// (`apply_frame_failure`), or the JS step at the end of the stream (`end_with_frame_failure`).
  /// The decode() calls still pending then reject (see `closed_error`). A failure from an older
  /// epoch (a reset() came) is kept: the next decode() fails and closes the decoder
  /// (`decode_cached_frame`). A close() that comes first clears it.
  frame_failed: Option<u64>,
  /// Whether the collector took the end of a JPEG stream while a frame failure waited for a
  /// decode() of the current epoch: the JS thread then decides how `completed` settles
  /// (`StreamEnd::FrameFailure`).
  end_deferred: bool,
  /// Track list
  tracks: ImageTrackList,
  /// Whether decoder is closed
  closed: bool,
  /// Cached frames (for animated images, populated on first decode)
  cached_frames: Option<Vec<CachedFrame>>,
  /// The frames Chrome tried and failed to decode. When they are all the frames, Chrome fails
  /// the decoder. A reset() keeps them, like Chrome's.
  failed_frames: HashSet<usize>,
  /// Whether every frame failed to decode: the decoder then closed, and the decode() calls still
  /// pending reject with EncodingError (see `closed_error`). As for `frame_failed`, a failure
  /// found by a decode() that a reset() made stale leaves the decoder open, and the next decode()
  /// closes it on the JS thread (`apply_frame_failure`). A close() that comes first clears it.
  frames_failed: bool,
  /// The ticket of the next decode() call. Like Chrome, the calls are handled one at a time, in
  /// call order: each one waits until `decode_turn` is its ticket, also one made on a closed
  /// decoder, so the promises settle in call order.
  next_decode: u64,
  /// The ticket of the decode() call being handled
  decode_turn: u64,
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

  /// What reset() does to an open decoder. Like Chrome, whose decoder stays failed after a
  /// reset(), it keeps `frame_failed` and `frames_failed`.
  fn reset(&mut self) {
    self.context = None;
    self.cached_frames = None;
    self.cache_epoch = self.cache_epoch.wrapping_add(1);
    self.cached_data_len = 0;
    // Like Chrome, the decode() calls that wait reject (`abort_if_reset`). A JPEG keeps its
    // choice of RGB (`rgb_decode`).
    self.jpeg_requests = 0;
    self.jpeg_unposted = 0;
    self.jpeg_drains.clear();
    self.decode_posted_bytes = 0;
    // Wake them. Only once the tracks are ready: before that, `ready_notify` would also resolve
    // `tracks.ready`, and the calls still wait for the tracks; they reject when the tracks are
    // ready.
    if self.tracks.ready.load(Ordering::Acquire) {
      self.tracks.ready_notify.notify_waiters();
    }

    // Reset frame_count for animated formats (will be re-detected on next decode)
    if let Ok(mut track_inner) = self.tracks.inner.lock()
      && let Some(track) = track_inner.tracks.get_mut(0)
      && track.animated
    {
      track.frame_count = 0;
    }
  }

  /// Whether the metadata was rejected or a JPEG frame failed (`frame_failed`). Every frame of a
  /// GIF or BMP failing (`frames_failed`) does not count here, only in `closed_error`.
  fn failed(&self) -> bool {
    self.tracks.failed.load(Ordering::Acquire) || self.frame_failed.is_some()
  }

  /// Send an item to the collector of a JPEG stream. Returns whether it is still there.
  fn send_jpeg_item(&self, item: JpegItem) -> bool {
    self
      .jpeg_items
      .as_ref()
      .and_then(WeakUnboundedSender::upgrade)
      .is_some_and(|items| items.send(item).is_ok())
  }
}

/// A frame of the cache
#[derive(Clone)]
enum CachedFrame {
  /// Shared through an Arc, with the WebCodecs format that describes it and its duration
  Decoded {
    frame: Arc<ParkingLotRwLock<Frame>>,
    format: VideoPixelFormat,
    duration: Option<i64>,
  },
  /// Chrome fails to decode it. `required`: the frame it is drawn on, when that one fails too.
  /// Chrome then tries both, and fails both.
  Failed { required: Option<usize> },
  /// Its header has arrived, and not yet all its data. Chrome counts it, and a decode() of it
  /// waits for the data.
  Pending,
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

    // Create async signaling primitives
    let complete = Arc::new(AtomicBool::new(false));
    let ready = Arc::new(AtomicBool::new(false));
    let ready_notify = Arc::new(Notify::new());

    let track_list_inner = Arc::new(Mutex::new(ImageTrackListInner {
      tracks: vec![ImageTrackData::initial(codec_id)],
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
      stream_bytes: 0,
      stream_ended: false,
      decode_posted_bytes: 0,
      stream_reads: 0,
      stream_closed: false,
      jpeg_unposted: 0,
      jpeg_drains: Vec::new(),
      jpeg_items: None,
      jpeg_requests: 0,
      frame_failed: None,
      end_deferred: false,
      tracks: tracks.clone(),
      closed: false,
      cached_frames: None,
      failed_frames: HashSet::new(),
      frames_failed: false,
      next_decode: 0,
      decode_turn: 0,
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
      let bytes = raw.to_vec();
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
          let _ = spawn_blocking(move || {
            cache_decoded_frames(&parse_inner, FrameCall::Probe, true, false)
          })
          .await;
          Ok(())
        },
        move |_, ()| {
          close_if_metadata_failed(&ready_inner);
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
      let mut reader = if codec_id == Some(AVCodecID::Mjpeg) {
        ChunkReader::Jpeg(JpegChunkReader::start(
          env,
          unsafe { init.data.cast::<Object>() }?,
          &inner,
        )?)
      } else {
        ChunkReader::Any(stream.read()?)
      };
      let inner_clone = inner.clone();
      if let Ok(mut inner_guard) = inner_clone.lock() {
        inner_guard.data = ImageDecoderData::Vec(Vec::new());
      }

      // The collector ends with whether the decoder failed before the data was complete: Chrome
      // then closes it and stops reading, and `completed` rejects (see `StreamEnd`).
      let data_complete = complete.clone();
      let collect = async move {
        let mut next_probe_size = 1usize;

        while let Some(item) = reader.next().await {
          let (chunk, arrival) = match item {
            Ok(StreamItem::Chunk(chunk, arrival)) => (
              chunk,
              arrival.filter(|&epoch| jpeg_try_wanted(&inner_clone, epoch)),
            ),
            // A JPEG decode() that waits tries the data so far, here, in the order of the
            // stream's results, as Chrome's decoder thread does (`FrameCall::Arrival`).
            Ok(StreamItem::Decode(epoch)) => {
              // Like Chrome's decoder thread, a decoder that failed fails the try at once
              // (ImageDecoderCore::Decode): the decode() that posted it applies the failure, and
              // its answer comes before what the stream sends later (the end, say).
              if take_frame_failure(&inner_clone, epoch) {
                signal_tracks_ready(&inner_clone);
                continue;
              }
              if jpeg_try_wanted(&inner_clone, epoch) {
                let decode_inner = inner_clone.clone();
                let tried = spawn_blocking(move || {
                  cache_decoded_frames(&decode_inner, FrameCall::Arrival(epoch), false, false)
                })
                .await;
                // A rejected metadata closes the decoder here, as for a probe.
                if close_if_metadata_failed(&inner_clone) {
                  return Ok(StreamEnd::Aborted(!data_complete.load(Ordering::Acquire)));
                }
                // A frame failure does not: Chrome applies a failed decode on the main thread,
                // in a task of its own, and a reset() made before that drops it
                // (ImageDecoderExternal::OnDecodeReady). The decode() that waits applies it on
                // the JS thread (`apply_frame_failure`). Either way the header is fine: the
                // tracks are ready, and the decode() wakes.
                if matches!(tried, Ok(Ok(()))) || frame_failed(&inner_clone) {
                  signal_tracks_ready(&inner_clone);
                }
              }
              continue;
            }
            // A decode() closed the decoder on a frame failure (`apply_frame_failure`): like
            // Chrome's close, the stream is no longer read, and `completed` rejects.
            Ok(StreamItem::Failed) => {
              if closed_on_failure(&inner_clone) {
                return Ok(StreamEnd::Aborted(!data_complete.load(Ordering::Acquire)));
              }
              continue;
            }
            Err(error) => {
              // The decoder closed on a failure before the error (a rejected metadata, or a
              // frame failure that a decode() applied): like Chrome, that close rejects
              // `completed`, and the stream is no longer read.
              if close_if_metadata_failed(&inner_clone) || closed_on_failure(&inner_clone) {
                return Ok(StreamEnd::Aborted(!data_complete.load(Ordering::Acquire)));
              }
              // A stream error is terminal. Like Chrome, decode the data that came, as at EOF,
              // and mark the input complete before waking decode waiters so they reject instead
              // of waiting for an update that can never arrive. A JPEG frame failure found
              // before the error that a decode() has not applied yet comes first: the JS thread
              // decides (`end_with_frame_failure`).
              let pending = inner_clone.lock().is_ok_and(|mut inner_guard| {
                inner_guard.all_data = true;
                frame_failure_pending(&inner_guard)
              });
              let final_inner = inner_clone.clone();
              let _ = spawn_blocking(move || {
                cache_decoded_frames(&final_inner, FrameCall::Probe, true, true)
              })
              .await;
              // A metadata that this refresh rejects closes the decoder, and `completed` rejects
              // with the error. So does a close on a failure that a decode() made meanwhile,
              // unless the failure was pending: the JS thread decides. A frame failure that the
              // refresh finds is only recorded (see `ImageDecoderInner::frame_failed`).
              if close_if_metadata_failed(&inner_clone)
                || (!pending && closed_on_failure(&inner_clone))
              {
                return Err(error);
              }
              if let Ok(mut inner_guard) = inner_clone.lock() {
                if pending {
                  inner_guard.end_deferred = true;
                } else {
                  inner_guard.complete.store(true, Ordering::Release);
                }
              }
              signal_tracks_ready(&inner_clone);
              if pending {
                return Ok(StreamEnd::FrameFailure(Some(error)));
              }
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

          // A chunk that came to an idle JPEG stream while a decode() waits is decoded now,
          // before the reader sees what came with it (the close, say), as Chrome does.
          let probe = buffered_len >= next_probe_size;
          if probe || arrival.is_some() {
            let call = arrival.map_or(FrameCall::Probe, FrameCall::Arrival);
            let probe_inner = inner_clone.clone();
            let tried =
              spawn_blocking(move || cache_decoded_frames(&probe_inner, call, false, false)).await;
            // A rejected metadata closes the decoder at once. A frame failure does not (see
            // `StreamItem::Decode`): a decode() that waits wakes and applies it.
            if close_if_metadata_failed(&inner_clone) {
              return Ok(StreamEnd::Aborted(!data_complete.load(Ordering::Acquire)));
            }
            if matches!(tried, Ok(Ok(()))) || frame_failed(&inner_clone) {
              signal_tracks_ready(&inner_clone);
            }
            if probe {
              next_probe_size = buffered_len.saturating_mul(2).max(next_probe_size * 2);
            }
          }
        }

        // Refresh once at EOF so animated images include frames that arrived
        // after an early successful probe. Like Chrome, the data is complete only
        // once it is decoded: a metadata rejected here still rejects `completed`
        // (not for an AVIF that lacks a box, see `cache_decoded_frames`).
        // A JPEG frame failure found before the end that a decode() has not applied yet comes
        // first, as Chrome's failed decode does: the JS thread decides (`end_with_frame_failure`).
        let pending = inner_clone.lock().is_ok_and(|mut inner_guard| {
          inner_guard.all_data = true;
          frame_failure_pending(&inner_guard)
        });
        let final_inner = inner_clone.clone();
        let _ =
          spawn_blocking(move || cache_decoded_frames(&final_inner, FrameCall::Probe, true, true))
            .await;
        if close_if_metadata_failed(&inner_clone) || closed_on_failure(&inner_clone) {
          return Ok(StreamEnd::Aborted(!data_complete.load(Ordering::Acquire)));
        }
        if let Ok(mut inner_guard) = inner_clone.lock() {
          if pending {
            inner_guard.end_deferred = true;
          } else {
            inner_guard.complete.store(true, Ordering::Release);
          }
        }
        signal_tracks_ready(&inner_clone);
        if pending {
          return Ok(StreamEnd::FrameFailure(None));
        }
        Ok(StreamEnd::Aborted(false))
      };
      let end_inner = inner.clone();
      let collection_promise = env.spawn_future_with_callback(collect, move |env, end| {
        let aborted = match end {
          StreamEnd::Aborted(aborted) => aborted,
          StreamEnd::FrameFailure(error) => match (end_with_frame_failure(&end_inner), error) {
            (false, Some(error)) => return Err(error),
            (aborted, _) => aborted,
          },
        };
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
  pub fn completed(&self, this: This) -> Result<PromiseRaw<'_, ()>> {
    // Get stored promise from this and chain a new promise for this access. Like Chrome, also
    // once the decoder is closed, and never report its rejection as unhandled.
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
    // after (see `closed_error`), and one made before a reset() rejects (see `abort_if_reset`).
    // Like Chrome, the calls are handled in call order: what one finds can depend on the frames
    // that the ones before it failed.
    let (ticket, closed, epoch, jpeg_request, post) = {
      let mut guard = inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      let jpeg_request =
        frame_index == 0 && !guard.closed && guard.codec_id == Some(AVCodecID::Mjpeg);
      // Like Chrome, a decode() made while the stream is open tries the data read so far: after
      // what the stream gave before the decode(), before what it gives after (see
      // `queue_jpeg_post`).
      let post = jpeg_request && guard.jpeg_items.is_some() && !guard.stream_ended;
      if jpeg_request {
        guard.jpeg_requests += 1;
      }
      if post {
        guard.jpeg_unposted += 1;
      }
      // Every call takes a ticket, also one made on a closed decoder (see `DecodeTurn`)
      let ticket = guard.next_decode;
      guard.next_decode += 1;
      (
        ticket,
        guard.closed,
        guard.cache_epoch,
        jpeg_request,
        post.then_some(guard.stream_reads),
      )
    };
    if let Some(reads) = post {
      let post = JpegPost {
        epoch,
        decode_reads: reads,
        reads,
        first: true,
      };
      queue_jpeg_post(env, &inner, post);
    }
    let request_inner = inner.clone();
    let settle_inner = inner.clone();

    // Keep the Rust error as the resolved future value so the completion
    // callback can construct the precise native JavaScript exception class.
    let turn_inner = inner.clone();
    let decode = async move {
      // Like Chrome, a call made on a closed decoder rejects with InvalidStateError. Chrome's
      // close() rejects the calls made before it at once; here they reject on their turns, so it
      // waits for them, and the promises settle in call order.
      if closed {
        wait_for_turn(&inner, ticket).await?;
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
        let turn = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?
          .decode_turn;
        if turn == ticket {
          // Only on its turn: every result, this error too, gives the turn to the next call.
          abort_if_reset(&inner, epoch)?;
          let decode_inner = inner.clone();
          let result =
            spawn_blocking(move || decode_cached_frame(&decode_inner, frame_index, epoch))
              .await
              .map_err(|join_error| {
                Error::new(
                  Status::GenericFailure,
                  format!("Decode task failed: {}", join_error),
                )
              })?;
          // A reset() that came while it ran: what it found is dropped.
          abort_if_reset(&inner, epoch)?;

          if let Some(result) = result? {
            return Ok(result);
          }
        }

        notified.await;
      }
    };
    // The decode() no longer waits (see `jpeg_requests`).
    let decode = async move {
      let result = decode.await;
      if jpeg_request
        && let Ok(mut guard) = request_inner.lock()
        && guard.cache_epoch == epoch
      {
        guard.jpeg_requests = guard.jpeg_requests.saturating_sub(1);
      }
      result
    };

    let turn = DecodeTurn(turn_inner);
    env.spawn_future_with_callback(async move { Ok(decode.await) }, move |env, result| {
      // A frame failure counts first (`apply_frame_failure`), then the next decode() call's
      // turn (see `DecodeTurn`)
      let result =
        result.map_err(|error| apply_frame_failure(&settle_inner, epoch, frame_index, error));
      drop(turn);
      match result {
        Ok(result) => Ok(result),
        Err(error) => Err(native_image_decode_error(env, error)?),
      }
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

    inner.reset();
    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut inner = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    // A metadata failure counts once `close_if_metadata_failed` has closed the decoder. A close()
    // that comes first wins, also when a worker thread already found the failure
    // (`reject_metadata`). A GIF track goes back to how it was built the same way: until
    // `tracks.ready` is signalled, what a worker thread may have filled in
    // (`cache_decoded_frames`) does not count. Only GIF: gif_decoder often finishes before the JS
    // task that made the decoder reaches close(), so without this the track after an early
    // close() changes from run to run. The other formats keep what the worker found, as before.
    if !inner.closed {
      // The same for a frame failure (`jpeg_frame`, every frame of a GIF or BMP), also one that a
      // reset() kept
      inner.frame_failed = None;
      inner.frames_failed = false;
      inner.tracks.failed.store(false, Ordering::Release);
      if inner.codec_id == Some(AVCodecID::Gif)
        && !inner.tracks.ready.load(Ordering::Acquire)
        && let Ok(mut tracks) = inner.tracks.inner.lock()
        && let Some(track) = tracks.tracks.get_mut(0)
      {
        *track = ImageTrackData::initial(inner.codec_id);
      }
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
  if let Some(message) = error.reason.strip_prefix("AbortError:") {
    return native_dom_exception_error(env, DOMExceptionName::AbortError, message.trim());
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
  epoch: u64,
) -> Result<Option<ImageDecodeResult>> {
  let (needs_cache, data_complete, jpeg) = {
    let guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    // A frame failure that left the decoder open: one that the collector found for this
    // decode(), or one that a reset() kept (`frame_failed`, and the frames that all failed
    // below). Like Chrome's decoder, which stays failed, the decode() fails, whatever the frame
    // index. It closes the decoder on the JS thread, where a reset() made first still keeps the
    // failure (`apply_frame_failure`). Once the JS thread has read the end of a JPEG stream, it
    // waits for the end's result (`complete`), as Chrome answers in the order it was asked,
    // unless the collector left that to the JS thread (`end_deferred`).
    if !guard.closed
      && (guard.frame_failed.is_some() || guard.frames_failed)
      && guard.cache_epoch == epoch
    {
      if guard.stream_ended && !guard.complete.load(Ordering::Acquire) && !guard.end_deferred {
        return Ok(None);
      }
      return Err(frame_failure(frame_index));
    }
    if guard.closed {
      return Err(closed_error(&guard, frame_index));
    }
    (
      guard.cached_frames.is_none(),
      guard.complete.load(Ordering::Acquire),
      guard.codec_id == Some(AVCodecID::Mjpeg),
    )
  };

  // Like Chrome, a JPEG has one frame: another index fails once the data is complete, before
  // any decode.
  if jpeg && frame_index > 0 {
    return if data_complete {
      Err(frame_index_error(frame_index, 1))
    } else {
      Ok(None)
    };
  }

  // A reset can clear the cache. For fully buffered input, reconstruct it now;
  // for a live stream, wait for the collector's next successful probe. A JPEG stream's data so
  // far is tried by the collector, in the stream's order (`FrameCall::Arrival`); here only once
  // the data is complete (`FrameCall::Decode`).
  if needs_cache && (data_complete || jpeg) {
    cache_decoded_frames(inner, FrameCall::Decode(epoch), data_complete, false)
      .map_err(|error| decode_failure(inner, epoch, frame_index, error))?;
  }

  let mut guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if guard.closed {
    return Err(closed_error(&guard, frame_index));
  }

  // A JPEG that was not decoded above because the data was not complete then waits for the
  // signal that comes with `complete` (`FrameCall::Decode`), even when the data is complete now.
  let data_complete = if jpeg {
    data_complete
  } else {
    guard.complete.load(Ordering::Acquire)
  };
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
    return Err(frame_index_error(frame_index, frames.len()));
  }

  // Clone the Arc to share the frame data (no pixel copy needed).
  let (frame_arc, format, duration) = match &frames[frame_index] {
    CachedFrame::Decoded {
      frame,
      format,
      duration,
    } => (frame.clone(), *format, *duration),
    CachedFrame::Pending if !data_complete => return Ok(None),
    CachedFrame::Pending => {
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "RangeError: Unexpected end of image. Request for frame index {frame_index} can't be satisfied."
        ),
      ));
    }
    CachedFrame::Failed { .. } => {
      // Chrome tries the frame, and the failed frames it is drawn on, and remembers them
      let mut failed = vec![frame_index];
      while let CachedFrame::Failed { required: Some(i) } = frames[failed[failed.len() - 1]] {
        failed.push(i);
      }
      let count = frames.len();
      guard.failed_frames.extend(failed);
      // Once every frame failed, Chrome fails the decoder and closes it. While more data can
      // come, it waits for the data before it rejects.
      if guard.failed_frames.len() == count {
        guard.frames_failed = true;
        // Not for a decode() that a reset() made stale (it built these frames after the reset()):
        // like Chrome, whose reset() drops the result of a decode in flight, it rejects with
        // AbortError (`abort_if_reset`), and the decoder stays failed: the next decode() closes it.
        if guard.cache_epoch == epoch {
          guard.close();
        }
        return Err(Error::new(
          Status::GenericFailure,
          format!("Failed to decode frame at index {frame_index}"),
        ));
      }
      if !data_complete {
        return Ok(None);
      }
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "RangeError: Unexpected end of image. Request for frame index {frame_index} can't be satisfied."
        ),
      ));
    }
  };
  let pts = frame_arc.read().pts();

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

/// The turn of a decode() call: dropping it gives the turn to the next call. decode() drops it on
/// the JS thread, in the callback that settles the call's promise, so the next call starts only
/// once this one settles, and the promises settle in call order, like Chrome's. Given away on the
/// worker thread instead, the next call could finish and settle first while that thread waits to
/// settle this one. It also drops when the callback never runs (the environment goes away), so
/// the calls after it do not wait forever.
struct DecodeTurn(Arc<Mutex<ImageDecoderInner>>);

impl Drop for DecodeTurn {
  fn drop(&mut self) {
    if let Ok(mut guard) = self.0.lock() {
      guard.decode_turn += 1;
      guard.tracks.ready_notify.notify_waiters();
    }
  }
}

/// Wait for the turn of the decode() call with this ticket (see `DecodeTurn`)
async fn wait_for_turn(inner: &Arc<Mutex<ImageDecoderInner>>, ticket: u64) -> Result<()> {
  let notify = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?
    .tracks
    .ready_notify
    .clone();
  loop {
    // Registered before the check, so a turn given in between still wakes it
    let mut notified = std::pin::pin!(notify.notified());
    notified.as_mut().enable();
    let turn = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?
      .decode_turn;
    if turn == ticket {
      return Ok(());
    }
    notified.await;
  }
}

/// Like Chrome, a reset() made after a decode() call rejects it with AbortError, in every format.
/// decode() checks it on the call's turn, so the calls made before the reset() reject in call
/// order, and each one gives the turn to the next: the calls made after it do not wait for them.
/// A close() also moves `cache_epoch`; the call then gets the error of a closed decoder
/// (`closed_error`).
fn abort_if_reset(inner: &Arc<Mutex<ImageDecoderInner>>, epoch: u64) -> Result<()> {
  let guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if !guard.closed && guard.cache_epoch != epoch {
    return Err(abort_error("Aborted by reset."));
  }
  Ok(())
}

/// The RangeError of a decode() of a frame that the image does not have.
fn frame_index_error(frame_index: usize, frame_count: usize) -> Error {
  Error::new(
    Status::InvalidArg,
    format!("RangeError: Frame index {frame_index} out of bounds (image has {frame_count} frames)"),
  )
}

/// The error of a decode() that finds the decoder closed. When the decoder closed because its
/// metadata was rejected, a frame failed (JPEG) or every frame failed (GIF, BMP), Chrome rejects
/// the decodes still pending with EncodingError.
fn closed_error(inner: &ImageDecoderInner, frame_index: usize) -> Error {
  if inner.failed() || inner.frames_failed {
    return frame_failure(frame_index);
  }
  invalid_state_error("ImageDecoder is closed")
}

/// The error of a decode() whose own work, made when `cache_epoch` was `epoch`, failed. A JPEG
/// frame failure leaves the decoder open: the decode() applies it on the JS thread
/// (`apply_frame_failure`). A rejected metadata closes the decoder here, unless a reset() came
/// while the work ran.
fn decode_failure(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  epoch: u64,
  frame_index: usize,
  error: Error,
) -> Error {
  let Ok(mut guard) = inner.lock() else {
    return error;
  };
  if !guard.closed {
    if guard.frame_failed.is_some() {
      return frame_failure(frame_index);
    }
    if guard.cache_epoch != epoch || !guard.tracks.failed.load(Ordering::Acquire) {
      return error;
    }
    guard.close();
  }
  if guard.failed() {
    closed_error(&guard, frame_index)
  } else {
    error
  }
}

/// The EncodingError of a decode() whose frame failed
fn frame_failure(frame_index: usize) -> Error {
  Error::new(
    Status::GenericFailure,
    format!("Failed to decode frame at index {frame_index}"),
  )
}

/// On the JS thread, as a decode() made when `cache_epoch` was `epoch` rejects with `error`: a
/// frame failure that left the decoder open (`decode_cached_frame`: a JPEG frame, or every frame
/// of a GIF or BMP) closes it now. Chrome applies a failed decode the same way, on the main
/// thread in a task of its own (ImageDecoderExternal::OnDecodeReady, SetFailed), so a reset()
/// made in the meantime, even in the same task as the decode(), drops it: the call rejects with
/// AbortError and the decoder stays failed for the next decode(). A close() made first wins
/// (`closed_error`).
fn apply_frame_failure(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  epoch: u64,
  frame_index: usize,
  error: Error,
) -> Error {
  if error.reason != frame_failure(frame_index).reason {
    return error;
  }
  let Ok(mut guard) = inner.lock() else {
    return error;
  };
  if !guard.closed {
    if guard.cache_epoch != epoch {
      return abort_error("Aborted by reset.");
    }
    if guard.frame_failed.is_none() && !guard.frames_failed {
      return error;
    }
    // A failure kept from before a reset(), while the JS thread has yet to settle the end of the
    // stream (`end_deferred`): this decode() was made after the end was read (a try it posted
    // would own the failure, see `take_frame_failure`). Chrome answers the end first
    // (OnMetadata), so the data is complete and `completed` resolves (see `StreamEnd`).
    if guard.end_deferred && !frame_failure_pending(&guard) {
      guard.complete.store(true, Ordering::Release);
    }
    guard.close();
    // The collector stops reading, like Chrome's close (`StreamItem::Failed`)
    guard.send_jpeg_item(JpegItem::Failed);
  }
  closed_error(&guard, frame_index)
}

/// For the try of the data so far that a decode() posted when `cache_epoch` was `epoch`
/// (`StreamItem::Decode`): when a frame failure is already recorded and no reset() came since
/// the decode(), the try fails at once and the failure is now this epoch's. Returns whether it
/// did.
fn take_frame_failure(inner: &Arc<Mutex<ImageDecoderInner>>, epoch: u64) -> bool {
  let Ok(mut guard) = inner.lock() else {
    return false;
  };
  if guard.closed || guard.cache_epoch != epoch || guard.frame_failed.is_none() {
    return false;
  }
  guard.frame_failed = Some(epoch);
  true
}

/// Whether a JPEG frame failure waits for a decode() of the current epoch to apply it: no reset()
/// came since the work that found it (see `ImageDecoderInner::frame_failed`).
fn frame_failure_pending(inner: &ImageDecoderInner) -> bool {
  !inner.closed && inner.frame_failed == Some(inner.cache_epoch)
}

/// Whether the decoder closed on a failure, not on a close()
fn closed_on_failure(inner: &Arc<Mutex<ImageDecoderInner>>) -> bool {
  inner
    .lock()
    .is_ok_and(|guard| guard.closed && guard.failed())
}

/// How the collector of a stream ends. `completed` settles on the JS thread.
enum StreamEnd {
  /// Whether `completed` rejects with AbortError: the decoder closed on a failure before the
  /// data was complete (Chrome then stops reading). Otherwise it resolves.
  Aborted(bool),
  /// The stream ended (or failed, with this error) while a JPEG frame failure found before the
  /// end waited for a decode() to apply it (`end_with_frame_failure`)
  FrameFailure(Option<Error>),
}

/// On the JS thread, for `StreamEnd::FrameFailure`. Chrome's failed decode comes before the end
/// (ImageDecoderExternal::OnDecodeReady, then OnMetadata). With no reset() since the decode()
/// that owns it, the decoder closes now (if that decode() has not closed it yet): the decode()
/// calls still pending reject (`closed_error`), and `completed` rejects. A reset() made before
/// drops it, as Chrome's reset() drops the result of a decode in flight: the data is complete,
/// `completed` settles as at any end, and the next decode() fails and closes the decoder.
/// Returns whether `completed` rejects with AbortError.
fn end_with_frame_failure(inner: &Arc<Mutex<ImageDecoderInner>>) -> bool {
  let Ok(mut guard) = inner.lock() else {
    return false;
  };
  if frame_failure_pending(&guard) {
    guard.close();
  }
  // Closed on the failure, before the end or, by a decode() made after it, once the data was
  // complete (`apply_frame_failure`)
  if guard.closed && guard.failed() {
    return !guard.complete.load(Ordering::Acquire);
  }
  guard.complete.store(true, Ordering::Release);
  drop(guard);
  signal_tracks_ready(inner);
  false
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

/// A JPEG decode() that waits tries the data so far (`StreamItem::Decode`), unless the stream
/// ended or no byte came since the last try. Called on the JS thread, which sends the try to the
/// collector after the results it has sent so far: the try sees exactly the data the JS thread
/// had read, also when the collector runs it late. Chrome posts its decodes to the decoder thread
/// in the same order (ImageDecoderExternal::MaybeSatisfyPendingDecodes).
fn post_jpeg_decode(guard: &mut ImageDecoderInner) {
  if guard.jpeg_requests == 0
    || guard.closed
    || guard.stream_ended
    || guard.stream_bytes <= guard.decode_posted_bytes
  {
    return;
  }
  if guard.send_jpeg_item(JpegItem::Decode(guard.cache_epoch)) {
    guard.decode_posted_bytes = guard.stream_bytes;
  }
}

/// A JPEG decode() on an open stream, whose try `queue_jpeg_post` posts
#[derive(Clone, Copy)]
struct JpegPost {
  /// `cache_epoch` at the decode()
  epoch: u64,
  /// `stream_reads` at the decode()
  decode_reads: u64,
  /// `stream_reads` at the last step
  reads: u64,
  /// Whether the next step is the first
  first: bool,
}

/// On the JS thread: post the try of a JPEG decode() made on an open stream (`post_jpeg_decode`)
/// from a microtask, once the JS thread has handled what the stream gave before the decode().
///
/// Chrome reads a stream inside controller.enqueue(), close() and error() when a read waits
/// (ReadableStreamBytesConsumer), so its decode() sees what came earlier in its task and nothing
/// that comes later. Here a read settles one microtask later, and the reaction of a read that
/// settled before the decode() is queued before the first step:
/// - The stream closed or failed before the decode() (`stream_closed`): no try. Like Chrome, the
///   decode() waits for the end and decodes all the data.
/// - No read was handled since the decode(): the try now. What comes later in the task comes
///   after it.
/// - Else a chunk came before the decode(). The next read, made in its reaction, gets at once what
///   the stream has queued, and its reaction comes before the next step. So each step waits one
///   more microtask while a read was handled since the last one, and posts when none was. When
///   the read made in that first reaction is the end, the end came after the decode() (else the
///   first step would have seen it): the try is posted before the end (`post_before_end`).
///   Promises cannot tell a second chunk (or a close behind it) that came after the decode() from
///   one that came before: both are read before the try.
fn queue_jpeg_post(env: &Env, inner: &Arc<Mutex<ImageDecoderInner>>, post: JpegPost) {
  let step_inner = inner.clone();
  let queued = env
    .create_function_from_closure::<(), (), _>("postJpegDecode", move |ctx| {
      jpeg_post_step(ctx.env, &step_inner, post);
      Ok(())
    })
    .and_then(|step| {
      let queue: Function<Function<(), ()>, Unknown> =
        env.get_global()?.get_named_property("queueMicrotask")?;
      queue.call(step).map(|_| ())
    });
  if queued.is_err() {
    // No microtask: post now
    clear_pending_exception(env);
    if let Ok(mut guard) = inner.lock() {
      finish_jpeg_post(&mut guard, post);
    }
  }
}

/// One step of `queue_jpeg_post`. A reset() or a close() since the decode() drops the try, as
/// Chrome's reset() aborts a decode it has not run yet.
fn jpeg_post_step(env: &Env, inner: &Arc<Mutex<ImageDecoderInner>>, post: JpegPost) {
  let Ok(mut guard) = inner.lock() else {
    return;
  };
  if guard.closed || guard.cache_epoch != post.epoch {
    return;
  }
  if post.first {
    if guard.stream_closed {
      guard.jpeg_unposted = guard.jpeg_unposted.saturating_sub(1);
      return;
    }
    if guard.stream_reads == post.decode_reads || guard.stream_ended {
      finish_jpeg_post(&mut guard, post);
      return;
    }
    guard.jpeg_drains.push(post.decode_reads);
  } else if guard.stream_reads == post.reads || guard.stream_ended {
    finish_jpeg_post(&mut guard, post);
    return;
  }
  let next = JpegPost {
    reads: guard.stream_reads,
    first: false,
    ..post
  };
  drop(guard);
  queue_jpeg_post(env, inner, next);
}

/// The last step of `queue_jpeg_post`: post the try, unless `post_before_end` did
fn finish_jpeg_post(guard: &mut ImageDecoderInner, post: JpegPost) {
  if guard.closed || guard.cache_epoch != post.epoch {
    return;
  }
  if !post.first {
    let Some(i) = guard
      .jpeg_drains
      .iter()
      .position(|&reads| reads == post.decode_reads)
    else {
      return;
    };
    guard.jpeg_drains.swap_remove(i);
  }
  guard.jpeg_unposted = guard.jpeg_unposted.saturating_sub(1);
  post_jpeg_decode(guard);
}

/// On the JS thread, as it reads the end (or an error) of a JPEG stream: the decode() calls whose
/// try waits for the reads after a chunk that came before them (`queue_jpeg_post`), when this is
/// the read made in that chunk's reaction. The end then came after the decode(): their try comes
/// before it, as in Chrome.
fn post_before_end(guard: &mut ImageDecoderInner) {
  let end_read = guard.stream_reads;
  let before = guard.jpeg_drains.len();
  guard
    .jpeg_drains
    .retain(|&decode_reads| decode_reads + 2 != end_read);
  let posted = (before - guard.jpeg_drains.len()) as u32;
  if posted > 0 && !guard.closed {
    guard.jpeg_unposted = guard.jpeg_unposted.saturating_sub(posted);
    post_jpeg_decode(guard);
  }
}

/// Whether the try of the data so far for a JPEG decode() that waits, made when `cache_epoch` was
/// `epoch`, still runs. Like Chrome, whose reset() sets the abort flag of the tries it posted
/// (image_decoder_core.cc Decode), not after a reset(), also when the collector takes it late.
fn jpeg_try_wanted(inner: &Arc<Mutex<ImageDecoderInner>>, epoch: u64) -> bool {
  inner.lock().is_ok_and(|guard| {
    guard.cache_epoch == epoch && guard.jpeg_requests > 0 && !guard.closed && !guard.all_data
  })
}

/// What the collector reads of a stream
enum StreamItem {
  /// A chunk, and, when a JPEG decode() that waits tries it at once (`FrameCall::Arrival`), the
  /// `cache_epoch` then (see `jpeg_try_wanted`)
  Chunk(Uint8ArrayCopy, Option<u64>),
  /// A JPEG decode() that waits tries the data so far (made when `cache_epoch` was this)
  Decode(u64),
  /// A JPEG decode() closed the decoder on a frame failure (`apply_frame_failure`)
  Failed,
}

/// The chunks of a stream (JPEG: with the decode tries, see `JpegItem`)
enum ChunkReader {
  Any(Reader<Uint8ArrayCopy>),
  Jpeg(JpegChunkReader),
}

impl ChunkReader {
  async fn next(&mut self) -> Option<Result<StreamItem>> {
    match self {
      Self::Any(reader) => reader
        .next()
        .await
        .map(|item| item.map(|chunk| StreamItem::Chunk(chunk, None))),
      Self::Jpeg(reader) => match reader.items.recv().await? {
        JpegItem::Chunk(chunk, arrival) => Some(Ok(StreamItem::Chunk(chunk, arrival))),
        JpegItem::Decode(epoch) => Some(Ok(StreamItem::Decode(epoch))),
        JpegItem::Failed => Some(Ok(StreamItem::Failed)),
        JpegItem::End => None,
        JpegItem::Error(error) => Some(Err(error)),
      },
    }
  }
}

/// What the JS thread sends the collector of a JPEG stream, in the order it saw it
enum JpegItem {
  /// A chunk, and, when it came to an idle stream while a decode() waits, the `cache_epoch` then:
  /// Chrome decodes it before it reads the next result (`FrameCall::Arrival`)
  Chunk(Uint8ArrayCopy, Option<u64>),
  /// A decode() that waits tries the data so far (see `post_jpeg_decode`)
  Decode(u64),
  /// A decode() closed the decoder on a frame failure (sent from the JS thread by
  /// `apply_frame_failure`)
  Failed,
  /// The end of the stream
  End,
  /// The stream failed
  Error(Error),
}

/// The collector's end of a JPEG stream's reader (see `JpegStream`). Dropping it lets go of
/// the stream's reader.
struct JpegChunkReader {
  items: UnboundedReceiver<JpegItem>,
  read: Arc<Mutex<Option<JpegReadFn>>>,
}

type JpegReadFn = FunctionRef<(), PromiseRaw<'static, Object<'static>>>;

impl Drop for JpegChunkReader {
  fn drop(&mut self) {
    // The collector is done: the JS thread reads no more (a FunctionRef may drop off-thread)
    if let Ok(mut read) = self.read.lock() {
      read.take();
    }
  }
}

impl JpegChunkReader {
  /// Start reading on the JS thread, at once, like Chrome's ReadableStreamBytesConsumer: the
  /// first read now, and the next one as soon as a chunk comes. So a read waits whenever the
  /// stream has nothing queued, whatever the collector is doing, and a chunk that comes then
  /// came to an idle stream.
  fn start(env: &Env, stream: Object, inner: &Arc<Mutex<ImageDecoderInner>>) -> Result<Self> {
    let get_reader: Function<(), Object> = stream.get_named_property("getReader")?;
    let reader = get_reader.apply(stream, ())?;
    // When the stream closes or fails, as the JS thread sees it (see `queue_jpeg_post`)
    watch_stream_close(env, reader, inner);
    let read: Function<(), PromiseRaw<Object>> = reader.get_named_property("read")?;
    let read: JpegReadFn = read.bind(reader)?.create_ref()?;
    let read = Arc::new(Mutex::new(Some(read)));
    let (sender, items) = unbounded_channel();
    if let Ok(mut guard) = inner.lock() {
      guard.jpeg_items = Some(sender.downgrade());
    }
    let js = Arc::new(JpegStream {
      read: read.clone(),
      items: sender,
      inner: Arc::downgrade(inner),
      idle_check: Mutex::new(None),
    });
    js.read_next(env);
    Ok(Self { items, read })
  }
}

/// The JS thread's end of a JPEG stream's reader
struct JpegStream {
  /// The reader's read(); None once the collector is done
  read: Arc<Mutex<Option<JpegReadFn>>>,
  /// The only strong sender (see `ImageDecoderInner::jpeg_items`)
  items: UnboundedSender<JpegItem>,
  inner: Weak<Mutex<ImageDecoderInner>>,
  /// The last read, while an idle check is scheduled (see `start_jpeg_read`)
  idle_check: Mutex<Option<Arc<JpegRead>>>,
}

const READ_WAITING: u8 = 0;
const READ_IDLE: u8 = 1;
const READ_SETTLED: u8 = 2;

/// One read() of a JPEG stream: READ_WAITING, READ_IDLE (still waiting after the task that made
/// it) or READ_SETTLED
struct JpegRead(AtomicU8);

impl JpegStream {
  /// On the JS thread: the next read()
  fn read_next(self: &Arc<Self>, env: &Env) {
    // Not under the lock: read() runs the stream's pull()
    let read = {
      let Ok(read) = self.read.lock() else {
        return;
      };
      match read.as_ref() {
        Some(read) => read.borrow_back(env),
        // the collector is done
        None => return,
      }
    };
    let promise = read.and_then(|read| read.call(()));
    start_jpeg_read(
      promise,
      env,
      self.clone(),
      Arc::new(JpegRead(AtomicU8::new(READ_WAITING))),
    );
  }

  /// The task that made the last read ended, and the read still waits: the stream is idle. Like
  /// Chrome, which tries again once its last try is back (ImageDecoderExternal::OnDecodeReady),
  /// a decode() that waits tries the data so far.
  fn check_idle(&self) {
    let Some(read) = self.idle_check.lock().ok().and_then(|mut read| read.take()) else {
      return;
    };
    if read
      .0
      .compare_exchange(READ_WAITING, READ_IDLE, Ordering::AcqRel, Ordering::Acquire)
      .is_err()
    {
      return;
    }
    if let Some(inner) = self.inner.upgrade()
      && let Ok(mut guard) = inner.lock()
    {
      post_jpeg_decode(&mut guard);
    }
  }

  /// The read's result (on the JS thread): a chunk, the end (None) or an error. Returns whether
  /// to read on.
  fn settle(&self, read: &JpegRead, result: Result<Option<Uint8ArrayCopy>>) -> bool {
    let idle = read.0.swap(READ_SETTLED, Ordering::AcqRel) == READ_IDLE;
    let mut arrival = None;
    if let Some(inner) = self.inner.upgrade()
      && let Ok(mut guard) = inner.lock()
    {
      guard.stream_reads += 1;
      match &result {
        Ok(Some(chunk)) => {
          guard.stream_bytes += chunk.len();
          // Chrome decodes a chunk that comes to a waiting read before it reads on
          // (ImageDecoderExternal::OnStateChange), for the decode() calls made before it
          // (`jpeg_unposted`)
          if idle && guard.jpeg_requests > guard.jpeg_unposted && !guard.closed {
            guard.decode_posted_bytes = guard.stream_bytes;
            arrival = Some(guard.cache_epoch);
          }
        }
        Ok(None) | Err(_) => {
          post_before_end(&mut guard);
          guard.stream_ended = true;
        }
      }
    }
    let more = matches!(result, Ok(Some(_)));
    let item = match result {
      Ok(Some(chunk)) => JpegItem::Chunk(chunk, arrival),
      Ok(None) => JpegItem::End,
      Err(error) => JpegItem::Error(error),
    };
    // A collector that is gone drops the chunk here, on the JS thread, and nothing reads on.
    self.items.send(item).is_ok() && more
  }
}

/// On the JS thread, with the promise of a read(): wait for its result (then read on at once),
/// and check once this task (with its microtasks) has ended whether the read still waits. Errors
/// are owned (no JS reference), as the collector may drop them on another thread. One then() with
/// both reactions: a rejected read is handled one microtask after it settles, like a chunk (see
/// `queue_jpeg_post`).
fn start_jpeg_read(
  promise: Result<PromiseRaw<Object>>,
  env: &Env,
  js: Arc<JpegStream>,
  read: Arc<JpegRead>,
) {
  let on_value = (js.clone(), read.clone());
  let on_error = (js.clone(), read.clone());
  let handlers = promise.and_then(|promise| {
    let on_value = env.create_function_from_closure::<(), (), _>("onRead", move |ctx| {
      let chunk = ctx.first_arg::<Object>().and_then(|result| {
        if result.get_named_property::<bool>("done")? {
          Ok(None)
        } else {
          // Copied here, on the JS thread, like the chunks of other formats.
          result
            .get_named_property::<Uint8ArrayCopy>("value")
            .map(Some)
        }
      });
      if chunk.is_err() {
        clear_pending_exception(ctx.env);
      }
      let (js, read) = &on_value;
      if js.settle(
        read,
        chunk.map_err(|error| Error::new(Status::GenericFailure, error.reason.clone())),
      ) {
        js.read_next(ctx.env);
      }
      Ok(())
    })?;
    let on_error = env.create_function_from_closure::<(), (), _>("onReadError", move |ctx| {
      let message = ctx.first_arg::<Unknown>().map_or_else(
        |_| "ReadableStream read error".to_owned(),
        rejection_message,
      );
      let (js, read) = &on_error;
      js.settle(read, Err(Error::new(Status::GenericFailure, message)));
      Ok(())
    })?;
    then_both(promise, on_value, on_error)
  });
  if let Err(error) = handlers {
    clear_pending_exception(env);
    let reason = if error.reason.is_empty() {
      format!("ReadableStream read failed: {}", error.status)
    } else {
      error.reason.clone()
    };
    js.settle(&read, Err(Error::new(Status::GenericFailure, reason)));
    return;
  }
  // setImmediate runs after this task and its microtasks: a chunk or the end already queued in
  // the stream has settled the read by then. One check at most is pending, for the last read:
  // a source that enqueues at once settles many reads in one task.
  let pending = js
    .idle_check
    .lock()
    .is_ok_and(|mut last| last.replace(read).is_some());
  if pending {
    return;
  }
  let check = js.clone();
  let scheduled = env
    .create_function_from_closure::<(), (), _>("checkStreamIdle", move |_| {
      check.check_idle();
      Ok(())
    })
    .and_then(|check| {
      let global = env.get_global()?;
      let schedule = global
        .get_named_property::<Function<Function<(), ()>, Unknown>>("setImmediate")
        .or_else(|_| global.get_named_property("setTimeout"))?;
      schedule.call(check).map(|_| ())
    });
  if scheduled.is_err() {
    clear_pending_exception(env);
    js.check_idle();
  }
}

/// `promise.then(on_value, on_error)`: either reaction runs one microtask after the promise
/// settles. Its own promise never rejects, as the reactions return nothing and do not throw.
fn then_both<'env>(
  promise: impl JsObjectValue<'env>,
  on_value: Function<(), ()>,
  on_error: Function<(), ()>,
) -> Result<()> {
  let then: ThenFn = promise.get_named_property("then")?;
  then.apply(promise, FnArgs::from((on_value, on_error)))?;
  Ok(())
}

type ThenFn<'env> =
  Function<'env, FnArgs<(Function<'env, (), ()>, Function<'env, (), ()>)>, Unknown<'env>>;

/// On the JS thread: set `stream_closed` when the reader's `closed` promise settles. It settles in
/// the call that closes or fails the stream, before the read that waits gets the end, so its
/// reaction comes before that of a decode() made after the close (see `queue_jpeg_post`).
fn watch_stream_close(env: &Env, reader: Object, inner: &Arc<Mutex<ImageDecoderInner>>) {
  let mark = |inner: Weak<Mutex<ImageDecoderInner>>| {
    move |_: FunctionCallContext| {
      if let Some(inner) = inner.upgrade()
        && let Ok(mut guard) = inner.lock()
      {
        guard.stream_closed = true;
      }
      Ok(())
    }
  };
  let watched = reader
    .get_named_property::<Object>("closed")
    .and_then(|closed| {
      let on_close = env
        .create_function_from_closure::<(), (), _>("onStreamClose", mark(Arc::downgrade(inner)))?;
      let on_error = env
        .create_function_from_closure::<(), (), _>("onStreamError", mark(Arc::downgrade(inner)))?;
      then_both(closed, on_close, on_error)
    });
  if watched.is_err() {
    // No `closed` promise: a decode() made after the close tries the data before the end
    clear_pending_exception(env);
  }
}

/// The message of a rejected read, like napi's `Reader`: the `message` property, else the
/// value as a string.
fn rejection_message(value: Unknown) -> String {
  fn message_property(value: Unknown) -> Result<String> {
    let message = value
      .coerce_to_object()?
      .get_named_property::<Unknown>("message")?;
    message.coerce_to_string()?.into_utf8()?.into_owned()
  }
  let message = if matches!(value.get_type(), Ok(ValueType::Object)) {
    message_property(value).ok()
  } else {
    None
  };
  let message = message.unwrap_or_else(|| {
    value
      .coerce_to_string()
      .and_then(|message| message.into_utf8())
      .and_then(|message| message.into_owned())
      .unwrap_or_else(|_| "ReadableStream read error".to_owned())
  });
  // A throwing getter or toString leaves an exception pending.
  clear_pending_exception(&Env::from_raw(value.value().env));
  message
}

fn clear_pending_exception(env: &Env) {
  let mut exception = std::ptr::null_mut();
  unsafe {
    napi::sys::napi_get_and_clear_last_exception(env.raw(), &mut exception);
  }
}

/// Who asks for the frames. Only the JPEG path tells them apart (see `jpeg_frame`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameCall {
  /// A probe for the metadata as the data arrives
  Probe,
  /// A decode() made when `cache_epoch` was this. It decodes a JPEG only with all the data, and
  /// waits for `complete`: Chrome resolves `completed` before it tries the frame. And not after a
  /// reset(), which aborts it (`abort_if_reset`). The data so far is tried by the collector
  /// (`Arrival`).
  Decode(u64),
  /// The collector, for a JPEG decode() that waits, in the order of the stream's results: Chrome
  /// posts its decodes to its decoder thread in that order. A chunk that came to an idle stream
  /// (Chrome decodes it before it reads the next result, ImageDecoderExternal::OnStateChange),
  /// the stream becoming idle (Chrome tries again once its last try is back, OnDecodeReady), or
  /// a decode() made while it is open (see `queue_jpeg_post`). Chrome decodes a frame in a task
  /// of its own, so results that come in one task (the last chunk and the close, say) reach it
  /// together: such a stream is never idle between them. Made when `cache_epoch` was this (see
  /// `jpeg_try_wanted`); a failure it finds belongs to that epoch.
  Arrival(u64),
}

/// What `jpeg_frame` needs of the decoder state, read with the data
#[derive(Clone, Copy)]
struct JpegState {
  all_data: bool,
  complete: bool,
  rgb_decode: bool,
  cache_epoch: u64,
}

/// Attempt to cache every frame currently present in the buffered bytes (`data_complete`: all
/// the bytes are there). Failures do not resolve `tracks.ready`, which lets streamed images retry
/// as more bytes arrive. A rejected metadata is left to `close_if_metadata_failed`, a failed frame
/// to the JS thread (see `ImageDecoderInner::frame_failed`).
/// `stream_ended`: the stream has ended, and this is its last refresh (an AVIF that still
/// lacks a box then fails, see `Avif::Incomplete`).
fn cache_decoded_frames(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  call: FrameCall,
  data_complete: bool,
  stream_ended: bool,
) -> Result<()> {
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
    jpeg_state,
    cache_epoch,
    complete,
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
      JpegState {
        all_data: guard.all_data,
        complete: guard.complete.load(Ordering::Acquire),
        rgb_decode: guard.rgb_decode,
        cache_epoch: guard.cache_epoch,
      },
      guard.cache_epoch,
      guard.complete.load(Ordering::Acquire),
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
    return match jpeg_frame(inner, &data_bytes, desired, jpeg_state, call)? {
      Some((frame, format)) => store_frames(
        inner,
        cache_epoch,
        &data_bytes,
        vec![CachedFrame::Decoded {
          frame: frame.into_shared(),
          format,
          duration: None,
        }],
        1,
        None,
        None,
      ),
      // the metadata, but no frame yet
      None => Ok(()),
    };
  }

  // Chrome decodes GIF with Wuffs and BMP with the image crate: gif_decoder and bmp_decoder
  // follow them. FFmpeg decodes the other formats.
  let gif_or_bmp = match codec_id {
    AVCodecID::Gif => {
      let (frames, repetition_count) =
        gif_frames(inner, &data_bytes, data_complete, prefer_animation)?;
      Some((frames, Some(repetition_count)))
    }
    AVCodecID::Bmp => Some((bmp_frames(inner, &data_bytes, data_complete)?, None)),
    _ => None,
  };
  if let Some((frames, repetition_count)) = gif_or_bmp {
    let frame_count = frames.len();
    return store_frames(
      inner,
      cache_epoch,
      &data_bytes,
      frames,
      frame_count,
      None,
      repetition_count,
    );
  }

  // Chrome reads an AVIF with CrabbyAvif, and so does `avif`. FFmpeg's mov demuxer decodes
  // what it declines (a grid, ...) as before.
  let avif = match codec_id {
    AVCodecID::Av1 => avif::parse(&data_bytes, prefer_animation),
    _ => Avif::Decline,
  };
  // An AVIF's durations come from its track (see `decode_avif`)
  let track_timing = matches!(avif, Avif::Image(_));
  let (context, frames, frame_count) = match avif {
    Avif::Image(image) => {
      let len = data_bytes.len();
      let size_hint = if complete {
        len as u64
      } else {
        avif::STREAM_SIZE_HINT
      };
      if std::iter::once(&image.color)
        .chain(&image.alpha)
        .any(|samples| samples.past_size_hint(size_hint))
      {
        return Err(reject_metadata(inner, false));
      }
      let (context, frames) = decode_avif(&data_bytes, &image).inspect_err(|_| {
        // The samples that have arrived do not decode: the frames cached from fewer bytes
        // are stale, and decode() then fails like Chrome's.
        if let Ok(mut guard) = inner.lock()
          && guard.cache_epoch == cache_epoch
          && len >= guard.cached_data_len
        {
          guard.cached_frames = None;
        }
      })?;
      // an item that ends past the data (see `AvifSamples::past_size_hint`)
      if complete && frames.len() < image.color.len() {
        return Err(Error::new(
          Status::GenericFailure,
          "Failed to decode frame at index 0",
        ));
      }
      (context, frames, image.color.len())
    }
    // Chrome waits for the boxes the brands need, and fails the metadata when the data ends
    // before them. At the end of a stream it marks the data complete first: `completed`
    // resolves.
    Avif::Incomplete if complete || stream_ended => {
      return Err(reject_metadata(inner, true));
    }
    Avif::Incomplete => {
      return Err(Error::new(
        Status::GenericFailure,
        "The AVIF data is incomplete",
      ));
    }
    _ => {
      let (context, frames) = decode_frames(codec_id, &data_bytes, prefer_animation)?;
      let frame_count = frames.len();
      (context, frames, frame_count)
    }
  };

  // Chrome gives every frame of an animation its duration. FFmpeg's 0 is no duration; a
  // track's 0 is one.
  let frames = frames
    .into_iter()
    .map(|(f, format)| {
      let duration = (frame_count > 1 && (f.duration() > 0 || track_timing)).then(|| f.duration());
      CachedFrame::Decoded {
        frame: f.into_shared(),
        format,
        duration,
      }
    })
    .collect();
  store_frames(
    inner,
    cache_epoch,
    &data_bytes,
    frames,
    frame_count,
    Some(context),
    None,
  )
}

/// Cache the frames of `data` (see `CachedFrame`), unless reset()/close() or a newer probe came
/// first. `frame_count` is the track's frame count: an AVIF track also counts the frames whose
/// samples have not arrived yet.
fn store_frames(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  cache_epoch: u64,
  data: &[u8],
  frames: Vec<CachedFrame>,
  frame_count: usize,
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
    if frame_count > 0 {
      track.frame_count = frame_count as u32;
    }
    if prefer_animation == Some(false) {
      track.animated = false;
      track.repetition_count = 0.0;
    } else if let Some(repetition_count) = repetition_count {
      track.repetition_count = repetition_count;
    }
  }

  guard.cached_frames = Some(frames);
  guard.context = context;
  guard.cached_data_len = data.len();

  Ok(())
}

/// Decode an image with FFmpeg and give every frame a WebCodecs format, from what the file
/// header says.
fn decode_frames(
  codec_id: AVCodecID,
  data_bytes: &[u8],
  prefer_animation: Option<bool>,
) -> Result<(CodecContext, Vec<(Frame, VideoPixelFormat)>)> {
  // Demux and decode all frames.
  let (context, mut frames) = decode_image_data(codec_id, data_bytes)?;
  let still = frames.len() == 1;

  // Apply preferAnimation: if false and format supports animation, only keep first frame
  if prefer_animation == Some(false) && !frames.is_empty() {
    frames.truncate(1);
  }

  // Give every frame a WebCodecs format that describes its bytes, converting the
  // FFmpeg formats WebCodecs has no name for (rgb24, pal8, gray, argb, ...).
  let header = match codec_id {
    AVCodecID::Png => png_header(data_bytes),
    _ => None,
  };
  let alpha = match codec_id {
    AVCodecID::Webp => webp_alpha_hint(data_bytes),
    _ => header.map(|header| header.alpha),
  };
  let yuv = match codec_id {
    // an AVIF that `avif::parse` declines: a single frame is a still image
    AVCodecID::Av1 => Some(still),
    AVCodecID::Webp => Some(webp_is_simple_lossy(data_bytes)),
    _ => None,
  };
  let source = DecodedSource::Image {
    codec: codec_id,
    alpha,
    yuv,
    pixels: header.map_or(PixelHint::None, |header| header.pixels),
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
  Ok((context, frames))
}

/// The frames of a GIF, drawn like Chrome draws them, and its repetition count. While more data
/// can come, the frames from the first one whose data ends early are pending. Like Chrome, each
/// frame lasts its Graphic Control delay, and starts when the frames before it end.
fn gif_frames(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  data: &[u8],
  data_complete: bool,
  prefer_animation: Option<bool>,
) -> Result<(Vec<CachedFrame>, f64)> {
  let gif = match GifDecoder::new(data) {
    Ok(gif) => gif,
    Err(gif_decoder::TRUNCATED) if !data_complete => return Err(incomplete_data()),
    Err(error) => return Err(reject_metadata(inner, error == gif_decoder::TRUNCATED)),
  };
  // preferAnimation false keeps the first frame only
  let count = if prefer_animation == Some(false) {
    1
  } else {
    gif.frames().len()
  };
  // a canvas too large for a frame only has its data checked (see `frame_fits`)
  let fits = frame_fits(gif.width(), gif.height());
  let decoded = if fits { gif.decode() } else { gif.check() };
  let mut frames = Vec::new();
  let mut timestamp = 0;
  for (info, pixels) in gif.frames().iter().zip(decoded).take(count) {
    let format = if info.has_alpha {
      VideoPixelFormat::BGRA
    } else {
      VideoPixelFormat::BGRX
    };
    let frame = match pixels {
      Err(gif_decoder::TRUNCATED) if !data_complete => {
        frames.resize(count, CachedFrame::Pending);
        break;
      }
      Ok(pixels) if fits => bgra_frame(gif.width(), gif.height(), &pixels, timestamp),
      _ => None,
    };
    let duration = i64::from(info.delay) * 10_000;
    frames.push(match frame {
      Some(frame) => CachedFrame::Decoded {
        frame: frame.into_shared(),
        format,
        duration: Some(duration),
      },
      None => CachedFrame::Failed {
        required: info
          .required
          .filter(|&i| matches!(frames[i], CachedFrame::Failed { .. })),
      },
    });
    timestamp += duration;
  }
  if frames.is_empty() && !data_complete {
    return Err(incomplete_data());
  }
  Ok((frames, gif.repetition_count()))
}

/// The frame of a BMP, decoded like Chrome decodes it. While more data can come, it is pending
/// while its data ends early.
fn bmp_frames(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  data: &[u8],
  data_complete: bool,
) -> Result<Vec<CachedFrame>> {
  let bmp = match BmpDecoder::new(data) {
    Ok(bmp) => bmp,
    Err(bmp_decoder::TRUNCATED) if !data_complete => return Err(incomplete_data()),
    Err(error) => return Err(reject_metadata(inner, error == bmp_decoder::TRUNCATED)),
  };
  let format = if bmp.has_alpha() {
    VideoPixelFormat::BGRA
  } else {
    VideoPixelFormat::BGRX
  };
  // an image too large for a frame only has its data checked (see `frame_fits`)
  let fits = frame_fits(bmp.width(), bmp.height());
  let pixels = if fits {
    bmp.decode()
  } else {
    bmp.check().map(|()| Vec::new())
  };
  let frame = match pixels {
    Err(bmp_decoder::TRUNCATED) if !data_complete => return Ok(vec![CachedFrame::Pending]),
    Ok(pixels) if fits => bgra_frame(bmp.width(), bmp.height(), &pixels, 0),
    _ => None,
  };
  Ok(vec![match frame {
    Some(frame) => CachedFrame::Decoded {
      frame: frame.into_shared(),
      format,
      duration: None,
    },
    None => CachedFrame::Failed { required: None },
  }])
}

fn incomplete_data() -> Error {
  Error::new(Status::GenericFailure, "Incomplete image data")
}

/// Whether `bgra_frame` can make a frame of this size. av_frame_get_buffer fails a frame that
/// av_image_check_size rejects ((8 * width + 1024) * (height + 128) must stay under 2^31 - 1), so
/// the decode of a larger GIF or BMP fails whatever its data holds. Their decoders then only
/// check the data, for the same outcome: decoding it first made up to 2 GiB of canvas from a file
/// of a few bytes.
fn frame_fits(width: u32, height: u32) -> bool {
  // SAFETY: av_image_check_size reads only its arguments; a null log context is allowed.
  unsafe { av_image_check_size(width, height, 0, std::ptr::null_mut()) >= 0 }
}

/// A BGRA frame with the rows of `pixels`, tagged RGB matrix and full range like the images
/// FFmpeg decodes to RGB. None without memory for it.
fn bgra_frame(width: u32, height: u32, pixels: &[u8], pts: i64) -> Option<Frame> {
  let mut frame = Frame::new_video(width, height, AVPixelFormat::Bgra).ok()?;
  let (row, stride) = (width as usize * 4, frame.linesize(0) as usize);
  let rows = frame.plane_data_mut(0)?.chunks_mut(stride);
  for (to, from) in rows.zip(pixels.chunks_exact(row)) {
    to[..row].copy_from_slice(from);
  }
  frame.set_pts(pts);
  frame.set_colorspace(AVColorSpace::Rgb);
  frame.set_color_range(AVColorRange::Jpeg);
  Some(frame)
}

/// Decode an AVIF like Chrome (see `avif`): the colour samples with one AV1 decoder and the alpha
/// samples with another, colour frame i with alpha frame i. While the data streams in, only the
/// frames whose samples have all arrived. Timestamps and durations come from the track.
fn decode_avif(
  data: &[u8],
  image: &AvifImage,
) -> Result<(CodecContext, Vec<(Frame, VideoPixelFormat)>)> {
  let arrived = |samples: &AvifSamples, i| samples.arrived(data.len(), i);
  let count = (0..image.color.len())
    .take_while(|&i| arrived(&image.color, i) && image.alpha.iter().all(|a| arrived(a, i)))
    .count();
  let (context, mut frames) = decode_av1_samples(data, &image.color, count)?;
  let alpha = match &image.alpha {
    Some(alpha) => decode_av1_samples(data, alpha, count)?.1,
    None => Vec::new(),
  };
  if frames.len() != count || (image.alpha.is_some() && alpha.len() != count) {
    return Err(Error::new(
      Status::GenericFailure,
      "Decode failed: an AV1 sample gave no frame",
    ));
  }
  // Chrome's timing: CrabbyAvif's image_timing gives ticks / timescale seconds as an f64, or 0
  // when the timescale is 0 (an item's), and base::Seconds truncates it to microseconds, with
  // the f64 product (4099999 for 41 ticks at a timescale of 10). `as` truncates and saturates
  // like its saturated_cast.
  let micros = |ticks: u64| match image.timescale {
    0 => 0,
    timescale => (ticks as f64 / f64::from(timescale) * 1_000_000.0) as i64,
  };
  let mut ticks = 0;
  for (i, frame) in frames.iter_mut().enumerate() {
    let duration = u64::from(image.color.duration(i));
    frame.set_pts(micros(ticks));
    frame.set_duration(micros(duration));
    ticks += duration;
  }
  let mut alpha = alpha.into_iter();
  let frames = frames
    .into_iter()
    .enumerate()
    .map(|(i, frame)| {
      avif_frame(frame, alpha.next().as_ref(), image).map_err(|_| {
        Error::new(
          Status::GenericFailure,
          format!("Failed to decode frame at index {i}"),
        )
      })
    })
    .collect::<Result<_>>()?;
  Ok((context, frames))
}

/// Decode the first `count` samples of an AVIF image with an AV1 decoder of their own
fn decode_av1_samples(
  data: &[u8],
  samples: &AvifSamples,
  count: usize,
) -> Result<(CodecContext, Vec<Frame>)> {
  let failed = |e: CodecError| Error::new(Status::GenericFailure, format!("Decode failed: {e}"));
  let mut context = create_image_decoder(AVCodecID::Av1, Some(samples.config.clone()))?;
  let mut frames = Vec::new();
  for parts in (0..count).filter_map(|i| samples.sample(data, i)) {
    let mut packet = Packet::new().map_err(failed)?;
    packet.copy_parts_from(&parts).map_err(failed)?;
    frames.extend(context.decode(Some(&packet)).map_err(failed)?);
  }
  frames.extend(context.flush_decoder().map_err(failed)?);
  Ok((context, frames))
}

/// Chrome's frame for a JPEG: Blink's JPEGImageDecoder on the libjpeg-turbo Chrome ships (see
/// `codec::jpeg`), at M/8 of its size for desiredWidth/desiredHeight (`jpeg_scale_numerator`).
/// - The header is the metadata. Chrome rejects it when it is bad, when the data ends before it
///   does, or when M is 0.
/// - Like Chrome, the frame is decoded only for a decode() (see `FrameCall`), or, once Chrome
///   decodes to RGB, as the data arrives. A decode of part of the data that gets to output rows
///   makes Chrome decode to RGB from then on (`rgb_decode`): the frame is YUV planes only when
///   no decode got that far before all the data was here, else BGRX.
/// - None: no frame (yet). The data so far ends before the image does, or nothing decoded it.
fn jpeg_frame(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  data: &[u8],
  desired: Option<(u32, u32)>,
  state: JpegState,
  call: FrameCall,
) -> Result<Option<(Frame, VideoPixelFormat)>> {
  let all_data = state.all_data;
  let allow_yuv = all_data && !state.rgb_decode;
  let info = match jpeg_info(data, 8, allow_yuv) {
    Ok(Some(info)) => info,
    Ok(None) if !all_data => {
      return Err(Error::new(
        Status::GenericFailure,
        "The JPEG header has not arrived yet",
      ));
    }
    // A header cut short by the end of the data fails only once the data is complete.
    Ok(None) => return Err(reject_metadata(inner, true)),
    Err(_) => return Err(reject_metadata(inner, false)),
  };
  let scale = desired.map_or(8, |desired| {
    jpeg_scale_numerator(info.width, info.height, desired)
  });
  // Chrome rejects the metadata and closes the decoder.
  if scale == 0 {
    return Err(reject_metadata(inner, false));
  }
  let decode = match call {
    FrameCall::Probe => state.rgb_decode,
    // Not after a reset(): Chrome checks the abort flag before it decodes
    // (image_decoder_core.cc Decode).
    FrameCall::Decode(epoch) => all_data && state.complete && epoch == state.cache_epoch,
    FrameCall::Arrival(_) => true,
  };
  if !decode {
    return Ok(None);
  }
  let attempt = call != FrameCall::Probe;
  let result = decode_jpeg(data, scale, allow_yuv);
  after_jpeg_decode();
  if attempt
    && !all_data
    && matches!(
      result,
      Ok(JpegDecode::Frame(_) | JpegDecode::Incomplete { rows_started: true })
    )
    && let Ok(mut guard) = inner.lock()
  {
    guard.rgb_decode = true;
  }
  match result {
    Ok(JpegDecode::Frame(mut frame)) => {
      frame.set_pts(0);
      let format = match frame.format() {
        // alpha 255
        AVPixelFormat::Bgra => VideoPixelFormat::BGRX,
        yuv => VideoPixelFormat::from_av_format(yuv).unwrap_or(VideoPixelFormat::I420),
      };
      Ok(Some((frame, format)))
    }
    // Part of the data. Chrome resolves `completed` before it tries the frame with all the data.
    Ok(JpegDecode::Incomplete { .. })
      if !all_data
        || !inner
          .lock()
          .is_ok_and(|guard| guard.complete.load(Ordering::Acquire)) =>
    {
      Ok(None)
    }
    // A frame Chrome fails to decode fails the decoder (see `frame_failed`) when Chrome tries
    // it: an error as soon as it decodes it, data that ends before the image does at a decode()
    // once the data is complete. Only recorded here, with the epoch this work was made for: the
    // JS thread applies it.
    result => {
      if let Ok(mut guard) = inner.lock() {
        // A close() that came first wins (`ImageDecoder::close`), as for a frame that decodes
        // (`store_frames`).
        if guard.closed {
          return Err(invalid_state_error("ImageDecoder is closed"));
        }
        if (attempt || result.is_err()) && guard.frame_failed.is_none() {
          guard.frame_failed = Some(match call {
            FrameCall::Decode(epoch) | FrameCall::Arrival(epoch) => epoch,
            FrameCall::Probe => state.cache_epoch,
          });
        }
      }
      Err(frame_failure(0))
    }
  }
}

/// Reject the metadata. The caller then closes the decoder with `close_if_metadata_failed`.
/// `at_end`: the data ends before the header does. Chrome finds that only once all the data is
/// in, so the data is complete and `completed` resolves.
fn reject_metadata(inner: &Arc<Mutex<ImageDecoderInner>>, at_end: bool) -> Error {
  if let Ok(guard) = inner.lock()
    && !guard.closed
  {
    guard.tracks.failed.store(true, Ordering::Release);
    if at_end {
      guard.complete.store(true, Ordering::Release);
    }
  }
  invalid_state_error("Failed to retrieve track metadata.")
}

/// Close the decoder if its metadata was rejected (`tracks.ready` rejects): the decode() calls
/// still pending reject (see `closed_error`). Like Chrome (ImageDecoderExternal::OnMetadata),
/// whatever reset() came since. Returns whether it was rejected.
fn close_if_metadata_failed(inner: &Arc<Mutex<ImageDecoderInner>>) -> bool {
  let Ok(mut guard) = inner.lock() else {
    return false;
  };
  let failed = guard.tracks.failed.load(Ordering::Acquire);
  if failed && !guard.closed {
    guard.close();
  }
  failed
}

/// Whether a frame failed (see `ImageDecoderInner::frame_failed`)
fn frame_failed(inner: &Arc<Mutex<ImageDecoderInner>>) -> bool {
  inner.lock().is_ok_and(|guard| guard.frame_failed.is_some())
}

/// Runs once `jpeg_frame` has decoded, before it records what it found: the tests make a reset()
/// or a close() land there, while the work runs.
#[cfg(test)]
fn after_jpeg_decode() {
  tests::AFTER_JPEG_DECODE.with(|hook| {
    if let Some(hook) = hook.borrow_mut().as_mut() {
      hook();
    }
  });
}

#[cfg(not(test))]
fn after_jpeg_decode() {}

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
fn decode_image_data(
  codec_id: AVCodecID,
  data: &[u8],
) -> Result<(CodecContext, Vec<crate::codec::Frame>)> {
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
      return Ok((context, frames));
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

  Ok((context, all_frames))
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
    jpeg_scale_numerator, normalize_frame_timing, webp_alpha_hint, webp_is_simple_lossy,
  };
  use crate::codec::Frame;
  use crate::ffi::types::AV_NOPTS_VALUE;

  use super::{
    AVCodecID, ColorSpaceConversion, FrameCall, ImageDecoder, ImageDecoderData, ImageDecoderInner,
    ImageTrackData, ImageTrackList, ImageTrackListInner, JpegPost, abort_if_reset,
    apply_frame_failure, cache_decoded_frames, decode_cached_frame, end_with_frame_failure,
    finish_jpeg_post, frame_failure, post_before_end, take_frame_failure,
  };
  use napi::tokio::sync::Notify;
  use std::cell::{Cell, RefCell};
  use std::collections::HashSet;
  use std::rc::Rc;
  use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
  };

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

  thread_local! {
    /// See `after_jpeg_decode`
    pub(super) static AFTER_JPEG_DECODE: RefCell<Option<Box<dyn FnMut()>>> =
      const { RefCell::new(None) };
  }

  /// A JPEG whose data ends inside its scan: Chrome fails its frame at a decode()
  const TRUNCATED_JPEG: &[u8] = include_bytes!("../../__test__/fixtures/jpeg/truncated.jpg");

  /// The state of an ImageDecoder made from a BufferSource with `data`, once its tracks are ready
  fn jpeg_decoder(data: &[u8]) -> Arc<Mutex<ImageDecoderInner>> {
    decoder(data, AVCodecID::Mjpeg, "image/jpeg")
  }

  /// `jpeg_decoder` for another format
  fn decoder(data: &[u8], codec_id: AVCodecID, mime_type: &str) -> Arc<Mutex<ImageDecoderInner>> {
    let tracks = ImageTrackList {
      inner: Arc::new(Mutex::new(ImageTrackListInner {
        tracks: vec![ImageTrackData {
          animated: false,
          frame_count: 1,
          repetition_count: 0.0,
        }],
        selected_index: Some(0),
      })),
      ready: Arc::new(AtomicBool::new(true)),
      failed: Arc::new(AtomicBool::new(false)),
      ready_notify: Arc::new(Notify::new()),
    };
    Arc::new(Mutex::new(ImageDecoderInner {
      data: ImageDecoderData::Vec(data.to_vec()),
      mime_type: mime_type.to_string(),
      codec_id: Some(codec_id),
      context: None,
      complete: Arc::new(AtomicBool::new(true)),
      all_data: true,
      rgb_decode: false,
      stream_bytes: 0,
      stream_ended: false,
      decode_posted_bytes: 0,
      stream_reads: 0,
      stream_closed: false,
      jpeg_unposted: 0,
      jpeg_drains: Vec::new(),
      jpeg_items: None,
      jpeg_requests: 0,
      frame_failed: None,
      end_deferred: false,
      tracks,
      closed: false,
      cached_frames: None,
      failed_frames: HashSet::new(),
      frames_failed: false,
      next_decode: 0,
      decode_turn: 0,
      cache_epoch: 0,
      cached_data_len: 0,
      color_space_conversion: ColorSpaceConversion::Default,
      desired_width: None,
      desired_height: None,
      prefer_animation: None,
    }))
  }

  /// The error reason of a result, or "ok"
  fn reason<T>(result: napi::Result<T>) -> String {
    match result {
      Ok(_) => "ok".to_string(),
      Err(error) => error.reason,
    }
  }

  /// `decode_cached_frame` for a decode() made at `epoch`, with `hook` run where libjpeg has
  /// decoded (`after_jpeg_decode`). Returns the error reason, and whether libjpeg ran.
  fn decode_with_hook(
    inner: &Arc<Mutex<ImageDecoderInner>>,
    frame_index: usize,
    epoch: u64,
    mut hook: impl FnMut() + 'static,
  ) -> (String, bool) {
    let ran = Rc::new(Cell::new(false));
    let hook_ran = ran.clone();
    AFTER_JPEG_DECODE.with(|slot| {
      *slot.borrow_mut() = Some(Box::new(move || {
        hook_ran.set(true);
        hook();
      }))
    });
    let result = decode_cached_frame(inner, frame_index, epoch);
    AFTER_JPEG_DECODE.with(|slot| *slot.borrow_mut() = None);
    (reason(result), ran.get())
  }

  /// What `ImageDecoder::reset` does to an open decoder (the method needs an Env)
  fn reset(inner: &Arc<Mutex<ImageDecoderInner>>) {
    let mut guard = inner.lock().unwrap();
    assert!(!guard.closed);
    guard.reset();
  }

  fn close(inner: &Arc<Mutex<ImageDecoderInner>>) {
    ImageDecoder {
      inner: inner.clone(),
    }
    .close()
    .unwrap();
  }

  #[test]
  fn a_jpeg_frame_failure_closes_the_decoder_on_the_js_thread() {
    let inner = jpeg_decoder(TRUNCATED_JPEG);
    let (error, ran) = decode_with_hook(&inner, 0, 0, || {});
    assert!(ran);
    assert_eq!(error, "Failed to decode frame at index 0");
    // The work only records the failure, with its epoch
    {
      let guard = inner.lock().unwrap();
      assert!(!guard.closed);
      assert_eq!(guard.frame_failed, Some(0));
    }
    // The decode() closes the decoder on the JS thread, as it settles
    assert_eq!(
      apply_frame_failure(&inner, 0, 0, frame_failure(0)).reason,
      "Failed to decode frame at index 0"
    );
    assert!(inner.lock().unwrap().closed);
  }

  #[test]
  fn a_reset_while_the_jpeg_work_runs_keeps_the_failure_for_the_next_decode() {
    let inner = jpeg_decoder(TRUNCATED_JPEG);
    let hook_inner = inner.clone();
    let (error, ran) = decode_with_hook(&inner, 0, 0, move || reset(&hook_inner));
    assert!(ran);
    assert_eq!(error, "Failed to decode frame at index 0");
    {
      let guard = inner.lock().unwrap();
      // Like Chrome, the reset() dropped what the work found: the decoder is still open
      assert!(!guard.closed);
      // and it stays failed, like Chrome's decoder: the failure of the work made at epoch 0
      assert_eq!(guard.frame_failed, Some(0));
    }
    assert_eq!(
      reason(abort_if_reset(&inner, 0)),
      "AbortError: Aborted by reset."
    );
    // Like Chrome, the next decode() fails, whatever the frame index, and closes the decoder on
    // the JS thread
    let (error, ran) = decode_with_hook(&inner, 1, 1, || {});
    assert!(!ran);
    assert_eq!(error, "Failed to decode frame at index 1");
    assert!(!inner.lock().unwrap().closed);
    assert_eq!(
      apply_frame_failure(&inner, 1, 1, frame_failure(1)).reason,
      "Failed to decode frame at index 1"
    );
    assert!(inner.lock().unwrap().closed);
  }

  #[test]
  fn a_reset_before_the_jpeg_work_starts_runs_no_libjpeg() {
    let inner = jpeg_decoder(TRUNCATED_JPEG);
    reset(&inner);
    let (_, ran) = decode_with_hook(&inner, 0, 0, || {});
    // Like Chrome, which checks the abort flag before it decodes
    assert!(!ran);
    {
      let guard = inner.lock().unwrap();
      assert!(!guard.closed);
      assert!(guard.frame_failed.is_none());
    }
    assert_eq!(
      reason(abort_if_reset(&inner, 0)),
      "AbortError: Aborted by reset."
    );
    // The next decode() does the work, and fails; it closes the decoder on the JS thread
    let (error, ran) = decode_with_hook(&inner, 0, 1, || {});
    assert!(ran);
    assert_eq!(error, "Failed to decode frame at index 0");
    assert!(!inner.lock().unwrap().closed);
    apply_frame_failure(&inner, 1, 0, frame_failure(0));
    assert!(inner.lock().unwrap().closed);
  }

  #[test]
  fn a_close_while_the_jpeg_work_runs_wins() {
    let inner = jpeg_decoder(TRUNCATED_JPEG);
    let hook_inner = inner.clone();
    let (error, ran) = decode_with_hook(&inner, 0, 0, move || close(&hook_inner));
    assert!(ran);
    assert_eq!(error, "InvalidStateError: ImageDecoder is closed");
    let guard = inner.lock().unwrap();
    assert!(guard.closed);
    assert!(guard.frame_failed.is_none());
  }

  #[test]
  fn a_close_after_a_kept_jpeg_failure_wins() {
    let inner = jpeg_decoder(TRUNCATED_JPEG);
    let hook_inner = inner.clone();
    decode_with_hook(&inner, 0, 0, move || reset(&hook_inner));
    close(&inner);
    let (error, ran) = decode_with_hook(&inner, 0, 1, || {});
    assert!(!ran);
    assert_eq!(error, "InvalidStateError: ImageDecoder is closed");
  }

  /// A 12-bit JPEG: Chrome fails its frame as soon as it decodes it, also with part of the data
  const TWELVE_BIT_JPEG: &[u8] = include_bytes!("../../__test__/fixtures/jpeg/12-bit.jpg");

  /// The state of an ImageDecoder on an open stream that has read all of `data`, once a decode()
  /// made at epoch 0 had the collector try it (`StreamItem::Decode`), which found a frame failure
  fn failed_open_jpeg_stream(data: &[u8]) -> Arc<Mutex<ImageDecoderInner>> {
    let inner = jpeg_decoder(data);
    {
      let mut guard = inner.lock().unwrap();
      guard.all_data = false;
      guard.complete.store(false, Ordering::Release);
      guard.jpeg_requests = 1;
    }
    assert_eq!(
      reason(cache_decoded_frames(
        &inner,
        FrameCall::Arrival(0),
        false,
        false
      )),
      "Failed to decode frame at index 0"
    );
    inner
  }

  #[test]
  fn a_jpeg_frame_failure_found_for_a_waiting_decode_closes_the_decoder_on_the_js_thread() {
    let inner = failed_open_jpeg_stream(TWELVE_BIT_JPEG);
    // The collector left the decoder open, and so does the decode(), off the JS thread
    assert!(!inner.lock().unwrap().closed);
    let error = decode_cached_frame(&inner, 0, 0).err().unwrap();
    assert!(!inner.lock().unwrap().closed);
    // It closes the decoder on the JS thread
    assert_eq!(
      apply_frame_failure(&inner, 0, 0, error).reason,
      "Failed to decode frame at index 0"
    );
    assert!(inner.lock().unwrap().closed);
  }

  #[test]
  fn a_reset_before_the_js_thread_keeps_a_jpeg_frame_failure_for_the_next_decode() {
    let inner = failed_open_jpeg_stream(TWELVE_BIT_JPEG);
    let error = decode_cached_frame(&inner, 0, 0).err().unwrap();
    // Like Chrome, a reset() made before the failure reaches the JS thread (in the task of the
    // decode(), say) drops it
    reset(&inner);
    assert_eq!(
      apply_frame_failure(&inner, 0, 0, error).reason,
      "AbortError: Aborted by reset."
    );
    {
      let guard = inner.lock().unwrap();
      assert!(!guard.closed);
      assert_eq!(guard.frame_failed, Some(0));
    }
    // The next decode() fails and closes the decoder
    let error = decode_cached_frame(&inner, 0, 1).err().unwrap();
    assert!(!inner.lock().unwrap().closed);
    assert_eq!(
      apply_frame_failure(&inner, 1, 0, error).reason,
      "Failed to decode frame at index 0"
    );
    assert!(inner.lock().unwrap().closed);
  }

  #[test]
  fn a_close_before_the_js_thread_wins_over_a_jpeg_frame_failure() {
    let inner = failed_open_jpeg_stream(TWELVE_BIT_JPEG);
    let error = decode_cached_frame(&inner, 0, 0).err().unwrap();
    close(&inner);
    assert_eq!(
      apply_frame_failure(&inner, 0, 0, error).reason,
      "InvalidStateError: ImageDecoder is closed"
    );
  }

  /// `failed_open_jpeg_stream`, once the JS thread has read the end of the stream (the collector
  /// has not taken it yet)
  fn failed_ended_jpeg_stream(data: &[u8]) -> Arc<Mutex<ImageDecoderInner>> {
    let inner = failed_open_jpeg_stream(data);
    inner.lock().unwrap().stream_ended = true;
    inner
  }

  /// What the collector does at the end with a frame failure that waits for a decode() of the
  /// current epoch: the data is all here, and the JS thread decides (`StreamEnd::FrameFailure`)
  fn take_the_end(inner: &Arc<Mutex<ImageDecoderInner>>) {
    let mut guard = inner.lock().unwrap();
    guard.all_data = true;
    guard.end_deferred = true;
  }

  #[test]
  fn the_end_of_a_jpeg_stream_comes_after_a_frame_failure_found_before_it() {
    // The JS step at the end comes first, or the decode() that owns the failure
    for end_first in [true, false] {
      let inner = failed_ended_jpeg_stream(TWELVE_BIT_JPEG);
      // The decode() waits for the end
      assert!(matches!(decode_cached_frame(&inner, 0, 0), Ok(None)));
      take_the_end(&inner);
      let aborted = if end_first {
        // No reset() since the try: Chrome's failed decode comes before its end, so the decoder
        // closes, and `completed` rejects
        let aborted = end_with_frame_failure(&inner);
        assert!(inner.lock().unwrap().closed);
        let error = decode_cached_frame(&inner, 0, 0).err().unwrap();
        assert_eq!(
          apply_frame_failure(&inner, 0, 0, error).reason,
          "Failed to decode frame at index 0"
        );
        aborted
      } else {
        let error = decode_cached_frame(&inner, 0, 0).err().unwrap();
        assert_eq!(
          apply_frame_failure(&inner, 0, 0, error).reason,
          "Failed to decode frame at index 0"
        );
        assert!(inner.lock().unwrap().closed);
        end_with_frame_failure(&inner)
      };
      assert!(aborted, "end first: {end_first}");
      assert!(!inner.lock().unwrap().complete.load(Ordering::Acquire));
    }
  }

  #[test]
  fn a_reset_before_the_end_of_a_jpeg_stream_keeps_the_frame_failure_and_completes_the_data() {
    for end_first in [true, false] {
      let inner = failed_ended_jpeg_stream(TWELVE_BIT_JPEG);
      reset(&inner);
      // A decode() made after the reset() and the end waits for the end
      assert!(matches!(decode_cached_frame(&inner, 0, 1), Ok(None)));
      take_the_end(&inner);
      // The reset() dropped the failure, like Chrome's: the data is complete and `completed`
      // settles as at any end, then the decode() fails and closes the decoder on the JS thread.
      // Whichever comes first on the JS thread.
      let aborted = if end_first {
        let aborted = end_with_frame_failure(&inner);
        assert!(!inner.lock().unwrap().closed);
        let error = decode_cached_frame(&inner, 0, 1).err().unwrap();
        apply_frame_failure(&inner, 1, 0, error);
        aborted
      } else {
        let error = decode_cached_frame(&inner, 0, 1).err().unwrap();
        assert_eq!(
          apply_frame_failure(&inner, 1, 0, error).reason,
          "Failed to decode frame at index 0"
        );
        end_with_frame_failure(&inner)
      };
      assert!(!aborted, "end first: {end_first}");
      let guard = inner.lock().unwrap();
      assert!(guard.closed);
      assert!(guard.complete.load(Ordering::Acquire));
    }
  }

  #[test]
  fn the_end_read_right_after_a_first_chunk_lets_the_decode_after_that_chunk_post_first() {
    let inner = jpeg_decoder(TWELVE_BIT_JPEG);
    let mut guard = inner.lock().unwrap();
    // Two decode() calls whose try waits for the reads after a chunk that came first: one made
    // when the JS thread had handled 0 reads, one at 1
    guard.jpeg_requests = 2;
    guard.jpeg_unposted = 2;
    guard.jpeg_drains = vec![0, 1];
    let post = |decode_reads| JpegPost {
      epoch: 0,
      decode_reads,
      reads: 2,
      first: false,
    };
    // The 2nd read is the end: it is the read made right after the first decode()'s chunk, so
    // that try goes first. The other decode() saw that read come after it.
    guard.stream_reads = 2;
    post_before_end(&mut guard);
    assert_eq!(guard.jpeg_drains, vec![1]);
    assert_eq!(guard.jpeg_unposted, 1);
    // Its last step posts nothing more; the other one posts once
    finish_jpeg_post(&mut guard, post(0));
    assert_eq!(guard.jpeg_unposted, 1);
    finish_jpeg_post(&mut guard, post(1));
    assert!(guard.jpeg_drains.is_empty());
    assert_eq!(guard.jpeg_unposted, 0);
    // A reset() drops the steps of the decode() calls made before it
    guard.jpeg_drains = vec![0];
    guard.jpeg_unposted = 1;
    guard.reset();
    assert!(guard.jpeg_drains.is_empty());
    assert_eq!(guard.jpeg_unposted, 0);
    finish_jpeg_post(&mut guard, post(0));
    assert_eq!(guard.jpeg_unposted, 0);
  }

  #[test]
  fn a_decode_made_after_a_kept_jpeg_failure_owns_it_through_its_try() {
    let inner = failed_open_jpeg_stream(TWELVE_BIT_JPEG);
    reset(&inner);
    // The try that a decode() made after the reset() posted fails at once, like Chrome's
    assert!(!take_frame_failure(&inner, 0));
    assert!(take_frame_failure(&inner, 1));
    let guard = inner.lock().unwrap();
    assert_eq!(guard.frame_failed, Some(1));
    assert!(super::frame_failure_pending(&guard));
  }

  /// gif(4, 2, PALETTE, [fullFrame({ codes: BAD_CODES })]) of image-decoder.spec.ts: its one frame
  /// fails, so every frame does
  const BAD_GIF: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x04, 0x00, 0x02, 0x00, 0xf1, 0x00, 0x00, 0xff, 0x00, 0x00,
    0x00, 0xff, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00,
    0x02, 0x00, 0x00, 0x02, 0x01, 0x3c, 0x00, 0x3b,
  ];

  #[test]
  fn a_gif_whose_frames_all_fail_closes_the_decoder() {
    let inner = decoder(BAD_GIF, AVCodecID::Gif, "image/gif");
    assert_eq!(
      reason(decode_cached_frame(&inner, 0, 0)),
      "Failed to decode frame at index 0"
    );
    let guard = inner.lock().unwrap();
    assert!(guard.closed);
    assert!(guard.frames_failed);
  }

  #[test]
  fn a_gif_decode_made_before_a_reset_keeps_the_failure_for_the_next_decode() {
    let inner = decoder(BAD_GIF, AVCodecID::Gif, "image/gif");
    // The reset() lands once the call's turn has come (`abort_if_reset`), before its work: the
    // call builds the frames again, for the new cache_epoch, and they all fail
    reset(&inner);
    assert_eq!(
      reason(decode_cached_frame(&inner, 0, 0)),
      "Failed to decode frame at index 0"
    );
    {
      let guard = inner.lock().unwrap();
      // Like Chrome, the reset() dropped what the call found: the decoder is still open
      assert!(!guard.closed);
      // and it stays failed, like Chrome's decoder
      assert!(guard.frames_failed);
    }
    assert_eq!(
      reason(abort_if_reset(&inner, 0)),
      "AbortError: Aborted by reset."
    );
    // Like Chrome, the next decode() fails, whatever the frame index (the GIF has no frame 1),
    // and closes the decoder on the JS thread
    let error = decode_cached_frame(&inner, 1, 1).err().unwrap();
    assert_eq!(error.reason, "Failed to decode frame at index 1");
    assert!(!inner.lock().unwrap().closed);
    assert_eq!(
      apply_frame_failure(&inner, 1, 1, error).reason,
      "Failed to decode frame at index 1"
    );
    assert!(inner.lock().unwrap().closed);
  }

  #[test]
  fn a_close_after_a_kept_gif_failure_wins() {
    let inner = decoder(BAD_GIF, AVCodecID::Gif, "image/gif");
    reset(&inner);
    let _ = decode_cached_frame(&inner, 0, 0);
    close(&inner);
    assert_eq!(
      reason(decode_cached_frame(&inner, 0, 1)),
      "InvalidStateError: ImageDecoder is closed"
    );
  }
}
