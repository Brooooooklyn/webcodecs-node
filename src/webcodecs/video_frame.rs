//! VideoFrame - WebCodecs API implementation
//!
//! Represents a frame of video data that can be displayed or encoded.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/VideoFrame

use crate::codec::{Frame, Scaler};
use crate::ffi::{
  AVColorPrimaries, AVColorRange, AVColorSpace, AVColorTransferCharacteristic, AVPixelFormat,
};
use crate::webcodecs::error::{
  dom_exception_for_rejection, enforce_range_long_long, enforce_range_long_long_optional,
  enforce_range_unsigned_long_long_optional, invalid_state_error, not_supported_error,
  take_pending_exception, throw_as_native_error, throw_invalid_state_error,
  throw_not_supported_error, type_error,
};
use crate::webcodecs::promise_reject::reject_with_type_error;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::UnknownReturnValue;
use napi_derive::napi;
use parking_lot::RwLock;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);
// Callbacks are weak native references kept alive by the per-env anchor store in
// event_target.rs. This keeps a callback that captures its frame collectible with that frame.
struct FrameCloseObservers {
  env: napi::sys::napi_env,
  next_id: u64,
  // (listener map key, napi_ref) pairs
  callbacks: Vec<(u64, usize)>,
  // The frame's anchor holder.
  holder: super::event_target::HolderRef,
}

impl FrameCloseObservers {
  fn new(env: napi::sys::napi_env, next_id: u64) -> Self {
    Self {
      env,
      next_id,
      callbacks: Vec::new(),
      holder: Default::default(),
    }
  }

  /// Delete every napi_ref this entry owns (JS thread).
  fn delete_refs(mut self) {
    for (_, reference) in self.callbacks.drain(..) {
      unsafe { napi::sys::napi_delete_reference(self.env, reference as napi::sys::napi_ref) };
    }
    self.holder.clear(self.env);
  }
}
thread_local! {
  static CLOSE_OBSERVERS: RefCell<HashMap<u64, FrameCloseObservers>> = RefCell::new(HashMap::new());
}

/// Video pixel format (WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPixelFormat {
  // 8-bit YUV formats
  /// Planar YUV 4:2:0, 12bpp, (1 Cr & Cb sample per 2x2 Y samples)
  I420,
  /// Planar YUV 4:2:0, 12bpp, with alpha plane
  I420A,
  /// Planar YUV 4:2:2, 16bpp
  I422,
  /// Planar YUV 4:2:2, 16bpp, with alpha plane
  I422A,
  /// Planar YUV 4:4:4, 24bpp
  I444,
  /// Planar YUV 4:4:4, 24bpp, with alpha plane
  I444A,

  // 10-bit YUV formats
  /// Planar YUV 4:2:0, 10-bit
  I420P10,
  /// Planar YUV 4:2:0, 10-bit, with alpha plane
  I420AP10,
  /// Planar YUV 4:2:2, 10-bit
  I422P10,
  /// Planar YUV 4:2:2, 10-bit, with alpha plane
  I422AP10,
  /// Planar YUV 4:4:4, 10-bit
  I444P10,
  /// Planar YUV 4:4:4, 10-bit, with alpha plane
  I444AP10,

  // 12-bit YUV formats
  /// Planar YUV 4:2:0, 12-bit
  I420P12,
  /// Planar YUV 4:2:2, 12-bit
  I422P12,
  /// Planar YUV 4:4:4, 12-bit
  I444P12,

  // Semi-planar formats
  /// Semi-planar YUV 4:2:0, 12bpp (Y plane + interleaved UV)
  NV12,
  /// Semi-planar YUV 4:2:0, 12bpp (Y plane + interleaved VU) - per W3C WebCodecs spec
  NV21,

  // RGB formats
  /// RGBA 32bpp
  RGBA,
  /// RGBX 32bpp (alpha ignored)
  RGBX,
  /// BGRA 32bpp
  BGRA,
  /// BGRX 32bpp (alpha ignored)
  BGRX,
}

impl VideoPixelFormat {
  /// Convert from FFmpeg pixel format
  pub fn from_av_format(format: AVPixelFormat) -> Option<Self> {
    match format {
      // 8-bit formats
      AVPixelFormat::Yuv420p => Some(VideoPixelFormat::I420),
      AVPixelFormat::Yuva420p => Some(VideoPixelFormat::I420A),
      AVPixelFormat::Yuv422p => Some(VideoPixelFormat::I422),
      AVPixelFormat::Yuva422p => Some(VideoPixelFormat::I422A),
      AVPixelFormat::Yuv444p => Some(VideoPixelFormat::I444),
      AVPixelFormat::Yuva444p => Some(VideoPixelFormat::I444A),
      AVPixelFormat::Nv12 => Some(VideoPixelFormat::NV12),
      AVPixelFormat::Nv21 => Some(VideoPixelFormat::NV21),
      AVPixelFormat::Rgba => Some(VideoPixelFormat::RGBA),
      AVPixelFormat::Bgra => Some(VideoPixelFormat::BGRA),
      // ARGB/ABGR map to RGBA/BGRA (closest WebCodecs equivalent - channel order adjusted)
      AVPixelFormat::Argb => Some(VideoPixelFormat::RGBA),
      AVPixelFormat::Abgr => Some(VideoPixelFormat::BGRA),
      // RGB24/BGR24 map to RGBX/BGRX (closest WebCodecs equivalent - alpha ignored)
      AVPixelFormat::Rgb24 => Some(VideoPixelFormat::RGBX),
      AVPixelFormat::Bgr24 => Some(VideoPixelFormat::BGRX),
      // 10-bit formats
      AVPixelFormat::Yuv420p10le => Some(VideoPixelFormat::I420P10),
      AVPixelFormat::Yuv422p10le => Some(VideoPixelFormat::I422P10),
      AVPixelFormat::Yuv444p10le => Some(VideoPixelFormat::I444P10),
      AVPixelFormat::Yuva420p10le => Some(VideoPixelFormat::I420AP10),
      AVPixelFormat::Yuva422p10le => Some(VideoPixelFormat::I422AP10),
      AVPixelFormat::Yuva444p10le => Some(VideoPixelFormat::I444AP10),
      // 12-bit formats
      AVPixelFormat::Yuv420p12le => Some(VideoPixelFormat::I420P12),
      AVPixelFormat::Yuv422p12le => Some(VideoPixelFormat::I422P12),
      AVPixelFormat::Yuv444p12le => Some(VideoPixelFormat::I444P12),
      _ => None,
    }
  }

  /// Convert to FFmpeg pixel format
  pub fn to_av_format(&self) -> AVPixelFormat {
    match self {
      // 8-bit formats
      VideoPixelFormat::I420 => AVPixelFormat::Yuv420p,
      VideoPixelFormat::I420A => AVPixelFormat::Yuva420p,
      VideoPixelFormat::I422 => AVPixelFormat::Yuv422p,
      VideoPixelFormat::I422A => AVPixelFormat::Yuva422p,
      VideoPixelFormat::I444 => AVPixelFormat::Yuv444p,
      VideoPixelFormat::I444A => AVPixelFormat::Yuva444p,
      VideoPixelFormat::NV12 => AVPixelFormat::Nv12,
      VideoPixelFormat::NV21 => AVPixelFormat::Nv21,
      VideoPixelFormat::RGBA => AVPixelFormat::Rgba,
      VideoPixelFormat::RGBX => AVPixelFormat::Rgba, // Treat as RGBA
      VideoPixelFormat::BGRA => AVPixelFormat::Bgra,
      VideoPixelFormat::BGRX => AVPixelFormat::Bgra, // Treat as BGRA
      // 10-bit formats
      VideoPixelFormat::I420P10 => AVPixelFormat::Yuv420p10le,
      VideoPixelFormat::I420AP10 => AVPixelFormat::Yuva420p10le,
      VideoPixelFormat::I422P10 => AVPixelFormat::Yuv422p10le,
      VideoPixelFormat::I422AP10 => AVPixelFormat::Yuva422p10le,
      VideoPixelFormat::I444P10 => AVPixelFormat::Yuv444p10le,
      VideoPixelFormat::I444AP10 => AVPixelFormat::Yuva444p10le,
      // 12-bit formats
      VideoPixelFormat::I420P12 => AVPixelFormat::Yuv420p12le,
      VideoPixelFormat::I422P12 => AVPixelFormat::Yuv422p12le,
      VideoPixelFormat::I444P12 => AVPixelFormat::Yuv444p12le,
    }
  }

  /// Get bytes per sample for this format (1 for 8-bit, 2 for 10/12-bit)
  pub fn bytes_per_sample(&self) -> usize {
    match self {
      // 8-bit formats
      VideoPixelFormat::I420
      | VideoPixelFormat::I420A
      | VideoPixelFormat::I422
      | VideoPixelFormat::I422A
      | VideoPixelFormat::I444
      | VideoPixelFormat::I444A
      | VideoPixelFormat::NV12
      | VideoPixelFormat::NV21 => 1,
      // 10/12-bit formats use 2 bytes per sample
      VideoPixelFormat::I420P10
      | VideoPixelFormat::I420AP10
      | VideoPixelFormat::I422P10
      | VideoPixelFormat::I422AP10
      | VideoPixelFormat::I444P10
      | VideoPixelFormat::I444AP10
      | VideoPixelFormat::I420P12
      | VideoPixelFormat::I422P12
      | VideoPixelFormat::I444P12 => 2,
      // RGBA formats: 4 bytes per pixel
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => 4,
    }
  }

  /// Returns true if this format has an alpha channel
  pub fn has_alpha(&self) -> bool {
    matches!(
      self,
      VideoPixelFormat::I420A
        | VideoPixelFormat::I422A
        | VideoPixelFormat::I444A
        | VideoPixelFormat::I420AP10
        | VideoPixelFormat::I422AP10
        | VideoPixelFormat::I444AP10
        | VideoPixelFormat::RGBA
        | VideoPixelFormat::BGRA
    )
  }

  /// Returns the equivalent format without alpha, or self if no alpha
  pub fn without_alpha(&self) -> Self {
    match self {
      VideoPixelFormat::I420A => VideoPixelFormat::I420,
      VideoPixelFormat::I422A => VideoPixelFormat::I422,
      VideoPixelFormat::I444A => VideoPixelFormat::I444,
      VideoPixelFormat::I420AP10 => VideoPixelFormat::I420P10,
      VideoPixelFormat::I422AP10 => VideoPixelFormat::I422P10,
      VideoPixelFormat::I444AP10 => VideoPixelFormat::I444P10,
      VideoPixelFormat::RGBA => VideoPixelFormat::RGBX,
      VideoPixelFormat::BGRA => VideoPixelFormat::BGRX,
      other => *other,
    }
  }

  /// Returns true for the packed RGB formats (RGBA, RGBX, BGRA, BGRX)
  pub fn is_rgb(&self) -> bool {
    matches!(
      self,
      VideoPixelFormat::RGBA
        | VideoPixelFormat::RGBX
        | VideoPixelFormat::BGRA
        | VideoPixelFormat::BGRX
    )
  }

  /// Check if conversion from self to target format is supported
  ///
  /// Per WPT videoFrame-copyTo-rgb.any.js:
  /// - YUV → RGB: supported (I420, I422, I444, NV12 → RGBA, RGBX, BGRA, BGRX)
  /// - RGB → RGB: supported (between RGBA, RGBX, BGRA, BGRX)
  /// - RGB → YUV: NOT supported (throws NotSupportedError)
  ///
  /// The same format passes here (allocationSize takes it), but copyTo rejects an explicit
  /// non-RGB format, as Chrome does.
  pub fn can_convert_to(&self, target: VideoPixelFormat) -> bool {
    *self == target || target.is_rgb()
  }
}

/// Video color primaries (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoColorPrimaries {
  /// BT.709 / sRGB primaries
  #[napi(value = "bt709")]
  Bt709,
  /// BT.470 BG (PAL)
  #[napi(value = "bt470bg")]
  Bt470bg,
  /// SMPTE 170M (NTSC)
  #[napi(value = "smpte170m")]
  Smpte170m,
  /// BT.2020 (UHD)
  #[napi(value = "bt2020")]
  Bt2020,
  /// SMPTE 432 (DCI-P3)
  #[napi(value = "smpte432")]
  Smpte432,
}

/// Video transfer characteristics (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoTransferCharacteristics {
  /// BT.709 transfer
  #[napi(value = "bt709")]
  Bt709,
  /// SMPTE 170M transfer
  #[napi(value = "smpte170m")]
  Smpte170m,
  /// IEC 61966-2-1 (sRGB) - technical name
  #[napi(value = "iec61966-2-1")]
  Iec6196621,
  /// sRGB transfer (alias for iec61966-2-1)
  #[napi(value = "srgb")]
  Srgb,
  /// Linear transfer
  #[napi(value = "linear")]
  Linear,
  /// Perceptual Quantizer (HDR)
  #[napi(value = "pq")]
  Pq,
  /// Hybrid Log-Gamma (HDR)
  #[napi(value = "hlg")]
  Hlg,
}

/// Video matrix coefficients (W3C WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoMatrixCoefficients {
  /// RGB (identity matrix)
  #[napi(value = "rgb")]
  Rgb,
  /// BT.709
  #[napi(value = "bt709")]
  Bt709,
  /// BT.470 BG
  #[napi(value = "bt470bg")]
  Bt470bg,
  /// SMPTE 170M
  #[napi(value = "smpte170m")]
  Smpte170m,
  /// BT.2020 non-constant luminance
  #[napi(value = "bt2020-ncl")]
  Bt2020Ncl,
}

/// VideoColorSpaceInit for constructing VideoColorSpace
#[derive(Debug, Clone, Default)]
pub struct VideoColorSpaceInit {
  /// Color primaries
  pub primaries: Option<VideoColorPrimaries>,
  /// Transfer characteristics
  pub transfer: Option<VideoTransferCharacteristics>,
  /// Matrix coefficients
  pub matrix: Option<VideoMatrixCoefficients>,
  /// Full range flag
  pub full_range: Option<bool>,
}

/// Helper to get a raw napi value from an object property
unsafe fn get_raw_property(
  env: napi::sys::napi_env,
  obj: napi::sys::napi_value,
  key: &str,
) -> napi::sys::napi_value {
  use napi::sys;
  let mut result: sys::napi_value = std::ptr::null_mut();
  let key_cstr = std::ffi::CString::new(key).unwrap();
  unsafe { sys::napi_get_named_property(env, obj, key_cstr.as_ptr(), &mut result) };
  result
}

/// Helper to check if a napi value is null or undefined
fn is_null_or_undefined(env: napi::sys::napi_env, value: napi::sys::napi_value) -> bool {
  use napi::sys;
  let mut result: sys::napi_valuetype = sys::ValueType::napi_undefined;
  unsafe {
    sys::napi_typeof(env, value, &mut result);
  }
  result == sys::ValueType::napi_null || result == sys::ValueType::napi_undefined
}

impl FromNapiValue for VideoColorSpaceInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let env_wrapper = Env::from_raw(env);
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // Validate primaries - optional but must be valid if present
    let primaries: Option<VideoColorPrimaries> = if obj.has_named_property("primaries")? {
      let raw_val = unsafe { get_raw_property(env, value, "primaries") };
      if is_null_or_undefined(env, raw_val) {
        None
      } else {
        let s: String = unsafe { FromNapiValue::from_napi_value(env, raw_val)? };
        match s.as_str() {
          "bt709" => Some(VideoColorPrimaries::Bt709),
          "bt470bg" => Some(VideoColorPrimaries::Bt470bg),
          "smpte170m" => Some(VideoColorPrimaries::Smpte170m),
          "bt2020" => Some(VideoColorPrimaries::Bt2020),
          "smpte432" => Some(VideoColorPrimaries::Smpte432),
          _ => {
            env_wrapper.throw_type_error(&format!("Invalid primaries value: {}", s), None)?;
            return Err(Error::new(Status::InvalidArg, "Invalid primaries value"));
          }
        }
      }
    } else {
      None
    };

    // Validate transfer - optional but must be valid if present
    let transfer: Option<VideoTransferCharacteristics> = if obj.has_named_property("transfer")? {
      let raw_val = unsafe { get_raw_property(env, value, "transfer") };
      if is_null_or_undefined(env, raw_val) {
        None
      } else {
        let s: String = unsafe { FromNapiValue::from_napi_value(env, raw_val)? };
        match s.as_str() {
          "bt709" => Some(VideoTransferCharacteristics::Bt709),
          "smpte170m" => Some(VideoTransferCharacteristics::Smpte170m),
          "iec61966-2-1" => Some(VideoTransferCharacteristics::Iec6196621),
          "srgb" => Some(VideoTransferCharacteristics::Srgb),
          "linear" => Some(VideoTransferCharacteristics::Linear),
          "pq" => Some(VideoTransferCharacteristics::Pq),
          "hlg" => Some(VideoTransferCharacteristics::Hlg),
          _ => {
            env_wrapper.throw_type_error(&format!("Invalid transfer value: {}", s), None)?;
            return Err(Error::new(Status::InvalidArg, "Invalid transfer value"));
          }
        }
      }
    } else {
      None
    };

    // Validate matrix - optional but must be valid if present
    let matrix: Option<VideoMatrixCoefficients> = if obj.has_named_property("matrix")? {
      let raw_val = unsafe { get_raw_property(env, value, "matrix") };
      if is_null_or_undefined(env, raw_val) {
        None
      } else {
        let s: String = unsafe { FromNapiValue::from_napi_value(env, raw_val)? };
        match s.as_str() {
          "rgb" => Some(VideoMatrixCoefficients::Rgb),
          "bt709" => Some(VideoMatrixCoefficients::Bt709),
          "bt470bg" => Some(VideoMatrixCoefficients::Bt470bg),
          "smpte170m" => Some(VideoMatrixCoefficients::Smpte170m),
          "bt2020-ncl" => Some(VideoMatrixCoefficients::Bt2020Ncl),
          _ => {
            env_wrapper.throw_type_error(&format!("Invalid matrix value: {}", s), None)?;
            return Err(Error::new(Status::InvalidArg, "Invalid matrix value"));
          }
        }
      }
    } else {
      None
    };

    // fullRange is optional boolean - null/undefined is allowed
    let full_range: Option<bool> = if obj.has_named_property("fullRange")? {
      let raw_val = unsafe { get_raw_property(env, value, "fullRange") };
      if is_null_or_undefined(env, raw_val) {
        None
      } else {
        Some(unsafe { FromNapiValue::from_napi_value(env, raw_val)? })
      }
    } else {
      None
    };

    Ok(VideoColorSpaceInit {
      primaries,
      transfer,
      matrix,
      full_range,
    })
  }
}

impl ToNapiValue for VideoColorSpaceInit {
  unsafe fn to_napi_value(env: napi::sys::napi_env, val: Self) -> Result<napi::sys::napi_value> {
    use napi::sys;

    // Create empty object
    let mut raw_obj: sys::napi_value = std::ptr::null_mut();
    let status = unsafe { sys::napi_create_object(env, &mut raw_obj) };
    if status != sys::Status::napi_ok {
      return Err(Error::new(
        Status::GenericFailure,
        "Failed to create object",
      ));
    }

    let mut obj = unsafe { Object::from_napi_value(env, raw_obj)? };

    // Set fields - Option<T> will serialize correctly
    if let Some(p) = val.primaries {
      obj.set("primaries", p)?;
    }
    if let Some(t) = val.transfer {
      obj.set("transfer", t)?;
    }
    if let Some(m) = val.matrix {
      obj.set("matrix", m)?;
    }
    if let Some(fr) = val.full_range {
      obj.set("fullRange", fr)?;
    }

    Ok(raw_obj)
  }
}

/// Video color space parameters (WebCodecs spec) - as a class per spec
#[napi]
#[derive(Debug, Clone, Default)]
pub struct VideoColorSpace {
  primaries: Option<VideoColorPrimaries>,
  transfer: Option<VideoTransferCharacteristics>,
  matrix: Option<VideoMatrixCoefficients>,
  full_range: Option<bool>,
}

#[napi]
impl VideoColorSpace {
  /// Create a new VideoColorSpace
  #[napi(constructor)]
  pub fn new(init: Option<VideoColorSpaceInit>) -> Self {
    match init {
      Some(init) => VideoColorSpace {
        primaries: init.primaries,
        transfer: init.transfer,
        matrix: init.matrix,
        full_range: init.full_range,
      },
      None => VideoColorSpace::default(),
    }
  }

  /// Create VideoColorSpace from individual components (internal use)
  pub fn from_components(
    primaries: Option<VideoColorPrimaries>,
    transfer: Option<VideoTransferCharacteristics>,
    matrix: Option<VideoMatrixCoefficients>,
    full_range: Option<bool>,
  ) -> Self {
    Self {
      primaries,
      transfer,
      matrix,
      full_range,
    }
  }

  /// Get color primaries
  #[napi(getter)]
  pub fn primaries(&self) -> Option<VideoColorPrimaries> {
    self.primaries
  }

  /// Get transfer characteristics
  #[napi(getter)]
  pub fn transfer(&self) -> Option<VideoTransferCharacteristics> {
    self.transfer
  }

  /// Get matrix coefficients
  #[napi(getter)]
  pub fn matrix(&self) -> Option<VideoMatrixCoefficients> {
    self.matrix
  }

  /// Get full range flag
  #[napi(getter)]
  pub fn full_range(&self) -> Option<bool> {
    self.full_range
  }

  /// Convert to VideoColorSpaceInit (plain object version)
  pub fn to_init(&self) -> VideoColorSpaceInit {
    VideoColorSpaceInit {
      primaries: self.primaries,
      transfer: self.transfer,
      matrix: self.matrix,
      full_range: self.full_range,
    }
  }

