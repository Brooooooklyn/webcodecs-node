//! ImageDecoder - WebCodecs API implementation
//!
//! Provides image decoding functionality: GIF and BMP with the decoders that follow Chrome
//! (gif_decoder, bmp_decoder); PNG and WebP read and drawn like Chrome (apng, webp), each frame
//! decoded with FFmpeg; the other formats with FFmpeg.
//! See: <https://developer.mozilla.org/en-US/docs/Web/API/ImageDecoder>

use crate::codec::demuxer::{DemuxerContext, MediaType};
use crate::codec::jpeg::{JpegDecode, decode_jpeg, jpeg_info};
use crate::codec::{CodecContext, CodecError, DecoderConfig, Frame, Packet};
use crate::ffi::avutil::{av_frame_copy_props, av_image_check_size, av_opt_set_int, av_rescale_q};
use crate::ffi::{
  AV_NOPTS_VALUE, AVCodecID, AVColorRange, AVColorSpace, AVPixelFormat, AVRational,
};
use crate::webcodecs::after_task::after_task;
use crate::webcodecs::apng::{self, CrcMemo, PngParse};
use crate::webcodecs::avif::{self, Avif, AvifImage, AvifSamples};
use crate::webcodecs::bmp_decoder::{self, BmpDecoder};
use crate::webcodecs::buffer_source::{self, Uint8ArrayCopy};
use crate::webcodecs::decoded_frame::{DecodedSource, avif_frame, decoded_frame_format};
use crate::webcodecs::error::{
  DOMExceptionName, abort_error, dom_exception_for_rejection, invalid_state_error,
  native_dom_exception_error, native_range_error, take_pending_exception,
  throw_invalid_state_error,
};
use crate::webcodecs::gif_decoder::{self, GifDecoder};
use crate::webcodecs::image_hints::PixelHint;
use crate::webcodecs::webp::{self, DemuxState};
use crate::webcodecs::{VideoFrame, VideoPixelFormat};
use futures::future::{Either, select};
use futures::stream::StreamExt;
use napi::bindgen_prelude::*;
use napi::tokio::sync::Notify;
use napi::tokio::sync::mpsc::{
  UnboundedReceiver, UnboundedSender, WeakUnboundedSender, unbounded_channel,
};
use napi_derive::napi;
use parking_lot::RwLock as ParkingLotRwLock;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::{
  Arc, Mutex, Weak,
  atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
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
      wait_for_flag(&tracks.ready, &tracks.ready_notify).await;
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
  /// The `cache_epoch` of the JPEG frame decode that a decode() runs with all the data
  /// (`FrameCall::Decode`, see `JpegFill`). Another decode() made in that epoch that finds no frame
  /// waits for it and takes its result, like Chrome's decoder thread, which decodes the frame once
  /// and answers the calls after that from it (ImageDecoderCore::Decode).
  jpeg_fill: Option<u64>,
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
  /// Whether a stream reached EOF or an error. Set before its final refresh and kept across
  /// reset(), so rebuilding its AVIF cache retains the terminal-stream parsing rules.
  terminal_stream: bool,
  /// The frame count of an AVIF track whose stream ended before the samples of its last frames
  /// arrived (`cached_frames` holds the frames before them). Chrome counts those frames, and the
  /// decode() of one fails (see `decode_cached_frame`).
  missing_samples: Option<usize>,
  /// What a PNG or a WebP keeps between two reads of its data (see `layered_frames`)
  layered: LayeredState,
  /// Track list
  tracks: ImageTrackList,
  /// Wakes the decode() calls that wait: for their turn (`DecodeTurn`), for data or a frame
  /// (`signal_tracks_ready`, `JpegFill`), or because a reset() or a close() rejected them
  /// (`reset_or_closed`). `tracks.ready_notify` is only for the waits for the tracks, so none of
  /// these can resolve `tracks.ready`.
  decode_notify: Arc<Notify>,
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
    self.decode_notify.notify_waiters();
  }

  /// What reset() does to an open decoder. Like Chrome, whose decoder stays failed after a
  /// reset(), it keeps `frame_failed` and `frames_failed`.
  fn reset(&mut self) {
    self.context = None;
    self.cached_frames = None;
    self.cache_epoch = self.cache_epoch.wrapping_add(1);
    self.cached_data_len = 0;
    // Like Chrome, the decode() calls that wait reject, at once (`ImageDecoder::reset`). A JPEG
    // keeps its choice of RGB (`rgb_decode`).
    self.jpeg_requests = 0;
    self.jpeg_unposted = 0;
    self.jpeg_drains.clear();
    self.decode_posted_bytes = 0;
    // Wake their work, also the work that waits for the tracks (`reset_or_closed`): it goes on
    // to its turn and ends there (`abort_if_reset`). `tracks.ready` stays as it is, like Chrome's.
    self.decode_notify.notify_waiters();

    // Reset frame_count for animated formats (will be re-detected on next decode). Not for PNG
    // and WebP: like Chrome, whose reset() leaves the decoder as it is, their track stays.
    if !layered_codec(self.codec_id)
      && let Ok(mut track_inner) = self.tracks.inner.lock()
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
  /// Where the promises of its decode() calls wait (`DECODE_CALLS`)
  id: u64,
}

