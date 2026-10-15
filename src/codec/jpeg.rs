//! JPEG decoding with Chrome 155's bytes: Blink's JPEGImageDecoder settings on the
//! libjpeg-turbo that Chrome ships (src/ffi/jpeg_blink.c, vendor/libjpeg-turbo).

// ImageDecoder does not use it yet.
#![allow(dead_code)]

use super::{CodecError, Frame};
use crate::ffi::jpeg_blink::{blink_jpeg_decode, blink_jpeg_info};
use crate::ffi::{AVPixelFormat, check_error};
use std::os::raw::c_int;

/// What a decode at a given scale gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JpegInfo {
  /// Image width in the header (Chrome's scale numerator comes from the image size)
  pub width: u32,
  /// Image height in the header
  pub height: u32,
  /// Decoded width: ceil(width * scale_num / 8)
  pub output_width: u32,
  /// Decoded height: ceil(height * scale_num / 8)
  pub output_height: u32,
  /// Yuv420p, Yuv422p or Yuv444p (full range) when Chrome decodes to YUV planes, else Bgra
  /// with alpha 255
  pub format: AVPixelFormat,
}

/// Read the header for a decode at `scale_num`/8 of the size
/// (JPEGImageDecoder::DesiredScaleNumerator: 1-8, 8 is full size).
pub fn jpeg_info(data: &[u8], scale_num: u32) -> Result<JpegInfo, CodecError> {
  let mut out = [0 as c_int; 5];
  check_error(unsafe {
    blink_jpeg_info(
      data.as_ptr(),
      data.len(),
      scale_num as c_int,
      out.as_mut_ptr(),
    )
  })?;
  Ok(JpegInfo {
    width: out[0] as u32,
    height: out[1] as u32,
    output_width: out[2] as u32,
    output_height: out[3] as u32,
    format: AVPixelFormat::from_raw(out[4]),
  })
}

