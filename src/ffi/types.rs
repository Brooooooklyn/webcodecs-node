//! Core FFmpeg type definitions
//!
//! All FFmpeg structs are opaque (zero-sized) to avoid version-specific layout dependencies.
//! Field access is done via the thin C accessor library in accessors.c

use std::marker::PhantomData;
use std::os::raw::c_int;

// ============================================================================
// Rational Number
// ============================================================================

/// Rational number for time bases and frame rates
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AVRational {
  /// Numerator
  pub num: c_int,
  /// Denominator
  pub den: c_int,
}

impl AVRational {
  pub const fn new(num: c_int, den: c_int) -> Self {
    Self { num, den }
  }

  pub fn as_f64(&self) -> f64 {
    if self.den == 0 {
      0.0
    } else {
      self.num as f64 / self.den as f64
    }
  }

  /// Microsecond time base (1/1000000)
  pub const MICROSECONDS: Self = Self {
    num: 1,
    den: 1_000_000,
  };
}

// ============================================================================
// Codec IDs
// ============================================================================

/// Supported codec IDs (video and audio)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AVCodecID {
  None = 0,
  // Video codecs
  Mjpeg = 7, // Motion JPEG
  H264 = 27,
  Png = 61, // PNG image
  Bmp = 78, // BMP image
  Gif = 97, // GIF image
  Vp8 = 139,
  Vp9 = 167,
  Webp = 171, // WebP image
  Hevc = 173, // H.265
  Av1 = 225,
  // Audio codecs (starting at 0x10000 = 65536)
  PcmS16le = 65536, // PCM signed 16-bit little-endian
  PcmS16be = 65537, // PCM signed 16-bit big-endian
  PcmU16le = 65538, // PCM unsigned 16-bit little-endian
  PcmU16be = 65539, // PCM unsigned 16-bit big-endian
  PcmS8 = 65540,    // PCM signed 8-bit
  PcmU8 = 65541,    // PCM unsigned 8-bit
  PcmF32le = 65557, // PCM 32-bit float little-endian
  PcmF32be = 65558, // PCM 32-bit float big-endian
  PcmF64le = 65559, // PCM 64-bit double little-endian
  PcmF64be = 65560, // PCM 64-bit double big-endian
  PcmS32le = 65544, // PCM signed 32-bit little-endian
  PcmS32be = 65545, // PCM signed 32-bit big-endian
  PcmS24le = 65566, // PCM signed 24-bit little-endian
  PcmS24be = 65567, // PCM signed 24-bit big-endian
  Mp2 = 86016,      // MPEG Audio Layer 2
  Mp3 = 86017,      // MPEG Audio Layer 3
  Aac = 86018,      // Advanced Audio Coding
  Ac3 = 86019,      // Dolby AC-3
  Vorbis = 86021,   // Vorbis
  Flac = 86028,     // Free Lossless Audio Codec
  Opus = 86076,     // Opus
  Alac = 86032,     // Apple Lossless
}