  /// Convert to JSON-compatible object (W3C spec uses toJSON)
  ///
  /// Per W3C spec, toJSON() returns explicit null for unset fields.
  #[napi(js_name = "toJSON")]
  pub fn to_json(&self, env: Env) -> Result<Object<'_>> {
    use napi::sys;
    let raw_env = env.raw();

    // Create empty object
    let mut raw_obj: sys::napi_value = std::ptr::null_mut();
    let status = unsafe { sys::napi_create_object(raw_env, &mut raw_obj) };
    if status != sys::Status::napi_ok {
      return Err(Error::new(
        Status::GenericFailure,
        "Failed to create object",
      ));
    }

    // Get null value
    let mut null_val: sys::napi_value = std::ptr::null_mut();
    let status = unsafe { sys::napi_get_null(raw_env, &mut null_val) };
    if status != sys::Status::napi_ok {
      return Err(Error::new(Status::GenericFailure, "Failed to get null"));
    }

    let mut obj = unsafe { Object::from_napi_value(raw_env, raw_obj)? };

    // Set primaries - null if not set
    match &self.primaries {
      Some(p) => obj.set("primaries", *p)?,
      None => unsafe {
        let key = std::ffi::CString::new("primaries").unwrap();
        sys::napi_set_named_property(raw_env, raw_obj, key.as_ptr(), null_val);
      },
    };

    // Set transfer - null if not set
    match &self.transfer {
      Some(t) => obj.set("transfer", *t)?,
      None => unsafe {
        let key = std::ffi::CString::new("transfer").unwrap();
        sys::napi_set_named_property(raw_env, raw_obj, key.as_ptr(), null_val);
      },
    };

    // Set matrix - null if not set
    match &self.matrix {
      Some(m) => obj.set("matrix", *m)?,
      None => unsafe {
        let key = std::ffi::CString::new("matrix").unwrap();
        sys::napi_set_named_property(raw_env, raw_obj, key.as_ptr(), null_val);
      },
    };

    // Set fullRange - null if not set
    match &self.full_range {
      Some(fr) => obj.set("fullRange", *fr)?,
      None => unsafe {
        let key = std::ffi::CString::new("fullRange").unwrap();
        sys::napi_set_named_property(raw_env, raw_obj, key.as_ptr(), null_val);
      },
    };

    Ok(obj)
  }
}

/// AVCOL_* tags [primaries, trc, matrix, range] for the copyTo RGB path. A member the
/// VideoFrame lacks is -1 ("use the AVFrame's own tag"): decoders set values the
/// VideoColorSpace enums cannot hold (e.g. Display P3). A VideoFrame with no colour space at
/// all (`colorSpace: {}`, ImageDecoder `colorSpaceConversion: "none"`) stays fully unspecified.
fn rgb_copy_tags(cs: &VideoColorSpace) -> [i32; 4] {
  if cs.primaries.is_none()
    && cs.transfer.is_none()
    && cs.matrix.is_none()
    && cs.full_range.is_none()
  {
    return [
      AVColorPrimaries::Unspecified as i32,
      AVColorTransferCharacteristic::Unspecified as i32,
      AVColorSpace::Unspecified as i32,
      AVColorRange::Unspecified as i32,
    ];
  }
  let primaries = cs.primaries.map_or(-1, |p| {
    (match p {
      VideoColorPrimaries::Bt709 => AVColorPrimaries::Bt709,
      VideoColorPrimaries::Bt470bg => AVColorPrimaries::Bt470bg,
      VideoColorPrimaries::Smpte170m => AVColorPrimaries::Smpte170m,
      VideoColorPrimaries::Bt2020 => AVColorPrimaries::Bt2020,
      VideoColorPrimaries::Smpte432 => AVColorPrimaries::Smpte432,
    }) as i32
  });
  let transfer = cs.transfer.map_or(-1, |t| {
    (match t {
      VideoTransferCharacteristics::Bt709 => AVColorTransferCharacteristic::Bt709,
      VideoTransferCharacteristics::Smpte170m => AVColorTransferCharacteristic::Smpte170m,
      VideoTransferCharacteristics::Iec6196621 | VideoTransferCharacteristics::Srgb => {
        AVColorTransferCharacteristic::Iec61966_2_1
      }
      VideoTransferCharacteristics::Linear => AVColorTransferCharacteristic::Linear,
      VideoTransferCharacteristics::Pq => AVColorTransferCharacteristic::Smpte2084,
      VideoTransferCharacteristics::Hlg => AVColorTransferCharacteristic::AribStdB67,
    }) as i32
  });
  let matrix = cs.matrix.map_or(-1, |m| {
    (match m {
      VideoMatrixCoefficients::Rgb => AVColorSpace::Rgb,
      VideoMatrixCoefficients::Bt709 => AVColorSpace::Bt709,
      VideoMatrixCoefficients::Bt470bg => AVColorSpace::Bt470bg,
      VideoMatrixCoefficients::Smpte170m => AVColorSpace::Smpte170m,
      VideoMatrixCoefficients::Bt2020Ncl => AVColorSpace::Bt2020Ncl,
    }) as i32
  });
  let range = cs.full_range.map_or(-1, |full| {
    (if full {
      AVColorRange::Jpeg
    } else {
      AVColorRange::Mpeg
    }) as i32
  });
  [primaries, transfer, matrix, range]
}

/// Extract color space metadata from an FFmpeg Frame
///
/// Converts FFmpeg color metadata (primaries, transfer, colorspace, range)
/// to WebCodecs VideoColorSpace. Used for colorSpaceConversion: "default".
pub fn color_space_from_frame(frame: &Frame) -> VideoColorSpace {
  let primaries = match frame.color_primaries() {
    AVColorPrimaries::Bt709 => Some(VideoColorPrimaries::Bt709),
    AVColorPrimaries::Bt470bg => Some(VideoColorPrimaries::Bt470bg),
    AVColorPrimaries::Smpte170m => Some(VideoColorPrimaries::Smpte170m),
    AVColorPrimaries::Bt2020 => Some(VideoColorPrimaries::Bt2020),
    AVColorPrimaries::Smpte432 => Some(VideoColorPrimaries::Smpte432),
    _ => None, // Unspecified or unsupported
  };

  let transfer = match frame.color_trc() {
    AVColorTransferCharacteristic::Bt709 => Some(VideoTransferCharacteristics::Bt709),
    AVColorTransferCharacteristic::Smpte170m => Some(VideoTransferCharacteristics::Smpte170m),
    AVColorTransferCharacteristic::Iec61966_2_1 => Some(VideoTransferCharacteristics::Iec6196621),
    AVColorTransferCharacteristic::Smpte2084 => Some(VideoTransferCharacteristics::Pq),
    AVColorTransferCharacteristic::AribStdB67 => Some(VideoTransferCharacteristics::Hlg),
    _ => None, // Unspecified or unsupported
  };

  let matrix = match frame.colorspace() {
    AVColorSpace::Rgb => Some(VideoMatrixCoefficients::Rgb),
    AVColorSpace::Bt709 => Some(VideoMatrixCoefficients::Bt709),
    AVColorSpace::Bt470bg => Some(VideoMatrixCoefficients::Bt470bg),
    AVColorSpace::Smpte170m => Some(VideoMatrixCoefficients::Smpte170m),
    AVColorSpace::Bt2020Ncl => Some(VideoMatrixCoefficients::Bt2020Ncl),
    _ => None, // Unspecified or unsupported
  };

  let full_range = match frame.color_range() {
    AVColorRange::Jpeg => Some(true),  // Full range (0-255)
    AVColorRange::Mpeg => Some(false), // Limited range (16-235)
    _ => None,                         // Unspecified
  };

  VideoColorSpace::from_components(primaries, transfer, matrix, full_range)
}

/// DOMRectReadOnly - W3C WebCodecs spec compliant rect class
/// Used for codedRect and visibleRect properties
#[napi(js_name = "DOMRectReadOnly")]
#[derive(Debug, Clone)]
pub struct DOMRectReadOnly {
  x: f64,
  y: f64,
  width: f64,
  height: f64,
}

/// NaN-safe (min, max) of `origin` and `origin + extent` (Geometry Interfaces spec).
///
/// Uses plain comparisons instead of `f64::min`/`f64::max`: those return the
/// non-NaN operand, and their choice between -0.0 and +0.0 differs per CPU.
/// Ties keep `origin`, matching Chromium.
fn rect_edges(origin: f64, extent: f64) -> (f64, f64) {
  let end = origin + extent;
  // `end` is NaN whenever `origin` or `extent` is NaN, or for Infinity + -Infinity.
  if end.is_nan() {
    return (f64::NAN, f64::NAN);
  }
  let min = if end < origin { end } else { origin };
  let max = if origin < end { end } else { origin };
  (min, max)
}

/// Result of DOMRectReadOnly.toJSON()
#[napi(object, js_name = "DOMRectJSON")]
pub struct DOMRectJSON {
  pub x: f64,
  pub y: f64,
  pub width: f64,
  pub height: f64,
  pub top: f64,
  pub right: f64,
  pub bottom: f64,
  pub left: f64,
}

/// WebIDL `unrestricted double` conversion (ToNumber) of an optional member that defaults to 0.
/// Errors thrown by `valueOf`, a Symbol or a BigInt stay pending and propagate.
fn to_unrestricted_double(value: Option<Unknown>) -> Result<f64> {
  match value {
    Some(value) if value.get_type()? != ValueType::Undefined => {
      value.coerce_to_number()?.get_double()
    }
    _ => Ok(0.0),
  }
}

/// IDL order of the DOMRectReadOnly prototype members (napi-derive defines them alphabetically).
const DOM_RECT_READ_ONLY_MEMBERS: [&str; 10] = [
  "x",
  "y",
  "width",
  "height",
  "top",
  "right",
  "bottom",
  "left",
  "toJSON",
  "constructor",
];

/// Gives the exported DOMRectReadOnly class the shape Chrome's has: prototype members in IDL
/// order (what `for...in` and `Object.getOwnPropertyNames` list) and a `Symbol.toStringTag`
/// of "DOMRectReadOnly".
fn install_dom_rect_read_only_shape(env: &Env, exports: &Object) -> Result<()> {
  // The class is a function, which `get_named_property::<Object>` rejects.
  let class = exports
    .get_named_property::<Unknown>("DOMRectReadOnly")?
    .coerce_to_object()?;
  let prototype: Object = class.get_named_property("prototype")?;
  let global = env.get_global()?;
  let reflect: Object = global.get_named_property("Reflect")?;
  let get_descriptor: Function<FnArgs<(Object, &str)>, Unknown> =
    reflect.get_named_property("getOwnPropertyDescriptor")?;
  let delete_property: Function<FnArgs<(Object, &str)>, bool> =
    reflect.get_named_property("deleteProperty")?;
  let define_property: Function<FnArgs<(Object, Unknown, Unknown)>, bool> =
    reflect.get_named_property("defineProperty")?;
  // Delete and redefine each member in IDL order, so every member moves to the end in turn.
  for name in DOM_RECT_READ_ONLY_MEMBERS {
    let descriptor = get_descriptor.call((prototype, name).into())?;
    if descriptor.get_type()? == ValueType::Undefined {
      return Err(Error::new(
        Status::GenericFailure,
        format!("DOMRectReadOnly.prototype.{} is missing", name),
      ));
    }
    delete_property.call((prototype, name).into())?;
    let key = env.create_string(name)?.to_unknown();
    define_property.call((prototype, key, descriptor).into())?;
  }
  let to_string_tag: Unknown = global
    .get_named_property::<Unknown>("Symbol")?
    .coerce_to_object()?
    .get_named_property("toStringTag")?;
  let mut tag = Object::new(env)?;
  tag.set_named_property("value", "DOMRectReadOnly")?;
  tag.set_named_property("configurable", true)?;
  define_property.call((prototype, to_string_tag, tag.to_unknown()).into())?;
  Ok(())
}

/// [`install_dom_rect_read_only_shape`] for the load hook: a failure is logged and cleared, so
/// it does not make the module fail to load.
fn try_install_dom_rect_read_only_shape(env: &Env, exports: &Object) {
  if let Err(error) = install_dom_rect_read_only_shape(env, exports) {
    tracing::warn!(target: "webcodecs", error = %error, "DOMRectReadOnly shape not installed");
    let mut exception = std::ptr::null_mut();
    unsafe { napi::sys::napi_get_and_clear_last_exception(env.raw(), &mut exception) };
  }
}

#[napi]
impl DOMRectReadOnly {
  /// Create a new DOMRectReadOnly
  ///
  /// Arguments convert like WebIDL `unrestricted double` (ToNumber), as in browsers.
  #[napi(
    constructor,
    ts_args_type = "x?: number | undefined | null, y?: number | undefined | null, width?: number | undefined | null, height?: number | undefined | null"
  )]
  pub fn new(
    x: Option<Unknown>,
    y: Option<Unknown>,
    width: Option<Unknown>,
    height: Option<Unknown>,
  ) -> Result<Self> {
    Ok(DOMRectReadOnly {
      x: to_unrestricted_double(x)?,
      y: to_unrestricted_double(y)?,
      width: to_unrestricted_double(width)?,
      height: to_unrestricted_double(height)?,
    })
  }

  /// Create a DOMRectReadOnly from a DOMRectInit dictionary (Geometry Interfaces `fromRect`)
  #[napi(js_name = "fromRect", ts_args_type = "other?: DOMRectInit")]
  pub fn from_rect(env: Env, other: Option<Unknown>) -> Result<DOMRectReadOnly> {
    // undefined and null convert to an empty dictionary.
    let Some(other) = other else {
      return Ok(DOMRectReadOnly {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
      });
    };
    if !matches!(other.get_type()?, ValueType::Object | ValueType::Function) {
      return Err(throw_type_error(
        env.raw(),
        "The provided value is not of type 'DOMRectInit'.",
      ));
    }
    let other = other.coerce_to_object()?;
    let member = |name: &str| -> Result<f64> {
      to_unrestricted_double(Some(other.get_named_property::<Unknown>(name)?))
    };
    // WebIDL reads dictionary members in lexicographic order.
    let height = member("height")?;
    let width = member("width")?;
    let x = member("x")?;
    let y = member("y")?;
    Ok(DOMRectReadOnly {
      x,
      y,
      width,
      height,
    })
  }

  /// X coordinate
  #[napi(getter)]
  pub fn x(&self) -> f64 {
    self.x
  }

  /// Y coordinate
  #[napi(getter)]
  pub fn y(&self) -> f64 {
    self.y
  }

  /// Width
  #[napi(getter)]
  pub fn width(&self) -> f64 {
    self.width
  }

  /// Height
  #[napi(getter)]
  pub fn height(&self) -> f64 {
    self.height
  }

  /// Top edge: min(y, y + height), NaN if either is NaN
  #[napi(getter)]
  pub fn top(&self) -> f64 {
    rect_edges(self.y, self.height).0
  }

  /// Right edge: max(x, x + width), NaN if either is NaN
  #[napi(getter)]
  pub fn right(&self) -> f64 {
    rect_edges(self.x, self.width).1
  }

  /// Bottom edge: max(y, y + height), NaN if either is NaN
  #[napi(getter)]
  pub fn bottom(&self) -> f64 {
    rect_edges(self.y, self.height).1
  }

  /// Left edge: min(x, x + width), NaN if either is NaN
  #[napi(getter)]
  pub fn left(&self) -> f64 {
    rect_edges(self.x, self.width).0
  }

  /// Convert to JSON (W3C spec uses toJSON)
  #[napi(js_name = "toJSON")]
  pub fn to_json(&self) -> DOMRectJSON {
    DOMRectJSON {
      x: self.x,
      y: self.y,
      width: self.width,
      height: self.height,
      top: self.top(),
      right: self.right(),
      bottom: self.bottom(),
      left: self.left(),
    }
  }
}

/// VideoFrameMetadata - metadata associated with a VideoFrame (W3C spec)
/// Members defined in VideoFrame Metadata Registry - currently empty per spec
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoFrameMetadata {}

/// Options for creating a VideoFrame from buffer data (VideoFrameBufferInit per spec)
pub struct VideoFrameBufferInit {
  /// Pixel format (required)
  pub format: VideoPixelFormat,
  /// Coded width in pixels (required)
  pub coded_width: u32,
  /// Coded height in pixels (required)
  pub coded_height: u32,
  /// Timestamp in microseconds (required)
  pub timestamp: i64,
  /// Duration in microseconds (optional)
  /// Note: W3C spec uses unsigned long long, but JS number can represent up to 2^53 safely
  pub duration: Option<i64>,
  /// Layout for input planes (optional, default is tightly-packed)
  pub layout: Option<Vec<PlaneLayout>>,
  /// Visible rect within coded size (optional, default is full coded size at 0,0)
  pub visible_rect: Option<DOMRectInit>,
  /// Rotation in degrees clockwise (0, 90, 180, 270) - default 0
  pub rotation: Option<f64>,
  /// Horizontal flip - default false
  pub flip: Option<bool>,
  /// Display width (defaults to visible width or coded_width)
  pub display_width: Option<u32>,
  /// Display height (defaults to visible height or coded_height)
  pub display_height: Option<u32>,
  /// Color space parameters (uses init object)
  pub color_space: Option<VideoColorSpaceInit>,
  /// Metadata associated with the frame
  pub metadata: Option<VideoFrameMetadata>,
  /// ArrayBuffers to transfer (W3C spec - ignored in Node.js, we always copy)
  pub transfer: Option<Vec<Uint8Array>>,
}

/// Helper to throw TypeError and return an error
fn throw_type_error(env: napi::sys::napi_env, message: &str) -> Error {
  let env_wrapper = Env::from_raw(env);
  let _ = env_wrapper.throw_type_error(message, None);
  Error::new(Status::InvalidArg, message)
}

impl FromNapiValue for VideoFrameBufferInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // Parse format string and validate - required field
    let format_str: Option<String> = obj.get("format")?;
    let format = match format_str {
      Some(s) => match s.as_str() {
        "I420" => VideoPixelFormat::I420,
        "I420A" => VideoPixelFormat::I420A,
        "I422" => VideoPixelFormat::I422,
        "I422A" => VideoPixelFormat::I422A,
        "I444" => VideoPixelFormat::I444,
        "I444A" => VideoPixelFormat::I444A,
        "I420P10" => VideoPixelFormat::I420P10,
        "I420AP10" => VideoPixelFormat::I420AP10,
        "I422P10" => VideoPixelFormat::I422P10,
        "I422AP10" => VideoPixelFormat::I422AP10,
        "I444P10" => VideoPixelFormat::I444P10,
        "I444AP10" => VideoPixelFormat::I444AP10,
        "I420P12" => VideoPixelFormat::I420P12,
        "I422P12" => VideoPixelFormat::I422P12,
        "I444P12" => VideoPixelFormat::I444P12,
        "NV12" => VideoPixelFormat::NV12,
        "NV21" => VideoPixelFormat::NV21,
        "RGBA" => VideoPixelFormat::RGBA,
        "RGBX" => VideoPixelFormat::RGBX,
        "BGRA" => VideoPixelFormat::BGRA,
        "BGRX" => VideoPixelFormat::BGRX,
        _ => return Err(throw_type_error(env, &format!("Invalid format: {}", s))),
      },
      None => return Err(throw_type_error(env, "format is required")),
    };

    // codedWidth - required
    let coded_width: u32 = match obj.get("codedWidth")? {
      Some(w) => w,
      None => return Err(throw_type_error(env, "codedWidth is required")),
    };

    // codedHeight - required
    let coded_height: u32 = match obj.get("codedHeight")? {
      Some(h) => h,
      None => return Err(throw_type_error(env, "codedHeight is required")),
    };

    // timestamp - required per WebIDL [EnforceRange] long long
    // Accept f64 and manually convert per WebIDL spec to handle floating-point values
    let env_wrapper = Env::from_raw(env);
    let timestamp_f64: Option<f64> = obj.get("timestamp")?;
    let timestamp = match timestamp_f64 {
      Some(ts) => enforce_range_long_long(&env_wrapper, ts, "timestamp")?,
      None => return Err(throw_type_error(env, "timestamp is required")),
    };

    // Duration is optional per WebIDL [EnforceRange] unsigned long long
    let duration_f64: Option<f64> = obj.get("duration")?;
    let duration =
      enforce_range_unsigned_long_long_optional(&env_wrapper, duration_f64, "duration")?
        .map(|v| v.min(i64::MAX as u64) as i64);
    let layout: Option<Vec<PlaneLayout>> = obj.get("layout")?;
    let visible_rect: Option<DOMRectInit> = obj.get("visibleRect")?;
    let rotation: Option<f64> = obj.get("rotation")?;
    let flip: Option<bool> = obj.get("flip")?;
    let display_width: Option<u32> = obj.get("displayWidth")?;
    let display_height: Option<u32> = obj.get("displayHeight")?;
    let color_space: Option<VideoColorSpaceInit> = obj.get("colorSpace")?;
    let metadata: Option<VideoFrameMetadata> = obj.get("metadata")?;
    let transfer: Option<Vec<Uint8Array>> = obj.get("transfer")?;

    Ok(VideoFrameBufferInit {
      format,
      coded_width,
      coded_height,
      timestamp,
      duration,
      layout,
      visible_rect,
      rotation,
      flip,
      display_width,
      display_height,
      color_space,
      metadata,
      transfer,
    })
  }
}

