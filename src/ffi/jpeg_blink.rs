//! Blink's JPEG decoder (src/ffi/jpeg_blink.c) on the libjpeg-turbo Chrome ships
//! (vendor/libjpeg-turbo).

use super::types::AVFrame;
use std::os::raw::c_int;

unsafe extern "C" {
  /// Read the header for a decode at `scale_num`/8 (1-8): out = [image width, image height,
  /// output width, output height, AVPixelFormat (yuv420p, yuv422p, yuv444p or bgra)].
  /// `allow_yuv` = 0 gives bgra for every image. Returns AVERROR(EAGAIN) when the data ends
  /// before the header does, another negative AVERROR when Chrome fails the image.
  pub fn blink_jpeg_info(
    data: *const u8,
    size: usize,
    scale_num: c_int,
    allow_yuv: c_int,
    out: *mut c_int,
  ) -> c_int;
  /// Decode at `scale_num`/8 into `dst`, allocated with the format and output size that
  /// `blink_jpeg_info` gives for the same `scale_num` and `allow_yuv`. Returns AVERROR(EAGAIN)
  /// when the data ends before the image does (`rows` is then 1 when Blink got to output rows),
  /// another negative AVERROR when Chrome fails the image.
  pub fn blink_jpeg_decode(
    data: *const u8,
    size: usize,
    scale_num: c_int,
    allow_yuv: c_int,
    dst: *mut AVFrame,
    rows: *mut c_int,
  ) -> c_int;
}
