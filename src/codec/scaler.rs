//! Safe wrapper around FFmpeg SwsContext
//!
//! Provides pixel format conversion and image scaling functionality.

use crate::ffi::{
  AVPixelFormat, SwsContext,
  swscale::{sws_freeContext, sws_getContext, sws_scale},
};
use std::ptr::NonNull;

use super::{CodecError, CodecResult, Frame};

/// Scaling algorithm
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleAlgorithm {
  /// Fast bilinear (fastest, lower quality)
  FastBilinear,
  /// Bilinear (good balance)
  #[default]
  Bilinear,
  /// Bicubic (higher quality, slower)
  Bicubic,
  /// Lanczos (highest quality, slowest)
  Lanczos,
  /// Point/nearest neighbor (fastest, blocky)
  Point,
}

impl ScaleAlgorithm {
  fn to_sws_flags(self) -> i32 {
    use crate::ffi::swscale::*;
    match self {
      ScaleAlgorithm::FastBilinear => SWS_FAST_BILINEAR,
      ScaleAlgorithm::Bilinear => SWS_BILINEAR,
      ScaleAlgorithm::Bicubic => SWS_BICUBIC,
      ScaleAlgorithm::Lanczos => SWS_LANCZOS,
      ScaleAlgorithm::Point => SWS_POINT,
    }
  }
}

/// Safe wrapper around SwsContext for pixel format conversion and scaling
pub struct Scaler {
  ptr: NonNull<SwsContext>,
  src_width: u32,
  src_height: u32,
  src_format: AVPixelFormat,
  dst_width: u32,
  dst_height: u32,
  dst_format: AVPixelFormat,
}

impl Scaler {
  /// Convert the rect (x, y, w, h) of `src` to 8-bit RGBA or BGRA rows `stride` bytes apart
  /// in `dst`, using AVCOL_* `tags` [primaries, trc, matrix, range] (-1 = the frame's own).
  /// `same_format`: the target is the VideoFrame's own format. `src_x`: the VideoFrame is
  /// RGBX/BGRX. Together they pick Chrome's alpha handling.
  #[allow(clippy::too_many_arguments)]
  pub fn copy_rect_to_rgb(
    src: &Frame,
    rect: (u32, u32, u32, u32),
    tags: [i32; 4],
    target_p3: bool,
    bgr: bool,
    same_format: bool,
    src_x: bool,
    dst: &mut [u8],
    stride: usize,
  ) -> CodecResult<()> {
    let (x, y, w, h) = rect;
    let end = (h as usize)
      .checked_sub(1)
      .and_then(|rows| rows.checked_mul(stride))
      .and_then(|v| v.checked_add(w as usize * 4));
    if end.is_none_or(|end| end > dst.len()) || stride > isize::MAX as usize {
      return Err(CodecError::InvalidConfig(
        "copy rect does not fit the destination".into(),
      ));
    }
    let result = unsafe {
      crate::ffi::accessors::ffframe_copy_rect_to_rgb(
        src.as_ptr(),
        x as i32,
        y as i32,
        w as i32,
        h as i32,
        tags[0],
        tags[1],
        tags[2],
        tags[3],
        target_p3 as i32,
        bgr as i32,
        same_format as i32,
        src_x as i32,
        dst.as_mut_ptr(),
        stride as isize,
      )
    };
    crate::ffi::check_error(result)?;
    Ok(())
  }