/// Options for creating a VideoFrame from an image source (VideoFrameInit per spec)
#[napi(object)]
pub struct VideoFrameInit {
  /// Timestamp in microseconds (required per spec when creating from VideoFrame)
  pub timestamp: Option<i64>,
  /// Duration in microseconds (optional)
  pub duration: Option<i64>,
  /// Alpha handling: "keep" (default) or "discard"
  pub alpha: Option<String>,
  /// Visible rect (optional)
  pub visible_rect: Option<DOMRectInit>,
  /// Rotation in degrees clockwise (0, 90, 180, 270) - default 0
  pub rotation: Option<f64>,
  /// Horizontal flip - default false
  pub flip: Option<bool>,
  /// Display width (optional)
  pub display_width: Option<u32>,
  /// Display height (optional)
  pub display_height: Option<u32>,
  /// Metadata associated with the frame
  pub metadata: Option<VideoFrameMetadata>,
}

/// Unified init type for VideoFrame constructor
/// Handles both VideoFrameBufferInit (buffer) and VideoFrameInit (frame clone) cases
/// Required field validation happens in constructor based on source type
pub struct VideoFrameConstructorInit {
  // Fields from VideoFrameBufferInit (required for buffer, optional for frame clone)
  pub format: Option<VideoPixelFormat>,
  pub coded_width: Option<u32>,
  pub coded_height: Option<u32>,
  pub timestamp: Option<i64>,
  // Optional fields (both cases)
  pub duration: Option<i64>,
  pub layout: Option<Vec<PlaneLayout>>,
  pub visible_rect: Option<DOMRectInit>,
  pub rotation: Option<f64>,
  pub flip: Option<bool>,
  pub display_width: Option<u32>,
  pub display_height: Option<u32>,
  pub color_space: Option<VideoColorSpaceInit>,
  pub metadata: Option<VideoFrameMetadata>,
  pub transfer: Option<Vec<Uint8Array>>,
  // Only for frame clone (VideoFrameInit)
  pub alpha: Option<String>,
}

/// Which WebIDL dictionary a VideoFrame init is. VideoFrameBufferInit has `colorSpace`;
/// VideoFrameInit (VideoFrame and canvas sources) has none, so Chrome never reads it there.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InitDictionary {
  Buffer,
  Image,
}

impl FromNapiValue for VideoFrameConstructorInit {
  unsafe fn from_napi_value(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
  ) -> Result<Self> {
    unsafe { Self::read(env, value, InitDictionary::Buffer) }
  }
}

impl VideoFrameConstructorInit {
  unsafe fn read(
    env: napi::sys::napi_env,
    value: napi::sys::napi_value,
    dictionary: InitDictionary,
  ) -> Result<Self> {
    let obj = unsafe { Object::from_napi_value(env, value)? };

    // Parse format string (optional - only required for buffer constructor)
    let format_str: Option<String> = obj.get("format")?;
    let format = match format_str {
      Some(s) => match s.as_str() {
        "I420" => Some(VideoPixelFormat::I420),
        "I420A" => Some(VideoPixelFormat::I420A),
        "I422" => Some(VideoPixelFormat::I422),
        "I422A" => Some(VideoPixelFormat::I422A),
        "I444" => Some(VideoPixelFormat::I444),
        "I444A" => Some(VideoPixelFormat::I444A),
        "I420P10" => Some(VideoPixelFormat::I420P10),
        "I420AP10" => Some(VideoPixelFormat::I420AP10),
        "I422P10" => Some(VideoPixelFormat::I422P10),
        "I422AP10" => Some(VideoPixelFormat::I422AP10),
        "I444P10" => Some(VideoPixelFormat::I444P10),
        "I444AP10" => Some(VideoPixelFormat::I444AP10),
        "I420P12" => Some(VideoPixelFormat::I420P12),
        "I422P12" => Some(VideoPixelFormat::I422P12),
        "I444P12" => Some(VideoPixelFormat::I444P12),
        "NV12" => Some(VideoPixelFormat::NV12),
        "NV21" => Some(VideoPixelFormat::NV21),
        "RGBA" => Some(VideoPixelFormat::RGBA),
        "RGBX" => Some(VideoPixelFormat::RGBX),
        "BGRA" => Some(VideoPixelFormat::BGRA),
        "BGRX" => Some(VideoPixelFormat::BGRX),
        _ => return Err(throw_type_error(env, &format!("Invalid format: {}", s))),
      },
      None => None,
    };

    // All fields optional - validation happens in constructor based on source type
    // Parse timestamp and duration as f64 for WebIDL [EnforceRange] long long compliance
    let env_wrapper = Env::from_raw(env);
    let timestamp_f64: Option<f64> = obj.get("timestamp")?;
    let timestamp = enforce_range_long_long_optional(&env_wrapper, timestamp_f64, "timestamp")?;
    let duration_f64: Option<f64> = obj.get("duration")?;
    let duration =
      enforce_range_unsigned_long_long_optional(&env_wrapper, duration_f64, "duration")?
        .map(|v| v.min(i64::MAX as u64) as i64);

    // One reader serves both VideoFrameBufferInit and VideoFrameInit, so the members keep the
    // order it always used. The checked members sit where WebIDL (lexicographic) order puts
    // them relative to the others: displayHeight/displayWidth after codedWidth/codedHeight and
    // before layout, rotation after flip. Before displayHeight, VideoFrameBufferInit has
    // colorSpace and VideoFrameInit has alpha.
    let coded_width: Option<u32> = obj.get("codedWidth")?;
    let coded_height: Option<u32> = obj.get("codedHeight")?;
    let mut color_space: Option<VideoColorSpaceInit> = None;
    let mut alpha: Option<String> = None;
    match dictionary {
      InitDictionary::Buffer => color_space = obj.get("colorSpace")?,
      InitDictionary::Image => alpha = obj.get("alpha")?,
    }
    // displayWidth/displayHeight are WebIDL [EnforceRange] unsigned long: a plain u32
    // conversion would wrap -1 or 2^32 + 1 into range instead of throwing. WebIDL converts
    // dictionary members in lexicographic order and stops at the first failure, so
    // displayHeight is read and checked before the displayWidth getter runs. null is 0.
    let display_height = super::error::enforce_range_unsigned_long_optional(
      &env_wrapper,
      super::error::get_number_member(&obj, "displayHeight")?,
      "displayHeight",
    )?;
    let display_width = super::error::enforce_range_unsigned_long_optional(
      &env_wrapper,
      super::error::get_number_member(&obj, "displayWidth")?,
      "displayWidth",
    )?;
    let layout: Option<Vec<PlaneLayout>> = obj.get("layout")?;
    let visible_rect: Option<DOMRectInit> = obj.get("visibleRect")?;
    let flip: Option<bool> = obj.get("flip")?;
    // rotation is a WebIDL (restricted) double. null is 0, as for displayWidth/displayHeight.
    let rotation: Option<f64> = super::error::get_number_member(&obj, "rotation")?;
    if rotation.is_some_and(|rotation| !rotation.is_finite()) {
      return Err(throw_type_error(env, "rotation must be a finite number"));
    }
    // VideoFrameInit has no colorSpace, but the canvas constructor still honours it: read it
    // after the members Chrome reads.
    if dictionary == InitDictionary::Image {
      color_space = obj.get("colorSpace")?;
    }

    Ok(VideoFrameConstructorInit {
      format,
      coded_width,
      coded_height,
      timestamp,
      duration,
      layout,
      visible_rect,
      rotation,
      flip,
      display_width,
      display_height,
      color_space,
      metadata: obj.get("metadata")?,
      transfer: obj.get("transfer")?,
      alpha: match dictionary {
        InitDictionary::Buffer => obj.get("alpha")?,
        InitDictionary::Image => alpha,
      },
    })
  }
}

/// predefined RGB target color spaces.
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredefinedColorSpace {
  #[napi(value = "srgb")]
  Srgb,
  #[napi(value = "display-p3")]
  DisplayP3,
}

/// options for copyTo operation.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameCopyToOptions {
  /// target RGB color space.
  pub color_space: Option<PredefinedColorSpace>,
  /// Target pixel format (for format conversion)
  pub format: Option<VideoPixelFormat>,
  /// Region to copy (not yet implemented)
  pub rect: Option<DOMRectInit>,
  /// Layout for output planes
  pub layout: Option<Vec<PlaneLayout>>,
}

/// DOMRectInit for specifying regions
#[napi(object, js_name = "DOMRectInit")]
#[derive(Debug, Clone)]
pub struct DOMRectInit {
  pub x: Option<f64>,
  pub y: Option<f64>,
  pub width: Option<f64>,
  pub height: Option<f64>,
}

/// Layout information for a single plane per WebCodecs spec
#[napi(object)]
#[derive(Debug, Clone)]
pub struct PlaneLayout {
  /// Byte offset from the start of the buffer to the start of the plane
  pub offset: u32,
  /// Number of bytes per row (stride)
  pub stride: u32,
}

/// PredefinedColorSpace values that Chrome knows but cannot copy to: NotSupportedError, not
/// TypeError.
const UNSUPPORTED_COPY_COLOR_SPACES: [&str; 6] = [
  "rec2020",
  "srgb-linear",
  "display-p3-linear",
  "rec2100-hlg",
  "rec2100-pq",
  "rec2100-linear",
];
const UNSUPPORTED_COLOR_SPACE_MESSAGE: &str =
  "This pixel conversion to this color space is not supported.";

/// copyTo / allocationSize options after WebIDL conversion.
struct CopyToOptions {
  options: VideoFrameCopyToOptions,
  /// colorSpace is one of `UNSUPPORTED_COPY_COLOR_SPACES`. Like Chrome, this NotSupportedError
  /// comes after the closed check.
  unsupported_color_space: bool,
}

impl CopyToOptions {
  /// Convert a VideoFrameCopyToOptions dictionary the way Chrome's bindings do. Members are read
  /// once each, in lexicographic order (colorSpace, format, layout, rect), and an undefined member
  /// is absent. Bad values give a "TypeError: ..." error. An exception from a getter, `toString`,
  /// `valueOf` or the `layout` iterator stays pending.
  fn from_js(value: Unknown) -> Result<Self> {
    let mut options = VideoFrameCopyToOptions {
      color_space: None,
      format: None,
      rect: None,
      layout: None,
    };
    let mut unsupported_color_space = false;
    let Some(dict) = webidl_dictionary(value, "VideoFrameCopyToOptions")? else {
      return Ok(Self {
        options,
        unsupported_color_space,
      });
    };
    if let Some(value) = webidl_member(&dict, "colorSpace")? {
      let name = value.coerce_to_string()?;
      match unsafe { PredefinedColorSpace::from_napi_value(name.value().env, name.raw()) } {
        Ok(color_space) => options.color_space = Some(color_space),
        Err(_) => {
          let name = name.into_utf8()?.into_owned()?;
          if !UNSUPPORTED_COPY_COLOR_SPACES.contains(&name.as_str()) {
            return Err(invalid_enum_value(&name, "PredefinedColorSpace"));
          }
          unsupported_color_space = true;
        }
      }
    }
    if let Some(value) = webidl_member(&dict, "format")? {
      let name = value.coerce_to_string()?;
      match unsafe { VideoPixelFormat::from_napi_value(name.value().env, name.raw()) } {
        Ok(format) => options.format = Some(format),
        Err(_) => {
          let name = name.into_utf8()?.into_owned()?;
          return Err(invalid_enum_value(&name, "VideoPixelFormat"));
        }
      }
    }
    if let Some(value) = webidl_member(&dict, "layout")? {
      options.layout = Some(plane_layouts_from_js(value)?);
    }
    if let Some(value) = webidl_member(&dict, "rect")? {
      options.rect = Some(dom_rect_init_from_js(value)?);
    }
    Ok(Self {
      options,
      unsupported_color_space,
    })
  }
}

fn invalid_enum_value(value: &str, enum_name: &str) -> Error {
  type_error(&format!(
    "The provided value '{}' is not a valid enum value of type {}.",
    value, enum_name
  ))
}

/// WebIDL dictionary conversion: `None` (every member absent) for undefined or null, the object
/// for an object or a function, and a TypeError for any other value.
fn webidl_dictionary<'env>(value: Unknown<'env>, type_name: &str) -> Result<Option<Object<'env>>> {
  match value.get_type()? {
    ValueType::Undefined | ValueType::Null => Ok(None),
    ValueType::Object | ValueType::Function => {
      Ok(Some(Object::from_raw(value.value().env, value.raw())))
    }
    _ => Err(type_error(&format!(
      "The provided value is not of type '{}'.",
      type_name
    ))),
  }
}

/// A dictionary member, read once with [[Get]]; `None` when it is undefined.
fn webidl_member<'env>(dict: &Object<'env>, name: &str) -> Result<Option<Unknown<'env>>> {
  let value: Unknown = dict.get_named_property(name)?;
  Ok((value.get_type()? != ValueType::Undefined).then_some(value))
}

/// `sequence<PlaneLayout>`, converted the way Chrome 155 does it:
/// - A JS Array (not a Proxy) is read by index, and its `length` is read again before each entry.
///   Like Chrome's fast path (WebIDL itself would iterate), its `@@iterator` and the array
///   iterator's `next` are not used, even when page code replaced them.
/// - Any other object goes through its `@@iterator`. Chrome reads each step's `value` before its
///   `done`, converts the entry before the next step, and does not call `return()` when an entry
///   fails.
///
/// A non-object, or an object without a callable `@@iterator`, is a TypeError. An exception from
/// the iterator or from an entry stays pending.
fn plane_layouts_from_js(value: Unknown) -> Result<Vec<PlaneLayout>> {
  if !is_js_object(&value)? {
    return Err(type_error(
      "layout: The provided value cannot be converted to a sequence.",
    ));
  }
  let env = value.value().env;
  let object = Object::from_raw(env, value.raw());
  let mut layouts = Vec::new();
  if value.is_array()? {
    let mut index = 0;
    while index < object.get_array_length_unchecked()? {
      layouts.push(plane_layout_from_js(object.get_element::<Unknown>(index)?)?);
      index += 1;
    }
    return Ok(layouts);
  }

  let method: Unknown = object.get_property(iterator_symbol(env)?)?;
  match method.get_type()? {
    ValueType::Function => {}
    ValueType::Undefined | ValueType::Null => {
      return Err(type_error(
        "layout: The object must have a callable @@iterator property.",
      ));
    }
    _ => return Err(type_error("layout: @@iterator must be a callable.")),
  }
  let iterator = call_with_no_arguments(env, value.raw(), method.raw())?;
  if !is_js_object(&iterator)? {
    return Err(type_error("layout: Iterator object must be an object."));
  }
  let next: Unknown = Object::from_raw(env, iterator.raw()).get_named_property("next")?;
  if next.get_type()? != ValueType::Function {
    return Err(type_error("layout: Expected next() function on iterator."));
  }
  loop {
    let result = call_with_no_arguments(env, iterator.raw(), next.raw())?;
    if !is_js_object(&result)? {
      return Err(type_error(
        "layout: Expected iterator.next() to return an Object.",
      ));
    }
    let result = Object::from_raw(env, result.raw());
    let entry: Unknown = result.get_named_property("value")?;
    let done: Unknown = result.get_named_property("done")?;
    if done.coerce_to_bool()? {
      return Ok(layouts);
    }
    layouts.push(plane_layout_from_js(entry)?);
  }
}

/// One `PlaneLayout` dictionary: required `[EnforceRange] unsigned long` members, read in
/// lexicographic order.
fn plane_layout_from_js(value: Unknown) -> Result<PlaneLayout> {
  let plane = webidl_dictionary(value, "PlaneLayout")?;
  let member = |name: &str| -> Result<u32> {
    let value = match &plane {
      Some(plane) => webidl_member(plane, name)?,
      None => None,
    };
    let Some(value) = value else {
      return Err(type_error(&format!(
        "PlaneLayout.{}: required member is undefined.",
        name
      )));
    };
    let number = value.coerce_to_number()?.get_double()?;
    if !number.is_finite() {
      return Err(type_error(&format!(
        "PlaneLayout.{} is not of type 'unsigned long'.",
        name
      )));
    }
    let number = number.trunc();
    if !(0.0..=f64::from(u32::MAX)).contains(&number) {
      return Err(type_error(&format!(
        "PlaneLayout.{} is outside the 'unsigned long' value range.",
        name
      )));
    }
    Ok(number as u32)
  };
  let offset = member("offset")?;
  let stride = member("stride")?;
  Ok(PlaneLayout { offset, stride })
}

/// Whether `value` is an ECMAScript Object (a function is one).
fn is_js_object(value: &Unknown) -> Result<bool> {
  Ok(matches!(
    value.get_type()?,
    ValueType::Object | ValueType::Function
  ))
}

/// `function.call(this)`. A JS exception stays pending.
fn call_with_no_arguments<'env>(
  env: napi::sys::napi_env,
  this: napi::sys::napi_value,
  function: napi::sys::napi_value,
) -> Result<Unknown<'env>> {
  let mut result = std::ptr::null_mut();
  napi::check_status!(unsafe {
    napi::sys::napi_call_function(env, this, function, 0, std::ptr::null(), &mut result)
  })?;
  Ok(unsafe { Unknown::from_raw_unchecked(env, result) })
}

/// `DOMRectInit`: `unrestricted double` members that default to 0, read in lexicographic order.
/// Null converts to the all-zero rect.
fn dom_rect_init_from_js(value: Unknown) -> Result<DOMRectInit> {
  let rect = webidl_dictionary(value, "DOMRectInit")?;
  let member = |name: &str| -> Result<f64> {
    let value = match &rect {
      Some(rect) => webidl_member(rect, name)?,
      None => None,
    };
    match value {
      Some(value) => value.coerce_to_number()?.get_double(),
      None => Ok(0.0),
    }
  };
  let height = member("height")?;
  let width = member("width")?;
  let x = member("x")?;
  let y = member("y")?;
  Ok(DOMRectInit {
    x: Some(x),
    y: Some(y),
    width: Some(width),
    height: Some(height),
  })
}

/// Rectangle for specifying a region
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameRect {
  pub x: u32,
  pub y: u32,
  pub width: u32,
  pub height: u32,
}

/// Owned native image data with no eight-bit staging or public pixel-format alias.
#[derive(Clone)]
enum VideoFrameResource {
  Software {
    frame: Arc<RwLock<Frame>>,
    format: VideoPixelFormat,
  },
  Rgba16Float {
    width: u32,
    height: u32,
    pixels: Arc<[u16]>,
  },
}

impl VideoFrameResource {
  fn dimensions(&self) -> (u32, u32) {
    match self {
      Self::Software { frame, .. } => {
        let frame = frame.read();
        (frame.width(), frame.height())
      }
      Self::Rgba16Float { width, height, .. } => (*width, *height),
    }
  }

  fn format(&self) -> Option<VideoPixelFormat> {
    match self {
      Self::Software { format, .. } => Some(*format),
      Self::Rgba16Float { .. } => None,
    }
  }

  fn software(&self) -> Result<(&Arc<RwLock<Frame>>, VideoPixelFormat)> {
    match self {
      Self::Software { frame, format } => Ok((frame, *format)),
      Self::Rgba16Float { .. } => Err(not_supported_error(
        "opaque float image cannot be copied to a WebCodecs pixel format",
      )),
    }
  }

  fn discard_alpha(&self) -> Result<Self> {
    match self {
      Self::Software { frame, format } if format.has_alpha() => {
        let source = frame.read();
        let target_format = format.without_alpha();
        let scaler = Scaler::new_converter(
          source.width(),
          source.height(),
          format.to_av_format(),
          target_format.to_av_format(),
        )
        .map_err(|e| {
          Error::new(
            Status::GenericFailure,
            format!("Failed to create converter for alpha discard: {}", e),
          )
        })?;
        let converted = scaler.scale_alloc(&source).map_err(|e| {
          Error::new(
            Status::GenericFailure,
            format!("Failed to convert frame for alpha discard: {}", e),
          )
        })?;
        Ok(Self::Software {
          frame: converted.into_shared(),
          format: target_format,
        })
      }
      Self::Rgba16Float {
        width,
        height,
        pixels,
      } => {
        let mut pixels = pixels.to_vec();
        for alpha in pixels.iter_mut().skip(3).step_by(4) {
          *alpha = 0x3c00;
        }
        Ok(Self::Rgba16Float {
          width: *width,
          height: *height,
          pixels: pixels.into(),
        })
      }
      _ => Ok(self.clone()),
    }
  }
}

/// Internal state for VideoFrame
struct VideoFrameInner {
  resource: VideoFrameResource,
  timestamp_us: i64,
  duration_us: Option<i64>,
  /// Visible rectangle internal slots per W3C spec
  visible_left: u32,
  visible_top: u32,
  visible_width: u32,
  visible_height: u32,
  /// Natural (unrotated) display size, like Chromium's natural_size().
  /// The displayWidth/displayHeight getters swap it for rotation 90/270.
  /// The spec text stores the display size already rotated instead.
  display_width: u32,
  display_height: u32,
  /// Rotation in degrees clockwise (0, 90, 180, 270)
  rotation: f64,
  /// Horizontal flip
  flip: bool,
  color_space: VideoColorSpace,
}