impl AVCodecID {
  /// Convert WebCodecs codec string to AVCodecID
  pub fn from_webcodecs_codec(codec: &str) -> Option<Self> {
    let codec_lower = codec.to_lowercase();

    // Video codecs
    // H.264/AVC: avc1.PPCCLL or avc3.PPCCLL
    if codec_lower.starts_with("avc1") || codec_lower.starts_with("avc3") {
      return Some(Self::H264);
    }
    // H.265/HEVC: hev1.P.T.Lxxx or hvc1.P.T.Lxxx
    if codec_lower.starts_with("hev1") || codec_lower.starts_with("hvc1") {
      return Some(Self::Hevc);
    }
    // VP8
    if codec_lower == "vp8" {
      return Some(Self::Vp8);
    }
    // VP9: vp09.PP.LL.DD or just "vp9"
    if codec_lower.starts_with("vp09") || codec_lower == "vp9" {
      return Some(Self::Vp9);
    }
    // AV1: av01.P.LLT.DD or just "av1"
    if codec_lower.starts_with("av01") || codec_lower == "av1" {
      return Some(Self::Av1);
    }

    // Audio codecs
    // AAC: mp4a.40.2 (AAC-LC), mp4a.40.5 (HE-AAC), mp4a.40.29 (HE-AACv2)
    if codec_lower.starts_with("mp4a.40") || codec_lower == "aac" {
      return Some(Self::Aac);
    }
    // Opus
    if codec_lower == "opus" {
      return Some(Self::Opus);
    }
    // MP3: mp4a.6b or "mp3"
    if codec_lower == "mp3" || codec_lower == "mp4a.6b" {
      return Some(Self::Mp3);
    }
    // FLAC
    if codec_lower == "flac" {
      return Some(Self::Flac);
    }
    // Vorbis
    if codec_lower == "vorbis" {
      return Some(Self::Vorbis);
    }
    // AC-3
    if codec_lower == "ac-3" || codec_lower == "ac3" {
      return Some(Self::Ac3);
    }
    // ALAC (Apple Lossless)
    if codec_lower == "alac" {
      return Some(Self::Alac);
    }
    // PCM formats
    if codec_lower == "pcm-s16" || codec_lower == "pcm" {
      return Some(Self::PcmS16le);
    }
    if codec_lower == "pcm-f32" {
      return Some(Self::PcmF32le);
    }

    None
  }

  /// Get the default WebCodecs codec string for this codec
  pub fn to_webcodecs_codec(&self) -> &'static str {
    match self {
      Self::None => "",
      // Video
      Self::H264 => "avc1.42001f",     // Baseline profile, level 3.1
      Self::Hevc => "hev1.1.6.L93.B0", // Main profile
      Self::Vp8 => "vp8",
      Self::Vp9 => "vp09.00.10.08", // Profile 0, level 1.0, 8-bit
      Self::Av1 => "av01.0.01M.08", // Main profile, level 2.1, 8-bit
      // Image (not standard WebCodecs but useful for ImageDecoder)
      Self::Mjpeg => "mjpeg",
      Self::Png => "png",
      Self::Gif => "gif",
      Self::Bmp => "bmp",
      Self::Webp => "webp",
      // Audio
      Self::Aac => "mp4a.40.2", // AAC-LC
      Self::Opus => "opus",
      Self::Mp3 => "mp3",
      Self::Flac => "flac",
      Self::Vorbis => "vorbis",
      Self::Ac3 => "ac-3",
      Self::Alac => "alac",
      Self::Mp2 => "mp4a.69", // MPEG Layer 2
      Self::PcmS16le | Self::PcmS16be => "pcm-s16",
      Self::PcmU16le | Self::PcmU16be => "pcm-u16",
      Self::PcmS8 => "pcm-s8",
      Self::PcmU8 => "pcm-u8",
      Self::PcmF32le | Self::PcmF32be => "pcm-f32",
      Self::PcmF64le | Self::PcmF64be => "pcm-f64",
      Self::PcmS32le | Self::PcmS32be => "pcm-s32",
      Self::PcmS24le | Self::PcmS24be => "pcm-s24",
    }
  }

  /// Check if this is an audio codec
  pub fn is_audio(&self) -> bool {
    (*self as c_int) >= 65536
  }

  /// Check if this is a video codec
  pub fn is_video(&self) -> bool {
    let raw = *self as c_int;
    raw > 0 && raw < 65536
  }

  /// Get the raw FFmpeg codec ID value
  pub fn as_raw(&self) -> c_int {
    *self as c_int
  }

  /// Create from raw FFmpeg codec ID value
  ///
  /// Returns the codec ID if it matches a known value, otherwise None.
  pub fn from_raw(raw: c_int) -> Self {
    match raw {
      0 => Self::None,
      7 => Self::Mjpeg,
      27 => Self::H264,
      61 => Self::Png,
      78 => Self::Bmp,
      97 => Self::Gif,
      139 => Self::Vp8,
      167 => Self::Vp9,
      171 => Self::Webp,
      173 => Self::Hevc,
      225 => Self::Av1,
      65536 => Self::PcmS16le,
      65537 => Self::PcmS16be,
      65538 => Self::PcmU16le,
      65539 => Self::PcmU16be,
      65540 => Self::PcmS8,
      65541 => Self::PcmU8,
      65544 => Self::PcmS32le,
      65545 => Self::PcmS32be,
      65557 => Self::PcmF32le,
      65558 => Self::PcmF32be,
      65559 => Self::PcmF64le,
      65560 => Self::PcmF64be,
      65566 => Self::PcmS24le,
      65567 => Self::PcmS24be,
      86016 => Self::Mp2,
      86017 => Self::Mp3,
      86018 => Self::Aac,
      86019 => Self::Ac3,
      86021 => Self::Vorbis,
      86028 => Self::Flac,
      86032 => Self::Alac,
      86076 => Self::Opus,
      _ => Self::None,
    }
  }
}

