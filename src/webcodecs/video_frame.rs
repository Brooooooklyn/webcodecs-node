//! VideoFrame - WebCodecs API implementation
//!
//! Represents a frame of video data that can be displayed or encoded.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/VideoFrame

use crate::codec::Frame;
use crate::ffi::AVPixelFormat;
use crate::webcodecs::error::invalid_state_error;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

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
  pub fn new(
    #[napi(ts_arg_type = "import('./standard').VideoColorSpaceInit")] init: Option<
      VideoColorSpaceInit,
    >,
  ) -> Self {
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

#[napi]
impl DOMRectReadOnly {
  /// Create a new DOMRectReadOnly
  #[napi(constructor)]
  pub fn new(x: Option<f64>, y: Option<f64>, width: Option<f64>, height: Option<f64>) -> Self {
    DOMRectReadOnly {
      x: x.unwrap_or(0.0),
      y: y.unwrap_or(0.0),
      width: width.unwrap_or(0.0),
      height: height.unwrap_or(0.0),
    }
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

  /// Top edge (same as y)
  #[napi(getter)]
  pub fn top(&self) -> f64 {
    self.y
  }

  /// Right edge (x + width)
  #[napi(getter)]
  pub fn right(&self) -> f64 {
    self.x + self.width
  }

  /// Bottom edge (y + height)
  #[napi(getter)]
  pub fn bottom(&self) -> f64 {
    self.y + self.height
  }

  /// Left edge (same as x)
  #[napi(getter)]
  pub fn left(&self) -> f64 {
    self.x
  }

  /// Convert to JSON (W3C spec uses toJSON)
  #[napi(js_name = "toJSON")]
  pub fn to_json(&self) -> DOMRectInit {
    DOMRectInit {
      x: Some(self.x),
      y: Some(self.y),
      width: Some(self.width),
      height: Some(self.height),
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

    // timestamp - required
    let timestamp: i64 = match obj.get("timestamp")? {
      Some(t) => t,
      None => return Err(throw_type_error(env, "timestamp is required")),
    };

    // Optional fields
    let duration: Option<i64> = obj.get("duration")?;
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

/// Options for copyTo operation
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameCopyToOptions {
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

/// Rectangle for specifying a region
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameRect {
  pub x: u32,
  pub y: u32,
  pub width: u32,
  pub height: u32,
}

/// Internal state for VideoFrame
struct VideoFrameInner {
  frame: Frame,
  /// Original pixel format (preserved since FFmpeg may convert RGBX→RGBA, etc.)
  original_format: VideoPixelFormat,
  timestamp_us: i64,
  duration_us: Option<i64>,
  display_width: u32,
  display_height: u32,
  /// Rotation in degrees clockwise (0, 90, 180, 270)
  rotation: f64,
  /// Horizontal flip
  flip: bool,
  color_space: VideoColorSpace,
  closed: bool,
}

/// VideoFrame - represents a frame of video
///
/// This is a WebCodecs-compliant VideoFrame implementation backed by FFmpeg.
#[napi]
pub struct VideoFrame {
  inner: Arc<Mutex<Option<VideoFrameInner>>>,
}

/// Parse rotation value per W3C spec algorithm
/// Rounds to nearest 90 degrees, normalizes to 0-359 range
fn parse_rotation(rotation: f64) -> f64 {
  // Round to nearest multiple of 90, ties towards positive infinity
  let aligned = (rotation / 90.0).round() * 90.0;
  // Normalize to 0-359 range
  let full_turns = (aligned / 360.0).floor() * 360.0;
  aligned - full_turns
}

#[napi]
impl VideoFrame {
  /// Create a new VideoFrame from raw buffer data (BufferSource per spec)
  ///
  /// This is the VideoFrameBufferInit constructor form.
  /// Use `fromVideoFrame()` to create from another VideoFrame.
  #[napi(constructor)]
  pub fn new(env: Env, data: Uint8Array, init: VideoFrameBufferInit) -> Result<Self> {
    let width = init.coded_width;
    let height = init.coded_height;

    // Validate zero dimensions
    if width == 0 {
      env.throw_type_error("codedWidth must be greater than 0", None)?;
      return Err(Error::new(
        Status::InvalidArg,
        "codedWidth must be greater than 0",
      ));
    }
    if height == 0 {
      env.throw_type_error("codedHeight must be greater than 0", None)?;
      return Err(Error::new(
        Status::InvalidArg,
        "codedHeight must be greater than 0",
      ));
    }

    // Validate buffer size before creating frame
    let expected_size = Self::calculate_buffer_size(init.format, width, height) as usize;
    if data.len() < expected_size {
      env.throw_type_error(
        &format!(
          "Buffer too small: need {} bytes, got {}",
          expected_size,
          data.len()
        ),
        None,
      )?;
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "Buffer too small: need {} bytes, got {}",
          expected_size,
          data.len()
        ),
      ));
    }

    let format = init.format.to_av_format();

    // Create internal frame
    let mut frame = Frame::new_video(width, height, format).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create frame: {}", e),
      )
    })?;

    // Copy data into the frame
    Self::copy_data_to_frame(&mut frame, &data, init.format, width, height)?;

    // Set timestamps (convert from microseconds to time_base units)
    // We use microseconds as time_base internally
    frame.set_pts(init.timestamp);
    if let Some(duration) = init.duration {
      frame.set_duration(duration);
    }

    // Parse rotation and flip per W3C spec
    let rotation = parse_rotation(init.rotation.unwrap_or(0.0));
    let flip = init.flip.unwrap_or(false);

    // Display dimensions default to visible/coded dimensions, swapped if rotation is 90/270
    let display_width = init.display_width.unwrap_or({
      if rotation == 90.0 || rotation == 270.0 {
        height
      } else {
        width
      }
    });
    let display_height = init.display_height.unwrap_or({
      if rotation == 90.0 || rotation == 270.0 {
        width
      } else {
        height
      }
    });

    let color_space = VideoColorSpace::new(init.color_space);

    let inner = VideoFrameInner {
      frame,
      original_format: init.format,
      timestamp_us: init.timestamp,
      duration_us: init.duration,
      display_width,
      display_height,
      rotation,
      flip,
      color_space,
      closed: false,
    };

    Ok(Self {
      inner: Arc::new(Mutex::new(Some(inner))),
    })
  }

  /// Create a new VideoFrame from another VideoFrame (image source constructor per spec)
  ///
  /// This clones the source VideoFrame and applies any overrides from init.
  /// Per W3C spec, this is equivalent to `new VideoFrame(videoFrame, init)`.
  #[napi(factory)]
  pub fn from_video_frame(source: &VideoFrame, init: Option<VideoFrameInit>) -> Result<Self> {
    source.with_inner(|source_inner| {
      // Clone the underlying frame data
      let cloned_frame = source_inner
        .frame
        .try_clone()
        .map_err(|e| Error::new(Status::GenericFailure, format!("Clone failed: {}", e)))?;

      let init = init.unwrap_or(VideoFrameInit {
        timestamp: None,
        duration: None,
        alpha: None,
        visible_rect: None,
        rotation: None,
        flip: None,
        display_width: None,
        display_height: None,
        metadata: None,
      });

      // Apply overrides from init
      let timestamp_us = init.timestamp.unwrap_or(source_inner.timestamp_us);
      let duration_us = init.duration.or(source_inner.duration_us);

      // Note: alpha handling and visible_rect cropping are not yet implemented
      // visible_rect would require sub-region copying which is complex
      if init.visible_rect.is_some() {
        return Err(Error::new(
          Status::GenericFailure,
          "VideoFrame visibleRect parameter is not yet implemented",
        ));
      }

      // Handle rotation per W3C spec "Add Rotations" algorithm
      let init_rotation = parse_rotation(init.rotation.unwrap_or(0.0));
      let base_rotation = source_inner.rotation;
      let base_flip = source_inner.flip;
      let init_flip = init.flip.unwrap_or(false);

      // Per spec: if baseFlip is false, combined = base + init; else combined = base - init
      let combined_rotation = if !base_flip {
        parse_rotation(base_rotation + init_rotation)
      } else {
        parse_rotation(base_rotation - init_rotation)
      };
      // Per spec: flip is XOR of base and init flip
      let combined_flip = base_flip != init_flip;

      let display_width = init.display_width.unwrap_or(source_inner.display_width);
      let display_height = init.display_height.unwrap_or(source_inner.display_height);

      let new_inner = VideoFrameInner {
        frame: cloned_frame,
        original_format: source_inner.original_format,
        timestamp_us,
        duration_us,
        display_width,
        display_height,
        rotation: combined_rotation,
        flip: combined_flip,
        color_space: source_inner.color_space.clone(),
        closed: false,
      };

      Ok(VideoFrame {
        inner: Arc::new(Mutex::new(Some(new_inner))),
      })
    })
  }

  /// Create a VideoFrame from an internal Frame (for decoder output)
  pub fn from_internal(frame: Frame, timestamp_us: i64, duration_us: Option<i64>) -> Self {
    let width = frame.width();
    let height = frame.height();
    let original_format =
      VideoPixelFormat::from_av_format(frame.format()).unwrap_or(VideoPixelFormat::I420);

    let inner = VideoFrameInner {
      frame,
      original_format,
      timestamp_us,
      duration_us,
      display_width: width,
      display_height: height,
      rotation: 0.0,
      flip: false,
      color_space: VideoColorSpace::default(),
      closed: false,
    };

    Self {
      inner: Arc::new(Mutex::new(Some(inner))),
    }
  }

  /// Create a VideoFrame from an internal Frame with rotation/flip (for decoder output)
  pub fn from_internal_with_orientation(
    frame: Frame,
    timestamp_us: i64,
    duration_us: Option<i64>,
    rotation: f64,
    flip: bool,
  ) -> Self {
    let width = frame.width();
    let height = frame.height();
    let parsed_rotation = parse_rotation(rotation);
    let original_format =
      VideoPixelFormat::from_av_format(frame.format()).unwrap_or(VideoPixelFormat::I420);

    // Display dimensions may be swapped based on rotation
    let (display_width, display_height) = if parsed_rotation == 90.0 || parsed_rotation == 270.0 {
      (height, width)
    } else {
      (width, height)
    };

    let inner = VideoFrameInner {
      frame,
      original_format,
      timestamp_us,
      duration_us,
      display_width,
      display_height,
      rotation: parsed_rotation,
      flip,
      color_space: VideoColorSpace::default(),
      closed: false,
    };

    Self {
      inner: Arc::new(Mutex::new(Some(inner))),
    }
  }

  /// Get the pixel format
  #[napi(getter)]
  pub fn format(&self) -> Result<Option<VideoPixelFormat>> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(Some(inner.original_format)),
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
      Some(inner) if !inner.closed => Ok(inner.frame.width()),
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
      Some(inner) if !inner.closed => Ok(inner.frame.height()),
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
      Some(inner) if !inner.closed => Ok(inner.display_width),
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
      Some(inner) if !inner.closed => Ok(inner.display_height),
      _ => Ok(0),
    }
  }

  /// Get the coded rect (the region containing valid pixel data)
  /// Returns DOMRectReadOnly per W3C WebCodecs spec
  /// Throws InvalidStateError if the VideoFrame is closed
  #[napi(getter)]
  pub fn coded_rect(&self) -> Result<DOMRectReadOnly> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(DOMRectReadOnly {
        x: 0.0,
        y: 0.0,
        width: inner.frame.width() as f64,
        height: inner.frame.height() as f64,
      }),
      _ => Err(invalid_state_error("VideoFrame is closed")),
    }
  }

  /// Get the visible rect (the region of coded data that should be displayed)
  /// Returns DOMRectReadOnly per W3C WebCodecs spec
  /// Throws InvalidStateError if the VideoFrame is closed
  #[napi(getter)]
  pub fn visible_rect(&self) -> Result<DOMRectReadOnly> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(DOMRectReadOnly {
        x: 0.0,
        y: 0.0,
        width: inner.display_width as f64,
        height: inner.display_height as f64,
      }),
      _ => Err(invalid_state_error("VideoFrame is closed")),
    }
  }

  /// Get the presentation timestamp in microseconds (returns 0 when closed per W3C spec)
  #[napi(getter)]
  pub fn timestamp(&self) -> Result<i64> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(inner.timestamp_us),
      _ => Ok(0),
    }
  }

  /// Get the duration in microseconds (returns null when closed per W3C spec)
  #[napi(getter)]
  pub fn duration(&self) -> Result<Option<i64>> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(inner.duration_us),
      _ => Ok(None),
    }
  }

  /// Get the color space parameters
  #[napi(getter)]
  pub fn color_space(&self) -> Result<VideoColorSpace> {
    self.with_inner(|inner| Ok(inner.color_space.clone()))
  }

  /// Get whether this VideoFrame has been closed (W3C WebCodecs spec)
  #[napi(getter)]
  pub fn closed(&self) -> Result<bool> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    Ok(guard.is_none() || guard.as_ref().is_none_or(|i| i.closed))
  }

  /// Get the number of planes in this VideoFrame (W3C WebCodecs spec)
  /// The number depends on the pixel format:
  /// - RGBA, RGBX, BGRA, BGRX: 1 plane
  /// - NV12, NV21: 2 planes
  /// - I420, I422, I444: 3 planes
  /// - I420A, I422A, I444A: 4 planes
  #[napi(getter)]
  pub fn number_of_planes(&self) -> Result<u32> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(Self::get_number_of_planes(inner.original_format)),
      _ => Err(invalid_state_error("VideoFrame is closed")),
    }
  }

  /// Get the rotation in degrees clockwise (0, 90, 180, 270) - W3C WebCodecs spec
  #[napi(getter)]
  pub fn rotation(&self) -> Result<f64> {
    self.with_inner(|inner| Ok(inner.rotation))
  }

  /// Get whether horizontal flip is applied - W3C WebCodecs spec
  #[napi(getter)]
  pub fn flip(&self) -> Result<bool> {
    self.with_inner(|inner| Ok(inner.flip))
  }

  /// Get the metadata associated with this VideoFrame - W3C WebCodecs spec
  /// Currently returns an empty metadata object as members are defined in the registry
  #[napi]
  pub fn metadata(&self) -> Result<VideoFrameMetadata> {
    self.with_inner(|_inner| Ok(VideoFrameMetadata {}))
  }

  /// Calculate the allocation size needed for copyTo
  #[napi]
  pub fn allocation_size(&self, options: Option<VideoFrameCopyToOptions>) -> Result<u32> {
    self.with_inner(|inner| {
      let format = options.as_ref().and_then(|o| o.format).unwrap_or_else(|| {
        VideoPixelFormat::from_av_format(inner.frame.format()).unwrap_or(VideoPixelFormat::I420)
      });

      let width = inner.frame.width();
      let height = inner.frame.height();

      Ok(Self::calculate_buffer_size(format, width, height))
    })
  }

  /// Copy frame data to a Uint8Array
  ///
  /// Returns a Promise that resolves with an array of PlaneLayout objects.
  /// Options can specify target format. The rect parameter is not yet implemented.
  #[napi]
  pub async fn copy_to(
    &self,
    mut destination: Uint8Array,
    options: Option<VideoFrameCopyToOptions>,
  ) -> Result<Vec<PlaneLayout>> {
    // Throw error if rect is specified since it's not implemented
    if options.as_ref().and_then(|o| o.rect.as_ref()).is_some() {
      return Err(Error::new(
        Status::GenericFailure,
        "VideoFrame.copyTo rect parameter is not yet implemented",
      ));
    }

    // Get format, size info and validate destination buffer (brief lock)
    let (format, width, height, size) = {
      let guard = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      let inner = match guard.as_ref() {
        Some(inner) if !inner.closed => inner,
        _ => return Err(invalid_state_error("VideoFrame is closed")),
      };

      let format =
        VideoPixelFormat::from_av_format(inner.frame.format()).unwrap_or(VideoPixelFormat::I420);
      let width = inner.frame.width();
      let height = inner.frame.height();
      let size = Self::calculate_buffer_size(format, width, height) as usize;

      (format, width, height, size)
    };

    if destination.len() < size {
      return Err(Error::new(
        Status::InvalidArg,
        format!(
          "TypeError: destination buffer too small: need {} bytes, got {}",
          size,
          destination.len()
        ),
      ));
    }

    // Clone inner Arc for the blocking thread
    let inner_clone = self.inner.clone();

    // Perform the copy in a blocking thread to not block the event loop
    let copied_data = spawn_blocking(move || -> Result<Vec<u8>> {
      let guard = inner_clone
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      let inner = match guard.as_ref() {
        Some(inner) if !inner.closed => inner,
        _ => return Err(invalid_state_error("VideoFrame is closed")),
      };

      // Allocate temporary buffer and copy frame data
      let mut temp_buffer = vec![0u8; size];
      inner
        .frame
        .copy_to_buffer(&mut temp_buffer)
        .map_err(|e| Error::new(Status::GenericFailure, format!("Copy failed: {}", e)))?;

      Ok(temp_buffer)
    })
    .await
    .map_err(|e| Error::new(Status::GenericFailure, format!("Copy task failed: {}", e)))??;

    // Copy from temp buffer to destination (this is fast since destination is already allocated)
    let dest_buffer = unsafe { destination.as_mut() };
    dest_buffer[..size].copy_from_slice(&copied_data);

    // Calculate and return plane layouts
    let layouts = Self::get_plane_layouts(format, width, height);
    Ok(layouts)
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
  #[napi(js_name = "clone")]
  pub fn clone_frame(&self) -> Result<VideoFrame> {
    self.with_inner(|inner| {
      let cloned_frame = inner
        .frame
        .try_clone()
        .map_err(|e| Error::new(Status::GenericFailure, format!("Clone failed: {}", e)))?;

      let new_inner = VideoFrameInner {
        frame: cloned_frame,
        original_format: inner.original_format,
        timestamp_us: inner.timestamp_us,
        duration_us: inner.duration_us,
        display_width: inner.display_width,
        display_height: inner.display_height,
        rotation: inner.rotation,
        flip: inner.flip,
        color_space: inner.color_space.clone(),
        closed: false,
      };

      Ok(VideoFrame {
        inner: Arc::new(Mutex::new(Some(new_inner))),
      })
    })
  }

  /// Close and release resources
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if let Some(inner) = guard.as_mut() {
      inner.closed = true;
    }
    *guard = None;

    Ok(())
  }

  // ========================================================================
  // Internal helpers (crate-visible only)
  // ========================================================================

  /// Borrow internal frame for encoding (crate internal use)
  #[allow(dead_code)]
  pub(crate) fn with_frame<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&Frame) -> R,
  {
    self.with_inner(|inner| Ok(f(&inner.frame)))
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
      Some(inner) if !inner.closed => f(inner),
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
  ) -> Result<()> {
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

    // Get all linesizes first to avoid borrow conflicts
    let linesize0 = frame.linesize(0) as usize;
    let linesize1 = frame.linesize(1) as usize;
    let linesize2 = frame.linesize(2) as usize;
    let linesize3 = frame.linesize(3) as usize;

    match format {
      VideoPixelFormat::I420 | VideoPixelFormat::I420A => {
        let y_size = (width * height) as usize;
        let u_width = (width / 2) as usize;
        let u_height = (height / 2) as usize;
        let v_offset = y_size + u_width * u_height;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * width as usize;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..u_height {
            let src_start = y_size + row * u_width;
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
            let src_start = v_offset + row * u_width;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + u_width]
              .copy_from_slice(&data[src_start..src_start + u_width]);
          }
        }

        // Copy A plane if present
        if format == VideoPixelFormat::I420A {
          let a_offset = v_offset + u_width * u_height;
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_offset + row * width as usize;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }
      }
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => {
        // NV12: Y plane + interleaved UV
        // NV21: Y plane + interleaved VU (same layout, just U/V swapped)
        let y_size = (width * height) as usize;
        let uv_height = (height / 2) as usize;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * width as usize;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy UV/VU plane (interleaved)
        {
          let uv_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get UV/VU plane"))?;
          for row in 0..uv_height {
            let src_start = y_size + row * width as usize;
            let dst_start = row * linesize1;
            uv_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }
      }
      VideoPixelFormat::I422 | VideoPixelFormat::I422A => {
        // I422: 4:2:2 - Y full resolution, U/V half width, full height
        let y_size = (width * height) as usize;
        let uv_width = (width / 2) as usize;
        let uv_size = uv_width * height as usize;
        let v_offset = y_size + uv_size;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * width as usize;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..height as usize {
            let src_start = y_size + row * uv_width;
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
            let src_start = v_offset + row * uv_width;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + uv_width]
              .copy_from_slice(&data[src_start..src_start + uv_width]);
          }
        }

        // Copy A plane if present
        if format == VideoPixelFormat::I422A {
          let a_offset = v_offset + uv_size;
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_offset + row * width as usize;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }
      }
      VideoPixelFormat::I444 | VideoPixelFormat::I444A => {
        // I444: 4:4:4 - Y, U, V all full resolution
        let plane_size = (width * height) as usize;
        let u_offset = plane_size;
        let v_offset = plane_size * 2;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * width as usize;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..height as usize {
            let src_start = u_offset + row * width as usize;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..height as usize {
            let src_start = v_offset + row * width as usize;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy A plane if present
        if format == VideoPixelFormat::I444A {
          let a_offset = plane_size * 3;
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_offset + row * width as usize;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }
      }
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => {
        let row_bytes = (width * 4) as usize;

        // Copy packed RGBA data
        let plane = frame
          .plane_data_mut(0)
          .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get plane"))?;
        for row in 0..height as usize {
          let src_start = row * row_bytes;
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
        let v_offset = y_size + uv_size;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * y_row_bytes;
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
            let src_start = y_size + row * uv_row_bytes;
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
            let src_start = v_offset + row * uv_row_bytes;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + uv_row_bytes]
              .copy_from_slice(&data[src_start..src_start + uv_row_bytes]);
          }
        }

        // Copy A plane if present (10-bit alpha)
        if format == VideoPixelFormat::I420AP10 {
          let a_offset = v_offset + uv_size;
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_offset + row * y_row_bytes;
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
        let v_offset = y_size + uv_size;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * y_row_bytes;
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
            let src_start = y_size + row * uv_row_bytes;
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
            let src_start = v_offset + row * uv_row_bytes;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + uv_row_bytes]
              .copy_from_slice(&data[src_start..src_start + uv_row_bytes]);
          }
        }

        // Copy A plane if present (10-bit alpha)
        if format == VideoPixelFormat::I422AP10 {
          let a_offset = v_offset + uv_size;
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_offset + row * y_row_bytes;
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
        let u_offset = plane_size;
        let v_offset = plane_size * 2;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * plane_row_bytes;
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
            let src_start = u_offset + row * plane_row_bytes;
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
            let src_start = v_offset + row * plane_row_bytes;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + plane_row_bytes]
              .copy_from_slice(&data[src_start..src_start + plane_row_bytes]);
          }
        }

        // Copy A plane if present (10-bit alpha)
        if format == VideoPixelFormat::I444AP10 {
          let a_offset = plane_size * 3;
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_offset + row * plane_row_bytes;
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