/// Get (horizontal_factor, vertical_factor) sub-sampling for chroma planes
/// Per W3C spec - used for Verify Rect Offset Alignment algorithm
fn get_subsampling_factors(format: VideoPixelFormat) -> (u32, u32) {
  match format {
    // 4:2:0 formats - 2x2 subsampling
    VideoPixelFormat::I420
    | VideoPixelFormat::I420A
    | VideoPixelFormat::I420P10
    | VideoPixelFormat::I420AP10
    | VideoPixelFormat::I420P12
    | VideoPixelFormat::NV12
    | VideoPixelFormat::NV21 => (2, 2),

    // 4:2:2 formats - 2x1 subsampling (horizontal only)
    VideoPixelFormat::I422
    | VideoPixelFormat::I422A
    | VideoPixelFormat::I422P10
    | VideoPixelFormat::I422AP10
    | VideoPixelFormat::I422P12 => (2, 1),

    // 4:4:4 formats and RGB - no subsampling
    VideoPixelFormat::I444
    | VideoPixelFormat::I444A
    | VideoPixelFormat::I444P10
    | VideoPixelFormat::I444AP10
    | VideoPixelFormat::I444P12
    | VideoPixelFormat::RGBA
    | VideoPixelFormat::RGBX
    | VideoPixelFormat::BGRA
    | VideoPixelFormat::BGRX => (1, 1),
  }
}

/// Calculate plane end offset with overflow checking
///
/// Computes `offset + stride * height` using u64 to prevent u32 overflow.
/// Returns TypeError on arithmetic overflow per W3C WebCodecs spec.
///
/// This is used to validate custom layout parameters and prevent buffer overflows
/// when large offset/stride values (like WPT's "address overflow" case: offset = 2^32 - 2)
/// would wrap in u32 arithmetic.
fn calculate_plane_end_checked(offset: u32, stride: u32, height: u32) -> Result<u64> {
  let stride_u64 = stride as u64;
  let height_u64 = height as u64;
  let offset_u64 = offset as u64;

  let plane_size = stride_u64.checked_mul(height_u64).ok_or_else(|| {
    Error::new(
      Status::InvalidArg,
      "TypeError: layout stride * height overflow",
    )
  })?;

  offset_u64.checked_add(plane_size).ok_or_else(|| {
    Error::new(
      Status::InvalidArg,
      "TypeError: layout offset + size overflow",
    )
  })
}

/// The bytes a Uint8Array views right now. They stay valid only until JS runs again, since JS
/// can detach, transfer or shrink the buffer: look them up after every getter has run and finish
/// writing before calling back into JS.
unsafe fn typed_array_bytes<'a>(
  env: napi::sys::napi_env,
  view: napi::sys::napi_value,
) -> Result<&'a mut [u8]> {
  let mut length = 0;
  let mut data = std::ptr::null_mut();
  napi::check_status!(
    unsafe {
      napi::sys::napi_get_typedarray_info(
        env,
        view,
        std::ptr::null_mut(),
        &mut length,
        &mut data,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
      )
    },
    "Get TypedArray info failed"
  )?;
  // A detached or out-of-bounds view reports length 0.
  if length == 0 || data.is_null() {
    return Ok(&mut []);
  }
  // `length` counts elements, and callers pass a Uint8Array.
  Ok(unsafe { std::slice::from_raw_parts_mut(data.cast::<u8>(), length) })
}

/// How many row bands an RGB copy of `width` x `height` is split into: one per 256K pixels, at
/// most 4, and at most the number of cores.
fn rgb_copy_bands(width: u32, height: u32) -> u32 {
  const PIXELS_PER_BAND: u64 = 256 * 1024;
  const MAX_BANDS: u64 = 4;
  static CORES: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
  let cores =
    *CORES.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u64));
  let by_size = width as u64 * height as u64 / PIXELS_PER_BAND;
  by_size.min(MAX_BANDS).min(cores).min(height as u64).max(1) as u32
}

/// `Scaler::copy_rect_to_rgb`, with a large rect split into row bands that run on scoped threads.
/// Each output row depends only on its own source row, so the bytes are the same either way.
/// Every band is written before this returns, and the calling (JS) thread waits meanwhile.
#[allow(clippy::too_many_arguments)]
fn copy_rect_to_rgb_in_bands(
  src: &Frame,
  rect: (u32, u32, u32, u32),
  tags: [i32; 4],
  target_p3: bool,
  bgr: bool,
  same_format: bool,
  src_x: bool,
  dst: &mut [u8],
  stride: usize,
) -> Result<()> {
  let (x, y, width, height) = rect;
  let copy = |top: u32, rows: u32, band: &mut [u8]| {
    Scaler::copy_rect_to_rgb(
      src,
      (x, y + top, width, rows),
      tags,
      target_p3,
      bgr,
      same_format,
      src_x,
      band,
      stride,
    )
    .map_err(|e| not_supported_error(&e.to_string()))
  };
  let bands = rgb_copy_bands(width, height);
  if bands <= 1 {
    return copy(0, height, dst);
  }

  // Disjoint parts of `dst`, one per band. The last band takes the rest.
  let rows_per_band = height.div_ceil(bands);
  let mut parts = Vec::with_capacity(bands as usize);
  let mut rest = dst;
  let mut top = 0;
  while top < height {
    let rows = rows_per_band.min(height - top);
    let take = if top + rows == height {
      rest.len()
    } else {
      (rows as usize * stride).min(rest.len())
    };
    let (band, tail) = std::mem::take(&mut rest).split_at_mut(take);
    parts.push(parking_lot::Mutex::new(Some((top, rows, band))));
    rest = tail;
    top += rows;
  }

  // Whoever takes a band from its slot first copies it. This thread then takes every band left,
  // which also covers a thread that failed to spawn or has not started yet.
  let run = |part: &parking_lot::Mutex<Option<(u32, u32, &mut [u8])>>| {
    let taken = part.lock().take();
    match taken {
      Some((top, rows, band)) => copy(top, rows, band),
      None => Ok(()),
    }
  };
  std::thread::scope(|scope| {
    let handles: Vec<_> = parts[1..]
      .iter()
      .filter_map(|part| {
        std::thread::Builder::new()
          .spawn_scoped(scope, || run(part))
          .ok()
      })
      .collect();
    let mut result = parts.iter().try_for_each(run);
    for handle in handles {
      let band = handle
        .join()
        .unwrap_or_else(|_| Err(Error::new(Status::GenericFailure, "copy thread panicked")));
      if result.is_ok() {
        result = band;
      }
    }
    result
  })
}

/// The engine's own `ArrayBuffer.prototype.resizable` / `.detached` and
/// `SharedArrayBuffer.prototype.growable` getters, captured when the module loads into an env.
/// Calling them reads the buffer's internal state and runs no page code, like Chrome: a later
/// patch to those prototypes, or an own property on a buffer, does not change the result.
/// A getter that was missing at load time is null and reads as `false`.
///
/// Limit: Node-API has no call that reads these flags (nothing for resizable or growable, and
/// `napi_is_detached_arraybuffer` needs napi7), so they are looked up through the env's globals.
/// Code that patches those prototypes, or `Object.getOwnPropertyDescriptor`, before the module
/// loads gets its own getter captured: it then runs on every copyTo and can change the resizable,
/// growable and detached answers, so the Chrome parity checks can go wrong. Memory safety does not
/// depend on them. copyTo reads the destination's pointer and length after every JS call that
/// comes before the write, and runs no JS while it writes (the detached check runs only on the
/// path that writes nothing). Only the thread that owns an ArrayBuffer can resize or detach it,
/// and that thread is inside copyTo; a growable SharedArrayBuffer can only grow.
#[derive(Clone, Copy)]
struct BufferFlagGetters {
  env: napi::sys::napi_env,
  resizable: napi::sys::napi_ref,
  detached: napi::sys::napi_ref,
  growable: napi::sys::napi_ref,
}

thread_local! {
  // Each env runs on its own thread. Its cleanup hook empties the slot.
  static BUFFER_FLAG_GETTERS: std::cell::Cell<Option<BufferFlagGetters>> =
    const { std::cell::Cell::new(None) };
}

/// Runs each time the module loads into an env (the main thread or a worker), after napi-rs has
/// defined the exported classes and before page code can use the module. napi-rs keeps one
/// `module_exports` hook per crate, and a second one silently replaces this one: add new
/// load-time work here, so that a failed step cannot skip the others.
#[napi(module_exports)]
// Only napi's load-time registration calls this, and `cargo test` builds leave it out.
#[cfg_attr(test, allow(dead_code))]
pub fn on_module_load(exports: Object, env: Env) -> Result<()> {
  // Codec event listeners and VideoFrame close observers. Cannot fail.
  super::event_target::install_callback_anchor_store(&env);
  super::buffer_source::capture_typed_array_set(&env)?;
  capture_buffer_flag_getters(&env)?;
  capture_iterator_symbol(&env)?;
  try_install_dom_rect_read_only_shape(&env, &exports);
  Ok(())
}

/// Captures the buffer flag getters of `env`, once per env.
fn capture_buffer_flag_getters(env: &Env) -> Result<()> {
  if BUFFER_FLAG_GETTERS
    .get()
    .is_some_and(|getters| getters.env == env.raw())
  {
    return Ok(());
  }
  let getters = BufferFlagGetters {
    env: env.raw(),
    resizable: prototype_getter_ref(env, c"ArrayBuffer", c"resizable")?,
    detached: prototype_getter_ref(env, c"ArrayBuffer", c"detached")?,
    growable: prototype_getter_ref(env, c"SharedArrayBuffer", c"growable")?,
  };
  unsafe extern "C" fn forget(env: *mut std::ffi::c_void) {
    let _ = BUFFER_FLAG_GETTERS.try_with(|slot| {
      if slot.get().is_some_and(|getters| getters.env.cast() == env) {
        slot.set(None);
      }
    });
  }
  // Node deletes the references of an env when it tears the env down.
  napi::check_status!(unsafe {
    napi::sys::napi_add_env_cleanup_hook(env.raw(), Some(forget), env.raw().cast())
  })?;
  BUFFER_FLAG_GETTERS.set(Some(getters));
  Ok(())
}

thread_local! {
  // The `Symbol.iterator` refs of the envs on this thread, by env pointer. Each env's one cleanup
  // hook removes its entry before the env goes away, so a reused pointer never finds a stale ref.
  static ITERATOR_SYMBOLS: RefCell<HashMap<usize, napi::sys::napi_ref>> =
    RefCell::new(HashMap::new());
}

/// Captures `Symbol.iterator` of `env`, once per env.
fn capture_iterator_symbol(env: &Env) -> Result<()> {
  use napi::sys;
  let key = env.raw() as usize;
  if ITERATOR_SYMBOLS.with(|symbols| symbols.borrow().contains_key(&key)) {
    return Ok(());
  }
  let env = env.raw();
  let mut global = std::ptr::null_mut();
  napi::check_status!(unsafe { sys::napi_get_global(env, &mut global) })?;
  let mut symbol = std::ptr::null_mut();
  napi::check_status!(unsafe {
    sys::napi_get_named_property(env, global, c"Symbol".as_ptr(), &mut symbol)
  })?;
  let mut kind = 0;
  napi::check_status!(unsafe { sys::napi_typeof(env, symbol, &mut kind) })?;
  let mut reference = std::ptr::null_mut();
  if kind == sys::ValueType::napi_function {
    let mut iterator = std::ptr::null_mut();
    napi::check_status!(unsafe {
      sys::napi_get_named_property(env, symbol, c"iterator".as_ptr(), &mut iterator)
    })?;
    napi::check_status!(unsafe { sys::napi_typeof(env, iterator, &mut kind) })?;
    if kind == sys::ValueType::napi_symbol {
      napi::check_status!(unsafe { sys::napi_create_reference(env, iterator, 1, &mut reference) })?;
    }
  }
  unsafe extern "C" fn forget(env: *mut std::ffi::c_void) {
    let _ = ITERATOR_SYMBOLS.try_with(|symbols| {
      symbols.borrow_mut().remove(&(env as usize));
    });
  }
  // Node deletes the references of an env when it tears the env down.
  if let Err(error) =
    napi::check_status!(unsafe { sys::napi_add_env_cleanup_hook(env, Some(forget), env.cast()) })
  {
    if !reference.is_null() {
      unsafe { sys::napi_delete_reference(env, reference) };
    }
    return Err(error);
  }
  ITERATOR_SYMBOLS.with(|symbols| symbols.borrow_mut().insert(key, reference));
  Ok(())
}

/// The `Symbol.iterator` captured for `env`.
fn iterator_symbol<'env>(env: napi::sys::napi_env) -> Result<Unknown<'env>> {
  let key = env as usize;
  let reference = match ITERATOR_SYMBOLS.with(|symbols| symbols.borrow().get(&key).copied()) {
    Some(reference) => reference,
    // The load hook runs for every env, so this is only a fallback.
    None => {
      capture_iterator_symbol(&Env::from_raw(env))?;
      ITERATOR_SYMBOLS
        .with(|symbols| symbols.borrow().get(&key).copied())
        .ok_or_else(|| Error::new(Status::GenericFailure, "Symbol.iterator missing"))?
    }
  };
  if reference.is_null() {
    return Err(Error::new(
      Status::GenericFailure,
      "Symbol.iterator was not a symbol when the module loaded",
    ));
  }
  let mut symbol = std::ptr::null_mut();
  napi::check_status!(unsafe { napi::sys::napi_get_reference_value(env, reference, &mut symbol) })?;
  Ok(unsafe { Unknown::from_raw_unchecked(env, symbol) })
}

/// A reference to `Object.getOwnPropertyDescriptor(globalThis[class].prototype, key).get`, or null
/// if there is none.
fn prototype_getter_ref(
  env: &Env,
  class: &std::ffi::CStr,
  key: &std::ffi::CStr,
) -> Result<napi::sys::napi_ref> {
  use napi::sys;
  let env = env.raw();
  let named = |object: sys::napi_value, name: &std::ffi::CStr| -> Result<sys::napi_value> {
    let mut value = std::ptr::null_mut();
    napi::check_status!(unsafe {
      sys::napi_get_named_property(env, object, name.as_ptr(), &mut value)
    })?;
    Ok(value)
  };
  let is_type = |value: sys::napi_value, expected: sys::napi_valuetype| -> Result<bool> {
    let mut actual = 0;
    napi::check_status!(unsafe { sys::napi_typeof(env, value, &mut actual) })?;
    Ok(actual == expected)
  };

  let mut global = std::ptr::null_mut();
  napi::check_status!(unsafe { sys::napi_get_global(env, &mut global) })?;
  let constructor = named(global, class)?;
  if !is_type(constructor, sys::ValueType::napi_function)? {
    return Ok(std::ptr::null_mut());
  }
  let prototype = named(constructor, c"prototype")?;
  let object = named(global, c"Object")?;
  let get_own_property_descriptor = named(object, c"getOwnPropertyDescriptor")?;
  let mut key_value = std::ptr::null_mut();
  napi::check_status!(unsafe {
    sys::napi_create_string_utf8(
      env,
      key.as_ptr(),
      key.count_bytes() as isize,
      &mut key_value,
    )
  })?;
  let mut descriptor = std::ptr::null_mut();
  napi::check_status!(unsafe {
    sys::napi_call_function(
      env,
      object,
      get_own_property_descriptor,
      2,
      [prototype, key_value].as_ptr(),
      &mut descriptor,
    )
  })?;
  if !is_type(descriptor, sys::ValueType::napi_object)? {
    return Ok(std::ptr::null_mut());
  }
  let getter = named(descriptor, c"get")?;
  if !is_type(getter, sys::ValueType::napi_function)? {
    return Ok(std::ptr::null_mut());
  }
  let mut reference = std::ptr::null_mut();
  napi::check_status!(unsafe { sys::napi_create_reference(env, getter, 1, &mut reference) })?;
  Ok(reference)
}

/// The buffer `view` is on, read from the engine (not through `view.buffer`), and whether it is
/// a SharedArrayBuffer.
fn view_buffer(env: &Env, view: napi::sys::napi_value) -> Result<(napi::sys::napi_value, bool)> {
  let mut buffer = std::ptr::null_mut();
  napi::check_status!(
    unsafe {
      napi::sys::napi_get_typedarray_info(
        env.raw(),
        view,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        &mut buffer,
        std::ptr::null_mut(),
      )
    },
    "Get TypedArray info failed"
  )?;
  // False for a SharedArrayBuffer.
  let mut is_array_buffer = false;
  napi::check_status!(unsafe {
    napi::sys::napi_is_arraybuffer(env.raw(), buffer, &mut is_array_buffer)
  })?;
  Ok((buffer, !is_array_buffer))
}

/// Calls the captured getter that `pick` selects on `buffer`.
fn buffer_flag(
  env: &Env,
  buffer: napi::sys::napi_value,
  pick: fn(&BufferFlagGetters) -> napi::sys::napi_ref,
) -> Result<bool> {
  let getters = match BUFFER_FLAG_GETTERS.get() {
    Some(getters) if getters.env == env.raw() => getters,
    // The load hook runs for every env, so this is only a fallback.
    _ => {
      capture_buffer_flag_getters(env)?;
      BUFFER_FLAG_GETTERS
        .get()
        .ok_or_else(|| Error::new(Status::GenericFailure, "buffer flag getters missing"))?
    }
  };
  let reference = pick(&getters);
  if reference.is_null() {
    return Ok(false);
  }
  let mut getter = std::ptr::null_mut();
  napi::check_status!(unsafe {
    napi::sys::napi_get_reference_value(env.raw(), reference, &mut getter)
  })?;
  let mut value = std::ptr::null_mut();
  napi::check_status!(unsafe {
    napi::sys::napi_call_function(env.raw(), buffer, getter, 0, std::ptr::null(), &mut value)
  })?;
  let mut flag = false;
  napi::check_status!(unsafe { napi::sys::napi_get_value_bool(env.raw(), value, &mut flag) })?;
  Ok(flag)
}

/// Whether `value` is a Uint8Array (a Buffer is one). Runs no page code.
fn is_uint8_array(env: &Env, value: napi::sys::napi_value) -> Result<bool> {
  let mut is_typed_array = false;
  napi::check_status!(unsafe {
    napi::sys::napi_is_typedarray(env.raw(), value, &mut is_typed_array)
  })?;
  if !is_typed_array {
    return Ok(false);
  }
  let mut kind = 0;
  napi::check_status!(unsafe {
    napi::sys::napi_get_typedarray_info(
      env.raw(),
      value,
      &mut kind,
      std::ptr::null_mut(),
      std::ptr::null_mut(),
      std::ptr::null_mut(),
      std::ptr::null_mut(),
    )
  })?;
  Ok(kind == napi::sys::TypedarrayType::uint8_array)
}

/// Whether `view` is on a resizable ArrayBuffer or a growable SharedArrayBuffer. Runs no page code
/// (see the limit on `BufferFlagGetters`).
fn is_resizable_view(env: &Env, view: napi::sys::napi_value) -> Result<bool> {
  let (buffer, shared) = view_buffer(env, view)?;
  if shared {
    buffer_flag(env, buffer, |getters| getters.growable)
  } else {
    buffer_flag(env, buffer, |getters| getters.resizable)
  }
}

/// Whether the ArrayBuffer under `view` is detached. Runs no page code (see the limit on
/// `BufferFlagGetters`).
fn is_detached_view(env: &Env, view: napi::sys::napi_value) -> Result<bool> {
  let (buffer, shared) = view_buffer(env, view)?;
  // A SharedArrayBuffer cannot be detached.
  if shared {
    return Ok(false);
  }
  buffer_flag(env, buffer, |getters| getters.detached)
}

/// Per W3C spec: Parse Visible Rect algorithm
/// Takes default rect, optional override rect, coded dimensions, and format
/// Returns (left, top, width, height) or error
fn parse_visible_rect(
  default_rect: (f64, f64, f64, f64), // (x, y, width, height)
  override_rect: Option<&DOMRectInit>,
  coded_width: u32,
  coded_height: u32,
  format: VideoPixelFormat,
) -> Result<(u32, u32, u32, u32)> {
  let (x, y, width, height) = match override_rect {
    Some(rect) => {
      let x = rect.x.unwrap_or(default_rect.0);
      let y = rect.y.unwrap_or(default_rect.1);
      let width = rect.width.unwrap_or(default_rect.2);
      let height = rect.height.unwrap_or(default_rect.3);
      (x, y, width, height)
    }
    None => default_rect,
  };

  // Validate: values must be non-negative and finite
  if x < 0.0 || y < 0.0 || width <= 0.0 || height <= 0.0 {
    return Err(Error::new(
      Status::InvalidArg,
      "TypeError: visibleRect dimensions must be positive",
    ));
  }
  if !x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() {
    return Err(Error::new(
      Status::InvalidArg,
      "TypeError: visibleRect values must be finite",
    ));
  }

  // Validate bounds on the raw doubles — computing x + width in u32 wraps
  // for large inputs (e.g. x = 5e9 saturates to u32::MAX, then + 1 wraps to
  // 0), admitting out-of-bounds rectangles that later read OOB in copyTo
  if x + width > f64::from(coded_width) {
    return Err(Error::new(
      Status::InvalidArg,
      "TypeError: visibleRect.x + width exceeds codedWidth",
    ));
  }
  if y + height > f64::from(coded_height) {
    return Err(Error::new(
      Status::InvalidArg,
      "TypeError: visibleRect.y + height exceeds codedHeight",
    ));
  }

  // Verify alignment on the untruncated offsets: a fractional offset is not
  // a multiple of the subsampling factor (x % 1 != 0 for RGBA too)
  let (h_factor, v_factor) = get_subsampling_factors(format);
  if x % f64::from(h_factor) != 0.0 || y % f64::from(v_factor) != 0.0 {
    return Err(Error::new(
      Status::InvalidArg,
      format!(
        "TypeError: visibleRect offset ({}, {}) not aligned for format {:?} (requires {}x{} alignment)",
        x, y, format, h_factor, v_factor
      ),
    ));
  }

  // Safe to truncate: validated finite, non-negative, in coded bounds
  let x = x as u32;
  let y = y as u32;
  let width = width as u32;
  let height = height as u32;

  Ok((x, y, width, height))
}