// ============================================================================
// Pixel Formats
// ============================================================================

/// Video pixel formats (subset supported by WebCodecs)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AVPixelFormat {
  None = -1,
  // Planar YUV formats (8-bit)
  Yuv420p = 0,   // I420
  Yuv422p = 4,   // I422
  Yuv444p = 5,   // I444
  Yuva420p = 33, // I420A (with alpha)
  Yuva422p = 78, // I422A (with alpha)
  Yuva444p = 79, // I444A (with alpha)
  // Semi-planar formats
  Nv12 = 23,
  Nv21 = 24,
  // RGB formats
  Rgb24 = 2,
  Bgr24 = 3,
  Argb = 25,
  Rgba = 26,
  Abgr = 27,
  Bgra = 28,
  // 10-bit YUV formats
  Yuv420p10le = 62, // I420P10
  Yuv422p10le = 64, // I422P10
  Yuv444p10le = 68, // I444P10
  // 12-bit YUV formats
  Yuv420p12le = 123, // I420P12
  Yuv422p12le = 127, // I422P12
  Yuv444p12le = 131, // I444P12
  // 10-bit YUV with alpha formats
  Yuva420p10le = 87, // I420AP10
  Yuva422p10le = 89, // I422AP10
  Yuva444p10le = 91, // I444AP10
  // Hardware formats
  Videotoolbox = 162,
  Cuda = 119,
  Vaapi = 53,
  Qsv = 173,
  D3d11 = 174,
  Dxva2Vld = 55,
  Vulkan = 185,
}

impl AVPixelFormat {
  /// Convert from WebCodecs VideoPixelFormat string
  pub fn from_webcodecs_format(format: &str) -> Option<Self> {
    match format {
      // 8-bit formats
      "I420" => Some(Self::Yuv420p),
      "I420A" => Some(Self::Yuva420p),
      "I422" => Some(Self::Yuv422p),
      "I422A" => Some(Self::Yuva422p),
      "I444" => Some(Self::Yuv444p),
      "I444A" => Some(Self::Yuva444p),
      "NV12" => Some(Self::Nv12),
      "NV21" => Some(Self::Nv21),
      "RGBA" | "RGBX" => Some(Self::Rgba),
      "BGRA" | "BGRX" => Some(Self::Bgra),
      // 10-bit formats
      "I420P10" => Some(Self::Yuv420p10le),
      "I422P10" => Some(Self::Yuv422p10le),
      "I444P10" => Some(Self::Yuv444p10le),
      "I420AP10" => Some(Self::Yuva420p10le),
      "I422AP10" => Some(Self::Yuva422p10le),
      "I444AP10" => Some(Self::Yuva444p10le),
      // 12-bit formats
      "I420P12" => Some(Self::Yuv420p12le),
      "I422P12" => Some(Self::Yuv422p12le),
      "I444P12" => Some(Self::Yuv444p12le),
      _ => None,
    }
  }

