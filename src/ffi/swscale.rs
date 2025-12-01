//! libswscale function declarations
//!
//! Provides pixel format conversion and image scaling functionality.

use super::types::*;
use std::os::raw::c_int;

extern "C" {
  // ========================================================================
  // Context Management
  // ========================================================================

  /// Allocate and return an SwsContext
  ///
  /// # Arguments
  /// * `srcW` - Source width
  /// * `srcH` - Source height
  /// * `srcFormat` - Source pixel format
  /// * `dstW` - Destination width
  /// * `dstH` - Destination height
  /// * `dstFormat` - Destination pixel format
  /// * `flags` - Scaling algorithm flags (SWS_*)
  /// * `srcFilter` - Source filter (NULL for none)
  /// * `dstFilter` - Destination filter (NULL for none)
  /// * `param` - Extra parameters for scaling algorithm (NULL for defaults)
  pub fn sws_getContext(
    srcW: c_int,
    srcH: c_int,
    srcFormat: c_int,
    dstW: c_int,
    dstH: c_int,
    dstFormat: c_int,
    flags: c_int,
    srcFilter: *mut SwsFilter,
    dstFilter: *mut SwsFilter,
    param: *const f64,
  ) -> *mut SwsContext;

  /// Get a cached context, reusing the existing one if parameters match
  ///
  /// If context is NULL, acts like sws_getContext.
  /// Otherwise returns the existing context if compatible, or a new one.
  pub fn sws_getCachedContext(
    context: *mut SwsContext,
    srcW: c_int,
    srcH: c_int,
    srcFormat: c_int,
    dstW: c_int,
    dstH: c_int,
    dstFormat: c_int,
    flags: c_int,
    srcFilter: *mut SwsFilter,
    dstFilter: *mut SwsFilter,
    param: *const f64,
  ) -> *mut SwsContext;

  /// Free the swscaler context
  pub fn sws_freeContext(swsContext: *mut SwsContext);

  // ========================================================================
  // Scaling Operations
  // ========================================================================

  /// Scale the image slice in srcSlice and put the resulting scaled
  /// slice in the image in dst
  ///
  /// # Arguments
  /// * `c` - The scaling context previously created with sws_getContext
  /// * `srcSlice` - Array of pointers to source plane data
  /// * `srcStride` - Array of source plane strides
  /// * `srcSliceY` - Position in source image of the slice to process
  /// * `srcSliceH` - Height of the source slice
  /// * `dst` - Array of pointers to destination plane data
  /// * `dstStride` - Array of destination plane strides
  ///
  /// # Returns
  /// Height of the output slice
  pub fn sws_scale(
    c: *mut SwsContext,
    srcSlice: *const *const u8,
    srcStride: *const c_int,
    srcSliceY: c_int,
    srcSliceH: c_int,
    dst: *const *mut u8,
    dstStride: *const c_int,
  ) -> c_int;

  /// Scale using AVFrame structures (FFmpeg 5.0+)
  ///
  /// # Arguments
  /// * `c` - The scaling context
  /// * `dst` - Destination frame
  /// * `src` - Source frame
  ///
  /// # Returns
  /// 0 on success, negative error code on failure
  pub fn sws_scale_frame(c: *mut SwsContext, dst: *mut AVFrame, src: *const AVFrame) -> c_int;

  // ========================================================================
  // Format Support
  // ========================================================================

  /// Check if a pixel format is supported as input
  pub fn sws_isSupportedInput(pix_fmt: c_int) -> c_int;

  /// Check if a pixel format is supported as output
  pub fn sws_isSupportedOutput(pix_fmt: c_int) -> c_int;

  /// Check if an endianness conversion is supported
  pub fn sws_isSupportedEndiannessConversion(pix_fmt: c_int) -> c_int;
}

// ============================================================================
// Opaque Filter Type
// ============================================================================

/// Opaque SwsFilter structure
#[repr(C)]
pub struct SwsFilter {
  _opaque: [u8; 0],
}

// ============================================================================
// Scaling Algorithm Flags
// ============================================================================

/// Fast bilinear scaling (low quality, fast)
pub const SWS_FAST_BILINEAR: c_int = 1;

/// Bilinear scaling
pub const SWS_BILINEAR: c_int = 2;

/// Bicubic scaling (good quality, slower)
pub const SWS_BICUBIC: c_int = 4;

/// Experimental X scaling
pub const SWS_X: c_int = 8;

/// Nearest neighbor (point) scaling (fastest, blocky)
pub const SWS_POINT: c_int = 0x10;

/// Area averaging scaling
pub const SWS_AREA: c_int = 0x20;

/// Bicubic for luma, bilinear for chroma
pub const SWS_BICUBLIN: c_int = 0x40;

/// Gaussian scaling
pub const SWS_GAUSS: c_int = 0x80;

/// Sinc scaling
pub const SWS_SINC: c_int = 0x100;

/// Lanczos scaling (high quality, slowest)
pub const SWS_LANCZOS: c_int = 0x200;

/// Natural bicubic spline
pub const SWS_SPLINE: c_int = 0x400;

// ============================================================================
// Additional Flags
// ============================================================================

/// Print sws info
pub const SWS_PRINT_INFO: c_int = 0x1000;

/// Perform full chroma upsampling when upscaling
pub const SWS_FULL_CHR_H_INT: c_int = 0x2000;

/// Perform full chroma interpolation when downscaling
pub const SWS_FULL_CHR_H_INP: c_int = 0x4000;

/// Use direct BGR instead of RGB
pub const SWS_DIRECT_BGR: c_int = 0x8000;

/// Use accurate rounding
pub const SWS_ACCURATE_RND: c_int = 0x40000;

/// Use bitexact operations
pub const SWS_BITEXACT: c_int = 0x80000;

/// Enable error diffusion dithering
pub const SWS_ERROR_DIFFUSION: c_int = 0x800000;

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if pixel format can be used as input
pub fn is_input_supported(format: AVPixelFormat) -> bool {
  unsafe { sws_isSupportedInput(format.as_raw()) != 0 }
}

/// Check if pixel format can be used as output
pub fn is_output_supported(format: AVPixelFormat) -> bool {
  unsafe { sws_isSupportedOutput(format.as_raw()) != 0 }
}