/// VideoFrame - represents a frame of video
///
/// This is a WebCodecs-compliant VideoFrame implementation backed by FFmpeg.
#[napi]
pub struct VideoFrame {
  observer_id: u64,
  inner: Arc<Mutex<Option<VideoFrameInner>>>,
  /// Frame metadata lives outside the detachable resource per WebCodecs.
  timestamp_us: i64,
  duration_us: Option<i64>,
}

/// A @napi-rs/canvas Canvas, found by duck typing: width, height and a data() method, but no
/// codedWidth/naturalWidth. Only objects are probed, so no exception is left pending.
fn canvas_source<'env>(source: &Unknown<'env>) -> Option<Object<'env>> {
  if !matches!(
    source.get_type(),
    Ok(ValueType::Object | ValueType::Function)
  ) {
    return None;
  }
  let source_obj = source.coerce_to_object().ok()?;
  let has_width = source_obj.has_named_property("width").unwrap_or(false);
  let has_height = source_obj.has_named_property("height").unwrap_or(false);
  let has_data = source_obj.has_named_property("data").unwrap_or(false);
  let has_coded_width = source_obj.has_named_property("codedWidth").unwrap_or(false);
  let has_natural_width = source_obj
    .has_named_property("naturalWidth")
    .unwrap_or(false);
  if !(has_width && has_height && has_data && !has_coded_width && !has_natural_width) {
    return None;
  }
  // Verify data is a function (Canvas.data() method vs property)
  let data_prop = source_obj.get_named_property::<Unknown>("data").ok()?;
  (data_prop.get_type().unwrap_or(ValueType::Undefined) == ValueType::Function)
    .then_some(source_obj)
}

/// Parse rotation value per W3C spec algorithm
/// Rounds to the nearest multiple of 90 (ties towards positive infinity), normalized to 0-270.
///
/// Same steps as Chromium's media::VideoTransformation(double, bool): flooring first keeps every
/// step exact, so huge values and negative ties (-315 -> 90) round like the spec says.
fn parse_rotation(rotation: f64) -> f64 {
  // An integer in (-360, 360); `%` on f64 is C fmod. Non-finite input (the VideoFrame
  // constructors reject it) gives NaN, which the cast turns into 0, so rotation 0.
  let bounded = rotation.floor() % 360.0;
  // Add 360 to make it non-negative and 45 to round ties up: quarter turns in [0, 8].
  let quarter_turns = (bounded as i32 + 360 + 45) / 90;
  f64::from((quarter_turns % 4) * 90)
}

/// Chromium media::limits::kMaxDimension and kMaxCanvas (media/base/limits.h)
const MAX_DIMENSION: u32 = (1 << 15) - 1;
const MAX_CANVAS: u64 = (1 << 29) - 1;

/// Whether a display size fits Chromium's media::VideoFrame size limits.
fn display_size_within_limits(width: u32, height: u32) -> bool {
  width <= MAX_DIMENSION
    && height <= MAX_DIMENSION
    && u64::from(width) * u64::from(height) <= MAX_CANVAS
}

/// Validates an explicit displayWidth/displayHeight pair like Chromium's
/// ParseAndValidateDisplaySize: both or neither, nonzero, within media limits.
/// Returns the TypeError message on failure.
fn parse_display_size(
  width: Option<u32>,
  height: Option<u32>,
) -> std::result::Result<Option<(u32, u32)>, String> {
  let (width, height) = match (width, height) {
    (None, None) => return Ok(None),
    (None, Some(_)) => return Err("displayHeight specified without displayWidth.".into()),
    (Some(_), None) => return Err("displayWidth specified without displayHeight.".into()),
    (Some(width), Some(height)) => (width, height),
  };
  if width == 0 {
    return Err("displayWidth must be nonzero.".into());
  }
  if height == 0 {
    return Err("displayHeight must be nonzero.".into());
  }
  if !display_size_within_limits(width, height) {
    return Err(format!(
      "Invalid display size ({}, {}); exceeds implementation limit.",
      width, height
    ));
  }
  Ok(Some((width, height)))
}

/// Natural size for a decoder's displayAspectWidth:displayAspectHeight, like Chromium's
/// VideoAspectRatio::DAR(width, height).GetNaturalSize(visible_rect): grow one side of the
/// visible size until it matches the ratio. None keeps the visible size (Chromium casts the
/// members to int, so a value above i32::MAX makes the ratio invalid).
fn natural_size_for_display_aspect(
  visible_width: u32,
  visible_height: u32,
  aspect_width: u32,
  aspect_height: u32,
) -> Option<(f64, f64)> {
  let (aspect_width, aspect_height) = (aspect_width as i32, aspect_height as i32);
  if aspect_height == 0 || visible_width == 0 || visible_height == 0 {
    return None;
  }
  let aspect = f64::from(aspect_width) / f64::from(aspect_height);
  if !aspect.is_finite() || aspect <= 0.0 {
    return None;
  }
  let (mut width, mut height) = (f64::from(visible_width), f64::from(visible_height));
  if aspect >= width / height {
    width = height * aspect;
  } else {
    height = width / aspect;
  }
  let (width, height) = (width.round(), height.round());
  let int_max = f64::from(i32::MAX);
  if width < 1.0 || width > int_max || height < 1.0 || height > int_max {
    return None;
  }
  Some((width, height))
}