  /// Convert to WebCodecs VideoPixelFormat string
  pub fn to_webcodecs_format(&self) -> Option<&'static str> {
    match self {
      // 8-bit formats
      Self::Yuv420p => Some("I420"),
      Self::Yuva420p => Some("I420A"),
      Self::Yuv422p => Some("I422"),
      Self::Yuva422p => Some("I422A"),
      Self::Yuv444p => Some("I444"),
      Self::Yuva444p => Some("I444A"),
      Self::Nv12 => Some("NV12"),
      Self::Nv21 => Some("NV21"),
      Self::Rgba => Some("RGBA"),
      Self::Bgra => Some("BGRA"),
      // 10-bit formats
      Self::Yuv420p10le => Some("I420P10"),
      Self::Yuv422p10le => Some("I422P10"),
      Self::Yuv444p10le => Some("I444P10"),
      Self::Yuva420p10le => Some("I420AP10"),
      Self::Yuva422p10le => Some("I422AP10"),
      Self::Yuva444p10le => Some("I444AP10"),
      // 12-bit formats
      Self::Yuv420p12le => Some("I420P12"),
      Self::Yuv422p12le => Some("I422P12"),
      Self::Yuv444p12le => Some("I444P12"),
      _ => None,
    }
  }

  /// Get the raw FFmpeg pixel format value
  pub fn as_raw(&self) -> c_int {
    *self as c_int
  }

  /// Number of planes for this pixel format
  pub fn num_planes(&self) -> usize {
    match self {
      // 3-plane formats (Y, U, V)
      Self::Yuv420p | Self::Yuv422p | Self::Yuv444p => 3,
      Self::Yuv420p10le | Self::Yuv422p10le | Self::Yuv444p10le => 3,
      Self::Yuv420p12le | Self::Yuv422p12le | Self::Yuv444p12le => 3,
      // 4-plane formats (Y, U, V, A)
      Self::Yuva420p | Self::Yuva422p | Self::Yuva444p => 4,
      Self::Yuva420p10le | Self::Yuva422p10le | Self::Yuva444p10le => 4,
      // 2-plane formats (Y, UV interleaved)
      Self::Nv12 | Self::Nv21 => 2,
      // 1-plane packed formats
      Self::Rgb24 | Self::Bgr24 | Self::Rgba | Self::Bgra | Self::Argb | Self::Abgr => 1,
      _ => 0,
    }
  }

  /// Whether this is a hardware pixel format
  pub fn is_hardware(&self) -> bool {
    matches!(
      self,
      Self::Videotoolbox
        | Self::Cuda
        | Self::Vaapi
        | Self::Qsv
        | Self::D3d11
        | Self::Dxva2Vld
        | Self::Vulkan
    )
  }

  /// Convert from raw FFmpeg pixel format value
  pub fn from_raw(value: c_int) -> Self {
    match value {
      0 => Self::Yuv420p,
      4 => Self::Yuv422p,
      5 => Self::Yuv444p,
      33 => Self::Yuva420p,
      78 => Self::Yuva422p,
      79 => Self::Yuva444p,
      23 => Self::Nv12,
      24 => Self::Nv21,
      2 => Self::Rgb24,
      3 => Self::Bgr24,
      25 => Self::Argb,
      26 => Self::Rgba,
      27 => Self::Abgr,
      28 => Self::Bgra,
      62 => Self::Yuv420p10le,
      64 => Self::Yuv422p10le,
      68 => Self::Yuv444p10le,
      123 => Self::Yuv420p12le,
      127 => Self::Yuv422p12le,
      131 => Self::Yuv444p12le,
      87 => Self::Yuva420p10le,
      89 => Self::Yuva422p10le,
      91 => Self::Yuva444p10le,
      162 => Self::Videotoolbox,
      119 => Self::Cuda,
      53 => Self::Vaapi,
      173 => Self::Qsv,
      174 => Self::D3d11,
      55 => Self::Dxva2Vld,
      185 => Self::Vulkan,
      _ => Self::None,
    }
  }
}

