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

/// Supported video codec IDs
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AVCodecID {
    None = 0,
    H264 = 27,
    Hevc = 173, // H.265
    Vp8 = 139,
    Vp9 = 167,
    Av1 = 226,
}

impl AVCodecID {
    /// Convert WebCodecs codec string to AVCodecID
    pub fn from_webcodecs_codec(codec: &str) -> Option<Self> {
        // H.264/AVC: avc1.PPCCLL or avc3.PPCCLL
        if codec.starts_with("avc1") || codec.starts_with("avc3") {
            return Some(Self::H264);
        }
        // H.265/HEVC: hev1.P.T.Lxxx or hvc1.P.T.Lxxx
        if codec.starts_with("hev1") || codec.starts_with("hvc1") {
            return Some(Self::Hevc);
        }
        // VP8
        if codec == "vp8" {
            return Some(Self::Vp8);
        }
        // VP9: vp09.PP.LL.DD or just "vp9"
        if codec.starts_with("vp09") || codec == "vp9" {
            return Some(Self::Vp9);
        }
        // AV1: av01.P.LLT.DD or just "av1"
        if codec.starts_with("av01") || codec == "av1" {
            return Some(Self::Av1);
        }
        None
    }

    /// Get the default WebCodecs codec string for this codec
    pub fn to_webcodecs_codec(&self) -> &'static str {
        match self {
            Self::None => "",
            Self::H264 => "avc1.42001f", // Baseline profile, level 3.1
            Self::Hevc => "hev1.1.6.L93.B0", // Main profile
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp09.00.10.08", // Profile 0, level 1.0, 8-bit
            Self::Av1 => "av01.0.01M.08", // Main profile, level 2.1, 8-bit
        }
    }

    /// Get the raw FFmpeg codec ID value
    pub fn as_raw(&self) -> c_int {
        *self as c_int
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
    // Planar YUV formats
    Yuv420p = 0,   // I420
    Yuv422p = 4,   // I422
    Yuv444p = 5,   // I444
    Yuva420p = 33, // I420A (with alpha)
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
    // 10-bit formats
    Yuv420p10le = 64,
    // Hardware formats
    Videotoolbox = 162,
    Cuda = 119,
    Vaapi = 53,
}

impl AVPixelFormat {
    /// Convert from WebCodecs VideoPixelFormat string
    pub fn from_webcodecs_format(format: &str) -> Option<Self> {
        match format {
            "I420" => Some(Self::Yuv420p),
            "I420A" => Some(Self::Yuva420p),
            "I422" => Some(Self::Yuv422p),
            "I444" => Some(Self::Yuv444p),
            "NV12" => Some(Self::Nv12),
            "RGBA" | "RGBX" => Some(Self::Rgba),
            "BGRA" | "BGRX" => Some(Self::Bgra),
            _ => None,
        }
    }

    /// Convert to WebCodecs VideoPixelFormat string
    pub fn to_webcodecs_format(&self) -> Option<&'static str> {
        match self {
            Self::Yuv420p => Some("I420"),
            Self::Yuva420p => Some("I420A"),
            Self::Yuv422p => Some("I422"),
            Self::Yuv444p => Some("I444"),
            Self::Nv12 => Some("NV12"),
            Self::Rgba => Some("RGBA"),
            Self::Bgra => Some("BGRA"),
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
            Self::Yuv420p | Self::Yuv422p | Self::Yuv444p => 3,
            Self::Yuva420p => 4,
            Self::Nv12 | Self::Nv21 => 2,
            Self::Rgb24 | Self::Bgr24 | Self::Rgba | Self::Bgra | Self::Argb | Self::Abgr => 1,
            _ => 0,
        }
    }

    /// Whether this is a hardware pixel format
    pub fn is_hardware(&self) -> bool {
        matches!(self, Self::Videotoolbox | Self::Cuda | Self::Vaapi)
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