#[napi]
impl VideoFrame {
  fn from_inner(inner: VideoFrameInner) -> Self {
    Self {
      observer_id: NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed),
      timestamp_us: inner.timestamp_us,
      duration_us: inner.duration_us,
      inner: Arc::new(Mutex::new(Some(inner))),
    }
  }

  /// Native host interop for a retained rgba16float canvas image.
  /// Public WebCodecs pixel formats intentionally have no alias for this resource.
  fn from_rgba16_float(data: Uint16Array, init: VideoFrameConstructorInit) -> Result<Self> {
    let width = init
      .coded_width
      .ok_or_else(|| type_error("codedWidth is required"))?;
    let height = init
      .coded_height
      .ok_or_else(|| type_error("codedHeight is required"))?;
    let timestamp = init
      .timestamp
      .ok_or_else(|| type_error("timestamp is required"))?;
    let expected = (width as usize)
      .checked_mul(height as usize)
      .and_then(|n| n.checked_mul(4))
      .filter(|_| width > 0 && height > 0)
      .ok_or_else(|| type_error("invalid rgba16float image dimensions"))?;
    if data.len() != expected {
      return Err(type_error(
        "rgba16float image size does not match its dimensions",
      ));
    }
    if init.format.is_some() || init.layout.is_some() || init.transfer.is_some() {
      return Err(type_error(
        "rgba16float images do not use a public buffer format, layout or transfer list",
      ));
    }
    let (visible_left, visible_top, visible_width, visible_height) = parse_visible_rect(
      (0., 0., width as f64, height as f64),
      init.visible_rect.as_ref(),
      width,
      height,
      VideoPixelFormat::RGBA,
    )?;
    let (display_width, display_height) =
      parse_display_size(init.display_width, init.display_height)
        .map_err(|message| type_error(&message))?
        .unwrap_or((visible_width, visible_height));
    if !matches!(init.alpha.as_deref(), None | Some("keep") | Some("discard")) {
      return Err(type_error("invalid alpha handling"));
    }
    let mut pixels = data.to_vec();
    if init.alpha.as_deref() == Some("discard") {
      for alpha in pixels.iter_mut().skip(3).step_by(4) {
        *alpha = 0x3c00;
      }
    }
    let resource = VideoFrameResource::Rgba16Float {
      width,
      height,
      pixels: pixels.into(),
    };
    let color_space = match init.color_space {
      Some(color) => VideoColorSpace::new(Some(color)),
      None => VideoColorSpace::from_components(
        Some(VideoColorPrimaries::Bt709),
        Some(VideoTransferCharacteristics::Iec6196621),
        Some(VideoMatrixCoefficients::Rgb),
        Some(true),
      ),
    };
    Ok(Self::from_inner(VideoFrameInner {
      resource,
      timestamp_us: timestamp,
      duration_us: init.duration,
      visible_left,
      visible_top,
      visible_width,
      visible_height,
      display_width,
      display_height,
      rotation: parse_rotation(init.rotation.unwrap_or(0.)),
      flip: init.flip.unwrap_or(false),
      color_space,
    }))
  }

  /// Native consumer interop; returns an independent copy without changing public copyTo.
  fn copy_rgba16_float(&self, env: Env) -> Result<Uint16Array> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    let inner = match guard.as_ref() {
      Some(inner) => inner,
      None => return throw_invalid_state_error(&env, "VideoFrame is closed"),
    };
    match &inner.resource {
      VideoFrameResource::Rgba16Float { pixels, .. } => Ok(Uint16Array::new(pixels.to_vec())),
      _ => throw_not_supported_error(&env, "VideoFrame does not contain an rgba16float image"),
    }
  }

  /// Create a new VideoFrame from buffer data, another VideoFrame, or a Canvas (W3C WebCodecs spec)
  ///
  /// Constructor forms per W3C spec:
  /// 1. `new VideoFrame(data, init)` - from BufferSource with VideoFrameBufferInit
  /// 2. `new VideoFrame(source, init?)` - from another VideoFrame with optional VideoFrameInit
  /// 3. `new VideoFrame(canvas, init)` - from @napi-rs/canvas Canvas (requires timestamp in init)
  #[napi(
    constructor,
    ts_args_type = "source: VideoFrame | Uint8Array | CanvasLike, init?: VideoFrameBufferInit | VideoFrameInit"
  )]
  pub fn new(env: Env, source: Unknown, init: Option<Unknown>) -> Result<Self> {
    // Try VideoFrame first. This is a real `instanceof` check rather than a probe for
    // `codedWidth`: any object could carry that property, and unwrapping a non-VideoFrame
    // would blindly cast a foreign pointer to `&VideoFrame`.
    let is_video_frame = VideoFrame::instance_of(&env, &source).unwrap_or(false);
    let canvas = if is_video_frame {
      None
    } else {
      canvas_source(&source)
    };
    // The source picks the dictionary (VideoFrameInit or VideoFrameBufferInit), and WebIDL
    // converts it before any constructor step below.
    let dictionary = if is_video_frame || canvas.is_some() {
      InitDictionary::Image
    } else {
      InitDictionary::Buffer
    };
    let init = match init {
      Some(init) => {
        Some(unsafe { VideoFrameConstructorInit::read(env.raw(), init.raw(), dictionary)? })
      }
      None => None,
    };

    if is_video_frame {
      // It's a VideoFrame - unwrap and use frame clone path.
      // `ClassInstance` is the only unwrap permitted inside a native call; `<&VideoFrame>`
      // registers a native borrow, which napi only allows while the generated argument
      // conversion is still in scope.
      let source_instance =
        unsafe { ClassInstance::<VideoFrame>::from_napi_value(env.raw(), source.raw())? };
      let video_frame: &VideoFrame = &source_instance;

      // Check if source is closed and throw native DOMException
      {
        let guard = video_frame
          .inner
          .lock()
          .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
        if guard.is_none() {
          return throw_invalid_state_error(&env, "VideoFrame is closed");
        }
      }

      return Self::new_from_video_frame(&env, video_frame, init)
        .map_err(|error| throw_as_native_error(&env, error));
    }

    if let Some(source_obj) = canvas {
      return Self::new_from_canvas(env, source_obj, init)
        .map_err(|error| throw_as_native_error(&env, error));
    }

    // Try as Uint8Array/Buffer
    let data = Uint8ArraySlice::from_unknown(source).map_err(|_| {
      let _ = env.throw_type_error(
        "First argument must be a VideoFrame, Canvas, or BufferSource (Uint8Array/Buffer)",
        None,
      );
      Error::new(
        Status::InvalidArg,
        "First argument must be a VideoFrame, Canvas, or BufferSource (Uint8Array/Buffer)",
      )
    })?;

    Self::new_from_buffer(env, &data, init).map_err(|error| throw_as_native_error(&env, error))
  }

  /// Internal: Create VideoFrame from buffer data (VideoFrameBufferInit constructor form)
  fn new_from_buffer(
    env: Env,
    data: &[u8],
    init: Option<VideoFrameConstructorInit>,
  ) -> Result<Self> {
    // init is required for buffer constructor
    let init = init.ok_or_else(|| {
      let _ = env.throw_type_error("init is required when creating from buffer", None);
      Error::new(
        Status::InvalidArg,
        "init is required when creating from buffer",
      )
    })?;

    // Validate required fields for buffer constructor
    let format = init.format.ok_or_else(|| {
      let _ = env.throw_type_error("format is required", None);
      Error::new(Status::InvalidArg, "format is required")
    })?;
    let width = init.coded_width.ok_or_else(|| {
      let _ = env.throw_type_error("codedWidth is required", None);
      Error::new(Status::InvalidArg, "codedWidth is required")
    })?;
    let height = init.coded_height.ok_or_else(|| {
      let _ = env.throw_type_error("codedHeight is required", None);
      Error::new(Status::InvalidArg, "codedHeight is required")
    })?;
    let timestamp = init.timestamp.ok_or_else(|| {
      let _ = env.throw_type_error("timestamp is required", None);
      Error::new(Status::InvalidArg, "timestamp is required")
    })?;

    // Validate zero dimensions
    if width == 0 {
      let _ = env.throw_type_error("codedWidth must be greater than 0", None);
      return Err(Error::new(
        Status::InvalidArg,
        "codedWidth must be greater than 0",
      ));
    }
    if height == 0 {
      let _ = env.throw_type_error("codedHeight must be greater than 0", None);
      return Err(Error::new(
        Status::InvalidArg,
        "codedHeight must be greater than 0",
      ));
    }

    // Validate buffer size before creating frame
    let expected_size = Self::calculate_buffer_size(format, width, height) as usize;
    if data.len() < expected_size {
      let _ = env.throw_type_error(
        &format!(
          "Buffer too small: need {} bytes, got {}",
          expected_size,
          data.len()
        ),
        None,
      );
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "Buffer too small: need {} bytes, got {}",
          expected_size,
          data.len()
        ),
      ));
    }

    let av_format = format.to_av_format();

    // Create internal frame
    let mut frame = Frame::new_video(width, height, av_format).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create frame: {}", e),
      )
    })?;

    // Copy data into the frame (with optional custom layout)
    Self::copy_data_to_frame(
      &mut frame,
      data,
      format,
      width,
      height,
      init.layout.as_deref(),
    )?;

    // Set timestamps (convert from microseconds to time_base units)
    // We use microseconds as time_base internally
    frame.set_pts(timestamp);
    if let Some(duration) = init.duration {
      frame.set_duration(duration);
    }

    // Parse rotation and flip per W3C spec
    let rotation = parse_rotation(init.rotation.unwrap_or(0.0));
    let flip = init.flip.unwrap_or(false);

    // Parse visible rect per W3C spec (default to full coded size)
    let default_rect = (0.0, 0.0, width as f64, height as f64);
    let (visible_left, visible_top, visible_width, visible_height) = parse_visible_rect(
      default_rect,
      init.visible_rect.as_ref(),
      width,
      height,
      format,
    )?;

    // Store the natural (unrotated) size. Explicit displayWidth/displayHeight are
    // taken as the natural size too, like Chromium (the spec text stores them as
    // already rotated). The displayWidth/displayHeight getters apply rotation.
    let (display_width, display_height) =
      parse_display_size(init.display_width, init.display_height)
        .map_err(|message| throw_type_error(env.raw(), &message))?
        .unwrap_or((visible_width, visible_height));

    // Color space: use provided value, or default to sRGB for RGB formats
    let color_space = if init.color_space.is_some() {
      VideoColorSpace::new(init.color_space)
    } else if matches!(
      format,
      VideoPixelFormat::RGBA
        | VideoPixelFormat::RGBX
        | VideoPixelFormat::BGRA
        | VideoPixelFormat::BGRX
    ) {
      // Default to sRGB color space for RGB formats per W3C spec
      VideoColorSpace::from_components(
        Some(VideoColorPrimaries::Bt709),
        Some(VideoTransferCharacteristics::Iec6196621), // sRGB
        Some(VideoMatrixCoefficients::Rgb),
        Some(true), // fullRange
      )
    } else {
      VideoColorSpace::new(None)
    };

    let inner = VideoFrameInner {
      resource: VideoFrameResource::Software {
        frame: frame.into_shared(),
        format,
      },
      timestamp_us: timestamp,
      duration_us: init.duration,
      visible_left,
      visible_top,
      visible_width,
      visible_height,
      display_width,
      display_height,
      rotation,
      flip,
      color_space,
    };

    Ok(Self::from_inner(inner))
  }

  /// Internal: Create VideoFrame from @napi-rs/canvas Canvas (CanvasImageSource constructor form)
  ///
  /// Per W3C spec, timestamp is REQUIRED when creating from Canvas.
  /// Canvas.data() returns RGBA pixel data as a Buffer.
  fn new_from_canvas(
    env: Env,
    canvas_obj: Object,
    init: Option<VideoFrameConstructorInit>,
  ) -> Result<Self> {
    // Get canvas dimensions
    let width: u32 = canvas_obj
      .get::<u32>("width")
      .map_err(|_| Error::new(Status::InvalidArg, "Canvas has no width property"))?
      .ok_or_else(|| Error::new(Status::InvalidArg, "Canvas has no width property"))?;
    let height: u32 = canvas_obj
      .get::<u32>("height")
      .map_err(|_| Error::new(Status::InvalidArg, "Canvas has no height property"))?
      .ok_or_else(|| Error::new(Status::InvalidArg, "Canvas has no height property"))?;

    // Validate dimensions (must be > 0)
    if width == 0 {
      let _ = env.throw_type_error("Canvas width must be greater than 0", None);
      return Err(Error::new(
        Status::InvalidArg,
        "Canvas width must be greater than 0",
      ));
    }
    if height == 0 {
      let _ = env.throw_type_error("Canvas height must be greater than 0", None);
      return Err(Error::new(
        Status::InvalidArg,
        "Canvas height must be greater than 0",
      ));
    }

    // init with timestamp is REQUIRED for Canvas source per W3C spec
    let init = init.ok_or_else(|| {
      let _ = env.throw_type_error(
        "init with timestamp is required when creating from Canvas",
        None,
      );
      Error::new(
        Status::InvalidArg,
        "init with timestamp is required when creating from Canvas",
      )
    })?;

    let timestamp = init.timestamp.ok_or_else(|| {
      let _ = env.throw_type_error(
        "timestamp is required when creating VideoFrame from Canvas",
        None,
      );
      Error::new(
        Status::InvalidArg,
        "timestamp is required when creating VideoFrame from Canvas",
      )
    })?;

    // Get pixel data by calling canvas.data() with proper 'this' context
    // Canvas.data() returns a Buffer containing raw RGBA pixel data
    // We need to use low-level napi to call with 'this' bound to canvas
    let data_fn: Unknown = canvas_obj
      .get_named_property("data")
      .map_err(|_| Error::new(Status::GenericFailure, "Failed to get Canvas.data method"))?;

    let pixel_data: &mut [u8] = {
      let mut result: napi::sys::napi_value = std::ptr::null_mut();
      let status = unsafe {
        napi::sys::napi_call_function(
          env.raw(),
          canvas_obj.raw(), // 'this' is the canvas object
          data_fn.raw(),    // the data() method
          0,                // no arguments
          std::ptr::null(), // no arguments array
          &mut result,      // result
        )
      };
      if status != napi::sys::Status::napi_ok {
        return Err(Error::new(
          Status::GenericFailure,
          "Canvas.data() call failed",
        ));
      }
      unsafe { FromNapiValue::from_napi_value(env.raw(), result)? }
    };

    // Validate buffer size (RGBA = 4 bytes per pixel)
    let expected_size = (width as usize) * (height as usize) * 4;
    if pixel_data.len() < expected_size {
      let _ = env.throw_type_error(
        &format!(
          "Canvas data buffer too small: need {} bytes, got {}",
          expected_size,
          pixel_data.len()
        ),
        None,
      );
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "Canvas data buffer too small: need {} bytes, got {}",
          expected_size,
          pixel_data.len()
        ),
      ));
    }

    // Check if alpha should be discarded per W3C spec
    // When alpha is "discard", we must set all alpha bytes to 255 (opaque)
    // per W3C spec: "When alpha is 'discard', the alpha plane should be discarded
    // and replaced with an opaque alpha value (255)."
    let should_discard_alpha = init.alpha.as_ref().map(|a| a == "discard").unwrap_or(false);

    let format = if should_discard_alpha {
      VideoPixelFormat::RGBX // Opaque format
    } else {
      VideoPixelFormat::RGBA // Keep alpha channel
    };

    // If discarding alpha, create a new buffer with all alpha bytes set to 255
    if should_discard_alpha {
      // Set all alpha bytes (every 4th byte, starting at index 3) to 255
      for i in (3..pixel_data.len()).step_by(4) {
        pixel_data[i] = 255; // Opaque alpha
      }
    };

    // Build init with Canvas-derived values, preserving user-provided options
    let canvas_init = VideoFrameConstructorInit {
      format: Some(format),
      coded_width: Some(width),
      coded_height: Some(height),
      timestamp: Some(timestamp),
      duration: init.duration,
      layout: None, // Canvas data is always tightly packed
      visible_rect: init.visible_rect,
      rotation: init.rotation,
      flip: init.flip,
      display_width: init.display_width,
      display_height: init.display_height,
      color_space: init.color_space, // Will default to sRGB in new_from_buffer
      metadata: init.metadata,
      transfer: None,
      alpha: init.alpha,
    };

    // Delegate to new_from_buffer with processed pixel data
    Self::new_from_buffer(env, pixel_data, Some(canvas_init))
  }

  /// Internal: Create VideoFrame from another VideoFrame (image source constructor form)
  fn new_from_video_frame(
    env: &Env,
    source: &VideoFrame,
    init: Option<VideoFrameConstructorInit>,
  ) -> Result<Self> {
    // The closure returns Err(message) for an OperationError, thrown once the source lock is
    // released: constructing a DOMException runs JS.
    let new_inner = source.with_inner(|source_inner| {
      // Check if alpha should be discarded per W3C spec
      let should_discard_alpha = init
        .as_ref()
        .and_then(|i| i.alpha.as_ref())
        .map(|a| a == "discard")
        .unwrap_or(false);

      let resource = if should_discard_alpha {
        source_inner.resource.discard_alpha()?
      } else {
        source_inner.resource.clone()
      };
      let (coded_width, coded_height) = resource.dimensions();
      // Opaque RGBA has one-pixel alignment even though it has no public format.
      let alignment_format = resource.format().unwrap_or(VideoPixelFormat::RGBA);

      // Apply overrides from init (all fields optional for frame clone)
      let timestamp_us = init
        .as_ref()
        .and_then(|i| i.timestamp)
        .unwrap_or(source_inner.timestamp_us);
      let duration_us = init
        .as_ref()
        .and_then(|i| i.duration)
        .or(source_inner.duration_us);

      // Parse visible rect per W3C spec (source's visible rect is the default)
      let default_rect = (
        source_inner.visible_left as f64,
        source_inner.visible_top as f64,
        source_inner.visible_width as f64,
        source_inner.visible_height as f64,
      );
      let (visible_left, visible_top, visible_width, visible_height) = parse_visible_rect(
        default_rect,
        init.as_ref().and_then(|i| i.visible_rect.as_ref()),
        coded_width,
        coded_height,
        alignment_format,
      )?;

      // Handle rotation per W3C spec "Add Rotations" algorithm
      let init_rotation = parse_rotation(init.as_ref().and_then(|i| i.rotation).unwrap_or(0.0));
      let base_rotation = source_inner.rotation;
      let base_flip = source_inner.flip;
      let init_flip = init.as_ref().and_then(|i| i.flip).unwrap_or(false);

      // Per spec: if baseFlip is false, combined = base + init; else combined = base - init
      let combined_rotation = if !base_flip {
        parse_rotation(base_rotation + init_rotation)
      } else {
        parse_rotation(base_rotation - init_rotation)
      };
      // Per spec: flip is XOR of base and init flip
      let combined_flip = base_flip != init_flip;

      // Per spec: display dimensions scale proportionally with visible rect changes
      // If not explicitly provided, scale based on ratio of new visible rect to old
      let explicit_display = parse_display_size(
        init.as_ref().and_then(|i| i.display_width),
        init.as_ref().and_then(|i| i.display_height),
      )
      .map_err(|message| throw_type_error(env.raw(), &message))?;
      let has_visible_rect = init.as_ref().is_some_and(|i| i.visible_rect.is_some());
      let (display_width, display_height) = match explicit_display {
        Some(size) => size,
        // Without a new visibleRect the source display size is kept as is
        None if !has_visible_rect => (source_inner.display_width, source_inner.display_height),
        None => {
          let scaled = |display: u32, visible: u32, source_visible: u32| {
            if source_visible > 0 {
              let scale = f64::from(visible) / f64::from(source_visible);
              (f64::from(display) * scale).round()
            } else {
              f64::from(visible)
            }
          };
          let width = scaled(
            source_inner.display_width,
            visible_width,
            source_inner.visible_width,
          );
          let height = scaled(
            source_inner.display_height,
            visible_height,
            source_inner.visible_height,
          );
          // Like Chromium's ParsedVideoFrameInit (spec step 8.5)
          if width == 0.0 {
            return Err(throw_type_error(
              env.raw(),
              "computed displayWidth must be nonzero",
            ));
          }
          if height == 0.0 {
            return Err(throw_type_error(
              env.raw(),
              "computed displayHeight must be nonzero",
            ));
          }
          // Chromium cannot wrap a frame whose natural size exceeds the media limits and
          // throws OperationError. Checking here also keeps the u32 casts below exact.
          if width > f64::from(MAX_DIMENSION)
            || height > f64::from(MAX_DIMENSION)
            || width * height > MAX_CANVAS as f64
          {
            return Ok(Err(format!(
              "Failed to create a VideoFrame: computed display size {}x{} exceeds implementation limit.",
              width, height
            )));
          }
          (width as u32, height as u32)
        }
      };

      let new_inner = VideoFrameInner {
        resource,
        timestamp_us,
        duration_us,
        visible_left,
        visible_top,
        visible_width,
        visible_height,
        display_width,
        display_height,
        rotation: combined_rotation,
        flip: combined_flip,
        color_space: source_inner.color_space.clone(),
      };

      Ok(Ok(new_inner))
    })?;
    match new_inner {
      Ok(new_inner) => Ok(VideoFrame::from_inner(new_inner)),
      Err(message) => super::error::throw_dom_exception(
        env,
        super::error::DOMExceptionName::OperationError,
        &message,
      ),
    }
  }

  /// Create a VideoFrame from an internal Frame (for decoder output)
  pub fn from_internal(frame: Frame, timestamp_us: i64, duration_us: Option<i64>) -> Self {
    let width = frame.width();
    let height = frame.height();
    let original_format =
      VideoPixelFormat::from_av_format(frame.format()).unwrap_or(VideoPixelFormat::I420);

    let inner = VideoFrameInner {
      resource: VideoFrameResource::Software {
        frame: frame.into_shared(),
        format: original_format,
      },
      timestamp_us,
      duration_us,
      visible_left: 0,
      visible_top: 0,
      visible_width: width,
      visible_height: height,
      display_width: width,
      display_height: height,
      rotation: 0.0,
      flip: false,
      color_space: VideoColorSpace::default(),
    };

    Self::from_inner(inner)
  }

  /// Create a VideoFrame from an Arc<RwLock<Frame>> (for sharing frame data)
  pub fn from_internal_arc(
    frame: Arc<RwLock<Frame>>,
    timestamp_us: i64,
    duration_us: Option<i64>,
  ) -> Self {
    let (width, height, original_format) = {
      let guard = frame.read();
      let width = guard.width();
      let height = guard.height();
      let format =
        VideoPixelFormat::from_av_format(guard.format()).unwrap_or(VideoPixelFormat::I420);
      (width, height, format)
    };

    let inner = VideoFrameInner {
      resource: VideoFrameResource::Software {
        frame,
        format: original_format,
      },
      timestamp_us,
      duration_us,
      visible_left: 0,
      visible_top: 0,
      visible_width: width,
      visible_height: height,
      display_width: width,
      display_height: height,
      rotation: 0.0,
      flip: false,
      color_space: VideoColorSpace::default(),
    };

    Self::from_inner(inner)
  }

  /// Create a VideoFrame from an internal Frame with rotation/flip (for decoder output)
  ///
  /// If `color_space_init` is provided, uses those values. Otherwise extracts from FFmpeg frame.
  pub fn from_internal_with_orientation(
    frame: Frame,
    timestamp_us: i64,
    duration_us: Option<i64>,
    rotation: f64,
    flip: bool,
    color_space_init: Option<&VideoColorSpaceInit>,
  ) -> Self {
    let width = frame.width();
    let height = frame.height();
    let parsed_rotation = parse_rotation(rotation);
    let original_format =
      VideoPixelFormat::from_av_format(frame.format()).unwrap_or(VideoPixelFormat::I420);

    let display_width = width;
    let display_height = height;

    // Use provided colorSpace from decoder config, or extract from FFmpeg frame
    // If colorSpace is empty (all None), extract from FFmpeg to get embedded bitstream values
    let color_space = match color_space_init {
      Some(init)
        if init.primaries.is_some()
          || init.transfer.is_some()
          || init.matrix.is_some()
          || init.full_range.is_some() =>
      {
        VideoColorSpace {
          primaries: init.primaries,
          transfer: init.transfer,
          matrix: init.matrix,
          full_range: init.full_range,
        }
      }
      // No colorSpace provided or empty colorSpace - extract from FFmpeg frame
      _ => color_space_from_frame(&frame),
    };

    let inner = VideoFrameInner {
      resource: VideoFrameResource::Software {
        frame: frame.into_shared(),
        format: original_format,
      },
      timestamp_us,
      duration_us,
      visible_left: 0,
      visible_top: 0,
      visible_width: width,
      visible_height: height,
      display_width,
      display_height,
      rotation: parsed_rotation,
      flip,
      color_space,
    };

    Self::from_inner(inner)
  }

  /// Create a VideoFrame from an internal Frame with color space control (for ImageDecoder)
  ///
  /// When `extract_color_space` is true, color space metadata is extracted from the FFmpeg
  /// frame and populated in the VideoFrame (colorSpaceConversion: "default").
  /// When false, the VideoFrame has an empty color space (colorSpaceConversion: "none").
  pub fn from_internal_with_color_space(
    frame: Frame,
    timestamp_us: i64,
    duration_us: Option<i64>,
    extract_color_space: bool,
  ) -> Self {
    let width = frame.width();
    let height = frame.height();
    let original_format =
      VideoPixelFormat::from_av_format(frame.format()).unwrap_or(VideoPixelFormat::I420);

    let color_space = if extract_color_space {
      color_space_from_frame(&frame)
    } else {
      VideoColorSpace::default()
    };

    let inner = VideoFrameInner {
      resource: VideoFrameResource::Software {
        frame: frame.into_shared(),
        format: original_format,
      },
      timestamp_us,
      duration_us,
      visible_left: 0,
      visible_top: 0,
      visible_width: width,
      visible_height: height,
      display_width: width,
      display_height: height,
      rotation: 0.0,
      flip: false,
      color_space,
    };

    Self::from_inner(inner)
  }

  /// Create a VideoFrame from a shared Arc<RwLock<Frame>> with color space control
  ///
  /// This constructor allows sharing the same frame data via Arc cloning,
  /// avoiding expensive pixel copies. Used by ImageDecoder cache.
  pub fn from_internal_arc_with_color_space(
    frame_arc: Arc<RwLock<Frame>>,
    timestamp_us: i64,
    duration_us: Option<i64>,
    extract_color_space: bool,
  ) -> Self {
    let frame_guard = frame_arc.read();
    let width = frame_guard.width();
    let height = frame_guard.height();
    let original_format =
      VideoPixelFormat::from_av_format(frame_guard.format()).unwrap_or(VideoPixelFormat::I420);

    let color_space = if extract_color_space {
      color_space_from_frame(&frame_guard)
    } else {
      VideoColorSpace::default()
    };
    drop(frame_guard);

    let inner = VideoFrameInner {
      resource: VideoFrameResource::Software {
        frame: frame_arc,
        format: original_format,
      },
      timestamp_us,
      duration_us,
      visible_left: 0,
      visible_top: 0,
      visible_width: width,
      visible_height: height,
      display_width: width,
      display_height: height,
      rotation: 0.0,
      flip: false,
      color_space,
    };

    Self::from_inner(inner)
  }

  /// Get the pixel format
  #[napi(getter)]
  pub fn format(&self) -> Result<Option<VideoPixelFormat>> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => Ok(inner.resource.format()),
      _ => Ok(None),
    }
  }

  /// Get the coded width in pixels (returns 0 when closed per W3C spec)
  #[napi(getter)]
  pub fn coded_width(&self) -> Result<u32> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => Ok(inner.resource.dimensions().0),
      _ => Ok(0),
    }
  }

  /// Get the coded height in pixels (returns 0 when closed per W3C spec)
  #[napi(getter)]
  pub fn coded_height(&self) -> Result<u32> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => Ok(inner.resource.dimensions().1),
      _ => Ok(0),
    }
  }

  /// Get the display width in pixels (returns 0 when closed per W3C spec)
  #[napi(getter)]
  pub fn display_width(&self) -> Result<u32> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => Ok(if inner.rotation == 90.0 || inner.rotation == 270.0 {
        inner.display_height
      } else {
        inner.display_width
      }),
      _ => Ok(0),
    }
  }

  /// Get the display height in pixels (returns 0 when closed per W3C spec)
  #[napi(getter)]
  pub fn display_height(&self) -> Result<u32> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => Ok(if inner.rotation == 90.0 || inner.rotation == 270.0 {
        inner.display_width
      } else {
        inner.display_height
      }),
      _ => Ok(0),
    }
  }

  /// Get the coded rect (the region containing valid pixel data)
  /// Returns DOMRectReadOnly per W3C WebCodecs spec
  /// Returns null after the resource is closed.
  #[napi(getter)]
  pub fn coded_rect(&self) -> Result<Option<DOMRectReadOnly>> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => {
        let (width, height) = inner.resource.dimensions();
        Ok(Some(DOMRectReadOnly {
          x: 0.0,
          y: 0.0,
          width: width as f64,
          height: height as f64,
        }))
      }
      _ => Ok(None),
    }
  }

  /// Get the visible rect (the region of coded data that should be displayed)
  /// Returns DOMRectReadOnly per W3C WebCodecs spec
  /// Returns null after the resource is closed.
  #[napi(getter)]
  pub fn visible_rect(&self) -> Result<Option<DOMRectReadOnly>> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => Ok(Some(DOMRectReadOnly {
        x: inner.visible_left as f64,
        y: inner.visible_top as f64,
        width: inner.visible_width as f64,
        height: inner.visible_height as f64,
      })),
      _ => Ok(None),
    }
  }

  /// Get the presentation timestamp in microseconds
  /// Per W3C spec: "The timestamp getter steps are to return [[timestamp]]"
  /// The timestamp is preserved even after close() - only resource reference is cleared
  #[napi(getter)]
  pub fn timestamp(&self) -> Result<i64> {
    Ok(self.timestamp_us)
  }

  /// Get the duration in microseconds
  /// Per W3C spec: "The duration getter steps are to return [[duration]]"
  /// The duration is preserved even after close() - only resource reference is cleared
  #[napi(getter)]
  pub fn duration(&self) -> Result<Option<i64>> {
    Ok(self.duration_us)
  }

  /// Get the color space parameters
  #[napi(getter)]
  pub fn color_space(&self) -> Result<VideoColorSpace> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(match guard.as_ref() {
      Some(inner) => inner.color_space.clone(),
      None => VideoColorSpace::new(None),
    })
  }

  /// Get whether this VideoFrame has been closed (W3C WebCodecs spec)
  #[napi(getter)]
  pub fn closed(&self) -> Result<bool> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    Ok(guard.is_none())
  }

  /// Get the number of planes in this VideoFrame (W3C WebCodecs spec)
  /// The number depends on the pixel format:
  /// - RGBA, RGBX, BGRA, BGRX: 1 plane
  /// - NV12, NV21: 2 planes
  /// - I420, I422, I444: 3 planes
  /// - I420A, I422A, I444A: 4 planes
  #[napi(getter)]
  pub fn number_of_planes(&self, env: Env) -> Result<u32> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => Ok(
        inner
          .resource
          .format()
          .map(Self::get_number_of_planes)
          .unwrap_or(1),
      ),
      _ => throw_invalid_state_error(&env, "VideoFrame is closed"),
    }
  }

  /// Get the rotation in degrees clockwise (0, 90, 180, 270) - W3C WebCodecs spec
  /// Returns 0 after close (Close VideoFrame resets [[rotation]]).
  #[napi(getter)]
  pub fn rotation(&self) -> Result<f64> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(guard.as_ref().map_or(0.0, |inner| inner.rotation))
  }

  /// Get whether horizontal flip is applied - W3C WebCodecs spec
  /// Returns false after close (Close VideoFrame resets [[flip]]).
  #[napi(getter)]
  pub fn flip(&self) -> Result<bool> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    Ok(guard.as_ref().is_some_and(|inner| inner.flip))
  }

  /// Get the metadata associated with this VideoFrame - W3C WebCodecs spec
  /// Currently returns an empty metadata object as members are defined in the registry
  #[napi]
  pub fn metadata(&self, env: Env) -> Result<VideoFrameMetadata> {
    self
      .with_inner(|_inner| Ok(VideoFrameMetadata {}))
      .map_err(|error| throw_as_native_error(&env, error))
  }

  /// Calculate the allocation size needed for copyTo
  #[napi(ts_args_type = "options?: VideoFrameCopyToOptions | undefined | null")]
  pub fn allocation_size(&self, env: Env, options: Unknown) -> Result<u32> {
    self
      .allocation_size_of(&env, options)
      .map_err(|error| throw_as_native_error(&env, error))
  }

  fn allocation_size_of(&self, env: &Env, options: Unknown) -> Result<u32> {
    // Like Chrome, the options convert (and their getters run) before the closed check.
    let CopyToOptions {
      options,
      unsupported_color_space,
    } = CopyToOptions::from_js(options)?;
    let options = Some(options);
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    let inner = match guard.as_ref() {
      Some(inner) => inner,
      _ => return throw_invalid_state_error(env, "VideoFrame is closed"),
    };

    if inner.resource.format().is_none() {
      return throw_not_supported_error(
        env,
        "Failed to execute 'allocationSize' on 'VideoFrame': Operation is not supported when format is null.",
      );
    }
    if unsupported_color_space {
      return Err(not_supported_error(UNSUPPORTED_COLOR_SPACE_MESSAGE));
    }

    let (frame, original_format) = inner.resource.software()?;
    let format = options
      .as_ref()
      .and_then(|o| o.format)
      .unwrap_or(original_format);

    // Check if format conversion is supported
    // Per WPT: YUV→RGB and RGB→RGB work; RGB→YUV throws NotSupportedError
    if format != original_format && !original_format.can_convert_to(format) {
      return throw_not_supported_error(
        env,
        &format!(
          "Format conversion from {:?} to {:?} is not supported",
          original_format, format
        ),
      );
    }

    // Use rect from options or default to visible rect
    let (width, height) = if let Some(ref opts) = options {
      if let Some(ref rect) = opts.rect {
        // Parse and validate rect
        let default_rect = (
          inner.visible_left as f64,
          inner.visible_top as f64,
          inner.visible_width as f64,
          inner.visible_height as f64,
        );
        // Conversion precedes cropping, so offsets follow the output format.
        let frame_guard = frame.read();
        let (_, _, w, h) = parse_visible_rect(
          default_rect,
          Some(rect),
          frame_guard.width(),
          frame_guard.height(),
          format,
        )?;
        (w, h)
      } else {
        (inner.visible_width, inner.visible_height)
      }
    } else {
      (inner.visible_width, inner.visible_height)
    };

    // If custom layout is provided, validate and calculate size from layout
    if let Some(ref opts) = options
      && let Some(ref layout) = opts.layout
    {
      // Validate the layout (throws TypeError on invalid layout)
      // Note: validate_copy_layout needs Env for throwing, but returns Result
      Self::validate_copy_layout(layout, format, width)?;

      // Calculate total size as max(offset + stride * plane_height) across all planes
      // Use u64 to prevent overflow and checked arithmetic per W3C spec
      let mut max_end: u64 = 0;

      for (plane_idx, plane_layout) in layout.iter().enumerate() {
        let plane_height = Self::get_plane_height(format, height, plane_idx as u32);
        let plane_end =
          calculate_plane_end_checked(plane_layout.offset, plane_layout.stride, plane_height)?;
        max_end = max_end.max(plane_end);
      }

      // Ensure result fits in u32 (reasonable limit for allocation size)
      if max_end > u32::MAX as u64 {
        return Err(Error::new(
          Status::InvalidArg,
          "TypeError: computed allocation size exceeds maximum",
        ));
      }

      return Ok(max_end as u32);
    }

    Ok(Self::calculate_buffer_size(format, width, height))
  }

  /// Copy frame data to a Uint8Array
  ///
  /// Returns a Promise that resolves with an array of PlaneLayout objects.
  /// Options can specify target format and rect for cropped copy.
  /// Like Chrome, the bytes are written before this call returns.
  //
  // JS can detach, transfer or shrink `destination` from an options getter or right after the
  // call. So the destination is looked up only after the getters have run, and it is never
  // written once JS can run again (no later write, no write from another thread).
  #[napi(
    ts_args_type = "destination: Uint8Array, options?: VideoFrameCopyToOptions | undefined | null",
    ts_return_type = "Promise<PlaneLayout[]>"
  )]
  pub fn copy_to<'env>(
    &self,
    env: &'env Env,
    destination: Unknown,
    options: Unknown,
  ) -> Result<PromiseRaw<'env, Vec<PlaneLayout>>> {
    // Like Chrome, every error (bad arguments included) rejects the promise; nothing throws.
    match self.copy_into(env, destination, options) {
      Ok(layouts) => PromiseRaw::resolve(env, layouts),
      Err(error) => {
        // An options getter, `toString` or `valueOf` threw: reject with what it threw.
        if let Some(exception) = take_pending_exception(env)? {
          return PromiseRaw::reject(env, exception);
        }
        match error.reason.strip_prefix("TypeError:") {
          Some(message) => reject_with_type_error(env, message.trim_start()),
          // Page code can break the global DOMException; that still rejects, never throws.
          None => PromiseRaw::reject(env, dom_exception_for_rejection(env, error)?),
        }
      }
    }
  }

  /// The synchronous body of copyTo. The destination bytes are looked up after every options
  /// getter has run and written before any JS runs again.
  fn copy_into(
    &self,
    env: &Env,
    destination: Unknown,
    options: Unknown,
  ) -> Result<Vec<PlaneLayout>> {
    // WebIDL order: the destination converts before the options, so a bad destination rejects
    // without running the options getters. The bytes it views can change once they run, so keep
    // only the JS value.
    let destination = destination.raw();
    if !is_uint8_array(env, destination)? {
      return Err(type_error("destination is not a Uint8Array."));
    }
    // Chrome rejects a resizable or growable buffer while it converts the destination.
    if is_resizable_view(env, destination)? {
      return Err(type_error(
        "The provided ArrayBufferView value must not be resizable.",
      ));
    }
    let CopyToOptions {
      options,
      unsupported_color_space,
    } = CopyToOptions::from_js(options)?;
    let options = Some(&options);

    // Get format, rect info and validate the options (brief lock)
    let (
      frame,
      color_tags,
      format,
      rect_x,
      rect_y,
      rect_width,
      rect_height,
      size,
      custom_layout,
      original_format,
      needs_conversion,
    ) = {
      let guard = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      let inner = match guard.as_ref() {
        Some(inner) => inner,
        _ => return Err(invalid_state_error("VideoFrame is closed")),
      };

      if inner.resource.format().is_none() {
        return Err(not_supported_error(
          "Failed to execute 'copyTo' on 'VideoFrame': Operation is not supported when format is null.",
        ));
      }
      if unsupported_color_space {
        return Err(not_supported_error(UNSUPPORTED_COLOR_SPACE_MESSAGE));
      }
      let (frame, original_format) = inner.resource.software()?;
      let format = options.and_then(|o| o.format).unwrap_or(original_format);

      // Check if format conversion is supported
      // Per WPT: YUV→RGB and RGB→RGB work; RGB→YUV throws NotSupportedError
      let needs_conversion = format != original_format;
      if needs_conversion && !original_format.can_convert_to(format) {
        return Err(not_supported_error(&format!(
          "Format conversion from {:?} to {:?} is not supported",
          original_format, format
        )));
      }

      // Parse rect (default to visible rect)
      // Conversion precedes cropping, so offsets follow the output format.
      let default_rect = (
        inner.visible_left as f64,
        inner.visible_top as f64,
        inner.visible_width as f64,
        inner.visible_height as f64,
      );
      let rect = options.and_then(|o| o.rect.as_ref());
      let frame_guard = frame.read();
      let (rect_x, rect_y, rect_width, rect_height) = parse_visible_rect(
        default_rect,
        rect,
        frame_guard.width(),
        frame_guard.height(),
        format,
      )?;
      drop(frame_guard);
      // Chromium's YUV view starts at the common chroma-aligned origin.
      let (h_factor, v_factor) = get_subsampling_factors(original_format);
      let rect_x = rect_x / h_factor * h_factor;
      let rect_y = rect_y / v_factor * v_factor;

      // Get and validate custom layout if provided
      let custom_layout = options.and_then(|o| o.layout.clone());
      if let Some(ref layout) = custom_layout {
        Self::validate_copy_layout(layout, format, rect_width)?;
      }

      // Calculate actual required size based on layout or default
      // Use u64 to prevent overflow and checked arithmetic per W3C spec
      let required_size = if let Some(ref layout) = custom_layout {
        // For custom layout, need space for all planes at their offsets + sizes
        let mut max_end: u64 = 0;
        for (plane_idx, plane_layout) in layout.iter().enumerate() {
          let plane_height = Self::get_plane_height(format, rect_height, plane_idx as u32);
          let plane_end =
            calculate_plane_end_checked(plane_layout.offset, plane_layout.stride, plane_height)?;
          max_end = max_end.max(plane_end);
        }

        // Ensure result fits in usize (prevent overflow on 32-bit systems)
        if max_end > usize::MAX as u64 {
          return Err(Error::new(
            Status::InvalidArg,
            "TypeError: computed buffer size exceeds maximum",
          ));
        }
        max_end as usize
      } else {
        Self::calculate_buffer_size(format, rect_width, rect_height) as usize
      };

      (
        frame.clone(),
        // color metadata belongs to the VideoFrame, not necessarily its AVFrame.
        rgb_copy_tags(&inner.color_space),
        format,
        rect_x,
        rect_y,
        rect_width,
        rect_height,
        required_size,
        custom_layout,
        original_format,
        needs_conversion,
      )
    };

    // No JS runs from here until the copy ends, so the destination cannot change under it.
    let destination_bytes = unsafe { typed_array_bytes(env.raw(), destination)? };
    let available = destination_bytes.len();
    if available < size {
      if is_detached_view(env, destination)? {
        return Err(type_error("destination is detached"));
      }
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "TypeError: destination buffer too small: need {} bytes, got {}",
          size, available
        ),
      ));
    }
    // Like Chrome, an explicit format must be an RGB one, even when it is the frame's own
    // format. Chrome checks this after the destination.
    if options.is_some_and(|o| o.format.is_some()) && !format.is_rgb() {
      return Err(not_supported_error(
        "copyTo() doesn't support explicit copy to non-RGB formats. Remove the format option to copy in the frame's pixel format.",
      ));
    }
    let dest = &mut destination_bytes[..size];

    // Hold the frame's read lock for the duration of the copy
    let frame_guard = frame.read();

    let target_color = options.and_then(|o| {
      o.format
        .filter(|f| {
          matches!(
            f,
            VideoPixelFormat::RGBA
              | VideoPixelFormat::BGRA
              | VideoPixelFormat::RGBX
              | VideoPixelFormat::BGRX
          )
        })
        .map(|_| o.color_space.unwrap_or(PredefinedColorSpace::Srgb))
    });
    if let Some(target) = target_color {
      // stride/offset of the single RGB plane in the destination
      let (offset, stride) = match custom_layout.as_deref() {
        Some(layout) => (layout[0].offset as usize, layout[0].stride as usize),
        None => (0, rect_width as usize * 4),
      };
      copy_rect_to_rgb_in_bands(
        &frame_guard,
        (rect_x, rect_y, rect_width, rect_height),
        color_tags,
        target == PredefinedColorSpace::DisplayP3,
        matches!(format, VideoPixelFormat::BGRA | VideoPixelFormat::BGRX),
        format == original_format,
        matches!(
          original_format,
          VideoPixelFormat::RGBX | VideoPixelFormat::BGRX
        ),
        dest.get_mut(offset..).unwrap_or_default(),
        stride,
      )?;
    } else if needs_conversion {
      let src_av_format = original_format.to_av_format();
      let dst_av_format = format.to_av_format();

      // Use Scaler for format conversion (operates on full frame, then crop)
      let scaler = Scaler::new_converter(
        frame_guard.width(),
        frame_guard.height(),
        src_av_format,
        dst_av_format,
      )
      .map_err(|e| {
        Error::new(
          Status::GenericFailure,
          format!(
            "NotSupportedError: Failed to create format converter: {}",
            e
          ),
        )
      })?;

      // scale_alloc creates a new frame with converted data
      let converted = scaler.scale_alloc(&frame_guard).map_err(|e| {
        Error::new(
          Status::GenericFailure,
          format!("EncodingError: Format conversion failed: {}", e),
        )
      })?;
      drop(frame_guard);

      // Copy cropped region from converted frame
      Self::copy_cropped_data(
        &converted,
        format,
        rect_x,
        rect_y,
        rect_width,
        rect_height,
        dest,
        custom_layout.as_deref(),
      )?;
    } else {
      // No conversion needed - copy straight from the read-locked frame
      Self::copy_cropped_data(
        &frame_guard,
        format,
        rect_x,
        rect_y,
        rect_width,
        rect_height,
        dest,
        custom_layout.as_deref(),
      )?;
    }

    // Return custom layout if provided, otherwise calculate default layout
    let layouts = if let Some(layout) = custom_layout {
      layout
    } else {
      Self::get_plane_layouts(format, rect_width, rect_height)
    };
    Ok(layouts)
  }

  /// Calculate minimum stride for a plane given format, width, and plane index
  fn get_min_plane_stride(format: VideoPixelFormat, width: u32, plane_idx: u32) -> u32 {
    let bps = format.bytes_per_sample() as u32;
    let (h_factor, _) = get_subsampling_factors(format);

    match (format, plane_idx) {
      // RGBA formats: 4 bytes per pixel
      (
        VideoPixelFormat::RGBA
        | VideoPixelFormat::RGBX
        | VideoPixelFormat::BGRA
        | VideoPixelFormat::BGRX,
        0,
      ) => width * 4,
      // Y plane and alpha plane: full width
      (_, 0) => width * bps,
      // Alpha plane for formats with alpha
      (
        VideoPixelFormat::I420A
        | VideoPixelFormat::I420AP10
        | VideoPixelFormat::I422A
        | VideoPixelFormat::I422AP10
        | VideoPixelFormat::I444A
        | VideoPixelFormat::I444AP10,
        3,
      ) => width * bps,
      // NV12/NV21 UV plane: interleaved, 2 bytes per sample position
      (VideoPixelFormat::NV12 | VideoPixelFormat::NV21, 1) => (width / h_factor) * 2,
      // U/V planes for planar formats: subsampled width
      _ => (width / h_factor) * bps,
    }
  }

  /// Get the height of a specific plane for a given format
  fn get_plane_height(format: VideoPixelFormat, frame_height: u32, plane_idx: u32) -> u32 {
    let (_, v_factor) = get_subsampling_factors(format);

    match (format, plane_idx) {
      // RGBA formats: single plane with full height
      (
        VideoPixelFormat::RGBA
        | VideoPixelFormat::RGBX
        | VideoPixelFormat::BGRA
        | VideoPixelFormat::BGRX,
        0,
      ) => frame_height,
      // Y plane (index 0): always full height
      (_, 0) => frame_height,
      // Alpha plane (index 3) for formats with alpha: full height
      (
        VideoPixelFormat::I420A
        | VideoPixelFormat::I420AP10
        | VideoPixelFormat::I422A
        | VideoPixelFormat::I422AP10
        | VideoPixelFormat::I444A
        | VideoPixelFormat::I444AP10,
        3,
      ) => frame_height,
      // UV planes (index 1, 2): subsampled height
      // NV12/NV21 interleaved UV plane: subsampled height
      _ => frame_height / v_factor,
    }
  }

  /// Validate custom layout for copyTo
  fn validate_copy_layout(
    layout: &[PlaneLayout],
    format: VideoPixelFormat,
    width: u32,
  ) -> Result<()> {
    let num_planes = Self::get_number_of_planes(format);

    if layout.len() != num_planes as usize {
      return Err(type_error(&format!(
        "layout must have {} entries for format {:?}, got {}",
        num_planes,
        format,
        layout.len()
      )));
    }

    for (i, plane) in layout.iter().enumerate() {
      let min_stride = Self::get_min_plane_stride(format, width, i as u32);
      if plane.stride < min_stride {
        return Err(type_error(&format!(
          "stride {} for plane {} is less than minimum required stride {}",
          plane.stride, i, min_stride
        )));
      }
    }

    Ok(())
  }

  /// Copy cropped region from frame to buffer (row-by-row per plane)
  /// If custom_layout is provided, uses its offset/stride for destination buffer.
  #[allow(clippy::too_many_arguments)]
  fn copy_cropped_data(
    frame: &Frame,
    format: VideoPixelFormat,
    rect_x: u32,
    rect_y: u32,
    rect_width: u32,
    rect_height: u32,
    dest: &mut [u8],
    custom_layout: Option<&[PlaneLayout]>,
  ) -> Result<()> {
    let (h_factor, v_factor) = get_subsampling_factors(format);
    let bps = format.bytes_per_sample() as u32;
    let num_planes = Self::get_number_of_planes(format);

    // Track default offset for when no custom layout is provided
    let mut default_dest_offset = 0usize;

    for plane_idx in 0..num_planes {
      // Determine plane's properties based on format and plane index
      let (plane_h_factor, plane_v_factor, plane_sample_bytes) = match (format, plane_idx) {
        // Y plane (or only plane for RGB) - no subsampling
        (
          VideoPixelFormat::RGBA
          | VideoPixelFormat::RGBX
          | VideoPixelFormat::BGRA
          | VideoPixelFormat::BGRX,
          0,
        ) => {
          (1, 1, 4) // RGBA: 4 bytes per pixel
        }
        (_, 0) => {
          // Y plane for YUV formats
          (1, 1, bps)
        }
        // NV12/NV21 UV plane - interleaved, 2 bytes per sample position
        (VideoPixelFormat::NV12 | VideoPixelFormat::NV21, 1) => {
          (h_factor, v_factor, 2) // UV interleaved: 2 bytes per chroma sample position
        }
        // Alpha plane (4th plane for formats with alpha)
        (
          VideoPixelFormat::I420A
          | VideoPixelFormat::I420AP10
          | VideoPixelFormat::I422A
          | VideoPixelFormat::I422AP10
          | VideoPixelFormat::I444A
          | VideoPixelFormat::I444AP10,
          3,
        ) => {
          (1, 1, bps) // Alpha plane - same resolution as Y
        }
        // U/V planes for planar formats
        _ => (h_factor, v_factor, bps),
      };

      // Calculate plane dimensions
      let plane_src_x = rect_x / plane_h_factor;
      let plane_src_y = rect_y / plane_v_factor;
      let plane_width = rect_width / plane_h_factor;
      let plane_height = rect_height / plane_v_factor;
      let default_bytes_per_row = plane_width * plane_sample_bytes;

      // Use custom layout offset/stride if provided, otherwise use calculated defaults
      let (dest_plane_offset, dest_stride) = match custom_layout {
        Some(layout) => (
          layout[plane_idx as usize].offset as usize,
          layout[plane_idx as usize].stride as usize,
        ),
        None => (default_dest_offset, default_bytes_per_row as usize),
      };

      // Get source plane data and stride
      let src_data = frame.data(plane_idx as usize);
      let src_stride = frame.linesize(plane_idx as usize) as usize;

      if src_data.is_null() {
        return Err(Error::new(
          Status::GenericFailure,
          format!("Plane {} data is null", plane_idx),
        ));
      }

      // Callers size `dest` from the same layout. Check anyway: `dest` can be JS memory.
      if plane_height > 0 {
        let plane_end = (plane_height as usize - 1)
          .checked_mul(dest_stride)
          .and_then(|v| v.checked_add(dest_plane_offset))
          .and_then(|v| v.checked_add(default_bytes_per_row as usize));
        if plane_end.is_none_or(|end| end > dest.len()) {
          return Err(Error::new(
            Status::GenericFailure,
            format!("Plane {} does not fit the destination", plane_idx),
          ));
        }
      }

      // Copy row by row
      for row in 0..plane_height {
        let src_row_offset = ((plane_src_y + row) as usize) * src_stride
          + (plane_src_x as usize) * (plane_sample_bytes as usize);
        let dest_row_offset = dest_plane_offset + (row as usize) * dest_stride;

        unsafe {
          std::ptr::copy_nonoverlapping(
            src_data.add(src_row_offset),
            dest.as_mut_ptr().add(dest_row_offset),
            default_bytes_per_row as usize, // Always copy the actual data width, not the padded stride
          );
        }
      }

      // Update default offset for next plane (only used when no custom layout)
      default_dest_offset += (plane_height * default_bytes_per_row) as usize;
    }

    Ok(())
  }

  /// Calculate plane layouts for a given format
  fn get_plane_layouts(format: VideoPixelFormat, width: u32, height: u32) -> Vec<PlaneLayout> {
    let bps = format.bytes_per_sample() as u32; // bytes per sample (1 for 8-bit, 2 for 10/12-bit)

    match format {
      // 4:2:0 formats (Y, U, V planes)
      VideoPixelFormat::I420 | VideoPixelFormat::I420P10 | VideoPixelFormat::I420P12 => {
        let y_stride = width * bps;
        let y_size = y_stride * height;
        let uv_stride = (width / 2) * bps;
        let uv_size = uv_stride * (height / 2);
        vec![
          PlaneLayout {
            offset: 0,
            stride: y_stride,
          },
          PlaneLayout {
            offset: y_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size,
            stride: uv_stride,
          },
        ]
      }
      // 4:2:0 with alpha (Y, U, V, A planes)
      VideoPixelFormat::I420A | VideoPixelFormat::I420AP10 => {
        let y_stride = width * bps;
        let y_size = y_stride * height;
        let uv_stride = (width / 2) * bps;
        let uv_size = uv_stride * (height / 2);
        vec![
          PlaneLayout {
            offset: 0,
            stride: y_stride,
          },
          PlaneLayout {
            offset: y_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size * 2,
            stride: y_stride,
          },
        ]
      }
      // 4:2:2 formats (Y, U, V planes)
      VideoPixelFormat::I422 | VideoPixelFormat::I422P10 | VideoPixelFormat::I422P12 => {
        let y_stride = width * bps;
        let y_size = y_stride * height;
        let uv_stride = (width / 2) * bps;
        let uv_size = uv_stride * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: y_stride,
          },
          PlaneLayout {
            offset: y_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size,
            stride: uv_stride,
          },
        ]
      }
      // 4:2:2 with alpha (Y, U, V, A planes)
      VideoPixelFormat::I422A | VideoPixelFormat::I422AP10 => {
        let y_stride = width * bps;
        let y_size = y_stride * height;
        let uv_stride = (width / 2) * bps;
        let uv_size = uv_stride * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: y_stride,
          },
          PlaneLayout {
            offset: y_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size * 2,
            stride: y_stride,
          },
        ]
      }
      // 4:4:4 formats (Y, U, V planes)
      VideoPixelFormat::I444 | VideoPixelFormat::I444P10 | VideoPixelFormat::I444P12 => {
        let plane_stride = width * bps;
        let plane_size = plane_stride * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: plane_stride,
          },
          PlaneLayout {
            offset: plane_size,
            stride: plane_stride,
          },
          PlaneLayout {
            offset: plane_size * 2,
            stride: plane_stride,
          },
        ]
      }
      // 4:4:4 with alpha (Y, U, V, A planes)
      VideoPixelFormat::I444A | VideoPixelFormat::I444AP10 => {
        let plane_stride = width * bps;
        let plane_size = plane_stride * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: plane_stride,
          },
          PlaneLayout {
            offset: plane_size,
            stride: plane_stride,
          },
          PlaneLayout {
            offset: plane_size * 2,
            stride: plane_stride,
          },
          PlaneLayout {
            offset: plane_size * 3,
            stride: plane_stride,
          },
        ]
      }
      // Semi-planar formats (Y plane + interleaved UV/VU)
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => {
        let y_size = width * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: width,
          },
          PlaneLayout {
            offset: y_size,
            stride: width,
          },
        ]
      }
      // RGBA formats (single packed plane)
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => {
        vec![PlaneLayout {
          offset: 0,
          stride: width * 4,
        }]
      }
    }
  }

  /// Clone this VideoFrame
  ///
  /// This creates a new VideoFrame that shares the underlying pixel data with the original.
  /// Both frames will reference the same Arc<RwLock<Frame>>, so no pixel data is copied.
  #[napi(js_name = "clone")]
  pub fn clone_frame(&self, env: Env) -> Result<VideoFrame> {
    // Check closed state and throw native DOMException
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    let inner = match guard.as_ref() {
      Some(inner) => inner,
      _ => return throw_invalid_state_error(&env, "VideoFrame is closed"),
    };

    // Clone the Arc - shares the same underlying Frame (no pixel data copy)
    let new_inner = VideoFrameInner {
      resource: inner.resource.clone(),
      timestamp_us: inner.timestamp_us,
      duration_us: inner.duration_us,
      visible_left: inner.visible_left,
      visible_top: inner.visible_top,
      visible_width: inner.visible_width,
      visible_height: inner.visible_height,
      display_width: inner.display_width,
      display_height: inner.display_height,
      rotation: inner.rotation,
      flip: inner.flip,
      color_space: inner.color_space.clone(),
    };

    Ok(VideoFrame::from_inner(new_inner))
  }

  /// Close and release resources
  /// Per W3C spec "Close VideoFrame" algorithm:
  /// 1. Assign null to frame's [[resource reference]]
  /// 2. Assign true to frame's [[Detached]]
  /// Note: timestamp and duration remain accessible after close; rotation becomes 0,
  /// flip false, codedRect and visibleRect null, every colorSpace field becomes null,
  /// and callbacks registered with registerVideoFrameCloseObserver run synchronously.
  #[napi(ts_args_type = "")]
  pub fn close(&self, env: Env) -> Result<()> {
    let mut guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    // Detach and drop the resource immediately. Timestamp and duration are stored
    // separately so their getters remain available after detachment.
    *guard = None;
    drop(guard);
    // Expire consumers synchronously on explicit close. Garbage collection
    // releases observer references without expiring independently owned images.
    let observers =
      CLOSE_OBSERVERS.with(|registry| registry.borrow_mut().remove(&self.observer_id));
    let (holder, callbacks) = match observers {
      Some(mut entry) => {
        let holder = entry.holder.get(&env);
        entry.holder.clear(env.raw());
        (holder, std::mem::take(&mut entry.callbacks))
      }
      None => (None, Vec::new()),
    };
    let mut thrown = Vec::new();
    for (id, reference) in callbacks {
      let callback = super::event_target::upgrade_ref(&env, reference)?;
      super::event_target::release_callback(&env, holder, id, reference);
      let Some(callback) = callback else { continue };
      let mut result = std::ptr::null_mut();
      let mut receiver = std::ptr::null_mut();
      unsafe { napi::sys::napi_get_undefined(env.raw(), &mut receiver) };
      let status = unsafe {
        napi::sys::napi_call_function(
          env.raw(),
          receiver,
          callback.raw(),
          0,
          std::ptr::null(),
          &mut result,
        )
      };
      if status != napi::sys::Status::napi_ok {
        let mut exception = std::ptr::null_mut();
        if unsafe { napi::sys::napi_get_and_clear_last_exception(env.raw(), &mut exception) }
          == napi::sys::Status::napi_ok
          && !exception.is_null()
        {
          thrown.push(exception);
        }
      }
    }
    for exception in thrown {
      super::event_target::report_exception(&env, exception);
    }
    Ok(())
  }

  // ========================================================================
  // Internal helpers (crate-visible only)
  // ========================================================================

  /// Get a clone of the Arc<RwLock<Frame>> for sharing with other components.
  ///
  /// This is the preferred way to pass frame data to encoders/other consumers
  /// that need to share the frame without deep copying.
  pub(crate) fn frame_arc(&self) -> Result<Arc<RwLock<Frame>>> {
    self.with_inner(|inner| Ok(inner.resource.software()?.0.clone()))
  }

  /// Applies a VideoDecoderConfig displayAspectWidth:displayAspectHeight to a decoded frame
  /// (spec "Create a VideoFrame" step 8): grows the display width or height to the ratio.
  ///
  /// Fails when the grown size exceeds the media limits; Chromium's decoder reports an
  /// EncodingError then.
  pub(crate) fn apply_display_aspect(&self, aspect_width: u32, aspect_height: u32) -> Result<()> {
    let mut guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;
    let Some(inner) = guard.as_mut() else {
      return Ok(());
    };
    let Some((width, height)) = natural_size_for_display_aspect(
      inner.visible_width,
      inner.visible_height,
      aspect_width,
      aspect_height,
    ) else {
      return Ok(());
    };
    if width > f64::from(MAX_DIMENSION)
      || height > f64::from(MAX_DIMENSION)
      || width * height > MAX_CANVAS as f64
    {
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "Display size {}x{} for display aspect {}:{} exceeds implementation limit",
          width, height, aspect_width, aspect_height
        ),
      ));
    }
    inner.display_width = width as u32;
    inner.display_height = height as u32;
    Ok(())
  }

  /// Borrow internal frame for read access (crate internal use)
  ///
  /// Acquires a read lock on the internal frame and calls the closure with it.
  #[allow(dead_code)]
  pub(crate) fn with_frame<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&Frame) -> R,
  {
    self.with_inner(|inner| {
      let frame_guard = inner.resource.software()?.0.read();
      Ok(f(&frame_guard))
    })
  }

  fn with_inner<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&VideoFrameInner) -> Result<R>,
  {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) => f(inner),
      _ => Err(invalid_state_error("VideoFrame is closed")),
    }
  }

  fn calculate_buffer_size(format: VideoPixelFormat, width: u32, height: u32) -> u32 {
    let w = width;
    let h = height;
    let bps = format.bytes_per_sample() as u32; // bytes per sample (1 for 8-bit, 2 for 10/12-bit)

    match format {
      // 4:2:0 formats (Y plane + U plane + V plane)
      VideoPixelFormat::I420 | VideoPixelFormat::I420P10 | VideoPixelFormat::I420P12 => {
        (w * h + (w / 2) * (h / 2) * 2) * bps
      }
      // 4:2:0 with alpha (Y + U + V + A)
      VideoPixelFormat::I420A | VideoPixelFormat::I420AP10 => {
        (w * h * 2 + (w / 2) * (h / 2) * 2) * bps
      }
      // 4:2:2 formats (Y + U + V)
      VideoPixelFormat::I422 | VideoPixelFormat::I422P10 | VideoPixelFormat::I422P12 => {
        (w * h + (w / 2) * h * 2) * bps
      }
      // 4:2:2 with alpha (Y + U + V + A)
      VideoPixelFormat::I422A | VideoPixelFormat::I422AP10 => (w * h * 2 + (w / 2) * h * 2) * bps,
      // 4:4:4 formats (Y + U + V)
      VideoPixelFormat::I444 | VideoPixelFormat::I444P10 | VideoPixelFormat::I444P12 => {
        w * h * 3 * bps
      }
      // 4:4:4 with alpha (Y + U + V + A)
      VideoPixelFormat::I444A | VideoPixelFormat::I444AP10 => w * h * 4 * bps,
      // Semi-planar (Y + interleaved UV/VU)
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => w * h + w * (h / 2),
      // RGBA formats (4 bytes per pixel)
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => w * h * 4,
    }
  }

  fn get_number_of_planes(format: VideoPixelFormat) -> u32 {
    match format {
      // RGBA formats: single packed plane
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => 1,
      // Semi-planar: Y plane + interleaved UV
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => 2,
      // 3-plane formats: Y, U, V
      VideoPixelFormat::I420
      | VideoPixelFormat::I420P10
      | VideoPixelFormat::I420P12
      | VideoPixelFormat::I422
      | VideoPixelFormat::I422P10
      | VideoPixelFormat::I422P12
      | VideoPixelFormat::I444
      | VideoPixelFormat::I444P10
      | VideoPixelFormat::I444P12 => 3,
      // 4-plane formats: Y, U, V, A
      VideoPixelFormat::I420A
      | VideoPixelFormat::I420AP10
      | VideoPixelFormat::I422A
      | VideoPixelFormat::I422AP10
      | VideoPixelFormat::I444A
      | VideoPixelFormat::I444AP10 => 4,
    }
  }

  fn copy_data_to_frame(
    frame: &mut Frame,
    data: &[u8],
    format: VideoPixelFormat,
    width: u32,
    height: u32,
    layout: Option<&[PlaneLayout]>,
  ) -> Result<()> {
    // Helper to get source offset and stride for a plane
    let get_src_layout =
      |plane_idx: usize, default_offset: usize, default_stride: usize| match layout {
        Some(l) if plane_idx < l.len() => {
          (l[plane_idx].offset as usize, l[plane_idx].stride as usize)
        }
        _ => (default_offset, default_stride),
      };

    // Validate layout and source data size
    if let Some(l) = layout {
      Self::validate_copy_layout(l, format, width)?;

      // Calculate required size from custom layout
      let mut max_end = 0usize;
      for (plane_idx, plane_layout) in l.iter().enumerate() {
        let plane_height = Self::get_plane_height(format, height, plane_idx as u32);
        let plane_end =
          plane_layout.offset as usize + (plane_layout.stride as usize * plane_height as usize);
        max_end = max_end.max(plane_end);
      }

      if data.len() < max_end {
        return Err(type_error(&format!(
          "Source data too small for custom layout: need {} bytes, got {}",
          max_end,
          data.len()
        )));
      }
    } else {
      // Default tightly-packed layout check
      let expected_size = Self::calculate_buffer_size(format, width, height) as usize;
      if data.len() < expected_size {
        return Err(Error::new(
          Status::GenericFailure,
          format!(
            "Input data too small: need {} bytes, got {}",
            expected_size,
            data.len()
          ),
        ));
      }
    }

    // Get all linesizes first to avoid borrow conflicts
    let linesize0 = frame.linesize(0) as usize;
    let linesize1 = frame.linesize(1) as usize;
    let linesize2 = frame.linesize(2) as usize;
    let linesize3 = frame.linesize(3) as usize;

    match format {
      VideoPixelFormat::I420 | VideoPixelFormat::I420A => {
        let y_row_bytes = width as usize;
        let u_width = (width / 2) as usize;
        let u_height = (height / 2) as usize;
        // Default offsets for tightly-packed layout
        let y_size = y_row_bytes * height as usize;
        let u_size = u_width * u_height;

        // Get source layout for each plane (custom or default)
        let (y_src_offset, y_src_stride) = get_src_layout(0, 0, y_row_bytes);
        let (u_src_offset, u_src_stride) = get_src_layout(1, y_size, u_width);
        let (v_src_offset, v_src_stride) = get_src_layout(2, y_size + u_size, u_width);

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = y_src_offset + row * y_src_stride;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..u_height {
            let src_start = u_src_offset + row * u_src_stride;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + u_width]
              .copy_from_slice(&data[src_start..src_start + u_width]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..u_height {
            let src_start = v_src_offset + row * v_src_stride;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + u_width]
              .copy_from_slice(&data[src_start..src_start + u_width]);
          }
        }

        // Copy A plane if present
        if format == VideoPixelFormat::I420A {
          let (a_src_offset, a_src_stride) = get_src_layout(3, y_size + u_size * 2, y_row_bytes);
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_src_offset + row * a_src_stride;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }
      }
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => {
        // NV12: Y plane + interleaved UV
        // NV21: Y plane + interleaved VU (same layout, just U/V swapped)
        let y_row_bytes = width as usize;
        let uv_row_bytes = width as usize; // Interleaved U+V = width bytes
        let uv_height = (height / 2) as usize;
        let y_size = y_row_bytes * height as usize;

        let (y_src_offset, y_src_stride) = get_src_layout(0, 0, y_row_bytes);
        let (uv_src_offset, uv_src_stride) = get_src_layout(1, y_size, uv_row_bytes);

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = y_src_offset + row * y_src_stride;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }

        // Copy UV/VU plane (interleaved)
        {
          let uv_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get UV/VU plane"))?;
          for row in 0..uv_height {
            let src_start = uv_src_offset + row * uv_src_stride;
            let dst_start = row * linesize1;
            uv_plane[dst_start..dst_start + uv_row_bytes]
              .copy_from_slice(&data[src_start..src_start + uv_row_bytes]);
          }
        }
      }
      VideoPixelFormat::I422 | VideoPixelFormat::I422A => {
        // I422: 4:2:2 - Y full resolution, U/V half width, full height
        let y_row_bytes = width as usize;
        let uv_width = (width / 2) as usize;
        let y_size = y_row_bytes * height as usize;
        let uv_size = uv_width * height as usize;

        let (y_src_offset, y_src_stride) = get_src_layout(0, 0, y_row_bytes);
        let (u_src_offset, u_src_stride) = get_src_layout(1, y_size, uv_width);
        let (v_src_offset, v_src_stride) = get_src_layout(2, y_size + uv_size, uv_width);

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = y_src_offset + row * y_src_stride;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..height as usize {
            let src_start = u_src_offset + row * u_src_stride;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + uv_width]
              .copy_from_slice(&data[src_start..src_start + uv_width]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..height as usize {
            let src_start = v_src_offset + row * v_src_stride;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + uv_width]
              .copy_from_slice(&data[src_start..src_start + uv_width]);
          }
        }

        // Copy A plane if present
        if format == VideoPixelFormat::I422A {
          let (a_src_offset, a_src_stride) = get_src_layout(3, y_size + uv_size * 2, y_row_bytes);
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_src_offset + row * a_src_stride;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }
      }
      VideoPixelFormat::I444 | VideoPixelFormat::I444A => {
        // I444: 4:4:4 - Y, U, V all full resolution
        let row_bytes = width as usize;
        let plane_size = row_bytes * height as usize;

        let (y_src_offset, y_src_stride) = get_src_layout(0, 0, row_bytes);
        let (u_src_offset, u_src_stride) = get_src_layout(1, plane_size, row_bytes);
        let (v_src_offset, v_src_stride) = get_src_layout(2, plane_size * 2, row_bytes);

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = y_src_offset + row * y_src_stride;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + row_bytes]
              .copy_from_slice(&data[src_start..src_start + row_bytes]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..height as usize {
            let src_start = u_src_offset + row * u_src_stride;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + row_bytes]
              .copy_from_slice(&data[src_start..src_start + row_bytes]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..height as usize {
            let src_start = v_src_offset + row * v_src_stride;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + row_bytes]
              .copy_from_slice(&data[src_start..src_start + row_bytes]);
          }
        }

        // Copy A plane if present
        if format == VideoPixelFormat::I444A {
          let (a_src_offset, a_src_stride) = get_src_layout(3, plane_size * 3, row_bytes);
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_src_offset + row * a_src_stride;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + row_bytes]
              .copy_from_slice(&data[src_start..src_start + row_bytes]);
          }
        }
      }
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => {
        let row_bytes = (width * 4) as usize;
        let (src_offset, src_stride) = get_src_layout(0, 0, row_bytes);

        // Copy packed RGBA data
        let plane = frame
          .plane_data_mut(0)
          .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get plane"))?;
        for row in 0..height as usize {
          let src_start = src_offset + row * src_stride;
          let dst_start = row * linesize0;
          plane[dst_start..dst_start + row_bytes]
            .copy_from_slice(&data[src_start..src_start + row_bytes]);
        }
      }
      // 10-bit and 12-bit 4:2:0 formats (2 bytes per sample)
      VideoPixelFormat::I420P10 | VideoPixelFormat::I420P12 | VideoPixelFormat::I420AP10 => {
        let bps = 2usize; // bytes per sample
        let y_row_bytes = width as usize * bps;
        let y_size = y_row_bytes * height as usize;
        let uv_width = (width / 2) as usize;
        let uv_row_bytes = uv_width * bps;
        let uv_height = (height / 2) as usize;
        let uv_size = uv_row_bytes * uv_height;

        let (y_src_offset, y_src_stride) = get_src_layout(0, 0, y_row_bytes);
        let (u_src_offset, u_src_stride) = get_src_layout(1, y_size, uv_row_bytes);
        let (v_src_offset, v_src_stride) = get_src_layout(2, y_size + uv_size, uv_row_bytes);

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = y_src_offset + row * y_src_stride;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..uv_height {
            let src_start = u_src_offset + row * u_src_stride;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + uv_row_bytes]
              .copy_from_slice(&data[src_start..src_start + uv_row_bytes]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..uv_height {
            let src_start = v_src_offset + row * v_src_stride;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + uv_row_bytes]
              .copy_from_slice(&data[src_start..src_start + uv_row_bytes]);
          }
        }

        // Copy A plane if present (10-bit alpha)
        if format == VideoPixelFormat::I420AP10 {
          let (a_src_offset, a_src_stride) = get_src_layout(3, y_size + uv_size * 2, y_row_bytes);
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_src_offset + row * a_src_stride;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }
      }
      // 10-bit and 12-bit 4:2:2 formats (2 bytes per sample)
      VideoPixelFormat::I422P10 | VideoPixelFormat::I422P12 | VideoPixelFormat::I422AP10 => {
        let bps = 2usize; // bytes per sample
        let y_row_bytes = width as usize * bps;
        let y_size = y_row_bytes * height as usize;
        let uv_width = (width / 2) as usize;
        let uv_row_bytes = uv_width * bps;
        let uv_size = uv_row_bytes * height as usize;

        let (y_src_offset, y_src_stride) = get_src_layout(0, 0, y_row_bytes);
        let (u_src_offset, u_src_stride) = get_src_layout(1, y_size, uv_row_bytes);
        let (v_src_offset, v_src_stride) = get_src_layout(2, y_size + uv_size, uv_row_bytes);

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = y_src_offset + row * y_src_stride;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..height as usize {
            let src_start = u_src_offset + row * u_src_stride;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + uv_row_bytes]
              .copy_from_slice(&data[src_start..src_start + uv_row_bytes]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..height as usize {
            let src_start = v_src_offset + row * v_src_stride;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + uv_row_bytes]
              .copy_from_slice(&data[src_start..src_start + uv_row_bytes]);
          }
        }

        // Copy A plane if present (10-bit alpha)
        if format == VideoPixelFormat::I422AP10 {
          let (a_src_offset, a_src_stride) = get_src_layout(3, y_size + uv_size * 2, y_row_bytes);
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_src_offset + row * a_src_stride;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + y_row_bytes]
              .copy_from_slice(&data[src_start..src_start + y_row_bytes]);
          }
        }
      }
      // 10-bit and 12-bit 4:4:4 formats (2 bytes per sample)
      VideoPixelFormat::I444P10 | VideoPixelFormat::I444P12 | VideoPixelFormat::I444AP10 => {
        let bps = 2usize; // bytes per sample
        let plane_row_bytes = width as usize * bps;
        let plane_size = plane_row_bytes * height as usize;

        let (y_src_offset, y_src_stride) = get_src_layout(0, 0, plane_row_bytes);
        let (u_src_offset, u_src_stride) = get_src_layout(1, plane_size, plane_row_bytes);
        let (v_src_offset, v_src_stride) = get_src_layout(2, plane_size * 2, plane_row_bytes);

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = y_src_offset + row * y_src_stride;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + plane_row_bytes]
              .copy_from_slice(&data[src_start..src_start + plane_row_bytes]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..height as usize {
            let src_start = u_src_offset + row * u_src_stride;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + plane_row_bytes]
              .copy_from_slice(&data[src_start..src_start + plane_row_bytes]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..height as usize {
            let src_start = v_src_offset + row * v_src_stride;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + plane_row_bytes]
              .copy_from_slice(&data[src_start..src_start + plane_row_bytes]);
          }
        }

        // Copy A plane if present (10-bit alpha)
        if format == VideoPixelFormat::I444AP10 {
          let (a_src_offset, a_src_stride) = get_src_layout(3, plane_size * 3, plane_row_bytes);
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_src_offset + row * a_src_stride;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + plane_row_bytes]
              .copy_from_slice(&data[src_start..src_start + plane_row_bytes]);
          }
        }
      }
    }

    Ok(())
  }
}