  /// Create a new scaler for the given conversion
  pub fn new(
    src_width: u32,
    src_height: u32,
    src_format: AVPixelFormat,
    dst_width: u32,
    dst_height: u32,
    dst_format: AVPixelFormat,
    algorithm: ScaleAlgorithm,
  ) -> CodecResult<Self> {
    // swscale asserts (aborting the process) on an invalid format; `None` is what
    // `Frame::format()` returns for a format AVPixelFormat does not list.
    if src_format == AVPixelFormat::None || dst_format == AVPixelFormat::None {
      return Err(CodecError::InvalidConfig(format!(
        "Cannot create scaler: unsupported pixel format ({:?} to {:?})",
        src_format, dst_format
      )));
    }
    let ptr = unsafe {
      sws_getContext(
        src_width as i32,
        src_height as i32,
        src_format.as_raw(),
        dst_width as i32,
        dst_height as i32,
        dst_format.as_raw(),
        algorithm.to_sws_flags(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null(),
      )
    };

    NonNull::new(ptr)
      .map(|ptr| Self {
        ptr,
        src_width,
        src_height,
        src_format,
        dst_width,
        dst_height,
        dst_format,
      })
      .ok_or(CodecError::InvalidConfig(format!(
        "Cannot create scaler from {:?} {}x{} to {:?} {}x{}",
        src_format, src_width, src_height, dst_format, dst_width, dst_height
      )))
  }

  /// Create a scaler for format conversion only (no scaling)
  pub fn new_converter(
    width: u32,
    height: u32,
    src_format: AVPixelFormat,
    dst_format: AVPixelFormat,
  ) -> CodecResult<Self> {
    Self::new(
      width,
      height,
      src_format,
      width,
      height,
      dst_format,
      ScaleAlgorithm::Bilinear,
    )
  }

  /// Scale/convert a frame
  ///
  /// The destination frame must already have buffers allocated with the correct format/dimensions
  pub fn scale(&self, src: &Frame, dst: &mut Frame) -> CodecResult<()> {
    // Verify dimensions match
    if src.width() != self.src_width
      || src.height() != self.src_height
      || dst.width() != self.dst_width
      || dst.height() != self.dst_height
    {
      return Err(CodecError::InvalidConfig(
        "Frame dimensions don't match scaler configuration".into(),
      ));
    }

    // Prepare source data pointers and strides
    let src_data: [*const u8; 4] = [src.data(0), src.data(1), src.data(2), src.data(3)];
    let src_linesize: [i32; 4] = [
      src.linesize(0),
      src.linesize(1),
      src.linesize(2),
      src.linesize(3),
    ];

    // Prepare destination data pointers and strides
    let dst_data: [*mut u8; 4] = [
      dst.data_mut(0),
      dst.data_mut(1),
      dst.data_mut(2),
      dst.data_mut(3),
    ];
    let dst_linesize: [i32; 4] = [
      dst.linesize(0),
      dst.linesize(1),
      dst.linesize(2),
      dst.linesize(3),
    ];

    let result = unsafe {
      sws_scale(
        self.ptr.as_ptr(),
        src_data.as_ptr(),
        src_linesize.as_ptr(),
        0,
        self.src_height as i32,
        dst_data.as_ptr(),
        dst_linesize.as_ptr(),
      )
    };

    if result != self.dst_height as i32 {
      return Err(CodecError::InvalidState(format!(
        "Scaling produced {} rows instead of {}",
        result, self.dst_height
      )));
    }

    // Copy metadata from source
    dst.set_pts(src.pts());
    dst.set_duration(src.duration());
    dst.set_color_primaries(src.color_primaries());
    dst.set_color_trc(src.color_trc());
    dst.set_colorspace(src.colorspace());
    dst.set_color_range(src.color_range());

    Ok(())
  }

  /// Scale/convert a frame, allocating a new destination frame
  pub fn scale_alloc(&self, src: &Frame) -> CodecResult<Frame> {
    let mut dst = Frame::new_video(self.dst_width, self.dst_height, self.dst_format)?;
    self.scale(src, &mut dst)?;
    Ok(dst)
  }

  // ========================================================================
  // Accessors
  // ========================================================================

  /// Get source width
  pub fn src_width(&self) -> u32 {
    self.src_width
  }

  /// Get source height
  pub fn src_height(&self) -> u32 {
    self.src_height
  }

  /// Get source format
  pub fn src_format(&self) -> AVPixelFormat {
    self.src_format
  }

  /// Get destination width
  pub fn dst_width(&self) -> u32 {
    self.dst_width
  }

  /// Get destination height
  pub fn dst_height(&self) -> u32 {
    self.dst_height
  }

  /// Get destination format
  pub fn dst_format(&self) -> AVPixelFormat {
    self.dst_format
  }

  /// Check if this is a format-only conversion (no scaling)
  pub fn is_converter_only(&self) -> bool {
    self.src_width == self.dst_width && self.src_height == self.dst_height
  }
}

impl Drop for Scaler {
  fn drop(&mut self) {
    unsafe { sws_freeContext(self.ptr.as_ptr()) }
  }
}

// SwsContext is thread-safe for reading, but we don't share mutable access
unsafe impl Send for Scaler {}

impl std::fmt::Debug for Scaler {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Scaler")
      .field(
        "src",
        &format!(
          "{}x{} {:?}",
          self.src_width, self.src_height, self.src_format
        ),
      )
      .field(
        "dst",
        &format!(
          "{}x{} {:?}",
          self.dst_width, self.dst_height, self.dst_format
        ),
      )
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_scaler_creation() {
    let scaler = Scaler::new(
      1920,
      1080,
      AVPixelFormat::Yuv420p,
      1280,
      720,
      AVPixelFormat::Yuv420p,
      ScaleAlgorithm::Bilinear,
    );
    assert!(scaler.is_ok());
  }