/// Decode a JPEG at `scale_num`/8 of its size, as Chrome does. An error is an image that
/// Chrome fails to decode.
pub fn decode_jpeg(data: &[u8], scale_num: u32) -> Result<Frame, CodecError> {
  let info = jpeg_info(data, scale_num)?;
  let mut frame = Frame::new_video(info.output_width, info.output_height, info.format)?;
  check_error(unsafe {
    blink_jpeg_decode(
      data.as_ptr(),
      data.len(),
      scale_num as c_int,
      frame.as_mut_ptr(),
    )
  })?;
  Ok(frame)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::ffi::{AVColorRange, AVColorSpace};
  use sha2::{Digest, Sha256};

  macro_rules! fixture {
    ($name:literal) => {
      include_bytes!(concat!("../../__test__/fixtures/jpeg/", $name)).as_slice()
    };
  }

  /// SHA-256 of the planes (Y, U, V) or BGRA rows without padding.
  fn digest(frame: &Frame) -> String {
    let mut bytes = vec![0u8; frame.width() as usize * frame.height() as usize * 4];
    let size = frame.copy_to_buffer(&mut bytes).unwrap();
    Sha256::digest(&bytes[..size])
      .iter()
      .map(|byte| format!("{byte:02x}"))
      .collect()
  }

  // The digests are of Chrome 155's own bytes: ImageDecoder.decode(), then VideoFrame.copyTo(),
  // with desiredWidth/desiredHeight for the scaled ones (BGRX there: alpha 255).
  #[test]
  fn decodes_chrome_bytes() {
    use AVPixelFormat::{Bgra, Yuv420p, Yuv422p, Yuv444p};
    let cases = [
      // 4:2:0 at an odd size: rows past the image during raw output, chroma 19x5
      (
        fixture!("420-37x9.jpg"),
        8,
        (37, 9),
        Yuv420p,
        "120646bc254fd13efce3ce6b8da37fab63ba1d06164098d2f3842ce1e1025fe8",
      ),
      // desired size: M/8 with libjpeg's scaled IDCT, always BGRX
      (
        fixture!("420-37x9.jpg"),
        5,
        (24, 6),
        Bgra,
        "ac1afec48a889ac8cf1508078866c04e2a69782308549d38042be943e426cd57",
      ),
      (
        fixture!("420-37x9.jpg"),
        1,
        (5, 2),
        Bgra,
        "5ad43d7526442dc2bc0bdd1c201df15e794d374084e3fb0e055f75310ba37cb1",
      ),
      (
        fixture!("422-15x15.jpg"),
        8,
        (15, 15),
        Yuv422p,
        "44a8fb19eec3d3b012a44ab06600f5242d840bd1177d2effc1b2fa9b3bede517",
      ),
      // multi-scan: buffered-image mode
      (
        fixture!("444-progressive-33x17.jpg"),
        8,
        (33, 17),
        Yuv444p,
        "faead195d1830cd7209a604c8916ee95276b8d5972bf8ab4c06eaf1d2a7f761d",
      ),
      // 4:4:0 is not a YUV layout Chrome keeps: fancy upsampling to BGRX
      (
        fixture!("440-37x9.jpg"),
        8,
        (37, 9),
        Bgra,
        "63e19493c9ee853f2862eafeb1d9b79faf5972d2f68704b94e035446d085ec94",
      ),
      // YCCK -> CMYK -> R = C * K / 255
      (
        fixture!("ycck-33x17.jpg"),
        8,
        (33, 17),
        Bgra,
        "c0f20c6e0b50d6f2771c7194f783f2f89476ce79fd3ae2b8a509d7fea6856a49",
      ),
      // a corrupt-data warning stops the scan reading early (Blink's buffered-image loop)
      (
        fixture!("progressive-junk-in-scan.jpg"),
        8,
        (64, 48),
        Yuv420p,
        "d6add19cb2298231cfcd4f3edd0c9b5f42b36b672d8f62489f331a630ef4eec0",
      ),
      // a 4:4:0 JPEG followed by a 4:2:0 one (like MPO): only the first counts
      (
        fixture!("concatenated-440-420.jpg"),
        8,
        (64, 48),
        Bgra,
        "e2b941269bd8c218d10ab825d44a1d51fbc46ad57c3c633e7892fd74d0387388",
      ),
    ];
    for (data, scale, (width, height), format, expected) in cases {
      let info = jpeg_info(data, scale).unwrap();
      assert_eq!(
        (info.output_width, info.output_height, info.format),
        (width, height, format)
      );
      let frame = decode_jpeg(data, scale).unwrap();
      assert_eq!(
        (frame.width(), frame.height(), frame.format()),
        (width, height, format)
      );
      assert_eq!(
        digest(&frame),
        expected,
        "{width}x{height} {format:?} at {scale}/8"
      );
      assert_eq!(frame.color_range(), AVColorRange::Jpeg);
      let matrix = if format == Bgra {
        AVColorSpace::Rgb
      } else {
        AVColorSpace::Bt470bg
      };
      assert_eq!(frame.colorspace(), matrix);
    }
  }

  // Chrome rejects these with EncodingError once all the data is there.
  #[test]
  fn fails_like_chrome() {
    for data in [
      fixture!("no-eoi.jpg"),
      fixture!("truncated.jpg"),
      fixture!("12-bit.jpg"),
      fixture!("arithmetic.jpg"),
      b"\xff\xd8\xff".as_slice(),
      b"".as_slice(),
    ] {
      assert!(decode_jpeg(data, 8).is_err());
    }
  }

  #[test]
  fn info_reads_the_header() {
    let data = fixture!("420-37x9.jpg");
    let info = jpeg_info(data, 3).unwrap();
    assert_eq!((info.width, info.height), (37, 9));
    assert_eq!((info.output_width, info.output_height), (14, 4));
    assert!(jpeg_info(data, 0).is_err());
    assert!(jpeg_info(data, 9).is_err());
    // A 12-bit header reads; like Blink, the decode is what fails.
    assert!(jpeg_info(fixture!("12-bit.jpg"), 8).is_ok());
  }
}