/// Native canvas-provider interop, separate from the browser VideoFrame interface.
#[napi(
  js_name = "createVideoFrameFromRGBA16Float",
  ts_args_type = "data: Uint16Array, init: Omit<VideoFrameBufferInit, 'format' | 'layout' | 'transfer'> & {rotation?: number; flip?: boolean; alpha?: 'keep' | 'discard'}"
)]
pub fn create_video_frame_from_rgba16_float(
  env: Env,
  data: Unknown,
  init: VideoFrameConstructorInit,
) -> Result<VideoFrame> {
  // Read the typed array only after `init` is converted: its getters may detach the buffer.
  let data = unsafe { Uint16Array::from_napi_value(env.raw(), data.raw())? };
  VideoFrame::from_rgba16_float(data, init).map_err(|error| {
    match error.reason.strip_prefix("TypeError:") {
      Some(message) => throw_type_error(env.raw(), message.trim_start()),
      None => error,
    }
  })
}

/// Unwraps a VideoFrame argument after a real `instanceof` check, like the constructor does.
fn video_frame_argument<'env>(
  env: &Env,
  value: Unknown<'env>,
) -> Result<ClassInstance<'env, VideoFrame>> {
  if !VideoFrame::instance_of(env, &value).unwrap_or(false) {
    return Err(throw_type_error(env.raw(), "frame must be a VideoFrame"));
  }
  unsafe { ClassInstance::<VideoFrame>::from_napi_value(env.raw(), value.raw()) }
}