// ============================================================================
// Audio Sample Formats
// ============================================================================

/// Audio sample formats (matches FFmpeg AVSampleFormat)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AVSampleFormat {
  None = -1,
  /// unsigned 8-bit (interleaved)
  U8 = 0,
  /// signed 16-bit (interleaved)
  S16 = 1,
  /// signed 32-bit (interleaved)
  S32 = 2,
  /// float (interleaved)
  Flt = 3,
  /// double (interleaved)
  Dbl = 4,
  /// unsigned 8-bit (planar)
  U8p = 5,
  /// signed 16-bit (planar)
  S16p = 6,
  /// signed 32-bit (planar)
  S32p = 7,
  /// float (planar) - most common for encoders
  Fltp = 8,
  /// double (planar)
  Dblp = 9,
  /// signed 64-bit (interleaved)
  S64 = 10,
  /// signed 64-bit (planar)
  S64p = 11,
}

impl AVSampleFormat {
  /// Get the raw FFmpeg sample format value
  pub fn as_raw(&self) -> c_int {
    *self as c_int
  }

  /// Check if this format is planar (separate buffers per channel)
  pub fn is_planar(&self) -> bool {
    matches!(
      self,
      Self::U8p | Self::S16p | Self::S32p | Self::Fltp | Self::Dblp | Self::S64p
    )
  }

  /// Get the interleaved version of this format (if planar)
  pub fn to_interleaved(&self) -> Self {
    match self {
      Self::U8p => Self::U8,
      Self::S16p => Self::S16,
      Self::S32p => Self::S32,
      Self::Fltp => Self::Flt,
      Self::Dblp => Self::Dbl,
      Self::S64p => Self::S64,
      other => *other,
    }
  }

  /// Get the planar version of this format (if interleaved)
  pub fn to_planar(&self) -> Self {
    match self {
      Self::U8 => Self::U8p,
      Self::S16 => Self::S16p,
      Self::S32 => Self::S32p,
      Self::Flt => Self::Fltp,
      Self::Dbl => Self::Dblp,
      Self::S64 => Self::S64p,
      other => *other,
    }
  }

  /// Get bytes per sample for this format
  pub fn bytes_per_sample(&self) -> usize {
    match self {
      Self::None => 0,
      Self::U8 | Self::U8p => 1,
      Self::S16 | Self::S16p => 2,
      Self::S32 | Self::S32p | Self::Flt | Self::Fltp => 4,
      Self::Dbl | Self::Dblp | Self::S64 | Self::S64p => 8,
    }
  }

  /// Convert from WebCodecs AudioSampleFormat string
  pub fn from_webcodecs_format(format: &str) -> Option<Self> {
    match format {
      "u8" => Some(Self::U8),
      "s16" => Some(Self::S16),
      "s32" => Some(Self::S32),
      "f32" => Some(Self::Flt),
      "u8-planar" => Some(Self::U8p),
      "s16-planar" => Some(Self::S16p),
      "s32-planar" => Some(Self::S32p),
      "f32-planar" => Some(Self::Fltp),
      _ => None,
    }
  }

  /// Convert to WebCodecs AudioSampleFormat string
  pub fn to_webcodecs_format(&self) -> Option<&'static str> {
    match self {
      Self::U8 => Some("u8"),
      Self::S16 => Some("s16"),
      Self::S32 => Some("s32"),
      Self::Flt => Some("f32"),
      Self::U8p => Some("u8-planar"),
      Self::S16p => Some("s16-planar"),
      Self::S32p => Some("s32-planar"),
      Self::Fltp => Some("f32-planar"),
      _ => None,
    }
  }

  /// Create from raw FFmpeg sample format value
  pub fn from_raw(value: c_int) -> Self {
    match value {
      0 => Self::U8,
      1 => Self::S16,
      2 => Self::S32,
      3 => Self::Flt,
      4 => Self::Dbl,
      5 => Self::U8p,
      6 => Self::S16p,
      7 => Self::S32p,
      8 => Self::Fltp,
      9 => Self::Dblp,
      10 => Self::S64,
      11 => Self::S64p,
      _ => Self::None,
    }
  }
}