/// The `id` of the next ImageDecoder
static NEXT_DECODER_ID: AtomicU64 = AtomicU64::new(0);

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
      jpeg_fill: None,
      frame_failed: None,
      end_deferred: false,
      terminal_stream: false,
      missing_samples: None,
      layered: LayeredState::default(),
      tracks: tracks.clone(),
      decode_notify: Arc::new(Notify::new()),
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
      // own, so a decode() made before that task is still pending. A PNG or a WebP sets
      // `tracks.ready` sooner, at its header, before its frames are drawn (`layered_frames`);
      // decode() still waits for this task, and so for the frames.
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
      // For the collector's callback (see `JpegRefs`)
      let jpeg_refs = match &reader {
        ChunkReader::Jpeg(reader) => Some(Arc::downgrade(&reader.refs)),
        ChunkReader::Any(_) => None,
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
        let jpeg = matches!(reader, ChunkReader::Jpeg(_));
        let mut retries = JpegRetries::default();
        // A PNG or a WebP is read again for every chunk: like Chrome, which reads each chunk, its
        // header and each frame count once the read of the chunk that brings their bytes is done
        // (see `layered_frames`).
        let layered = layered_codec(codec_id);

        loop {
          // Like Chrome's close, a PNG or WebP decoder that a failure closed (a decode() of a
          // frame that fails it) stops reading at once, and `completed` rejects while the data
          // is not complete.
          let item = if layered {
            match select(
              std::pin::pin!(reader.next()),
              std::pin::pin!(failure_closed(&inner_clone)),
            )
            .await
            {
              Either::Left((item, _)) => item,
              Either::Right(((), _)) => {
                return Ok(StreamEnd::Aborted(!data_complete.load(Ordering::Acquire)));
              }
            }
          } else {
            reader.next().await
          };
          let Some(item) = item else {
            break;
          };
          let (chunk, arrival) = match item {
            Ok(StreamItem::Chunk(chunk, arrival)) => {
              if jpeg {
                retries.chunk(&chunk);
              }
              let arrival = arrival.filter(|&epoch| {
                jpeg_try_wanted(&inner_clone, epoch) && retries.run(&inner_clone, epoch)
              });
              (chunk, arrival)
            }
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
              if jpeg_try_wanted(&inner_clone, epoch) && retries.run(&inner_clone, epoch) {
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
              if close_if_metadata_failed(&inner_clone)
                || closed_on_failure(&inner_clone)
                || (layered && closed_on_frame_failure(&inner_clone))
              {
                return Ok(StreamEnd::Aborted(!data_complete.load(Ordering::Acquire)));
              }
              // A stream error is terminal. Like Chrome, decode the data that came, as at EOF,
              // and mark the input complete before waking decode waiters so they reject instead
              // of waiting for an update that can never arrive. A JPEG frame failure found
              // before the error that a decode() has not applied yet comes first: the JS thread
              // decides (`end_with_frame_failure`).
              let pending = begin_terminal_refresh(&inner_clone);
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
                || (layered && closed_on_frame_failure(&inner_clone))
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
          // before the reader sees what came with it (the close, say), as Chrome does, unless
          // the try can only give what the last one gave (`JpegRetries`).
          let probe = buffered_len >= next_probe_size || layered;
          if probe || arrival.is_some() {
            let call = arrival.map_or(FrameCall::Probe, FrameCall::Arrival);
            let probe_inner = inner_clone.clone();
            let tried =
              spawn_blocking(move || cache_decoded_frames(&probe_inner, call, false, false)).await;
            // A rejected metadata closes the decoder at once. A frame failure does not (see
            // `StreamItem::Decode`): a decode() that waits wakes and applies it. A PNG or a WebP
            // has drawn the whole frames of the chunk by now, and stored them with the track.
            if close_if_metadata_failed(&inner_clone)
              || (layered && closed_on_frame_failure(&inner_clone))
            {
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
        let pending = begin_terminal_refresh(&inner_clone);
        let final_inner = inner_clone.clone();
        let _ =
          spawn_blocking(move || cache_decoded_frames(&final_inner, FrameCall::Probe, true, true))
            .await;
        if close_if_metadata_failed(&inner_clone)
          || closed_on_failure(&inner_clone)
          || (layered && closed_on_frame_failure(&inner_clone))
        {
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
        // The collector is done: let go of its JPEG stream's read() and reactions here, on the JS
        // thread. A stream error ends the collector without this callback: the JS thread let go
        // of them as it read the error.
        if let Some(refs) = jpeg_refs.as_ref().and_then(Weak::upgrade) {
          release_jpeg_refs(&refs);
        }
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
        wait_for_flag(&ready_flag, &ready_notifier).await;
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

    Ok(Self {
      inner,
      id: NEXT_DECODER_ID.fetch_add(1, Ordering::Relaxed),
    })
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
    // The call's promise. Like Chrome's, it is settled on the JS thread: by the work's callback
    // below, or at once by a reset() or a close() (see `DecodeCall`).
    let (deferred, promise) = create_call_promise(env)?;
    let promise = PromiseRaw::new(env.raw(), promise);

    let id = self.id;
    let inner = self.inner.clone();
    let frame_index = options.as_ref().and_then(|o| o.frame_index).unwrap_or(0) as usize;
    // Read now: a decode() made before the decoder fails rejects differently from one made
    // after (see `closed_error`), and one made before a reset() rejects (see `abort_if_reset`).
    // Like Chrome, the calls are handled in call order: what one finds can depend on the frames
    // that the ones before it failed.
    let call = inner.lock().map_err(|_| ()).map(|mut guard| {
      // Like Chrome, a call made on a closed decoder rejects at once, after the calls that close()
      // rejected (see `ImageDecoder::close`). Not while calls made before it wait: this one then
      // waits for its turn. Those are calls made before a failure closed the decoder, which reject
      // on their turns (`closed_error`), and a call whose rejection is being made (page code can
      // run there, see the work's callback below).
      if guard.closed && !has_calls(id) {
        return None;
      }
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
      Some((
        ticket,
        guard.closed,
        guard.cache_epoch,
        jpeg_request,
        post.then_some(guard.stream_reads),
      ))
    });
    let Ok(call) = call else {
      let error = Error::new(Status::GenericFailure, "Lock poisoned");
      settle_call(env, deferred, Err(error_value(env, error)));
      return Ok(promise);
    };
    let Some((ticket, closed, epoch, jpeg_request, post)) = call else {
      let error = decode_rejection(env, invalid_state_error("ImageDecoder is closed"));
      settle_call(env, deferred, Err(error));
      return Ok(promise);
    };
    push_call(id, ticket, deferred);
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
      // Made on a decoder that a failure closed while calls made before it were pending: like
      // Chrome, it rejects with InvalidStateError, after those calls, which reject on their turns.
      if closed {
        wait_for_turn(&inner, ticket).await?;
        return Err(invalid_state_error("ImageDecoder is closed"));
      }

      // Wait for ready promise (ensures initial pre-parsing is complete: for a buffer, its whole
      // probe, also the frames of a PNG or a WebP drawn after `tracks.ready`). A reset() or a
      // close() rejects the call at once and stops this wait (`reset_or_closed`): the work goes
      // on to its turn, and ends there.
      if let Either::Left((ready, _)) = select(
        ready_promise,
        std::pin::pin!(reset_or_closed(&inner, epoch)),
      )
      .await
      {
        ready?;
      }

      let update_notify = {
        let guard = inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
        guard.decode_notify.clone()
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
    let work =
      env.spawn_future_with_callback(async move { Ok(decode.await) }, move |env, result| {
        // A frame failure counts first (`apply_frame_failure`), then the next decode() call's
        // turn (see `DecodeTurn`)
        let result =
          result.map_err(|error| apply_frame_failure(&settle_inner, epoch, frame_index, error));
        drop(turn);
        // The call's promise, unless a reset() or a close() rejected it first: like Chrome, which
        // drops the result of a decode in flight, the result is dropped here, with its frame
        // (never given to JS)
        if !has_call(id, ticket) {
          return Ok(());
        }
        let value = match result {
          // SAFETY: on the JS thread, with its env
          Ok(result) => unsafe { ImageDecodeResult::to_napi_value(env.raw(), result) }
            .map_err(|error| decode_rejection(env, error)),
          Err(error) => Err(decode_rejection(env, error)),
        };
        settle_listed_call(env, id, ticket, value);
        Ok(())
      });
    let work = match work {
      Ok(work) => work,
      // No work: the callback is dropped, and with it the turn
      Err(error) => {
        let value = Err(decode_rejection(env, error));
        settle_listed_call(env, id, ticket, value);
        return Ok(promise);
      }
    };
    // The work's own promise rejects only when its callback never ran (a panic in the work): the
    // call then rejects with that reason, as before this promise was the call's. Never reported
    // as unhandled.
    let caught = work.catch(move |ctx: CallbackContext<Unknown>| {
      if let Some(deferred) = take_call(id, ticket) {
        settle_call(&ctx.env, deferred, Err(ctx.value.raw()));
      }
      Ok(())
    });
    if caught.is_err() {
      clear_pending_exception(env);
    }
    Ok(promise)
  }

  /// Reset the decoder
  /// Clears cached frames - next decode() will re-decode from stored data
  #[napi]
  pub fn reset(&self, env: Env) -> Result<()> {
    let (closed, made) = {
      let mut inner = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
      if !inner.closed {
        inner.reset();
      }
      (inner.closed, inner.next_decode)
    };
    // JS errors only once the lock is let go: page code can run there (a replaced global
    // DOMException) and call the decoder
    if closed {
      return throw_invalid_state_error(&env, "ImageDecoder is closed");
    }
    // Like Chrome (ImageDecoderExternal::reset), every decode() call not settled yet rejects
    // now, in call order: one that waits for the tracks (they stay pending), for data, for its
    // frame work, or whose frame waits to be delivered. Its work ends on its turn
    // (`abort_if_reset`).
    reject_calls(&env, self.id, made, "Aborted by reset.");
    Ok(())
  }

  /// Close the decoder
  #[napi]
  pub fn close(&self, env: Env) -> Result<()> {
    // Like Chrome (ImageDecoderExternal::close), every decode() call not settled yet rejects
    // now, in call order, with Chrome's message for a close(): "Aborted by failure." (its two
    // labels look swapped; a failure closes with "Aborted by close.", see `StreamEnd`). Not when
    // a failure closed the decoder first: those calls reject with EncodingError on their turns
    // (`closed_error`).
    if close_decoder(&self.inner)? {
      let made = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?
        .next_decode;
      reject_calls(&env, self.id, made, "Aborted by failure.");
    }
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

/// What close() does to the decoder's state. Returns whether it was open.
fn close_decoder(inner: &Arc<Mutex<ImageDecoderInner>>) -> Result<bool> {
  let mut inner = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  // A metadata failure counts once `close_if_metadata_failed` has closed the decoder. A close()
  // that comes first wins, also when a worker thread already found the failure
  // (`reject_metadata`). A GIF track goes back to how it was built the same way: until
  // `tracks.ready` is signalled, what a worker thread may have filled in
  // (`cache_decoded_frames`) does not count. Only GIF: gif_decoder often finishes before the JS
  // task that made the decoder reaches close(), so without this the track after an early
  // close() changes from run to run. The other formats keep what the worker found, as before.
  let open = !inner.closed;
  if open {
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
  Ok(open)
}

thread_local! {
  /// The decode() calls whose promise is not settled yet, by decoder (`ImageDecoder::id`), in
  /// call order, like Chrome's `pending_decodes_`. A decoder has an entry only while it has such
  /// calls.
  static DECODE_CALLS: RefCell<HashMap<u64, Vec<DecodeCall>>> = RefCell::new(HashMap::new());
}

/// A decode() call whose promise is not settled yet: its ticket (see `DecodeTurn`) and the
/// promise's napi_deferred. Like Chrome, which settles these on the main thread, also inside
/// reset() and close() (ImageDecoderExternal::reset), so a call they reject is rejected before
/// they return; napi-rs settles its own deferreds only in a later callback.
///
/// Safety rule for `deferred`: decode() makes it with `create_call_promise`, on the JS thread,
/// and keeps it only in that thread's `DECODE_CALLS`. Whoever takes it out of the map settles it
/// at once with `settle_call`, on the same thread and with the decoder's env: decode()'s work
/// callback or its catch reaction (`take_call`), reset() or close() (`take_calls`). So it is
/// settled once, and never used after. The decode work and its callback capture only the
/// decoder's id and the ticket: the callback may be dropped on another thread (when the work
/// panics, or at env teardown) and owns nothing of JS. A raw pointer is neither Send nor Sync,
/// so the compiler keeps a DecodeCall on its thread. A call left in the map at env teardown is
/// never settled; dropping it makes no napi call.
struct DecodeCall {
  ticket: u64,
  deferred: napi::sys::napi_deferred,
}

/// On the JS thread: a decode() call's promise and its napi_deferred (see `DecodeCall`)
fn create_call_promise(env: &Env) -> Result<(napi::sys::napi_deferred, napi::sys::napi_value)> {
  let mut deferred = std::ptr::null_mut();
  let mut promise = std::ptr::null_mut();
  // SAFETY: on the JS thread, with its env; both out pointers are valid
  napi::check_status!(
    unsafe { napi::sys::napi_create_promise(env.raw(), &mut deferred, &mut promise) },
    "Failed to create the promise of decode()"
  )?;
  Ok((deferred, promise))
}

fn push_call(id: u64, ticket: u64, deferred: napi::sys::napi_deferred) {
  let _ = DECODE_CALLS.try_with(|calls| {
    calls
      .borrow_mut()
      .entry(id)
      .or_default()
      .push(DecodeCall { ticket, deferred })
  });
}

/// Take the promise of a decoder's decode() call out of `DECODE_CALLS`, to settle it now. None
/// when a reset() or a close() took it first.
fn take_call(id: u64, ticket: u64) -> Option<napi::sys::napi_deferred> {
  DECODE_CALLS
    .try_with(|calls| {
      let mut calls = calls.borrow_mut();
      let pending = calls.get_mut(&id)?;
      let call = pending.remove(pending.iter().position(|call| call.ticket == ticket)?);
      if pending.is_empty() {
        calls.remove(&id);
      }
      Some(call.deferred)
    })
    .ok()
    .flatten()
}

/// Take the promises of a decoder's decode() calls whose ticket is below `made` out of
/// `DECODE_CALLS`, in call order, to settle them now
fn take_calls(id: u64, made: u64) -> Vec<DecodeCall> {
  DECODE_CALLS
    .try_with(|calls| {
      let mut calls = calls.borrow_mut();
      let Some(pending) = calls.get_mut(&id) else {
        return Vec::new();
      };
      // in call order: the calls made before are first
      let before = pending.iter().take_while(|call| call.ticket < made).count();
      let taken = pending.drain(..before).collect();
      if pending.is_empty() {
        calls.remove(&id);
      }
      taken
    })
    .unwrap_or_default()
}

/// Whether a decoder's decode() call is still in `DECODE_CALLS`
fn has_call(id: u64, ticket: u64) -> bool {
  DECODE_CALLS
    .try_with(|calls| {
      calls
        .borrow()
        .get(&id)
        .is_some_and(|pending| pending.iter().any(|call| call.ticket == ticket))
    })
    .unwrap_or(false)
}

/// On the JS thread: settle a decode() call with `value`, made while the call was still in
/// `DECODE_CALLS`, unless page code that ran while `value` was made settled it first. A rejection
/// is made through the global DOMException, which page code can replace: a decode() made there
/// finds this call still pending, and waits for its turn rather than reject before it, and a
/// reset() or close() made there takes this call and rejects it first.
fn settle_listed_call(
  env: &Env,
  id: u64,
  ticket: u64,
  value: std::result::Result<napi::sys::napi_value, napi::sys::napi_value>,
) {
  if let Some(deferred) = take_call(id, ticket) {
    settle_call(env, deferred, value);
  }
}

/// Whether a decoder has decode() calls whose promise is not settled yet
fn has_calls(id: u64) -> bool {
  DECODE_CALLS
    .try_with(|calls| calls.borrow().contains_key(&id))
    .unwrap_or(false)
}

/// On the JS thread: settle a decode() call's promise, taken out of `DECODE_CALLS` (or never put
/// there) just before. Ok resolves with the value, Err rejects with it.
fn settle_call(
  env: &Env,
  deferred: napi::sys::napi_deferred,
  value: std::result::Result<napi::sys::napi_value, napi::sys::napi_value>,
) {
  // SAFETY: see `DecodeCall`: `deferred` was made with this env, on this thread, and is not
  // settled yet. Settling it frees it, and nothing holds it any more.
  let status = unsafe {
    match value {
      Ok(value) => napi::sys::napi_resolve_deferred(env.raw(), deferred, value),
      Err(value) => napi::sys::napi_reject_deferred(env.raw(), deferred, value),
    }
  };
  // napi settles nothing while an exception is pending: reject with it rather than leave the
  // promise pending for ever
  if status == napi::sys::Status::napi_pending_exception
    && let Ok(Some(exception)) = take_pending_exception(env)
  {
    // SAFETY: as above; the deferred was not settled
    unsafe { napi::sys::napi_reject_deferred(env.raw(), deferred, exception.raw()) };
  }
}

/// `error` as a JS value: the JS value it holds, as it is (a DOMException is no native Error,
/// which `JsError::into_value` would replace with a new Error), else a new Error with its message
fn error_value(env: &Env, error: Error) -> napi::sys::napi_value {
  // SAFETY: on the JS thread, with its env
  unsafe { ToNapiValue::to_napi_value(env.raw(), error) }
    .unwrap_or_else(|error| unsafe { napi::JsError::from(error).into_value(env.raw()) })
}

/// What a decode() call rejects with for `error`: the JS error it names
/// (`native_image_decode_error`), or, when page code broke that (a replaced global DOMException),
/// the exception it left pending
fn decode_rejection(env: &Env, error: Error) -> napi::sys::napi_value {
  match native_image_decode_error(env, error) {
    Ok(error) => error_value(env, error),
    Err(error) => match take_pending_exception(env) {
      Ok(Some(exception)) => exception.raw(),
      _ => error_value(env, error),
    },
  }
}

/// On the JS thread, in reset() or close(): reject the decoder's decode() calls made before it
/// (whose ticket is below `made`, `ImageDecoderInner::next_decode` then) and not settled yet, in
/// call order, with one AbortError, like Chrome. Made once the decoder's lock is let go: page code
/// can run while the error is made (a replaced global DOMException) and call the decoder. So the
/// calls are taken only after the error is made: a close() made there rejects them first, with
/// the calls made there, in call order, and leaves none here. A decode() made there that no
/// close() rejects is a call made after this one, and stays.
fn reject_calls(env: &Env, id: u64, made: u64, message: &str) {
  if !has_calls(id) {
    return;
  }
  let error = dom_exception_for_rejection(env, abort_error(message))
    .map(|error| error.raw())
    .unwrap_or_else(|_| error_value(env, abort_error(message)));
  for call in take_calls(id, made) {
    settle_call(env, call.deferred, Err(error));
  }
}

/// Ends once a reset() or a close() came after the decode() call made when `cache_epoch` was
/// `epoch`. A reset() or a close() rejects the call at once; a close on a failure leaves it to
/// its turn (`closed_error`). Either way its work stops waiting for the tracks.
async fn reset_or_closed(inner: &Arc<Mutex<ImageDecoderInner>>, epoch: u64) {
  let Ok(notify) = inner.lock().map(|guard| guard.decode_notify.clone()) else {
    return;
  };
  loop {
    // Registered before the check, so a reset() in between still wakes it
    let mut notified = std::pin::pin!(notify.notified());
    notified.as_mut().enable();
    match inner.lock() {
      Ok(guard) if !guard.closed && guard.cache_epoch == epoch => {}
      _ => return,
    }
    notified.await;
  }
}

/// Wait until `flag` (`tracks.ready`) is set. `notify` is woken once it is set
/// (`signal_tracks_ready`, `ImageDecoderInner::close`); the flag is read again after each wake.
async fn wait_for_flag(flag: &AtomicBool, notify: &Notify) {
  loop {
    // Registered before the check, so a notify_waiters() in between still wakes it
    let mut notified = std::pin::pin!(notify.notified());
    notified.as_mut().enable();
    if flag.load(Ordering::Acquire) {
      return;
    }
    notified.await;
  }
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
    // A WebP whose data had an error after its header: like Chrome, whose demuxer is gone until
    // the end of the data, every decode() waits (see `WebpState::lost`).
    if guard.layered.webp.lost
      && guard.layered.webp.header.is_some()
      && !guard.complete.load(Ordering::Acquire)
    {
      return Ok(None);
    }
    // A read of a PNG or a WebP draws the frames of this epoch now (`LayeredState::drawing`): like
    // Chrome's decoder, which decodes a frame once, wait for them rather than draw them again
    if drawing_for(&guard, epoch) {
      return Ok(None);
    }
    (
      guard.cached_frames.is_none(),
      guard.complete.load(Ordering::Acquire),
      guard.codec_id == Some(AVCodecID::Mjpeg),
    )
  };
  let layered = inner
    .lock()
    .is_ok_and(|guard| layered_codec(guard.codec_id));

  // Like Chrome, a JPEG has one frame: another index fails once the data is complete, before
  // any decode.
  if jpeg && frame_index > 0 {
    return if data_complete {
      Err(frame_index_error(frame_index, 1))
    } else {
      Ok(None)
    };
  }

  // One decode() at a time decodes a JPEG frame with all the data: another one made in the same
  // epoch waits for it, and takes its frame or its failure (see `JpegFill`).
  let (needs_cache, _fill) = if jpeg && needs_cache && data_complete {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if guard.jpeg_fill == Some(epoch) {
      return Ok(None);
    }
    let needs_cache = guard.cached_frames.is_none();
    let fill = (needs_cache && guard.cache_epoch == epoch).then(|| {
      guard.jpeg_fill = Some(epoch);
      JpegFill { inner, epoch }
    });
    (needs_cache, fill)
  } else {
    (needs_cache, None)
  };

  // A reset can clear the cache. For fully buffered input, reconstruct it now;
  // for a live stream, wait for the collector's next successful probe. A JPEG stream's data so
  // far is tried by the collector, in the stream's order (`FrameCall::Arrival`); here only once
  // the data is complete (`FrameCall::Decode`). A PNG or a WebP is read again now, like Chrome,
  // whose decoder has the data so far after a reset().
  if needs_cache && (data_complete || jpeg || layered) {
    before_decode_read();
    cache_decoded_frames(inner, FrameCall::Decode(epoch), data_complete, false)
      .map_err(|error| decode_failure(inner, epoch, frame_index, error))?;
  }

  let mut guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if guard.closed {
    return Err(closed_error(&guard, frame_index));
  }
  // A read that started drawing the frames after the check above: the read made here left them
  // to it (`layered_frames`), and this waits for them
  if drawing_for(&guard, epoch) {
    return Ok(None);
  }

  // A JPEG that was not decoded above because the data was not complete then waits for the
  // signal that comes with `complete` (`FrameCall::Decode`), even when the data is complete now.
  let data_complete = if jpeg {
    data_complete
  } else {
    guard.complete.load(Ordering::Acquire)
  };
  // A simple lossy WebP whose frame a decode() tries while its data is not all here: like
  // Chrome, the frame is RGB from then on, also after a reset() (image_decoder_core.cc
  // have_completed_rgb_decode_). Else it is YUV planes, decoded once all the data is here.
  let webp = guard.codec_id == Some(AVCodecID::Webp);
  let webp_yuv = webp
    && guard
      .layered
      .webp
      .header
      .is_some_and(|header| header.simple_lossy);
  if webp_yuv
    && frame_index == 0
    && !guard.all_data
    && guard
      .cached_frames
      .as_ref()
      .is_some_and(|frames| !frames.is_empty())
  {
    guard.rgb_decode = true;
  }
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
    // A frame of an AVIF track whose samples did not all arrive before the stream ended: like
    // Chrome, the decode() fails with EncodingError, and the decoder closes on the JS thread
    // (`apply_frame_failure`).
    let frame_count = guard.missing_samples.unwrap_or(frames.len());
    if frame_index < frame_count {
      guard.frame_failed = Some(epoch);
      return Err(frame_failure(frame_index));
    }
    return Err(frame_index_error(frame_index, frame_count));
  }

  // Clone the Arc to share the frame data (no pixel copy needed).
  let (frame_arc, format, duration) = match &frames[frame_index] {
    CachedFrame::Decoded {
      frame,
      format,
      duration,
    } => (frame.clone(), *format, *duration),
    // A still WebP whose frame has only its image chunk header (`WebpState::unnumbered`): like
    // Chrome, a decode() made now fails the decoder (see below)
    CachedFrame::Pending if webp && guard.layered.webp.unnumbered => {
      return Err(fail_webp_decoder(&mut guard, epoch, frame_index));
    }
    CachedFrame::Pending if !data_complete => return Ok(None),
    CachedFrame::Pending => {
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "RangeError: Unexpected end of image. Request for frame index {frame_index} can't be satisfied."
        ),
      ));
    }
    // Like Chrome, a WebP frame that fails fails the decoder at once (`fail_webp_decoder`)
    CachedFrame::Failed { .. } if webp => {
      return Err(fail_webp_decoder(&mut guard, epoch, frame_index));
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
  // The YUV frame of a simple lossy WebP that Chrome now decodes to RGB (see `webp_yuv`)
  let (frame_arc, format) =
    if webp_yuv && frame_index == 0 && format == VideoPixelFormat::I420 && guard.rgb_decode {
      let source = DecodedSource::Image {
        codec: AVCodecID::Webp,
        alpha: Some(false),
        yuv: Some(false),
        pixels: PixelHint::None,
      };
      let converted = frame_arc
        .read()
        .shallow_clone()
        .and_then(|frame| decoded_frame_format(frame, source))
        .map_err(|e| {
          Error::new(
            Status::GenericFailure,
            format!("Failed to convert decoded image: {e}"),
          )
        })?;
      let frame = converted.0.into_shared();
      if let Some(slot) = guard
        .cached_frames
        .as_mut()
        .and_then(|frames| frames.get_mut(0))
      {
        *slot = CachedFrame::Decoded {
          frame: frame.clone(),
          format: converted.1,
          duration,
        };
      }
      (frame, converted.1)
    } else {
      (frame_arc, format)
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
/// the JS thread, in the callback that settles the call's promise (unless a reset() or a close()
/// did), so the next call starts only once this one settles, and the promises settle in call
/// order, like Chrome's. Given away on the worker thread instead, the next call could finish and
/// settle first while that thread waits to settle this one. It also drops when the callback never
/// runs (the environment goes away), so the calls after it do not wait forever.
struct DecodeTurn(Arc<Mutex<ImageDecoderInner>>);

impl Drop for DecodeTurn {
  fn drop(&mut self) {
    if let Ok(mut guard) = self.0.lock() {
      guard.decode_turn += 1;
      guard.decode_notify.notify_waiters();
    }
  }
}

/// Wait for the turn of the decode() call with this ticket (see `DecodeTurn`)
async fn wait_for_turn(inner: &Arc<Mutex<ImageDecoderInner>>, ticket: u64) -> Result<()> {
  let notify = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?
    .decode_notify
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
/// The reset() rejects it at once (`ImageDecoder::reset`); its work ends here, on the call's turn,
/// with what it found dropped, and gives the turn to the next call, so the calls made after the
/// reset() do not wait for its result. A close() also moves `cache_epoch`; the work then gets the
/// error of a closed decoder (`closed_error`).
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

/// Like Chrome, a WebP frame that fails fails the decoder at once (WEBPImageDecoder::Decode,
/// SetFailed), while more data can come too: the decode() of `frame_index`, made when
/// `cache_epoch` was `epoch`, rejects, the decoder closes, and the decode() calls still pending
/// reject with EncodingError (`closed_error`). Not for a decode() that a reset() made stale: like
/// Chrome, whose reset() drops the result of a decode in flight, it rejects with AbortError
/// (`abort_if_reset`), and the decoder stays failed: the next decode() closes it.
fn fail_webp_decoder(guard: &mut ImageDecoderInner, epoch: u64, frame_index: usize) -> Error {
  guard.frames_failed = true;
  if guard.cache_epoch == epoch {
    guard.close();
  }
  frame_failure(frame_index)
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

/// Mark a stream EOF or error before starting its final refresh. The return value lets the
/// collector defer its end to a pending JPEG frame failure.
fn begin_terminal_refresh(inner: &Arc<Mutex<ImageDecoderInner>>) -> bool {
  inner.lock().is_ok_and(|mut guard| {
    guard.all_data = true;
    guard.terminal_stream = true;
    frame_failure_pending(&guard)
  })
}

/// The JPEG frame decode that a decode() runs (see `ImageDecoderInner::jpeg_fill`). It ends when
/// that decode() has its result, also on an error or a panic, and wakes the calls that wait for
/// it. A reset() moves `cache_epoch`: the calls made after it do not wait for this one.
struct JpegFill<'a> {
  inner: &'a Arc<Mutex<ImageDecoderInner>>,
  epoch: u64,
}

impl Drop for JpegFill<'_> {
  fn drop(&mut self) {
    if let Ok(mut guard) = self.inner.lock() {
      if guard.jpeg_fill == Some(self.epoch) {
        guard.jpeg_fill = None;
      }
      guard.decode_notify.notify_waiters();
    }
  }
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
    set_tracks_ready(&inner_guard);
    // The decode() calls that wait for data or a frame look again
    inner_guard.decode_notify.notify_waiters();
  }
}

/// Set `tracks.ready`, under the decoder's lock, and wake what waits for it
fn set_tracks_ready(inner: &ImageDecoderInner) {
  inner.tracks.ready.store(true, Ordering::Release);
  // notify_waiters wakes listeners that are already registered, while
  // notify_one stores a permit for the narrow interval between the atomic
  // recheck and the first poll of a Notified future.
  inner.tracks.ready_notify.notify_waiters();
  inner.tracks.ready_notify.notify_one();
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
///
/// Each step is the reaction to a promise made here and resolved at once. Its job goes to the end
/// of the microtask queue in the then() call, as a queueMicrotask() callback would, so the steps
/// keep their place among the reactions of the reads. Fake timers cannot hold them back: they
/// replace `queueMicrotask`, not promise jobs (Chrome's decoder does not use the page's globals).
/// Page code that replaces `Promise.prototype.then` can.
fn queue_jpeg_post(env: &Env, inner: &Arc<Mutex<ImageDecoderInner>>, post: JpegPost) {
  let step_inner = inner.clone();
  let queued = PromiseRaw::<()>::resolve(env, ()).and_then(|resolved| {
    resolved
      .then(move |ctx| {
        jpeg_post_step(&ctx.env, &step_inner, post);
        Ok(())
      })
      .map(|_| ())
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

/// Which wanted tries of a JPEG stream's data so far run (`StreamItem::Decode`,
/// `FrameCall::Arrival`). decode_jpeg() reads the data from its first byte each time, so a try for
/// every chunk of a stream that trickles in, while a decode() waits, decodes ever longer prefixes:
/// the work grows with the square of the chunk count (Chrome keeps one Blink JPEGImageDecoder,
/// which reads each byte once). A try that can only give what the last one gave (no frame, no
/// failure) is skipped. One runs when it is the first of its epoch (a reset() dropped what the
/// last one found), when a frame failure is recorded (those paths stay as they are), or when
/// bytes that can change what libjpeg reports came since the last one (`JpegScan`).
#[derive(Default)]
struct JpegRetries {
  scan: JpegScan,
  /// The epoch of the last try that ran
  last: Option<u64>,
  /// Whether bytes that can change a try's result came since then
  changed: bool,
}

impl JpegRetries {
  /// A chunk of the stream, before the collector appends it
  fn chunk(&mut self, chunk: &[u8]) {
    self.changed |= self.scan.feed(chunk);
  }

  /// Whether a wanted try (`jpeg_try_wanted`) made when `cache_epoch` was `epoch` runs
  fn run(&mut self, inner: &Arc<Mutex<ImageDecoderInner>>, epoch: u64) -> bool {
    let failed = !inner.lock().is_ok_and(|guard| guard.frame_failed.is_none());
    if !self.changed && self.last == Some(epoch) && !failed {
      return false;
    }
    self.last = Some(epoch);
    self.changed = false;
    true
  }
}

/// Reads the markers of a JPEG stream as its chunks come, to tell which bytes can change what a
/// try of the data so far reports (`jpeg_frame`: a frame, rows started, a failure):
/// - The frame is complete only at the EOI marker, and Blink reads nothing after it.
/// - Blink gets to output rows only at the start of a scan (a multi-scan sequential file: at the
///   EOI marker).
/// - libjpeg fails on markers, at the start of a scan (jdmarker.c, jdinput.c, the start_pass of
///   jdhuff.c and jdphuff.c) and on the 100th scan (`progress_monitor` in jpeg_blink.c). In the
///   entropy-coded data of a scan, with its restart markers and stuffed bytes, it only warns (a
///   progressive decode then stops reading, `num_corrupt_warnings` in jpeg_blink.c), with one
///   exception: a DC value that overflows in a progressive DC first scan (jdphuff.c
///   decode_mcu_DC_first, JERR_BAD_DCT_COEF). That takes more than 65536 DC differences of 15
///   bits, 16 bits each with their code, so at least 128 KiB of the scan.
///
/// So the bytes of a scan's data (only the first 128 KiB of a progressive DC first scan) and the
/// bytes after the EOI marker change nothing; every other byte, garbage too, does.
#[derive(Default)]
struct JpegScan {
  state: ScanState,
  /// Whether any input byte has been seen; the first byte must establish a
  /// baseline libjpeg result before quiet segments can be skipped.
  started: bool,
  /// A progressive frame (SOF2, or SOF6, SOF10, SOF14, which libjpeg fails)
  progressive: bool,
  /// The SOS segment read so far
  sos: Vec<u8>,
  /// How many more bytes of the current scan's data change nothing
  quiet: usize,
}

#[derive(Default, Clone, Copy)]
enum ScanState {
  /// Between marker segments, or after 0xFF there
  #[default]
  Between,
  Ff,
  /// The two length bytes of a marker's segment (with the first once read)
  Length(u8, Option<u8>),
  /// In a marker's segment, with the bytes left
  Segment(u8, usize),
  /// In the entropy-coded data of a scan, or after 0xFF there
  Scan,
  ScanFf,
  /// After the EOI marker
  Done,
}

/// How many bytes of a progressive DC first scan's data cannot fail (see `JpegScan`)
const DC_FIRST_QUIET: usize = 1 << 17;

/// APP1, APP2 and APP11 are saved opaque metadata, not a reason to re-read the
/// accumulated image for every arriving chunk. Their complete segments are
/// quiet; structural markers and the final parse still retain their bytes.
fn saved_app(code: u8) -> bool {
  matches!(code, 0xE1 | 0xE2 | 0xEB)
}

impl JpegScan {
  /// The next bytes of the stream. Returns whether any of them can change a try's result.
  fn feed(&mut self, bytes: &[u8]) -> bool {
    let mut changed = false;
    for &byte in bytes {
      let first = !self.started;
      self.started = true;
      let quiet = match self.state {
        ScanState::Done => true,
        ScanState::Scan => {
          if byte == 0xFF {
            self.state = ScanState::ScanFf;
          }
          self.scan_byte()
        }
        // A stuffed 0xFF, a restart marker, a fill byte, or the marker that ends the scan
        ScanState::ScanFf => match byte {
          0x00 | 0xD0..=0xD7 => {
            self.state = ScanState::Scan;
            self.scan_byte()
          }
          0xFF => self.scan_byte(),
          code => {
            self.marker(code);
            false
          }
        },
        ScanState::Between => {
          if byte == 0xFF {
            self.state = ScanState::Ff;
            // Wait for the marker code: a saved APP segment is quiet, while a
            // structural marker still makes the try significant.
            true
          } else {
            false
          }
        }
        ScanState::Ff => match byte {
          0xFF => true,
          0x00 => {
            self.state = ScanState::Between;
            false
          }
          code => {
            self.marker(code);
            saved_app(code)
          }
        },
        ScanState::Length(code, None) => {
          self.state = ScanState::Length(code, Some(byte));
          saved_app(code)
        }
        ScanState::Length(code, Some(high)) => {
          let length = usize::from(high) << 8 | usize::from(byte);
          self.segment(code, length.saturating_sub(2));
          // A malformed segment length still needs libjpeg's error path.
          saved_app(code) && length >= 2
        }
        ScanState::Segment(code, left) => {
          if code == 0xDA {
            self.sos.push(byte);
          }
          self.segment(code, left - 1);
          saved_app(code)
        }
      };
      changed |= first || !quiet;
    }
    changed
  }

  fn marker(&mut self, code: u8) {
    self.state = match code {
      // EOI
      0xD9 => ScanState::Done,
      // SOI, TEM, RSTn: no segment
      0xD8 | 0x01 | 0xD0..=0xD7 => ScanState::Between,
      _ => {
        if matches!(code, 0xC2 | 0xC6 | 0xCA | 0xCE) {
          self.progressive = true;
        }
        self.sos.clear();
        ScanState::Length(code, None)
      }
    };
  }

  fn segment(&mut self, code: u8, left: usize) {
    self.state = if left > 0 {
      ScanState::Segment(code, left)
    } else if code == 0xDA {
      // SOS: Ns, two bytes per component, Ss, Se, Ah and Al
      let components = usize::from(self.sos.first().copied().unwrap_or(0));
      let ss = self.sos.get(1 + 2 * components);
      let ah_al = self.sos.get(3 + 2 * components);
      self.quiet = match (ss, ah_al) {
        (Some(0), Some(ah_al)) if self.progressive && ah_al >> 4 == 0 => DC_FIRST_QUIET,
        (Some(_), Some(_)) => usize::MAX,
        _ => 0,
      };
      ScanState::Scan
    } else {
      ScanState::Between
    };
  }

  /// A byte of a scan's data: whether it changes nothing
  fn scan_byte(&mut self) -> bool {
    let quiet = self.quiet > 0;
    self.quiet = self.quiet.saturating_sub(1);
    quiet
  }
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

/// The collector's end of a JPEG stream's reader (see `JpegStream`). Dropping it ends the
/// reading: the JS thread finds no collector at its next result and reads no more. The JS
/// references are let go on the JS thread (see `JpegRefs`).
struct JpegChunkReader {
  items: UnboundedReceiver<JpegItem>,
  refs: Arc<Mutex<JpegRefs>>,
}

type JpegReadFn = FunctionRef<(), PromiseRaw<'static, Object<'static>>>;

/// A JPEG stream's `onRead` and `onReadError`
type JpegReactions = (FunctionRef<(), ()>, FunctionRef<(), ()>);

/// The same, to hand to then()
type JpegReactionFns<'env> = (Function<'env, (), ()>, Function<'env, (), ()>);

/// The JS references of a JPEG stream's reader, under one lock. The JS thread makes the reactions
/// only while `read` is there (`JpegStream::reactions`), and only the JS thread lets go of them
/// (`release_jpeg_refs`): at the end or an error of the stream, in the collector's callback once
/// the collector is done, or at the env's teardown (`release_jpeg_streams`).
///
/// Never on the collector's thread: napi lets go of a reference dropped there through a
/// thread-safe function of the env, and while the env is torn down, once Node has started to
/// close that function, napi before 3.15 takes every such drop after the first one as an error
/// (`napi_invalid_arg`) and panics; the second panic in one drop aborts the process. A collector
/// can drop then in two ways: the async runtime is shut down at the teardown of the last env that
/// loaded the addon (a worker, when only workers load it), which drops the collector on its
/// thread; or the collector ends by itself then (a JPEG whose header fails, say).
struct JpegRefs {
  /// The reader's read(); None once the JS thread let go of it
  read: Option<JpegReadFn>,
  /// The reactions to every read of the stream, made at its first read. They hold the stream
  /// (`JpegStream`), which holds them, until the JS thread lets go of them with `read`, which
  /// ends that cycle. So while `read` is there, the JS thread keeps the stream, and with it these
  /// references, alive: the collector never holds the last of them.
  reactions: Option<JpegReactions>,
  /// The JS thread
  thread: std::thread::ThreadId,
}

impl Drop for JpegRefs {
  fn drop(&mut self) {
    // Only the JS thread holds the last of these with references in it (see above). Should it
    // ever be another thread, they leak rather than call napi there: the env frees them.
    if std::thread::current().id() != self.thread {
      std::mem::forget(self.read.take());
      std::mem::forget(self.reactions.take());
    }
  }
}

impl Drop for JpegChunkReader {
  fn drop(&mut self) {
    // The collector is done. On the JS thread (the collector was never started), let go of the
    // references now; else the collector's callback does, on the JS thread.
    if self
      .refs
      .lock()
      .is_ok_and(|refs| refs.thread == std::thread::current().id())
    {
      release_jpeg_refs(&self.refs);
    }
  }
}

/// On the JS thread: let go of a JPEG stream's read() and reactions (see `JpegRefs`). They are
/// taken out under the lock and dropped after it.
fn release_jpeg_refs(refs: &Mutex<JpegRefs>) {
  let taken = refs
    .lock()
    .ok()
    .map(|mut refs| (refs.read.take(), refs.reactions.take()));
  drop(taken);
}

/// The open JPEG streams of each env, by env pointer (see `JPEG_STREAMS`)
type JpegStreamsByEnv = std::collections::HashMap<usize, Vec<Weak<Mutex<JpegRefs>>>>;

thread_local! {
  /// The JPEG streams of each env on this thread, for that env's teardown
  /// (`release_jpeg_streams`). A thread can hold more than one env: Node makes one for each load
  /// of the addon, and an embedder like Electron can run several Node environments on one thread
  /// and tear one down while the others go on. An env's first stream adds its entry and its hook;
  /// the hook removes the entry before the env goes away, so a later env at the same address never
  /// finds it.
  static JPEG_STREAMS: std::cell::RefCell<JpegStreamsByEnv> =
    std::cell::RefCell::new(JpegStreamsByEnv::new());
}

/// On the JS thread, at a JPEG stream's start: let the teardown of its env see its references.
/// The first stream of the env adds the env's teardown hook (`release_jpeg_streams`).
fn watch_jpeg_refs(env: &Env, refs: &Arc<Mutex<JpegRefs>>) {
  let key = env.raw() as usize;
  let _ = JPEG_STREAMS.try_with(|streams| {
    let mut streams = streams.borrow_mut();
    if !streams.contains_key(&key) {
      // SAFETY: on the env's JS thread; the hook's data is the env pointer, used only as the key
      let status = unsafe {
        napi::sys::napi_add_env_cleanup_hook(
          env.raw(),
          Some(release_jpeg_streams),
          env.raw().cast(),
        )
      };
      if status != napi::sys::Status::napi_ok {
        return;
      }
    }
    let list = streams.entry(key).or_default();
    list.retain(|stream| stream.strong_count() > 0);
    list.push(Arc::downgrade(refs));
  });
}

/// The teardown of one env (the hook's data), on its JS thread: let go of the references of every
/// JPEG stream of that env still open (see `JpegRefs`). Only that env's: another env on the
/// thread keeps reading its streams, and its own hook lets go of them at its own teardown. Node
/// runs the cleanup hooks from the one added last. This one is added at the env's first stream,
/// after the hooks napi added when the addon was registered in the env: the one that shuts the
/// async runtime down at the last env (and with it the collectors), and the one that closes napi's
/// thread-safe function. So it runs before them, while the env is whole. A collector dropped
/// later finds nothing to let go of.
///
/// napi ties each reference to the registration of the addon made last on this thread when the
/// reference was made. Once that registration's env is gone, the drop leaks the reference. Else
/// it deletes it at once while that registration is still the last one on this thread, or lets it
/// go through that registration's thread-safe function. That function is closing here only when
/// its registration came after this hook was added (a stream started in this env after the addon
/// was loaded again) and is not the last one: older napi can panic. Each reference drops on its own,
/// and that panic is caught: an unwind must not leave this extern "C" function.
unsafe extern "C" fn release_jpeg_streams(env: *mut std::ffi::c_void) {
  let streams = JPEG_STREAMS
    .try_with(|streams| streams.borrow_mut().remove(&(env as usize)))
    .ok()
    .flatten();
  for refs in streams
    .into_iter()
    .flatten()
    .filter_map(|refs| refs.upgrade())
  {
    let Some((read, reactions)) = refs
      .lock()
      .ok()
      .map(|mut refs| (refs.read.take(), refs.reactions.take()))
    else {
      continue;
    };
    let (on_value, on_error) = reactions.unzip();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(read)));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(on_value)));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(on_error)));
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
    let refs = Arc::new(Mutex::new(JpegRefs {
      read: Some(read),
      reactions: None,
      thread: std::thread::current().id(),
    }));
    watch_jpeg_refs(env, &refs);
    let (sender, items) = unbounded_channel();
    if let Ok(mut guard) = inner.lock() {
      guard.jpeg_items = Some(sender.downgrade());
    }
    let js = Arc::new(JpegStream {
      refs: refs.clone(),
      items: sender,
      inner: Arc::downgrade(inner),
      idle_check: Mutex::new(None),
      waiting: Mutex::new(None),
    });
    js.read_next(env);
    Ok(Self { items, refs })
  }
}

/// The JS thread's end of a JPEG stream's reader
struct JpegStream {
  /// The reader's read() and the reactions (see `JpegRefs`)
  refs: Arc<Mutex<JpegRefs>>,
  /// The only strong sender (see `ImageDecoderInner::jpeg_items`)
  items: UnboundedSender<JpegItem>,
  inner: Weak<Mutex<ImageDecoderInner>>,
  /// The last read, while an idle check is scheduled (see `start_jpeg_read`)
  idle_check: Mutex<Option<Arc<JpegRead>>>,
  /// The read that waits, for the reactions. One at a time: each read starts in the reaction to
  /// the last one.
  waiting: Mutex<Option<Arc<JpegRead>>>,
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
      let Ok(refs) = self.refs.lock() else {
        return;
      };
      match refs.read.as_ref() {
        Some(read) => read.borrow_back(env),
        // nothing reads on (see `JpegRefs`)
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

  /// On the JS thread: the reactions to the read that waits (`waiting`), the same two functions
  /// for every read of the stream, made at its first read. None once the JS thread let go of
  /// `read` (see `JpegRefs`): then no reaction is made, as nothing would let it go. The collector
  /// can stop on another thread between `read_next` and this call (read() runs the stream's
  /// pull(), which can close the decoder); it leaves the references to its callback, which runs on
  /// the JS thread after this call and lets go of a pair made here.
  fn reactions<'env>(self: &Arc<Self>, env: &'env Env) -> Result<Option<JpegReactionFns<'env>>> {
    let mut refs = self
      .refs
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if refs.read.is_none() {
      return Ok(None);
    }
    if refs.reactions.is_none() {
      let js = self.clone();
      let on_value = env.create_function_from_closure::<(), (), _>("onRead", move |ctx| {
        let Some(read) = js.take_waiting() else {
          return Ok(());
        };
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
        if js.settle(
          &read,
          chunk.map_err(|error| Error::new(Status::GenericFailure, error.reason.clone())),
        ) {
          js.read_next(ctx.env);
        } else {
          js.drop_reactions();
        }
        Ok(())
      })?;
      let js = self.clone();
      let on_error = env.create_function_from_closure::<(), (), _>("onReadError", move |ctx| {
        let Some(read) = js.take_waiting() else {
          return Ok(());
        };
        let message = ctx.first_arg::<Unknown>().map_or_else(
          |_| "ReadableStream read error".to_owned(),
          rejection_message,
        );
        js.settle(&read, Err(Error::new(Status::GenericFailure, message)));
        js.drop_reactions();
        Ok(())
      })?;
      refs.reactions = Some((on_value.create_ref()?, on_error.create_ref()?));
    }
    match refs.reactions.as_ref() {
      Some((on_value, on_error)) => Ok(Some((
        on_value.borrow_back(env)?,
        on_error.borrow_back(env)?,
      ))),
      None => Ok(None),
    }
  }

  /// The read that waits, for its reaction
  fn take_waiting(&self) -> Option<Arc<JpegRead>> {
    self.waiting.lock().ok().and_then(|mut read| read.take())
  }

  /// On the JS thread, when nothing reads on: let go of read() and the reactions, which hold this
  /// stream (see `JpegRefs`)
  fn drop_reactions(&self) {
    release_jpeg_refs(&self.refs);
  }
}

/// On the JS thread, with the promise of a read(): wait for its result (then read on at once),
/// and check once this task (with its microtasks) has ended whether the read still waits. Errors
/// are owned (no JS reference), as the collector may drop them on another thread. One then() with
/// both reactions: a rejected read is handled one microtask after it settles, like a chunk (see
/// `queue_jpeg_post`). The reactions are the stream's (`JpegStream::reactions`): a read makes no
/// new function, so a source that gives many chunks at once leaves no function per read behind.
fn start_jpeg_read(
  promise: Result<PromiseRaw<Object>>,
  env: &Env,
  js: Arc<JpegStream>,
  read: Arc<JpegRead>,
) {
  if let Ok(mut waiting) = js.waiting.lock() {
    *waiting = Some(read.clone());
  }
  let handlers = promise.and_then(|promise| match js.reactions(env)? {
    Some((on_value, on_error)) => then_both(promise, on_value, on_error),
    // The collector is done: nothing takes the result, but a rejection is still handled.
    None => {
      let ignore = env.create_function_from_closure::<(), (), _>("ignoreRead", |_| Ok(()))?;
      then_both(promise, ignore, ignore)
    }
  });
  if let Err(error) = handlers {
    clear_pending_exception(env);
    let reason = if error.reason.is_empty() {
      format!("ReadableStream read failed: {}", error.status)
    } else {
      error.reason.clone()
    };
    js.take_waiting();
    js.settle(&read, Err(Error::new(Status::GenericFailure, reason)));
    js.drop_reactions();
    return;
  }
  // The check runs after this task and its microtasks: a chunk or the end already queued in the
  // stream has settled the read by then. One check at most is pending, for the last read: a source
  // that enqueues at once settles many reads in one task. It runs in the loop's check phase
  // (`after_task`), out of reach of fake timers; where that is not there, from setImmediate.
  let pending = js
    .idle_check
    .lock()
    .is_ok_and(|mut last| last.replace(read).is_some());
  if pending {
    return;
  }
  let stream = Arc::downgrade(&js);
  if after_task(
    env,
    Box::new(move || {
      if let Some(stream) = stream.upgrade() {
        stream.check_idle();
      }
    }),
  ) {
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
  // A PNG or a WebP reads only what it needs of the data (see `layered_frames`)
  if inner
    .lock()
    .is_ok_and(|guard| layered_codec(guard.codec_id))
  {
    return layered_frames(inner, call, data_complete, stream_ended);
  }
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
    terminal_stream,
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
      guard.terminal_stream,
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
      None,
    );
  }

  // Chrome reads an AVIF with CrabbyAvif, and so does `avif`. FFmpeg's mov demuxer decodes
  // what it declines (a grid, ...) as before.
  // A reset clears the cache but not terminal-stream provenance. Rebuild with the final probe's
  // size hint and missing-sample rules, even when that probe's commit became stale.
  let complete = complete && !terminal_stream;
  let stream_ended = stream_ended || terminal_stream;
  let size_hint = if complete {
    data_bytes.len() as u64
  } else {
    avif::STREAM_SIZE_HINT
  };
  let avif = match codec_id {
    AVCodecID::Av1 => avif::parse(&data_bytes, prefer_animation, size_hint),
    _ => Avif::Decline,
  };
  // An AVIF's durations come from its track (see `decode_avif`)
  let track_timing = matches!(avif, Avif::Image(_));
  let (context, frames, frame_count, missing_samples) = match avif {
    Avif::Image(image) => {
      let len = data_bytes.len();
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
      // A stream that ended before the samples of a track's last frames: Chrome counts them, and
      // the decode() of one fails (CrabbyAvif's data is cut short); the frames before them decode
      let missing_samples =
        (stream_ended && image.color.is_track() && frames.len() < image.color.len())
          .then_some(image.color.len());
      (context, frames, image.color.len(), missing_samples)
    }
    // a track sample past the size hint (see `Avif::PastSizeHint`)
    Avif::PastSizeHint => return Err(reject_metadata(inner, false)),
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
      (context, frames, frame_count, None)
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
    missing_samples,
  )
}

/// Cache the frames of `data` (see `CachedFrame`), unless reset()/close() or a newer probe came
/// first. `frame_count` is the track's frame count: an AVIF track also counts the frames whose
/// samples have not arrived yet. `missing_samples`: see `ImageDecoderInner::missing_samples`.
#[allow(clippy::too_many_arguments)]
fn store_frames(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  cache_epoch: u64,
  data: &[u8],
  frames: Vec<CachedFrame>,
  frame_count: usize,
  context: Option<CodecContext>,
  repetition_count: Option<f64>,
  missing_samples: Option<usize>,
) -> Result<()> {
  before_store_frames();
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
  guard.missing_samples = missing_samples;

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
  // FFmpeg formats WebCodecs has no name for (rgb24, pal8, gray, argb, ...). PNG and WebP do
  // not come here (see `layered_frames`).
  let yuv = match codec_id {
    // an AVIF that `avif::parse` declines: a single frame is a still image
    AVCodecID::Av1 => Some(still),
    _ => None,
  };
  let source = DecodedSource::Image {
    codec: codec_id,
    alpha: None,
    yuv,
    pixels: PixelHint::None,
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

/// Whether this format is read and drawn like Chrome, frame by frame, with FFmpeg decoding each
/// frame on its own (see `layered_frames`): PNG (`apng`) and WebP (`webp`)
fn layered_codec(codec_id: Option<AVCodecID>) -> bool {
  matches!(codec_id, Some(AVCodecID::Png | AVCodecID::Webp))
}

/// What a PNG or a WebP keeps between two reads of its data. A reset() keeps it: Chrome's reset()
/// leaves its decoder as it is.
#[derive(Debug, Default)]
struct LayeredState {
  /// PNG: the CRC checks made so far
  crc: CrcMemo,
  /// WebP: what Blink's decoder keeps
  webp: WebpState,
  /// The read whose frames are being drawn, as (`cache_epoch`, its number from `draws`). From a
  /// buffer, its track is stored, and `tracks.ready` set, before the draw (see `layered_frames`).
  /// A decode() that finds no frames in that epoch waits for them (`decode_cached_frame`), rather
  /// than read the data and draw the same frames again.
  drawing: Option<(u64, u64)>,
  /// How many reads have drawn frames
  draws: u64,
  /// The data length of the read whose track is stored: the track of a read of less data (one
  /// that a reset() made stale, say) does not replace it (`store_track`)
  track_len: usize,
}

/// A read of a PNG or a WebP that draws its frames (see `LayeredState::drawing`). Dropped once
/// they are stored or dropped, also on a panic: it then wakes the decode() calls that wait.
struct LayeredDrawing<'a> {
  inner: &'a Arc<Mutex<ImageDecoderInner>>,
  id: u64,
}

impl Drop for LayeredDrawing<'_> {
  fn drop(&mut self) {
    if let Ok(mut guard) = self.inner.lock() {
      if guard.layered.drawing.is_some_and(|(_, id)| id == self.id) {
        guard.layered.drawing = None;
      }
      guard.decode_notify.notify_waiters();
    }
  }
}

/// What Blink's WEBPImageDecoder keeps between two reads of the data (UpdateDemuxer)
#[derive(Debug, Default)]
struct WebpState {
  /// Read once the demuxer has a frame (IsDecodedSizeAvailable): then fixed
  header: Option<WebpHeader>,
  /// The last read found no demuxer (an error). Blink then reads the data again at its end
  /// (`consolidated_data_` is reset), and fails if it still finds none. Until then its frame
  /// count is 0 for a decode(): after the header, every decode() waits.
  lost: bool,
  /// The data length of the last read. A read of the same data only reads it again when the
  /// last one was lost: Blink fails a file cut short only on a read of new data with all the
  /// data (`truncated_file`).
  parsed_len: usize,
  /// The last read found the frame of a still image with only its image chunk header, not the
  /// start of its data: libwebp's demuxer has it with frame number 0, so Blink finds no frame 1
  /// (WebPDemuxGetFrame) and fails a decode() made now. The frame itself stays pending: once
  /// more data comes, it decodes.
  unnumbered: bool,
}

/// A WebP header as Blink reads it, once
#[derive(Clone, Copy, Debug)]
struct WebpHeader {
  /// The demuxer's format flags (WEBP_FF_FORMAT_FLAGS)
  flags: u32,
  /// The track's repetition count (Infinity, n, or 0 for kAnimationLoopOnce and
  /// kAnimationNone), or None for a still image
  repetition: Option<f64>,
  /// A simple lossy file (first chunk VP8): Chrome decodes it to YUV planes once all the data is
  /// here (IsSimpleLossyWebPImage)
  simple_lossy: bool,
}

/// Whether a read of a PNG or a WebP draws the frames of `epoch` now, with none stored yet
/// (`LayeredState::drawing`)
fn drawing_for(guard: &ImageDecoderInner, epoch: u64) -> bool {
  guard.cached_frames.is_none()
    && guard.cache_epoch == epoch
    && guard
      .layered
      .drawing
      .is_some_and(|(drawing, _)| drawing == epoch)
}

/// Ends once the decoder closed on a failure (see `closed_on_frame_failure`)
async fn failure_closed(inner: &Arc<Mutex<ImageDecoderInner>>) {
  let Ok(notify) = inner.lock().map(|guard| guard.decode_notify.clone()) else {
    return;
  };
  loop {
    // Registered before the check: `ImageDecoderInner::close` notifies the waiters
    let mut notified = std::pin::pin!(notify.notified());
    notified.as_mut().enable();
    if closed_on_frame_failure(inner) {
      return;
    }
    notified.await;
  }
}

/// Whether the decoder closed on a failure: the metadata, a frame that fails a WebP, or every
/// frame of a PNG
fn closed_on_frame_failure(inner: &Arc<Mutex<ImageDecoderInner>>) -> bool {
  inner
    .lock()
    .is_ok_and(|guard| guard.closed && (guard.failed() || guard.frames_failed))
}

/// What the read of a PNG or a WebP found
enum LayeredRead {
  /// The header has not arrived: Chrome waits
  Wait,
  /// Chrome fails the metadata (`at_end`: the data is complete, see `reject_metadata`)
  RejectMetadata { at_end: bool },
  /// Chrome fails the decoder after its header, once the data is complete: `completed` resolves,
  /// the decoder closes, and the decode() calls still pending reject with EncodingError
  FailDecoder,
  /// The header is here, nothing changed: the track and the frames stay as they are
  Keep,
  /// The track and the frames to decode
  Frames(LayeredPlan),
}

/// A frame of a `LayeredPlan`
enum PlannedFrame {
  /// As it is
  Ready(CachedFrame),
  /// Decode this still image (a PNG, or a WebP) and draw it
  Decode(Vec<u8>),
}

/// The track, and what each frame needs, read under the decoder's lock
struct LayeredPlan {
  epoch: u64,
  data_len: usize,
  track: ImageTrackData,
  frames: Vec<PlannedFrame>,
  draw: LayeredDraw,
}

/// How the frames of a `LayeredPlan` are drawn
enum LayeredDraw {
  Png(apng::PngLayout),
  /// A still WebP: its alpha flag, and whether Chrome decodes it to YUV planes
  WebpStill {
    alpha: bool,
    yuv: bool,
  },
  /// An animated WebP
  WebpAnim {
    width: u32,
    height: u32,
    frames: Vec<webp::AnimFrame>,
    alpha: Vec<bool>,
  },
}

/// The frames of a PNG or a WebP, read and drawn like Chrome (see `apng` and `webp`), and its
/// track. The header is the metadata, and Chrome decodes a frame only for a decode(). Here the
/// read first parses all the data so far (for a PNG, checking every chunk's CRC) and copies the
/// data of each whole frame not decoded yet. With all the data from the start (a buffer), it then
/// stores the track and sets `tracks.ready`, under the same lock, before any frame is drawn; so
/// much data at once sets it later than in Chrome, whose decoder stops at the header. On a
/// stream, the track is stored once the frames are drawn, with them, and the collector sets
/// `tracks.ready` after that read (`signal_tracks_ready`): a chunk with no whole frame (the header
/// alone) sets it at once, a chunk with the header and whole frames only once those are drawn. A
/// close() or a reset() stops the draw before its next frame, and its frames are dropped; after a
/// reset(), a stream's read still stores the track. Every frame is decoded once its data is
/// complete, with FFmpeg, on its own; a read of more data decodes only the frames that became
/// complete. While more data can come, a frame whose data is not complete is pending; once the
/// data is complete, it fails. A bare VP8 or VP8L bitstream (a WebP with no chunk header) has no
/// size: its frame stays pending until the end of the stream (`webp::image_complete`).
fn layered_frames(
  inner: &Arc<Mutex<ImageDecoderInner>>,
  call: FrameCall,
  data_complete: bool,
  stream_ended: bool,
) -> Result<()> {
  // Whether the track is stored and `tracks.ready` set before the draw: a read with all the data
  // that is not the last read of a stream (see above)
  let early = data_complete && !stream_ended;
  let (read, drawing) = {
    let mut guard = inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    if guard.closed {
      return Err(invalid_state_error("ImageDecoder is closed"));
    }
    // A decode() whose read finds a read of its epoch drawing the frames (one that started after
    // the decode() looked, `decode_cached_frame`) does not read: it waits for those frames
    if let FrameCall::Decode(epoch) = call
      && drawing_for(&guard, epoch)
    {
      return Ok(());
    }
    let read = if guard.codec_id == Some(AVCodecID::Png) {
      read_png(&mut guard, data_complete, stream_ended)
    } else {
      read_webp(&mut guard, data_complete)
    };
    let drawing = match &read {
      LayeredRead::Frames(plan) => {
        // All the data from the start (a buffer; a decode() made after a reset() once a stream
        // ended): the data does not change, so no other read stores another track
        if early {
          store_track(&mut guard, plan.data_len, &plan.track);
          set_tracks_ready(&guard);
        }
        guard.layered.draws += 1;
        let id = guard.layered.draws;
        guard.layered.drawing = Some((plan.epoch, id));
        Some(LayeredDrawing { inner, id })
      }
      _ => None,
    };
    (read, drawing)
  };
  let plan = match read {
    LayeredRead::Wait => return Err(incomplete_data()),
    LayeredRead::RejectMetadata { at_end } => return Err(reject_metadata(inner, at_end)),
    LayeredRead::FailDecoder => {
      if let Ok(mut guard) = inner.lock()
        && !guard.closed
      {
        // Chrome resolves `completed` first (OnMetadata), then fails
        guard.complete.store(true, Ordering::Release);
        guard.frames_failed = true;
        guard.close();
      }
      return Err(frame_failure(0));
    }
    LayeredRead::Keep => return Ok(()),
    LayeredRead::Frames(plan) => plan,
  };
  before_layered_draw();
  // A close() or a reset() stops the draw before its next frame
  let frames = draw_frames(plan.frames, &plan.draw, || {
    inner.lock().map_or(true, |guard| {
      guard.closed || guard.cache_epoch != plan.epoch
    })
  });
  // Stored or dropped with the lock below held; `drawing` drops after it, and wakes the decode()
  // calls that wait for these frames
  let _drawing = drawing;
  let mut guard = inner
    .lock()
    .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
  if guard.closed {
    return Err(invalid_state_error("ImageDecoder is closed"));
  }
  // A stream's read: the track of the data it read, also when a reset() came and its frames are
  // dropped. The track comes from the layout, and Chrome's reset() keeps it; the collector then
  // sets `tracks.ready` as after any read.
  if !early {
    store_track(&mut guard, plan.data_len, &plan.track);
  }
  // reset() came, or a read of more data stored its frames first
  let Some(frames) = frames else {
    return Ok(());
  };
  if guard.cache_epoch != plan.epoch || plan.data_len < guard.cached_data_len {
    return Ok(());
  }
  guard.cached_frames = Some(frames);
  guard.context = None;
  guard.cached_data_len = plan.data_len;
  guard.missing_samples = None;
  Ok(())
}

/// Store the track of a PNG or a WebP read from `data_len` bytes, under the decoder's lock, unless
/// the stored one is from more data (`LayeredState::track_len`)
fn store_track(guard: &mut ImageDecoderInner, data_len: usize, track: &ImageTrackData) {
  if data_len < guard.layered.track_len {
    return;
  }
  guard.layered.track_len = data_len;
  if let Ok(mut tracks) = guard.tracks.inner.lock()
    && let Some(stored) = tracks.tracks.get_mut(0)
  {
    *stored = track.clone();
  }
}

/// What Chrome reports for a track: animated when it has more than one frame or a repetition
/// (ImageTrack), and its repetition count
fn layered_track(frame_count: usize, repetition: Option<f64>) -> ImageTrackData {
  ImageTrackData {
    animated: frame_count > 1 || repetition.is_some(),
    frame_count: frame_count as u32,
    repetition_count: repetition.unwrap_or(0.0),
  }
}

/// The frame cached for `i` that a read of more data keeps: a decoded frame, or a failure
fn kept_frame(cached: Option<&Vec<CachedFrame>>, i: usize) -> Option<CachedFrame> {
  cached
    .and_then(|frames| frames.get(i))
    .filter(|frame| !matches!(frame, CachedFrame::Pending))
    .cloned()
}

/// Read a PNG like Chrome (see `apng`), under the decoder's lock
fn read_png(guard: &mut ImageDecoderInner, data_complete: bool, stream_ended: bool) -> LayeredRead {
  let ImageDecoderInner {
    data,
    layered,
    cached_frames,
    cache_epoch,
    ..
  } = guard;
  let ImageDecoderData::Vec(data) = data else {
    return LayeredRead::Wait;
  };
  let mut layout = match apng::parse_with(data, &mut layered.crc) {
    // Chrome fails a header that the data ends before
    PngParse::Incomplete if data_complete => return LayeredRead::RejectMetadata { at_end: true },
    PngParse::Incomplete => return LayeredRead::Wait,
    PngParse::Bad => {
      return LayeredRead::RejectMetadata {
        at_end: stream_ended,
      };
    }
    PngParse::Ready(layout) => layout,
  };
  layout.resolve();
  let repetition = match layout.repetition {
    apng::Repetition::None => None,
    apng::Repetition::Once => Some(0.0),
    apng::Repetition::Infinite => Some(f64::INFINITY),
    apng::Repetition::Count(n) => Some(f64::from(n)),
  };
  let mut frames: Vec<PlannedFrame> = Vec::with_capacity(layout.frames.len());
  let failed = |frames: &[PlannedFrame], r: Option<usize>| {
    r.filter(|&r| matches!(frames[r], PlannedFrame::Ready(CachedFrame::Failed { .. })))
  };
  for (i, frame) in layout.frames.iter().enumerate() {
    let required = failed(&frames, frame.required);
    let planned = match frame.state {
      apng::DataState::Failed => CachedFrame::Failed { required },
      apng::DataState::Incomplete if data_complete => CachedFrame::Failed { required },
      apng::DataState::Incomplete => CachedFrame::Pending,
      apng::DataState::Complete => match kept_frame(cached_frames.as_ref(), i) {
        Some(kept) => kept,
        // drawn on a frame that fails: it fails too
        None if required.is_some() => CachedFrame::Failed { required },
        None => {
          frames.push(PlannedFrame::Decode(layout.still_png(data, i)));
          continue;
        }
      },
    };
    frames.push(PlannedFrame::Ready(planned));
  }
  LayeredRead::Frames(LayeredPlan {
    epoch: *cache_epoch,
    data_len: data.len(),
    track: layered_track(layout.frames.len(), repetition),
    frames,
    draw: LayeredDraw::Png(layout),
  })
}

/// Read a WebP like Chrome (Blink's WEBPImageDecoder::UpdateDemuxer over `webp::demux`), under
/// the decoder's lock
fn read_webp(guard: &mut ImageDecoderInner, data_complete: bool) -> LayeredRead {
  let rgb_decode = guard.rgb_decode;
  let ImageDecoderInner {
    data,
    layered,
    cached_frames,
    cache_epoch,
    ..
  } = guard;
  let ImageDecoderData::Vec(data) = data else {
    return LayeredRead::Wait;
  };
  let state = &mut layered.webp;
  let fail = |state: &WebpState| match state.header {
    None => LayeredRead::RejectMetadata { at_end: true },
    Some(_) => LayeredRead::FailDecoder,
  };
  let (demux, demux_state) = webp::demux(data);
  // A bare bitstream can be valid with fewer than 20 bytes; RIFF and raw chunks still wait for
  // their header and chunk header.
  if data.len() < 20
    && !demux
      .as_ref()
      .is_some_and(|demux| webp::bare_bitstream(data, demux))
  {
    return if data_complete {
      fail(state)
    } else {
      LayeredRead::Wait
    };
  }
  let fresh = data.len() != state.parsed_len || state.lost;
  state.parsed_len = data.len();
  let truncated = data_complete && fresh && demux_state != DemuxState::Done;
  let demux = match demux {
    Some(demux) if demux_state >= DemuxState::ParsedHeader && !truncated => demux,
    demux => {
      state.lost = demux.is_none();
      if truncated {
        return fail(state);
      }
      // ImageDecoderCore: no header once all the data is here fails the metadata
      if state.header.is_none() && data_complete {
        return LayeredRead::RejectMetadata { at_end: true };
      }
      return if state.header.is_some() {
        LayeredRead::Keep
      } else {
        LayeredRead::Wait
      };
    }
  };
  state.lost = false;
  state.unnumbered = false;
  if demux.frames.is_empty() {
    // the frame data has not arrived
    if state.header.is_none() && data_complete {
      return LayeredRead::RejectMetadata { at_end: true };
    }
    return LayeredRead::Wait;
  }
  let (width, height) = (demux.canvas_width as u32, demux.canvas_height as u32);
  let header = match state.header {
    Some(header) => header,
    None => {
      // ImageDecoder::SetSize: 4 bytes a pixel must fit an int32
      if u64::from(width) * u64::from(height) * 4 > i32::MAX as u64 {
        return LayeredRead::RejectMetadata {
          at_end: data_complete,
        };
      }
      let flags = demux.flags;
      let repetition = (flags & webp::ANIMATION_FLAG != 0).then(|| match demux.loop_count {
        0 => f64::INFINITY,
        n => f64::from(n - 1),
      });
      // a still image without alpha: the features of the data so far must read
      if flags & (webp::ANIMATION_FLAG | webp::ALPHA_FLAG) == 0 && webp::features(data).is_err() {
        return LayeredRead::RejectMetadata {
          at_end: data_complete,
        };
      }
      let header = WebpHeader {
        flags,
        repetition,
        simple_lossy: webp_is_simple_lossy(data),
      };
      state.header = Some(header);
      header
    }
  };
  let count = demux.frames.len();
  let track = layered_track(count, header.repetition);
  let epoch = *cache_epoch;
  let data_len = data.len();
  // Blink fails the decode of the last frame once all the data is here and the file is cut short
  // (WEBPImageDecoder::Decode), but not on its YUV path
  let truncated_last = data_complete && demux_state != DemuxState::Done;
  if header.flags & webp::ANIMATION_FLAG == 0 {
    let yuv = header.simple_lossy && !rgb_decode;
    let frame = &demux.frames[0];
    let planned = if frame.frame_num == 0 && !data_complete {
      // The image chunk header only: Blink finds no frame 1 in the demuxer (WebPDemuxGetFrame),
      // and fails a decode() made now (`WebpState::unnumbered`). The frame waits for more data.
      state.unnumbered = true;
      PlannedFrame::Ready(CachedFrame::Pending)
    } else if frame.frame_num == 0 || (truncated_last && !yuv) {
      PlannedFrame::Ready(CachedFrame::Failed { required: None })
    } else if webp::image_complete(data, &demux, data_complete) {
      match kept_frame(cached_frames.as_ref(), 0) {
        Some(kept) => PlannedFrame::Ready(kept),
        None => PlannedFrame::Decode(webp::image_still(data, &demux)),
      }
    } else if data_complete {
      PlannedFrame::Ready(CachedFrame::Failed { required: None })
    } else {
      PlannedFrame::Ready(CachedFrame::Pending)
    };
    return LayeredRead::Frames(LayeredPlan {
      epoch,
      data_len,
      track,
      frames: vec![planned],
      draw: LayeredDraw::WebpStill {
        alpha: header.flags & webp::ALPHA_FLAG != 0,
        yuv,
      },
    });
  }
  let anim = webp::anim_frames(&demux);
  let alpha = webp::frame_alpha(&anim, header.flags, width, height);
  let mut frames: Vec<PlannedFrame> = Vec::with_capacity(count);
  for (i, frame) in demux.frames.iter().enumerate() {
    let required = anim[i]
      .required
      .filter(|&r| matches!(frames[r], PlannedFrame::Ready(CachedFrame::Failed { .. })));
    let planned = if (truncated_last && i + 1 == count) || !frame.complete {
      // Blink fails a counted frame whose data is not complete (WebPIUpdate)
      CachedFrame::Failed { required }
    } else if let Some(kept) = kept_frame(cached_frames.as_ref(), i) {
      kept
    } else if required.is_some() {
      CachedFrame::Failed { required }
    } else {
      frames.push(PlannedFrame::Decode(webp::frame_still(data, frame)));
      continue;
    };
    frames.push(PlannedFrame::Ready(planned));
  }
  LayeredRead::Frames(LayeredPlan {
    epoch,
    data_len,
    track,
    frames,
    draw: LayeredDraw::WebpAnim {
      width,
      height,
      frames: anim,
      alpha,
    },
  })
}

/// Decode the frames a `LayeredPlan` asks for and draw them on their canvas, in order: a frame
/// drawn on another one needs that one's canvas. Before it decodes a frame it asks `stopped`
/// (a close() or a reset() came): then it stops, and returns None, dropping what it drew.
fn draw_frames(
  planned: Vec<PlannedFrame>,
  draw: &LayeredDraw,
  stopped: impl Fn() -> bool,
) -> Option<Vec<CachedFrame>> {
  let mut frames: Vec<CachedFrame> = Vec::with_capacity(planned.len());
  let mut timestamp = 0i64;
  for (i, planned) in planned.into_iter().enumerate() {
    // Chrome gives every frame of an animation its duration, and starts it when the frames
    // before it end
    let duration = match draw {
      LayeredDraw::Png(layout) => (layout.repetition != apng::Repetition::None)
        .then(|| i64::from(layout.frames[i].duration_ms) * 1000),
      LayeredDraw::WebpStill { .. } => None,
      LayeredDraw::WebpAnim { frames, .. } => Some(i64::from(frames[i].duration_ms) * 1000),
    };
    let frame = match planned {
      PlannedFrame::Ready(frame) => frame,
      PlannedFrame::Decode(input) => {
        before_layered_frame();
        if stopped() {
          return None;
        }
        draw_frame(&frames, i, &input, draw, timestamp, duration).unwrap_or_else(|failed| failed)
      }
    };
    frames.push(frame);
    timestamp += duration.unwrap_or(0);
  }
  Some(frames)
}

/// Decode frame `i` and draw it (see `draw_frames`). Err: the failure to cache.
fn draw_frame(
  frames: &[CachedFrame],
  i: usize,
  input: &[u8],
  draw: &LayeredDraw,
  pts: i64,
  duration: Option<i64>,
) -> std::result::Result<CachedFrame, CachedFrame> {
  let fail = || CachedFrame::Failed { required: None };
  // The canvas of the frame drawn on, read from its cached frame
  let base = |required: Option<usize>| match required {
    None => Ok(None),
    Some(r) => match &frames[r] {
      CachedFrame::Decoded { frame, .. } => Ok(Some(canvas_bytes(&frame.read()))),
      _ => Err(CachedFrame::Failed {
        required: Some(r).filter(|&r| matches!(frames[r], CachedFrame::Failed { .. })),
      }),
    },
  };
  let decoded = |codec, source| {
    decode_still(codec, input)
      .ok()
      .and_then(|frame| decoded_frame_format(frame, source).ok())
  };
  let (frame, format) = match draw {
    LayeredDraw::Png(layout) => {
      let info = &layout.frames[i];
      let base = base(info.required)?;
      let source = DecodedSource::Image {
        codec: AVCodecID::Png,
        alpha: Some(layout.header.alpha),
        yuv: None,
        pixels: layout.header.pixels,
      };
      let (sub, _) = decoded(AVCodecID::Png, source).ok_or_else(fail)?;
      let rect = info.rect;
      if (sub.width(), sub.height()) != (rect.x1 - rect.x0, rect.y1 - rect.y0) {
        return Err(fail());
      }
      let mut canvas = layout.start(i, info.required.zip(base.as_deref()));
      let pixels = sub.plane_data(0).ok_or_else(fail)?;
      layout.draw(i, &mut canvas, pixels, sub.linesize(0) as usize);
      let format = if info.has_alpha {
        VideoPixelFormat::BGRA
      } else {
        VideoPixelFormat::BGRX
      };
      let frame = canvas_frame(layout.width, layout.height, &canvas, &sub, pts).ok_or_else(fail)?;
      (frame, format)
    }
    LayeredDraw::WebpStill { alpha, yuv } => {
      let source = DecodedSource::Image {
        codec: AVCodecID::Webp,
        alpha: Some(*alpha),
        yuv: Some(*yuv),
        pixels: PixelHint::None,
      };
      let (mut frame, format) = decoded(AVCodecID::Webp, source).ok_or_else(fail)?;
      frame.set_pts(0);
      (frame, format)
    }
    LayeredDraw::WebpAnim {
      width,
      height,
      frames: anim,
      alpha,
    } => {
      let info = &anim[i];
      let base = base(info.required)?;
      // every frame's own alpha (an ALPH chunk, or VP8L's), blended by Blink
      let source = DecodedSource::Image {
        codec: AVCodecID::Webp,
        alpha: Some(true),
        yuv: Some(false),
        pixels: PixelHint::None,
      };
      let (sub, _) = decoded(AVCodecID::Webp, source).ok_or_else(fail)?;
      let rect = info.rect;
      if (sub.width(), sub.height()) != (rect.x1 - rect.x0, rect.y1 - rect.y0) {
        return Err(fail());
      }
      let mut canvas = webp::start_canvas(anim, i, *width, *height, base.as_deref());
      let pixels = sub.plane_data(0).ok_or_else(fail)?;
      webp::draw(
        anim,
        i,
        *width,
        &mut canvas,
        pixels,
        sub.linesize(0) as usize,
        base.as_deref(),
      );
      let format = if alpha[i] {
        VideoPixelFormat::BGRA
      } else {
        VideoPixelFormat::BGRX
      };
      let frame = canvas_frame(*width, *height, &canvas, &sub, pts).ok_or_else(fail)?;
      (frame, format)
    }
  };
  Ok(CachedFrame::Decoded {
    frame: frame.into_shared(),
    format,
    duration,
  })
}

/// Decode a still image (a PNG or a WebP of one frame) with FFmpeg, on this thread. Any error,
/// or anything but one frame, fails. A PNG whose image data ends early fails too, like the png
/// crate (FFmpeg keeps an image with up to 95% of its rows missing by default).
fn decode_still(codec_id: AVCodecID, data: &[u8]) -> Result<Frame> {
  let failed = |e: CodecError| Error::new(Status::GenericFailure, format!("Decode failed: {e}"));
  let mut context = CodecContext::new_decoder(codec_id).map_err(failed)?;
  context
    .configure_decoder(&DecoderConfig {
      codec_id,
      thread_count: 1,
      extradata: None,
      low_latency: false,
      width: None,
      height: None,
    })
    .map_err(failed)?;
  if codec_id == AVCodecID::Png {
    // SAFETY: the context is a valid AVCodecContext that is not opened yet
    let set = unsafe {
      av_opt_set_int(
        context.as_mut_ptr().cast(),
        c"discard_damaged_percentage".as_ptr(),
        0,
        0,
      )
    };
    if set < 0 {
      return Err(Error::new(
        Status::GenericFailure,
        "Failed to configure the PNG decoder",
      ));
    }
  }
  context.open().map_err(failed)?;
  let mut packet = Packet::new().map_err(failed)?;
  packet.copy_data_from(data).map_err(failed)?;
  let mut frames = context.decode(Some(&packet)).map_err(failed)?;
  frames.extend(context.flush_decoder().map_err(failed)?);
  match <[Frame; 1]>::try_from(frames) {
    Ok([frame]) => Ok(frame),
    Err(_) => Err(Error::new(
      Status::GenericFailure,
      "Decode failed: not one frame",
    )),
  }
}

/// The rows of a BGRA frame, packed
fn canvas_bytes(frame: &Frame) -> Vec<u8> {
  let (row, stride) = (frame.width() as usize * 4, frame.linesize(0) as usize);
  let mut out = Vec::with_capacity(row * frame.height() as usize);
  if let Some(data) = frame.plane_data(0) {
    for line in data.chunks(stride).take(frame.height() as usize) {
      out.extend_from_slice(&line[..row]);
    }
  }
  out
}

/// A BGRA frame of `width` x `height` with the packed rows of `canvas`, the properties of the
/// decoded frame `props` (colour tags, side data), and `pts`. None without memory for it.
fn canvas_frame(width: u32, height: u32, canvas: &[u8], props: &Frame, pts: i64) -> Option<Frame> {
  let mut frame = Frame::new_video(width, height, AVPixelFormat::Bgra).ok()?;
  // SAFETY: both frames are valid
  if unsafe { av_frame_copy_props(frame.as_mut_ptr(), props.as_ptr()) } < 0 {
    return None;
  }
  let (row, stride) = (width as usize * 4, frame.linesize(0) as usize);
  let rows = frame.plane_data_mut(0)?.chunks_mut(stride);
  for (to, from) in rows.zip(canvas.chunks_exact(row)) {
    to[..row].copy_from_slice(from);
  }
  frame.set_pts(pts);
  frame.set_duration(0);
  frame.set_colorspace(AVColorSpace::Rgb);
  frame.set_color_range(AVColorRange::Jpeg);
  Some(frame)
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

/// Runs before a frame cache commit: the tests make a reset() land there so the commit is stale.
#[cfg(test)]
fn before_store_frames() {
  tests::BEFORE_STORE_FRAMES.with(|hook| {
    if let Some(hook) = hook.borrow_mut().as_mut() {
      hook();
    }
  });
}

/// Runs just before a read of a PNG or a WebP draws its frames (from a buffer, once it has stored
/// the track): the tests look at the decoder there, and make a decode() land there.
#[cfg(test)]
fn before_layered_draw() {
  tests::BEFORE_LAYERED_DRAW.with(|hook| {
    if let Some(hook) = hook.borrow_mut().as_mut() {
      hook();
    }
  });
}

#[cfg(not(test))]
fn before_store_frames() {}
#[cfg(not(test))]
fn before_layered_draw() {}

/// Runs in `draw_frames` before each frame it decodes, before it looks for a close() or a
/// reset(): the tests make one land there, while the frames are drawn.
#[cfg(test)]
fn before_layered_frame() {
  tests::BEFORE_LAYERED_FRAME.with(|hook| {
    if let Some(hook) = hook.borrow_mut().as_mut() {
      hook();
    }
  });
}

#[cfg(not(test))]
fn before_layered_frame() {}

/// Runs in `decode_cached_frame` after its first look, just before a decode() reads the data
/// itself: the tests make a read start its draw there.
#[cfg(test)]
fn before_decode_read() {
  tests::BEFORE_DECODE_READ.with(|hook| {
    if let Some(hook) = hook.borrow_mut().as_mut() {
      hook();
    }
  });
}

#[cfg(not(test))]
fn before_decode_read() {}

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
  use super::{jpeg_scale_numerator, normalize_frame_timing, webp_is_simple_lossy};
  use crate::codec::Frame;
  use crate::ffi::types::AV_NOPTS_VALUE;

  use super::{
    AVCodecID, ColorSpaceConversion, DecodeTurn, FrameCall, ImageDecoderData, ImageDecoderInner,
    ImageTrackData, ImageTrackList, ImageTrackListInner, JpegPost, abort_if_reset,
    apply_frame_failure, begin_terminal_refresh, cache_decoded_frames, close_decoder,
    decode_cached_frame, end_with_frame_failure, finish_jpeg_post, frame_failure, post_before_end,
    reset_or_closed, take_frame_failure,
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
    /// See `before_store_frames`
    pub(super) static BEFORE_STORE_FRAMES: RefCell<Option<Box<dyn FnMut()>>> =
      const { RefCell::new(None) };
    /// See `before_decode_read`
    pub(super) static BEFORE_DECODE_READ: RefCell<Option<Box<dyn FnMut()>>> =
      const { RefCell::new(None) };
    /// See `before_layered_draw`
    pub(super) static BEFORE_LAYERED_DRAW: RefCell<Option<Box<dyn FnMut()>>> =
      const { RefCell::new(None) };
    /// See `before_layered_frame`
    pub(super) static BEFORE_LAYERED_FRAME: RefCell<Option<Box<dyn FnMut()>>> =
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
      jpeg_fill: None,
      frame_failed: None,
      end_deferred: false,
      terminal_stream: false,
      missing_samples: None,
      layered: super::LayeredState::default(),
      tracks,
      decode_notify: Arc::new(Notify::new()),
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

  /// What `ImageDecoder::close` does to the decoder (the method needs an Env)
  fn close(inner: &Arc<Mutex<ImageDecoderInner>>) {
    close_decoder(inner).unwrap();
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

  /// A JPEG that decodes
  const JPEG_420: &[u8] = include_bytes!("../../__test__/fixtures/jpeg/420-37x9.jpg");

  /// `decode_cached_frame` of frame 0 for a decode() made at `epoch`, on another thread (no hook
  /// there): "pending", "frame" or the error reason
  fn decode_elsewhere(inner: &Arc<Mutex<ImageDecoderInner>>, epoch: u64) -> String {
    let inner = inner.clone();
    std::thread::spawn(move || match decode_cached_frame(&inner, 0, epoch) {
      Ok(None) => "pending".to_string(),
      Ok(Some(_)) => "frame".to_string(),
      Err(error) => error.reason,
    })
    .join()
    .unwrap()
  }

  /// A decode() of a buffered JPEG made while another one's libjpeg decode runs, and what the
  /// first one returns. Checks that the second one waits and is woken when the first one ends.
  fn decode_during_another(data: &[u8]) -> (Arc<Mutex<ImageDecoderInner>>, String) {
    let inner = jpeg_decoder(data);
    let notify = inner.lock().unwrap().decode_notify.clone();
    let mut notified = std::pin::pin!(notify.notified());
    notified.as_mut().enable();
    let second = Rc::new(RefCell::new(String::new()));
    let (hook_inner, hook_second) = (inner.clone(), second.clone());
    let (first, ran) = decode_with_hook(&inner, 0, 0, move || {
      *hook_second.borrow_mut() = decode_elsewhere(&hook_inner, 0);
    });
    assert!(ran);
    // Like Chrome's decoder thread, which decodes the frame once: the second call runs no libjpeg
    assert_eq!(*second.borrow(), "pending");
    // The end of the first call's decode wakes it
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(notified.as_mut().poll(&mut cx).is_ready());
    assert_eq!(inner.lock().unwrap().jpeg_fill, None);
    (inner, first)
  }

  #[test]
  fn a_jpeg_decode_made_while_another_decodes_takes_its_frame() {
    let (inner, first) = decode_during_another(JPEG_420);
    assert_eq!(first, "ok");
    let (second, ran) = decode_with_hook(&inner, 0, 0, || {});
    assert!(!ran);
    assert_eq!(second, "ok");
  }

  #[test]
  fn a_jpeg_decode_made_while_another_decodes_takes_its_frame_failure() {
    let (inner, first) = decode_during_another(TRUNCATED_JPEG);
    assert_eq!(first, "Failed to decode frame at index 0");
    let (second, ran) = decode_with_hook(&inner, 0, 0, || {});
    assert!(!ran);
    assert_eq!(second, "Failed to decode frame at index 0");
    // Still only recorded: a decode() closes the decoder on the JS thread
    assert!(!inner.lock().unwrap().closed);
  }

  #[test]
  fn a_jpeg_decode_made_after_a_reset_does_not_wait_for_the_decode_before_it() {
    let inner = jpeg_decoder(JPEG_420);
    let second = Rc::new(RefCell::new(String::new()));
    let (hook_inner, hook_second) = (inner.clone(), second.clone());
    let (_, ran) = decode_with_hook(&inner, 0, 0, move || {
      reset(&hook_inner);
      *hook_second.borrow_mut() = decode_elsewhere(&hook_inner, 1);
    });
    assert!(ran);
    assert_eq!(*second.borrow(), "frame");
    assert_eq!(
      reason(abort_if_reset(&inner, 0)),
      "AbortError: Aborted by reset."
    );
    assert_eq!(inner.lock().unwrap().jpeg_fill, None);
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

  /// What a try of the data so far reports (`jpeg_frame` on a stream that has not ended): a
  /// frame and its bytes, the data ends before the image does (with or without rows), or a
  /// failure
  fn jpeg_try(data: &[u8]) -> String {
    use crate::codec::jpeg::{JpegDecode, decode_jpeg};
    use std::hash::{DefaultHasher, Hash, Hasher};
    match decode_jpeg(data, 8, false) {
      Ok(JpegDecode::Frame(frame)) => {
        let mut bytes = vec![0u8; frame.width() as usize * frame.height() as usize * 4];
        let size = frame.copy_to_buffer(&mut bytes).unwrap();
        let mut hasher = DefaultHasher::new();
        bytes[..size].hash(&mut hasher);
        format!("frame {:016x}", hasher.finish())
      }
      Ok(JpegDecode::Incomplete { rows_started }) => format!("incomplete, rows {rows_started}"),
      Err(error) => format!("failed: {error:?}"),
    }
  }

  /// The JPEG fixtures, read at run time so that new ones are checked too
  fn jpeg_fixtures() -> Vec<(String, Vec<u8>)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/__test__/fixtures/jpeg");
    let mut files: Vec<_> = std::fs::read_dir(dir)
      .unwrap()
      .map(|entry| {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        (name, std::fs::read(&path).unwrap())
      })
      .collect();
    files.sort();
    assert!(files.len() >= 17);
    files
  }

  // `JpegRetries` skips a try whose new bytes `JpegScan` calls quiet. Such a try must report what
  // the try before it reported: for every fixture (good, cut, failing, progressive, 99 and 100
  // scans, two images) and chunk size, the bytes it calls quiet never change what libjpeg reports.
  #[test]
  fn a_jpeg_try_on_quiet_bytes_reports_what_the_last_try_reported() {
    for (name, data) in jpeg_fixtures() {
      for chunk in [1, 13, 64] {
        let mut scan = super::JpegScan::default();
        let mut last: Option<String> = None;
        let mut quiet = 0;
        let mut start = 0;
        while start < data.len() {
          let end = (start + chunk).min(data.len());
          let changed = scan.feed(&data[start..end]);
          let report = jpeg_try(&data[..end]);
          if !changed {
            quiet += end - start;
            assert_eq!(
              Some(&report),
              last.as_ref(),
              "{name}, chunks of {chunk}, at {end}"
            );
          }
          last = Some(report);
          start = end;
        }
        // The data of a scan is quiet: most of the bytes of these files
        if name == "cmyk-33x17.jpg" {
          assert!(
            quiet > 1400,
            "{name}, chunks of {chunk}: {quiet} quiet bytes"
          );
        }
      }
    }
  }

  #[test]
  fn jpeg_scan_counts_markers_segments_and_the_dc_first_limit() {
    let mut scan = super::JpegScan::default();
    // SOI, a COM segment whose payload looks like markers, then SOF2 and a DC first SOS
    assert!(scan.feed(&[0xFF, 0xD8, 0xFF, 0xFE, 0x00, 0x06, 0xFF, 0xD9, 0xFF, 0xDA]));
    assert!(scan.feed(&[0xFF, 0xC2, 0x00, 0x02]));
    assert!(scan.feed(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x00, 0x01]));
    // Its data, stuffed bytes, fill bytes and restart markers: quiet up to 128 KiB
    assert!(!scan.feed(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xFF, 0xD3, 0x56]));
    assert!(!scan.feed(&vec![0x55; super::DC_FIRST_QUIET - 8]));
    assert!(scan.feed(&[0x55]));
    // A marker that ends the scan, then an AC scan (Ss 1): quiet however long
    assert!(scan.feed(&[0xFF, 0xC4, 0x00, 0x02]));
    assert!(scan.feed(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x01, 0x3F, 0x01]));
    assert!(!scan.feed(&vec![0x55; super::DC_FIRST_QUIET * 2]));
    // A marker split across chunks counts in the chunk that ends it
    assert!(!scan.feed(&[0x55, 0xFF]));
    assert!(scan.feed(&[0xD9]));
    // Nothing after EOI is read
    assert!(!scan.feed(&[0xFF, 0xD8, 0xFF, 0xDA, 0x00]));
  }

  #[test]
  fn jpeg_scan_quiets_complete_saved_app_segments() {
    let inner = jpeg_decoder(include_bytes!("../../__test__/fixtures/jpeg/420-37x9.jpg"));
    let mut retries = super::JpegRetries::default();
    retries.chunk(&[0xFF, 0xD8]);
    assert!(retries.run(&inner, 0));

    // Split every byte, including the marker prefix, length, and payload. The
    // payload deliberately contains bytes that look like structural markers.
    for code in [0xE1, 0xE2, 0xEB] {
      for byte in [0xFF, code, 0x00, 0x06, 0xFF, 0xD9, 0x00, 0x11] {
        retries.chunk(&[byte]);
        assert!(
          !retries.run(&inner, 0),
          "saved APP {code:02X} was not quiet"
        );
      }
    }

    // A malformed saved-APP length still reports libjpeg's failure.
    retries.chunk(&[0xFF, 0xE2, 0x00, 0x01]);
    assert!(retries.run(&inner, 0));

    // An empty saved APP is quiet, but a following structural marker is not.
    retries.chunk(&[0xFF, 0xE2, 0x00, 0x02]);
    assert!(!retries.run(&inner, 0));
    retries.chunk(&[0xFF]);
    assert!(!retries.run(&inner, 0));
    retries.chunk(&[0xD9]);
    assert!(retries.run(&inner, 0));
  }

  #[test]
  fn jpeg_retries_run_once_per_epoch_and_change() {
    let inner = jpeg_decoder(include_bytes!("../../__test__/fixtures/jpeg/420-37x9.jpg"));
    let mut retries = super::JpegRetries::default();
    // SOI and a baseline SOS (Ns 1, Ss 0, Se 63)
    retries.chunk(&[
      0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00,
    ]);
    assert!(retries.run(&inner, 0));
    assert!(!retries.run(&inner, 0));
    // Scan data changes nothing
    retries.chunk(&[0x11; 100]);
    assert!(!retries.run(&inner, 0));
    // A new epoch (a reset()) runs again
    assert!(retries.run(&inner, 1));
    assert!(!retries.run(&inner, 1));
    // So does a marker, also one split across chunks
    retries.chunk(&[0xFF]);
    assert!(!retries.run(&inner, 1));
    retries.chunk(&[0xD9]);
    assert!(retries.run(&inner, 1));
    assert!(!retries.run(&inner, 1));
    // And any try while a frame failure is recorded
    inner.lock().unwrap().frame_failed = Some(1);
    assert!(retries.run(&inner, 1));
    assert!(retries.run(&inner, 1));
  }

  #[test]
  fn a_reset_during_the_final_avif_probe_preserves_terminal_stream_rules() {
    let data = include_bytes!("../../__test__/fixtures/avif/anim8_420.avif");
    let inner = decoder(&data[..2871], AVCodecID::Av1, "image/avif");
    {
      let mut guard = inner.lock().unwrap();
      guard.all_data = false;
      guard.complete.store(false, Ordering::Release);
      assert_eq!(guard.missing_samples, None);
    }
    assert!(!begin_terminal_refresh(&inner));

    // Reset after the final probe decoded its frames but before its cache commit. The stale
    // commit must not be the only place that terminal-stream state survives.
    let hook_inner = inner.clone();
    BEFORE_STORE_FRAMES.with(|slot| {
      *slot.borrow_mut() = Some(Box::new(move || reset(&hook_inner)));
    });
    let result = cache_decoded_frames(&inner, FrameCall::Probe, true, true);
    BEFORE_STORE_FRAMES.with(|slot| *slot.borrow_mut() = None);
    assert_eq!(reason(result), "ok");
    {
      let guard = inner.lock().unwrap();
      assert!(guard.terminal_stream);
      assert_eq!(guard.cache_epoch, 1);
      assert!(guard.cached_frames.is_none());
      assert_eq!(guard.missing_samples, None);
    }

    // The collector marks the data complete after the stale probe. A later decode must rebuild
    // the two available frames with the stream size hint and retain the missing third sample.
    inner
      .lock()
      .unwrap()
      .complete
      .store(true, Ordering::Release);
    assert!(matches!(decode_cached_frame(&inner, 0, 1), Ok(Some(_))));
    {
      let guard = inner.lock().unwrap();
      assert!(!guard.closed);
      assert!(!guard.tracks.failed.load(Ordering::Acquire));
      assert_eq!(guard.missing_samples, Some(3));
      assert_eq!(guard.cached_frames.as_ref().map(Vec::len), Some(2));
    }
    assert_eq!(
      reason(decode_cached_frame(&inner, 2, 1)),
      "Failed to decode frame at index 2"
    );
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

  /// `jpeg_decoder` before its tracks are ready, with a waiter enabled on `tracks.ready_notify`
  /// (what `tracks.ready` waits on) and one on `decode_notify` (what decode() calls wait on)
  fn waiters_before_the_tracks() -> (Arc<Mutex<ImageDecoderInner>>, Arc<Notify>, Arc<Notify>) {
    let inner = jpeg_decoder(JPEG_420);
    let (ready_notify, decode_notify) = {
      let guard = inner.lock().unwrap();
      guard.tracks.ready.store(false, Ordering::Release);
      (
        guard.tracks.ready_notify.clone(),
        guard.decode_notify.clone(),
      )
    };
    (inner, ready_notify, decode_notify)
  }

  #[test]
  fn a_turn_given_before_the_tracks_are_ready_does_not_wake_tracks_ready() {
    let (inner, ready_notify, decode_notify) = waiters_before_the_tracks();
    let mut ready = std::pin::pin!(ready_notify.notified());
    ready.as_mut().enable();
    let mut decode = std::pin::pin!(decode_notify.notified());
    decode.as_mut().enable();
    // A decode() call gives its turn (a call that a reset() rejected while the tracks are not
    // ready, say)
    drop(DecodeTurn(inner.clone()));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert_eq!(inner.lock().unwrap().decode_turn, 1);
    assert!(decode.as_mut().poll(&mut cx).is_ready());
    // tracks.ready still waits, like Chrome's
    assert!(ready.as_mut().poll(&mut cx).is_pending());
  }

  #[test]
  fn a_reset_before_the_tracks_are_ready_wakes_the_decode_calls_not_tracks_ready() {
    let (inner, ready_notify, decode_notify) = waiters_before_the_tracks();
    let mut ready = std::pin::pin!(ready_notify.notified());
    ready.as_mut().enable();
    let mut decode = std::pin::pin!(decode_notify.notified());
    decode.as_mut().enable();
    reset(&inner);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    // The work of the calls that the reset() rejected goes on to its turn
    assert!(decode.as_mut().poll(&mut cx).is_ready());
    assert!(ready.as_mut().poll(&mut cx).is_pending());
    assert!(!inner.lock().unwrap().tracks.ready.load(Ordering::Acquire));
  }

  #[test]
  fn the_wait_for_the_tracks_ends_at_a_reset_or_a_close() {
    for close_it in [false, true] {
      let (inner, _, _) = waiters_before_the_tracks();
      let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
      let mut wait = std::pin::pin!(reset_or_closed(&inner, 0));
      assert!(wait.as_mut().poll(&mut cx).is_pending());
      // A turn given, or data that came, does not end it
      drop(DecodeTurn(inner.clone()));
      super::signal_tracks_ready(&inner);
      assert!(wait.as_mut().poll(&mut cx).is_pending());
      if close_it {
        close(&inner);
      } else {
        reset(&inner);
      }
      assert!(wait.as_mut().poll(&mut cx).is_ready(), "close: {close_it}");
      // A call made after the reset() waits on
      if !close_it {
        let mut later = std::pin::pin!(reset_or_closed(&inner, 1));
        assert!(later.as_mut().poll(&mut cx).is_pending());
      }
    }
  }

  /// A PNG or a WebP decoder before its tracks are ready: made from a buffer (`open` false), or
  /// from a stream that is still open and has given all of `data`
  fn layered_decoder(
    data: &[u8],
    codec_id: AVCodecID,
    open: bool,
  ) -> Arc<Mutex<ImageDecoderInner>> {
    let mime_type = if codec_id == AVCodecID::Png {
      "image/png"
    } else {
      "image/webp"
    };
    let inner = decoder(data, codec_id, mime_type);
    {
      let mut guard = inner.lock().unwrap();
      guard.tracks.ready.store(false, Ordering::Release);
      guard.tracks.inner.lock().unwrap().tracks[0] = ImageTrackData::initial(Some(codec_id));
      if open {
        guard.all_data = false;
        guard.complete.store(false, Ordering::Release);
      }
    }
    inner
  }

  /// The read that the collector or the buffer's probe makes (`FrameCall::Probe`), with `hook` run
  /// where it is about to draw the frames (`before_layered_draw`; from a buffer, the track is
  /// stored then). Returns the error reason, and whether the hook ran.
  fn read_with_hook(
    inner: &Arc<Mutex<ImageDecoderInner>>,
    data_complete: bool,
    mut hook: impl FnMut() + 'static,
  ) -> (String, bool) {
    let ran = Rc::new(Cell::new(false));
    let hook_ran = ran.clone();
    BEFORE_LAYERED_DRAW.with(|slot| {
      *slot.borrow_mut() = Some(Box::new(move || {
        hook_ran.set(true);
        hook();
      }))
    });
    let result = cache_decoded_frames(inner, FrameCall::Probe, data_complete, false);
    BEFORE_LAYERED_DRAW.with(|slot| *slot.borrow_mut() = None);
    (reason(result), ran.get())
  }

  /// A track as (animated, frameCount, repetitionCount)
  type TrackOf = (bool, u32, f64);

  /// The track of a decoder
  fn track_of(inner: &Arc<Mutex<ImageDecoderInner>>) -> TrackOf {
    let guard = inner.lock().unwrap();
    let tracks = guard.tracks.inner.lock().unwrap();
    let track = &tracks.tracks[0];
    (track.animated, track.frame_count, track.repetition_count)
  }

  /// Whether every cached frame is decoded, and how many there are
  fn decoded_frames(inner: &Arc<Mutex<ImageDecoderInner>>) -> Option<(bool, usize)> {
    let guard = inner.lock().unwrap();
    guard.cached_frames.as_ref().map(|frames| {
      (
        frames
          .iter()
          .all(|frame| matches!(frame, super::CachedFrame::Decoded { .. })),
        frames.len(),
      )
    })
  }

  /// PNG and WebP files whose frames are all whole, with their codec and their track
  const LAYERED_FILES: [(&str, &[u8], AVCodecID, TrackOf); 4] = [
    (
      "apng-3f-inf.png",
      include_bytes!("../../__test__/fixtures/png-webp/apng-3f-inf.png"),
      AVCodecID::Png,
      (true, 3, f64::INFINITY),
    ),
    (
      "four-colors.png",
      include_bytes!("../../__test__/fixtures/wpt/four-colors.png"),
      AVCodecID::Png,
      (false, 1, 0.0),
    ),
    (
      "anim_lossy_opaque.webp",
      include_bytes!("../../__test__/fixtures/png-webp/anim_lossy_opaque.webp"),
      AVCodecID::Webp,
      (true, 3, f64::INFINITY),
    ),
    (
      "webp-valid.webp",
      include_bytes!("../../__test__/fixtures/png-webp/webp-valid.webp"),
      AVCodecID::Webp,
      (false, 1, 0.0),
    ),
  ];

  /// From a buffer, the read stores the track and sets tracks.ready before it draws the frames. On
  /// a stream, a chunk with the header and whole frames: the read stores the track with the
  /// frames, once they are drawn, and leaves tracks.ready to the collector, which sets it after
  /// the read (`signal_tracks_ready`)
  #[test]
  fn a_png_or_webp_track_and_tracks_ready_come_before_the_draw_from_a_buffer_and_after_it_on_a_stream()
   {
    for (name, data, codec_id, track) in LAYERED_FILES {
      // a buffer, or one chunk with every byte on a stream left open
      for open in [false, true] {
        let inner = layered_decoder(data, codec_id, open);
        let ready_notify = inner.lock().unwrap().tracks.ready_notify.clone();
        let seen = Rc::new(RefCell::new(None));
        let (hook_inner, hook_seen) = (inner.clone(), seen.clone());
        let (result, ran) = read_with_hook(&inner, !open, move || {
          let ready = hook_inner
            .lock()
            .unwrap()
            .tracks
            .ready
            .load(Ordering::Acquire);
          // set_tracks_ready leaves a permit: a waiter made now wakes at once
          let mut woken = std::pin::pin!(ready_notify.notified());
          let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
          let woken = woken.as_mut().poll(&mut cx).is_ready();
          *hook_seen.borrow_mut() = Some((
            ready,
            woken,
            track_of(&hook_inner),
            decoded_frames(&hook_inner),
          ));
        });
        let label = format!("{name} open {open}");
        assert_eq!(result, "ok", "{label}");
        assert!(ran, "{label}");
        // At the hook, no frame drawn yet. A buffer: ready, its waiters woken, the track stored.
        // A stream: not ready, the track as it was.
        let initial = ImageTrackData::initial(Some(codec_id));
        let at_hook = if open {
          (
            false,
            false,
            (
              initial.animated,
              initial.frame_count,
              initial.repetition_count,
            ),
            None,
          )
        } else {
          (true, true, track, None)
        };
        assert_eq!(seen.borrow_mut().take(), Some(at_hook), "{label}");
        // Then the frames, every one decoded, with the track
        assert_eq!(
          decoded_frames(&inner),
          Some((true, track.1 as usize)),
          "{label}"
        );
        assert_eq!(track_of(&inner), track, "{label}");
        assert_eq!(inner.lock().unwrap().layered.drawing, None, "{label}");
        // A stream's read leaves tracks.ready to the collector
        let ready = inner.lock().unwrap().tracks.ready.load(Ordering::Acquire);
        assert_eq!(ready, !open, "{label}");
      }
    }
  }

  #[test]
  fn a_png_or_webp_decode_made_while_the_frames_are_drawn_waits_for_them_and_draws_nothing() {
    for (name, data, codec_id, track) in LAYERED_FILES {
      for open in [false, true] {
        let inner = layered_decoder(data, codec_id, open);
        let notify = inner.lock().unwrap().decode_notify.clone();
        let mut notified = std::pin::pin!(notify.notified());
        notified.as_mut().enable();
        let during = Rc::new(RefCell::new(String::new()));
        let (hook_inner, hook_during) = (inner.clone(), during.clone());
        let (result, ran) = read_with_hook(&inner, !open, move || {
          // a decode() of frame 0 runs now, on another thread (from a buffer, tracks.ready is
          // set; on a stream, a decode() made after a reset() finds no frames either)
          *hook_during.borrow_mut() = decode_elsewhere(&hook_inner, 0);
        });
        let label = format!("{name} open {open}");
        assert_eq!(result, "ok", "{label}");
        assert!(ran, "{label}");
        // It waited: no error, no frame of its own
        assert_eq!(*during.borrow(), "pending", "{label}");
        // The end of the draw wakes it, and it takes the frame drawn once
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(notified.as_mut().poll(&mut cx).is_ready(), "{label}");
        assert_eq!(decode_elsewhere(&inner, 0), "frame", "{label}");
        let last = track.1 as usize - 1;
        let inner2 = inner.clone();
        let last_frame =
          std::thread::spawn(move || matches!(decode_cached_frame(&inner2, last, 0), Ok(Some(_))))
            .join()
            .unwrap();
        assert!(last_frame, "{label}");
        assert_eq!(inner.lock().unwrap().layered.draws, 1, "{label}");
      }
    }
  }

  /// A decode() that found no read drawing, and whose own read then finds that one started (the
  /// collector's, on another thread, drawing under no lock), waits for that read's frames: the
  /// frames are drawn once
  #[test]
  fn a_png_or_webp_decode_whose_read_finds_a_draw_started_after_its_look_waits_for_it() {
    for (name, data, codec_id, track) in LAYERED_FILES {
      for open in [false, true] {
        let label = format!("{name} open {open}");
        let inner = layered_decoder(data, codec_id, open);
        let (at_draw, drawing) = std::sync::mpsc::channel::<()>();
        let (go, wait_go) = std::sync::mpsc::channel::<()>();
        let wait_go = Arc::new(Mutex::new(wait_go));
        let reader = Rc::new(RefCell::new(None));
        let (hook_inner, hook_reader) = (inner.clone(), reader.clone());
        BEFORE_DECODE_READ.with(|slot| {
          *slot.borrow_mut() = Some(Box::new(move || {
            // the read of the collector starts now, and stops where it draws
            let (inner, at_draw, wait_go) = (hook_inner.clone(), at_draw.clone(), wait_go.clone());
            *hook_reader.borrow_mut() = Some(std::thread::spawn(move || {
              BEFORE_LAYERED_DRAW.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move || {
                  at_draw.send(()).unwrap();
                  wait_go.lock().unwrap().recv().unwrap();
                }))
              });
              reason(cache_decoded_frames(&inner, FrameCall::Probe, !open, false))
            }));
            drawing.recv().unwrap();
          }))
        });
        let during = match decode_cached_frame(&inner, 0, 0) {
          Ok(None) => "pending".to_string(),
          Ok(Some(_)) => "frame".to_string(),
          Err(error) => error.reason,
        };
        BEFORE_DECODE_READ.with(|slot| *slot.borrow_mut() = None);
        go.send(()).unwrap();
        let reader = reader.borrow_mut().take().expect("the read started");
        assert_eq!(reader.join().unwrap(), "ok", "{label}");
        // It waited for the read's frames, and drew none of its own
        assert_eq!(during, "pending", "{label}");
        assert_eq!(inner.lock().unwrap().layered.draws, 1, "{label}");
        assert_eq!(
          decoded_frames(&inner),
          Some((true, track.1 as usize)),
          "{label}"
        );
        assert_eq!(decode_elsewhere(&inner, 0), "frame", "{label}");
        assert_eq!(inner.lock().unwrap().layered.draws, 1, "{label}");
      }
    }
  }

  #[test]
  fn a_png_or_webp_decode_made_after_a_reset_does_not_wait_for_the_draw_before_it() {
    let (_, data, codec_id, _) = LAYERED_FILES[0];
    let inner = layered_decoder(data, codec_id, true);
    let during = Rc::new(RefCell::new(String::new()));
    let (hook_inner, hook_during) = (inner.clone(), during.clone());
    let (result, ran) = read_with_hook(&inner, false, move || {
      reset(&hook_inner);
      // the decode() made after the reset() reads the data itself
      *hook_during.borrow_mut() = decode_elsewhere(&hook_inner, 1);
    });
    assert_eq!(result, "ok");
    assert!(ran);
    assert_eq!(*during.borrow(), "frame");
    // the draw of the stale read is dropped, and the track stays (reset() keeps it)
    assert_eq!(track_of(&inner), (true, 3, f64::INFINITY));
    assert_eq!(inner.lock().unwrap().layered.drawing, None);
  }

  /// The read that the collector or the buffer's probe makes (`FrameCall::Probe`), with `hook` run
  /// before each frame it decodes (`before_layered_frame`), given how many it ran before. Returns
  /// the error reason, and how many times the hook ran.
  fn read_with_frame_hook(
    inner: &Arc<Mutex<ImageDecoderInner>>,
    data_complete: bool,
    mut hook: impl FnMut(usize) + 'static,
  ) -> (String, usize) {
    let ran = Rc::new(Cell::new(0));
    let hook_ran = ran.clone();
    BEFORE_LAYERED_FRAME.with(|slot| {
      *slot.borrow_mut() = Some(Box::new(move || {
        let n = hook_ran.get();
        hook_ran.set(n + 1);
        hook(n);
      }))
    });
    let result = cache_decoded_frames(inner, FrameCall::Probe, data_complete, false);
    BEFORE_LAYERED_FRAME.with(|slot| *slot.borrow_mut() = None);
    (reason(result), ran.get())
  }

  /// On a stream, a reset() made while the read of a chunk with the header and whole frames draws
  /// them (before the first frame, or before the second one of an animation): the read stores
  /// the track it read, like Chrome, whose reset() keeps the track, drops the frames, and returns
  /// Ok, so the collector sets tracks.ready after it as after any read (`signal_tracks_ready`)
  #[test]
  fn a_reset_while_a_png_or_webp_stream_draws_keeps_the_track_it_read() {
    for (name, data, codec_id, track) in LAYERED_FILES {
      let places: &[usize] = if track.1 > 1 { &[0, 1] } else { &[0] };
      for &at in places {
        let label = format!("{name} reset before frame {at}");
        let inner = layered_decoder(data, codec_id, true);
        let hook_inner = inner.clone();
        let (result, frames) = read_with_frame_hook(&inner, false, move |n| {
          if n == at {
            reset(&hook_inner);
          }
        });
        assert_eq!(result, "ok", "{label}");
        assert_eq!(track_of(&inner), track, "{label}");
        let guard = inner.lock().unwrap();
        assert!(guard.cached_frames.is_none(), "{label}");
        assert_eq!(guard.layered.drawing, None, "{label}");
        // Left to the collector, which sets it after the read: never at the reset()
        assert!(!guard.tracks.ready.load(Ordering::Acquire), "{label}");
        // The draw stopped there: no frame was decoded after the reset()
        assert_eq!(frames, at + 1, "{label}");
      }
    }
  }

  /// A close() or a reset() made while a read draws the frames of an animation, from a buffer or
  /// on a stream, stops the draw before the next frame: nothing of it is stored, and the draw
  /// guard is cleared. A decode() of frame 0 that waited for those frames wakes, and ends as after
  /// any close() or reset(): with the closed decoder's error, or with AbortError on its turn
  /// (`abort_if_reset`); a decode() made after the reset() reads the data itself.
  #[test]
  fn a_close_or_a_reset_stops_a_png_or_webp_draw_before_its_next_frame() {
    for (name, data, codec_id, track) in LAYERED_FILES {
      if track.1 < 3 {
        continue;
      }
      for open in [false, true] {
        for close_it in [false, true] {
          let op = if close_it { "close" } else { "reset" };
          let label = format!("{name} open {open} {op} before frame 1");
          let inner = layered_decoder(data, codec_id, open);
          let notify = inner.lock().unwrap().decode_notify.clone();
          let mut notified = std::pin::pin!(notify.notified());
          notified.as_mut().enable();
          let waiting = Rc::new(RefCell::new(String::new()));
          let (hook_inner, hook_waiting) = (inner.clone(), waiting.clone());
          let (result, frames) = read_with_frame_hook(&inner, !open, move |n| match n {
            // a decode() of frame 0 made now waits for these frames
            0 => *hook_waiting.borrow_mut() = decode_elsewhere(&hook_inner, 0),
            1 if close_it => close(&hook_inner),
            1 => reset(&hook_inner),
            _ => {}
          });
          assert_eq!(*waiting.borrow(), "pending", "{label}");
          let expected = if close_it {
            "InvalidStateError: ImageDecoder is closed"
          } else {
            "ok"
          };
          assert_eq!(result, expected, "{label}");
          assert_eq!(frames, 2, "{label}");
          {
            let guard = inner.lock().unwrap();
            assert!(guard.cached_frames.is_none(), "{label}");
            assert_eq!(guard.layered.drawing, None, "{label}");
            assert_eq!(guard.layered.draws, 1, "{label}");
          }
          let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
          assert!(notified.as_mut().poll(&mut cx).is_ready(), "{label}");
          if close_it {
            assert_eq!(
              decode_elsewhere(&inner, 0),
              "InvalidStateError: ImageDecoder is closed",
              "{label}"
            );
          } else {
            assert_eq!(
              reason(abort_if_reset(&inner, 0)),
              "AbortError: Aborted by reset.",
              "{label}"
            );
            assert_eq!(track_of(&inner), track, "{label}");
            assert_eq!(decode_elsewhere(&inner, 1), "frame", "{label}");
          }
        }
      }
    }
  }

  /// VP8X still WebPs and the bytes that end with their image chunk header or soon after it,
  /// before the image chunk's first bytes can be read (Chrome 155: a decode() made then fails;
  /// one made once the rest came decodes)
  const UNNUMBERED_CUTS: [(&str, &[u8], &[usize]); 2] = [
    (
      "craft_vp8x_opaque_33x17.webp",
      include_bytes!("../../__test__/fixtures/png-webp/craft_vp8x_opaque_33x17.webp"),
      &[38, 39, 42, 47],
    ),
    (
      "vp8x-vp8l.webp",
      include_bytes!("../../__test__/fixtures/png-webp/vp8x-vp8l.webp"),
      &[38, 39, 42],
    ),
  ];

  /// A WebP decoder on an open stream that has given the first `cut` bytes of `data`, read
  fn webp_stream_at(data: &[u8], cut: usize) -> Arc<Mutex<ImageDecoderInner>> {
    let inner = layered_decoder(&data[..cut], AVCodecID::Webp, true);
    assert_eq!(
      reason(cache_decoded_frames(&inner, FrameCall::Probe, false, false)),
      "ok"
    );
    inner
  }

  #[test]
  fn a_vp8x_still_webp_split_at_its_image_chunk_header_decodes_once_the_rest_comes() {
    for (name, data, cuts) in UNNUMBERED_CUTS {
      for &cut in cuts {
        let label = format!("{name} cut at {cut}");
        let inner = webp_stream_at(data, cut);
        assert_eq!(track_of(&inner), (false, 1, 0.0), "{label}");
        // the rest, then the end of the stream
        {
          let mut guard = inner.lock().unwrap();
          guard.data = ImageDecoderData::Vec(data.to_vec());
          guard.all_data = true;
        }
        assert_eq!(
          reason(cache_decoded_frames(&inner, FrameCall::Probe, false, false)),
          "ok",
          "{label}"
        );
        assert_eq!(
          reason(cache_decoded_frames(&inner, FrameCall::Probe, true, true)),
          "ok",
          "{label}"
        );
        inner
          .lock()
          .unwrap()
          .complete
          .store(true, Ordering::Release);
        assert_eq!(decode_elsewhere(&inner, 0), "frame", "{label}");
        assert!(!inner.lock().unwrap().closed, "{label}");
      }
    }
  }

  #[test]
  fn a_decode_made_while_a_vp8x_still_webp_has_only_its_image_chunk_header_fails_like_chrome() {
    for (name, data, cuts) in UNNUMBERED_CUTS {
      for &cut in cuts {
        let label = format!("{name} cut at {cut}");
        let inner = webp_stream_at(data, cut);
        assert_eq!(
          decode_elsewhere(&inner, 0),
          "Failed to decode frame at index 0",
          "{label}"
        );
        let guard = inner.lock().unwrap();
        assert!(guard.closed && guard.frames_failed, "{label}");
      }
    }
  }
}