/// Native image-consumer interop: copies the half-float bits of an RGBA16Float frame into a new
/// `Uint16Array`. Returns the whole coded image; `visibleRect` is ignored. Throws a TypeError if
/// `frame` is not a VideoFrame, InvalidStateError if it is closed, and NotSupportedError if it does
/// not hold an RGBA16Float image.
#[napi(
  js_name = "copyVideoFrameRGBA16Float",
  ts_args_type = "frame: VideoFrame"
)]
pub fn copy_video_frame_rgba16_float(env: Env, frame: Unknown) -> Result<Uint16Array> {
  video_frame_argument(&env, frame)?.copy_rgba16_float(env)
}

impl Drop for VideoFrame {
  fn drop(&mut self) {
    // A frame wrapper can be collected while its external texture still owns
    // an image. Removing its callbacks must not perform an explicit close.
    let _ = CLOSE_OBSERVERS.try_with(|registry| {
      let observers = registry.borrow_mut().remove(&self.observer_id);
      if let Some(observers) = observers {
        observers.delete_refs();
      }
    });
  }
}

/// Native image consumers subscribe to explicit close without changing the browser interface.
#[napi(
  js_name = "registerVideoFrameCloseObserver",
  ts_args_type = "frame: VideoFrame, callback: () => void"
)]
pub fn register_video_frame_close_observer(
  env: Env,
  frame: Unknown,
  callback: FunctionRef<(), UnknownReturnValue>,
) -> Result<()> {
  let frame = video_frame_argument(&env, frame)?;
  if frame.closed()? {
    return throw_invalid_state_error(&env, "VideoFrame is closed");
  }
  let callback = callback.borrow_back(&env)?;
  // Reserve the id and find or make the frame's holder under a short borrow.
  // Linking a new holder can run JS when code that ran before this module
  // loaded replaced WeakMap methods, and that JS may close or register on
  // frames, so no borrow is held across it.
  let (id, anchor) = CLOSE_OBSERVERS.with(|registry| {
    let mut registry = registry.borrow_mut();
    let observers = registry
      .entry(frame.observer_id)
      .or_insert_with(|| FrameCloseObservers::new(env.raw(), 0));
    let id = observers.next_id;
    observers.next_id += 1;
    (id, observers.holder.prepare(&env))
  });
  let anchor = anchor?;
  if let Some(ref anchor) = anchor
    && let Err(error) = super::event_target::link_holder(&env, frame.value, anchor)
  {
    // An unlinked holder keeps nothing alive: forget it so the next
    // registration makes a new one.
    CLOSE_OBSERVERS.with(|registry| {
      if let Some(observers) = registry.borrow_mut().get_mut(&frame.observer_id) {
        observers.holder.clear(env.raw());
      }
    });
    return Err(error);
  }
  let holder = anchor.map(|anchor| anchor.holder);
  let reference = super::event_target::anchor_callback(&env, holder, id, callback.raw())?;
  // JS that ran while linking may have closed the frame.
  if frame.closed()? {
    super::event_target::release_callback(&env, holder, id, reference);
    return throw_invalid_state_error(&env, "VideoFrame is closed");
  }
  CLOSE_OBSERVERS.with(|registry| {
    registry
      .borrow_mut()
      .entry(frame.observer_id)
      .or_insert_with(|| FrameCloseObservers::new(env.raw(), id + 1))
      .callbacks
      .push((id, reference));
  });
  Ok(())
}