// ============================================================================
// Hardware Device Types
// ============================================================================

/// Hardware acceleration device types
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AVHWDeviceType {
  None = 0,
  Vdpau = 1,
  Cuda = 2,
  Vaapi = 3,
  Dxva2 = 4,
  Qsv = 5,
  Videotoolbox = 6,
  D3d11va = 7,
  Drm = 8,
  Opencl = 9,
  Mediacodec = 10,
  Vulkan = 11,
}

impl AVHWDeviceType {
  /// Get the raw FFmpeg hardware device type value
  pub fn as_raw(&self) -> c_int {
    *self as c_int
  }

  /// Get the hardware pixel format for this device type
  pub fn pixel_format(&self) -> AVPixelFormat {
    match self {
      Self::Videotoolbox => AVPixelFormat::Videotoolbox,
      Self::Cuda => AVPixelFormat::Cuda,
      Self::Vaapi => AVPixelFormat::Vaapi,
      _ => AVPixelFormat::None,
    }
  }
}

// ============================================================================
// Color Space
// ============================================================================

/// Color space (matrix coefficients)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AVColorSpace {
  Rgb = 0,
  Bt709 = 1,
  #[default]
  Unspecified = 2,
  Fcc = 4,
  Bt470bg = 5,
  Smpte170m = 6,
  Smpte240m = 7,
  Ycgco = 8,
  Bt2020Ncl = 9,
  Bt2020Cl = 10,
}

/// Color primaries
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AVColorPrimaries {
  Bt709 = 1,
  #[default]
  Unspecified = 2,
  Bt470m = 4,
  Bt470bg = 5,
  Smpte170m = 6,
  Smpte240m = 7,
  Film = 8,
  Bt2020 = 9,
  Smpte428 = 10,
  Smpte431 = 11,
  Smpte432 = 12,
  JedecP22 = 22,
}

/// Color transfer characteristics
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AVColorTransferCharacteristic {
  Bt709 = 1,
  #[default]
  Unspecified = 2,
  Gamma22 = 4,
  Gamma28 = 5,
  Smpte170m = 6,
  Smpte240m = 7,
  Linear = 8,
  Log = 9,
  LogSqrt = 10,
  Iec61966_2_4 = 11,
  Bt1361Ecg = 12,
  Iec61966_2_1 = 13, // sRGB
  Bt2020_10 = 14,
  Bt2020_12 = 15,
  Smpte2084 = 16, // PQ/HDR10
  Smpte428 = 17,
  AribStdB67 = 18, // HLG
}

/// Color range
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AVColorRange {
  #[default]
  Unspecified = 0,
  Mpeg = 1, // Limited range (16-235 for Y, 16-240 for UV)
  Jpeg = 2, // Full range (0-255)
}

// ============================================================================
// Picture Type
// ============================================================================

/// Picture/frame type
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AVPictureType {
  None = 0,
  I = 1, // Intra (key frame)
  P = 2, // Predicted
  B = 3, // Bi-directionally predicted
  S = 4, // S(GMC)-VOP MPEG-4
  Si = 5,
  Sp = 6,
  Bi = 7,
}

// ============================================================================
// Opaque FFmpeg Types
// ============================================================================