  #[test]
  fn test_converter_creation() {
    let converter = Scaler::new_converter(1920, 1080, AVPixelFormat::Rgba, AVPixelFormat::Yuv420p);
    assert!(converter.is_ok());
    assert!(converter.unwrap().is_converter_only());
  }

  /// BT.709 limited-range tags [primaries, trc, matrix, range].
  const BT709_LIMITED: [i32; 4] = [
    crate::ffi::AVColorPrimaries::Bt709 as i32,
    crate::ffi::AVColorTransferCharacteristic::Bt709 as i32,
    crate::ffi::AVColorSpace::Bt709 as i32,
    crate::ffi::AVColorRange::Mpeg as i32,
  ];

  /// 4x2 I420 frame: row 0 is Y=235 (white), row 1 is Y=16 (black), U=V=128.
  fn white_black_i420() -> Frame {
    let mut frame = Frame::new_video(4, 2, AVPixelFormat::Yuv420p).unwrap();
    let y_stride = frame.linesize(0) as usize;
    let y = frame.plane_data_mut(0).unwrap();
    y[..4].fill(235);
    y[y_stride..y_stride + 4].fill(16);
    for plane in 1..3 {
      frame.plane_data_mut(plane).unwrap().fill(128);
    }
    frame
  }

  #[test]
  fn copy_rect_to_rgb_limited_white_black() {
    let frame = white_black_i420();
    for bgr in [false, true] {
      let mut dst = [0u8; 4 * 4 * 2];
      Scaler::copy_rect_to_rgb(
        &frame,
        (0, 0, 4, 2),
        BT709_LIMITED,
        false,
        bgr,
        false,
        false,
        &mut dst,
        16,
      )
      .unwrap();
      for px in dst[..16].chunks(4) {
        assert_eq!(px, [255, 255, 255, 255]);
      }
      for px in dst[16..].chunks(4) {
        assert_eq!(px, [0, 0, 0, 255]);
      }
    }
  }

  #[test]
  fn copy_rect_to_rgb_rejects_out_of_bounds() {
    let frame = white_black_i420();
    let copy = |rect, dst: &mut [u8]| {
      Scaler::copy_rect_to_rgb(
        &frame,
        rect,
        BT709_LIMITED,
        false,
        false,
        false,
        false,
        dst,
        8,
      )
    };
    let mut dst = [0u8; 2 * 4 * 2];
    // x + width runs past the 4-pixel-wide frame
    assert!(copy((3, 0, 2, 2), &mut dst).is_err());
    // the last row needs 8 bytes at offset 8, so 15 bytes is one too few
    assert!(copy((0, 0, 2, 2), &mut dst[..15]).is_err());
    assert!(copy((0, 0, 2, 2), &mut dst).is_ok());
  }
}
