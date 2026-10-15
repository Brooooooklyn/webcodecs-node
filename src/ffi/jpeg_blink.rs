//! Blink's JPEG decoder (src/ffi/jpeg_blink.c) on the libjpeg-turbo Chrome ships
//! (vendor/libjpeg-turbo).

use super::types::AVFrame;
use std::os::raw::c_int;

unsafe extern "C" {
  /// Read the header for a decode at `scale_num`/8 (1-8): out = [image width, image height,
  /// output width, output height, AVPixelFormat (yuv420p, yuv422p, yuv444p or bgra)].
  /// Returns a negative AVERROR when Chrome fails the image.
  pub fn blink_jpeg_info(data: *const u8, size: usize, scale_num: c_int, out: *mut c_int) -> c_int;
  /// Decode at `scale_num`/8 into `dst`, allocated with the format and output size that
  /// `blink_jpeg_info` gives. Returns a negative AVERROR when Chrome fails the image.
  pub fn blink_jpeg_decode(
    data: *const u8,
    size: usize,
    scale_num: c_int,
    dst: *mut AVFrame,
  ) -> c_int;
}