/// Opaque AVCodec structure (codec implementation descriptor)
#[repr(C)]
pub struct AVCodec {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVCodecContext structure (encoder/decoder instance)
#[repr(C)]
pub struct AVCodecContext {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVFrame structure (uncompressed video/audio data)
#[repr(C)]
pub struct AVFrame {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVPacket structure (compressed data)
#[repr(C)]
pub struct AVPacket {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVBufferRef structure (reference-counted buffer)
#[repr(C)]
pub struct AVBufferRef {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque SwsContext structure (software scaler context)
#[repr(C)]
pub struct SwsContext {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque SwrContext structure (software resampler context for audio)
#[repr(C)]
pub struct SwrContext {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// AVChannelLayout structure (audio channel layout - FFmpeg 5.1+)
///
/// This is NOT opaque - FFmpeg documents that sizeof(AVChannelLayout) is part
/// of the public ABI and may be allocated on stack. The struct is 24 bytes:
/// - order: 4 bytes (enum AVChannelOrder)
/// - nb_channels: 4 bytes (int)
/// - u: 8 bytes (union of u64 mask or pointer to AVChannelCustom)
/// - opaque: 8 bytes (void*)
#[repr(C, align(8))]
pub struct AVChannelLayout {
  _data: [u8; 24],
}

/// Opaque AVDictionary structure (key-value options)
#[repr(C)]
pub struct AVDictionary {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVHWFramesContext structure (hardware frames pool)
#[repr(C)]
pub struct AVHWFramesContext {
  _opaque: [u8; 0],
  _marker: PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

// ============================================================================
// Constants
// ============================================================================

/// No timestamp value
pub const AV_NOPTS_VALUE: i64 = 0x8000000000000000u64 as i64;

/// Packet flags
pub mod pkt_flag {
  use std::os::raw::c_int;

  pub const KEY: c_int = 0x0001;
  pub const CORRUPT: c_int = 0x0002;
  pub const DISCARD: c_int = 0x0004;
  pub const TRUSTED: c_int = 0x0008;
  pub const DISPOSABLE: c_int = 0x0010;
}

/// Legacy channel layout masks (for older FFmpeg versions)
/// These are bitmasks used by FFmpeg < 5.1 for channel configuration
pub mod channel_layout {
  pub const AV_CH_FRONT_LEFT: u64 = 0x00000001;
  pub const AV_CH_FRONT_RIGHT: u64 = 0x00000002;
  pub const AV_CH_FRONT_CENTER: u64 = 0x00000004;
  pub const AV_CH_LOW_FREQUENCY: u64 = 0x00000008;
  pub const AV_CH_BACK_LEFT: u64 = 0x00000010;
  pub const AV_CH_BACK_RIGHT: u64 = 0x00000020;
  pub const AV_CH_BACK_CENTER: u64 = 0x00000100;
  pub const AV_CH_SIDE_LEFT: u64 = 0x00000200;
  pub const AV_CH_SIDE_RIGHT: u64 = 0x00000400;

  // Common layouts
  pub const AV_CH_LAYOUT_MONO: u64 = AV_CH_FRONT_CENTER;
  pub const AV_CH_LAYOUT_STEREO: u64 = AV_CH_FRONT_LEFT | AV_CH_FRONT_RIGHT;
  pub const AV_CH_LAYOUT_2_1: u64 = AV_CH_LAYOUT_STEREO | AV_CH_BACK_CENTER;
  pub const AV_CH_LAYOUT_SURROUND: u64 = AV_CH_LAYOUT_STEREO | AV_CH_FRONT_CENTER;
  pub const AV_CH_LAYOUT_4POINT0: u64 = AV_CH_LAYOUT_SURROUND | AV_CH_BACK_CENTER;
  pub const AV_CH_LAYOUT_5POINT0: u64 = AV_CH_LAYOUT_SURROUND | AV_CH_SIDE_LEFT | AV_CH_SIDE_RIGHT;
  pub const AV_CH_LAYOUT_5POINT1: u64 = AV_CH_LAYOUT_5POINT0 | AV_CH_LOW_FREQUENCY;
  pub const AV_CH_LAYOUT_7POINT1: u64 = AV_CH_LAYOUT_5POINT1 | AV_CH_BACK_LEFT | AV_CH_BACK_RIGHT;

  /// Get the number of channels for a given channel layout
  pub fn count_channels(layout: u64) -> u32 {
    layout.count_ones()
  }

  /// Get a default channel layout for the given number of channels
  pub fn default_for_channels(channels: u32) -> u64 {
    match channels {
      1 => AV_CH_LAYOUT_MONO,
      2 => AV_CH_LAYOUT_STEREO,
      3 => AV_CH_LAYOUT_SURROUND,
      4 => AV_CH_LAYOUT_4POINT0,
      5 => AV_CH_LAYOUT_5POINT0,
      6 => AV_CH_LAYOUT_5POINT1,
      8 => AV_CH_LAYOUT_7POINT1,
      _ => 0,
    }
  }
}

/// Packet side data types
/// These are used for attaching additional data to AVPacket
pub mod pkt_side_data_type {
  use std::os::raw::c_int;

  /// An AV_PKT_DATA_PALETTE side data packet contains exactly AVPALETTE_SIZE
  /// bytes worth of palette. This side data signals that a new palette is
  /// present.
  pub const AV_PKT_DATA_PALETTE: c_int = 0;

  /// The AV_PKT_DATA_NEW_EXTRADATA is used to notify the codec or the format
  /// that the extradata buffer was changed and the receiving side should
  /// act upon it appropriately. The new extradata is embedded in the side
  /// data buffer and should be immediately used for processing the current
  /// frame or packet.
  pub const AV_PKT_DATA_NEW_EXTRADATA: c_int = 1;

  /// An AV_PKT_DATA_PARAM_CHANGE side data packet is laid out as follows:
  /// @code
  /// u32le param_flags
  /// if (param_flags & AV_SIDE_DATA_PARAM_CHANGE_CHANNEL_COUNT)
  ///     s32le channel_count
  /// if (param_flags & AV_SIDE_DATA_PARAM_CHANGE_CHANNEL_LAYOUT)
  ///     u64le channel_layout
  /// if (param_flags & AV_SIDE_DATA_PARAM_CHANGE_SAMPLE_RATE)
  ///     s32le sample_rate
  /// if (param_flags & AV_SIDE_DATA_PARAM_CHANGE_DIMENSIONS)
  ///     s32le width
  ///     s32le height
  /// @endcode
  pub const AV_PKT_DATA_PARAM_CHANGE: c_int = 2;

  /// An AV_PKT_DATA_H263_MB_INFO side data packet contains a number of
  /// structures with info about macroblocks relevant to splitting the
  /// packet into smaller packets on macroblock edges (e.g. as for RFC 2190).
  pub const AV_PKT_DATA_H263_MB_INFO: c_int = 3;

  /// AV_PKT_DATA_QUALITY_STATS - quality statistics from encoder
  pub const AV_PKT_DATA_QUALITY_STATS: c_int = 8;

  /// This side data contains Matroska BlockAdditional data. It is used to
  /// store additional data needed for proper playback of VP9 alpha encoded
  /// videos. The data is a raw byte buffer.
  ///
  /// For VP9 alpha, this contains the encoded alpha channel data that must
  /// be written to WebM/MKV BlockAdditions element.
  /// Format: 8-byte BlockAddId (big-endian) followed by the actual alpha data.
  pub const AV_PKT_DATA_MATROSKA_BLOCKADDITIONAL: c_int = 15;

  /// The optional first identifier line of a WebVTT cue
  pub const AV_PKT_DATA_WEBVTT_IDENTIFIER: c_int = 16;

  /// The optional settings (rendering instructions) that immediately
  /// follow the timestamp specifier of a WebVTT cue.
  pub const AV_PKT_DATA_WEBVTT_SETTINGS: c_int = 17;

  /// An AV_PKT_DATA_ALPHA_MODE side data contains the alpha mode value.
  /// This is used to signal alpha channel information in VP9 encoded videos.
  pub const AV_PKT_DATA_ALPHA_MODE: c_int = 22;
}
