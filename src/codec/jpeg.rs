//! JPEG decoding with Chrome 155's bytes: Blink's JPEGImageDecoder settings on the
//! libjpeg-turbo that Chrome ships (src/ffi/jpeg_blink.c, vendor/libjpeg-turbo).
//!
//! Each call reads the data given so far. `Ok(None)` (or [`JpegDecode::Incomplete`]) means that
//! the data ends before the header (or the image) does: Blink waits for more, and fails the image
//! if no more comes.

use super::{CodecError, Frame};
use crate::ffi::AVPixelFormat;
use crate::ffi::error::check_error_except_eagain;
use crate::ffi::jpeg_blink::{blink_jpeg_decode, blink_jpeg_info};
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
///
/// `allow_yuv: false` gives Bgra where Chrome would give YUV planes. Chrome does that when it
/// decoded rows to RGB before all the data came (a stream that closes after a pause).
pub fn jpeg_info(
  data: &[u8],
  scale_num: u32,
  allow_yuv: bool,
) -> Result<Option<JpegInfo>, CodecError> {
  let mut out = [0 as c_int; 5];
  let read = check_error_except_eagain(unsafe {
    blink_jpeg_info(
      data.as_ptr(),
      data.len(),
      scale_num as c_int,
      allow_yuv.into(),
      out.as_mut_ptr(),
    )
  })?;
  Ok(read.map(|_| JpegInfo {
    width: out[0] as u32,
    height: out[1] as u32,
    output_width: out[2] as u32,
    output_height: out[3] as u32,
    format: AVPixelFormat::from_raw(out[4]),
  }))
}

/// What a decode of the data so far gives.
pub enum JpegDecode {
  /// The whole image
  Frame(Frame),
  /// The data ends before the image does. `rows_started`: Blink got to output rows, so its frame
  /// is partial, no longer empty (JPEGImageDecoder::OutputScanlines). From then on Chrome's
  /// ImageDecoder keeps decoding the image to RGB.
  Incomplete { rows_started: bool },
}

/// Decode a JPEG at `scale_num`/8 of its size, as Chrome does (`allow_yuv`: see [`jpeg_info`]).
/// An error is an image that Chrome fails to decode.
pub fn decode_jpeg(data: &[u8], scale_num: u32, allow_yuv: bool) -> Result<JpegDecode, CodecError> {
  let Some(info) = jpeg_info(data, scale_num, allow_yuv)? else {
    return Ok(JpegDecode::Incomplete {
      rows_started: false,
    });
  };
  // av_frame_get_buffer: an image too large to allocate is an error, not an abort.
  let mut frame = Frame::new_video(info.output_width, info.output_height, info.format)?;
  let mut rows_started: c_int = 0;
  let decoded = check_error_except_eagain(unsafe {
    blink_jpeg_decode(
      data.as_ptr(),
      data.len(),
      scale_num as c_int,
      allow_yuv.into(),
      frame.as_mut_ptr(),
      &mut rows_started,
    )
  })?;
  Ok(match decoded {
    Some(_) => JpegDecode::Frame(frame),
    None => JpegDecode::Incomplete {
      rows_started: rows_started != 0,
    },
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::ffi::{AVColorPrimaries, AVColorRange, AVColorSpace, AVColorTransferCharacteristic};
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
  // with desiredWidth/desiredHeight for the scaled ones (BGRX there: alpha 255). The ones with
  // allow_yuv false come from a ReadableStream that closes 30 ms after its last chunk.
  #[test]
  fn decodes_chrome_bytes() {
    use AVPixelFormat::{Bgra, Yuv420p, Yuv422p, Yuv444p};
    let cases = [
      // 4:2:0 at an odd size: rows past the image during raw output, chroma 19x5
      (
        fixture!("420-37x9.jpg"),
        8,
        true,
        (37, 9),
        Yuv420p,
        "120646bc254fd13efce3ce6b8da37fab63ba1d06164098d2f3842ce1e1025fe8",
      ),
      // streamed: RGB from the first decode on
      (
        fixture!("420-37x9.jpg"),
        8,
        false,
        (37, 9),
        Bgra,
        "634235a6bc72fe6d9a6dc91352b1124bf100d1c0b44cb912207f20af461e51b2",
      ),
      // desired size: M/8 with libjpeg's scaled IDCT, always BGRX
      (
        fixture!("420-37x9.jpg"),
        5,
        true,
        (24, 6),
        Bgra,
        "ac1afec48a889ac8cf1508078866c04e2a69782308549d38042be943e426cd57",
      ),
      (
        fixture!("420-37x9.jpg"),
        1,
        true,
        (5, 2),
        Bgra,
        "5ad43d7526442dc2bc0bdd1c201df15e794d374084e3fb0e055f75310ba37cb1",
      ),
      (
        fixture!("422-15x15.jpg"),
        8,
        true,
        (15, 15),
        Yuv422p,
        "44a8fb19eec3d3b012a44ab06600f5242d840bd1177d2effc1b2fa9b3bede517",
      ),
      (
        fixture!("422-15x15.jpg"),
        8,
        false,
        (15, 15),
        Bgra,
        "065ee0bdba7f0be384ab06ad9d89fabd6cba63f8981ee9812bb3812688917e48",
      ),
      // multi-scan: buffered-image mode
      (
        fixture!("444-progressive-33x17.jpg"),
        8,
        true,
        (33, 17),
        Yuv444p,
        "faead195d1830cd7209a604c8916ee95276b8d5972bf8ab4c06eaf1d2a7f761d",
      ),
      (
        fixture!("444-progressive-33x17.jpg"),
        8,
        false,
        (33, 17),
        Bgra,
        "9aa93940af52ce3d3c37026abbc1dfe33ee2d25115367b5a6cc803ca85afa88b",
      ),
      // 4:4:0 is not a YUV layout Chrome keeps: fancy upsampling to BGRX
      (
        fixture!("440-37x9.jpg"),
        8,
        true,
        (37, 9),
        Bgra,
        "63e19493c9ee853f2862eafeb1d9b79faf5972d2f68704b94e035446d085ec94",
      ),
      // YCCK -> CMYK -> R = C * K / 255
      (
        fixture!("ycck-33x17.jpg"),
        8,
        true,
        (33, 17),
        Bgra,
        "c0f20c6e0b50d6f2771c7194f783f2f89476ce79fd3ae2b8a509d7fea6856a49",
      ),
      // a corrupt-data warning stops the scan reading early (Blink's buffered-image loop)
      (
        fixture!("progressive-junk-in-scan.jpg"),
        8,
        true,
        (64, 48),
        Yuv420p,
        "d6add19cb2298231cfcd4f3edd0c9b5f42b36b672d8f62489f331a630ef4eec0",
      ),
      (
        fixture!("progressive-junk-in-scan.jpg"),
        8,
        false,
        (64, 48),
        Bgra,
        "615276558fa25c8b0ef43c1127f598965db1340b3ae6b1ae4868e2cbd5cbac81",
      ),
      // a 4:4:0 JPEG followed by a 4:2:0 one (like MPO): only the first counts
      (
        fixture!("concatenated-440-420.jpg"),
        8,
        true,
        (64, 48),
        Bgra,
        "e2b941269bd8c218d10ab825d44a1d51fbc46ad57c3c633e7892fd74d0387388",
      ),
    ];
    for (data, scale, allow_yuv, size, format, expected) in cases {
      check_chrome_bytes(data, scale, allow_yuv, size, format, expected);
    }
  }

  // Coefficients that overflow the IDCT (a crafted quantization table and AC values). At
  // M = 8, 4 and 2 Chrome runs libjpeg-turbo's SIMD IDCT, which rounds these otherwise than
  // the C one, and Neon otherwise than SSE2 and AVX2 (which agree), so Chrome's bytes depend
  // on the CPU. The digests come from Chrome on arm64 and on x86_64 (with and without AVX2):
  // they fail when the SIMD code does not run.
  #[cfg(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "arm"))]
  #[test]
  fn decodes_chrome_simd_bytes() {
    use AVPixelFormat::{Bgra, Yuv420p};
    // (data, M, size, format, [Neon digest, x86 digest])
    let cases = [
      // the 8x8 IDCT, YUV planes
      (
        fixture!("idct-overflow-420-17x9.jpg"),
        8,
        (17, 9),
        Yuv420p,
        [
          "b05ce82cd46f642a308e5bf3980f0d8a0544d4f80a4bb139ae24a22c71114df4",
          "045947c818f8273c20067bac8892193e36357ad27d064db9812d60ebaa488d34",
        ],
      ),
      // the 4x4 and 2x2 IDCTs on luma (chroma gets the 8x8 one in place of upsampling), then
      // YCbCr to BGRA
      (
        fixture!("idct-overflow-420-17x9.jpg"),
        4,
        (9, 5),
        Bgra,
        [
          "4f652a9a6a473ea76a09bea005e7a452380e460e2b32b5db1ed530b683581d78",
          "99b7f4fdfedbfa6e442946a387d6b7404bbfc2b928e40ffe086acba1f7346513",
        ],
      ),
      (
        fixture!("idct-overflow-420-17x9.jpg"),
        2,
        (5, 3),
        Bgra,
        [
          "fd2011e376346dc2fb35c075bdae33c8eef80b562e28245eba348206ff9bf652",
          "390dce6990d29bbedc88fedf24e2f357bc2b38e32a77c439da3add89e3a18290",
        ],
      ),
      // 4:4:0: h1v2 upsampling at 8/8 (Neon on Arm, C on x86) and YCbCr to BGRA
      (
        fixture!("idct-overflow-440-17x9.jpg"),
        8,
        (17, 9),
        Bgra,
        [
          "11e9675210370ef354772bf5f89c7456a94a5c3a60cbf584d686ef45b6c15e76",
          "740a4a6ff62357e7449da3ac4fe15fde035a6a4a894389cb1eb083643907200d",
        ],
      ),
      (
        fixture!("idct-overflow-440-17x9.jpg"),
        4,
        (9, 5),
        Bgra,
        [
          "2d54e9ec6820d25afedfe96d8c56ec463799a8f5ebfef0b7ecbcfa7a64689aa7",
          "4ac855b9f29d62fe38ed2affa68482a21d619ed2fe1086745f79f2758a7edff3",
        ],
      ),
      (
        fixture!("idct-overflow-440-17x9.jpg"),
        2,
        (5, 3),
        Bgra,
        [
          "0883f1a6db87a842b9c8bbf95b2147d526f1c13cfb1d9c99867afbf8bb16f6e1",
          "35aef74c2372daf07e2635f5bfe88e5e08f9ad03b114739ed5c6230ca6a9da17",
        ],
      ),
    ];
    for (data, scale, size, format, [neon, x86]) in cases {
      let expected = if cfg!(target_arch = "x86_64") {
        x86
      } else {
        neon
      };
      check_chrome_bytes(data, scale, true, size, format, expected);
    }
  }

  fn check_chrome_bytes(
    data: &[u8],
    scale: u32,
    allow_yuv: bool,
    (width, height): (u32, u32),
    format: AVPixelFormat,
    expected: &str,
  ) {
    let info = jpeg_info(data, scale, allow_yuv).unwrap().unwrap();
    assert_eq!(
      (info.output_width, info.output_height, info.format),
      (width, height, format)
    );
    let Ok(JpegDecode::Frame(frame)) = decode_jpeg(data, scale, allow_yuv) else {
      panic!("{width}x{height} {format:?} at {scale}/8 did not decode");
    };
    assert_eq!(
      (frame.width(), frame.height(), frame.format()),
      (width, height, format)
    );
    assert_eq!(
      digest(&frame),
      expected,
      "{width}x{height} {format:?} at {scale}/8"
    );
    // Chrome's colorSpace: {primaries: bt709, transfer: iec61966-2-1, fullRange: true} and
    // matrix rgb, or smpte170m for YUV
    assert_eq!(frame.color_primaries(), AVColorPrimaries::Bt709);
    assert_eq!(
      frame.color_trc(),
      AVColorTransferCharacteristic::Iec61966_2_1
    );
    assert_eq!(frame.color_range(), AVColorRange::Jpeg);
    let matrix = if format == AVPixelFormat::Bgra {
      AVColorSpace::Rgb
    } else {
      AVColorSpace::Smpte170m
    };
    assert_eq!(frame.colorspace(), matrix);
  }

  // Chrome rejects these with EncodingError once all the data is there: some need more data,
  // which never comes, the others are errors.
  #[test]
  fn fails_like_chrome() {
    for data in [
      fixture!("no-eoi.jpg"),
      fixture!("truncated.jpg"),
      b"\xff\xd8\xff".as_slice(),
      b"".as_slice(),
    ] {
      assert!(matches!(
        decode_jpeg(data, 8, true),
        Ok(JpegDecode::Incomplete { .. })
      ));
    }
    for data in [fixture!("12-bit.jpg"), fixture!("arithmetic.jpg")] {
      assert!(decode_jpeg(data, 8, true).is_err());
    }
  }

  // Blink's frame turns partial once its output loop runs: right after the header for a
  // sequential JPEG, once the first scan has begun for a progressive one. Chrome's ImageDecoder
  // stays with RGB once a decode of part of the data got that far.
  #[test]
  fn rows_start_with_the_first_scan() {
    for (name, data) in [
      ("420-37x9.jpg", fixture!("420-37x9.jpg")),
      (
        "444-progressive-33x17.jpg",
        fixture!("444-progressive-33x17.jpg"),
      ),
    ] {
      let sos = data.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
      let header_end = sos + 2 + usize::from(u16::from_be_bytes([data[sos + 2], data[sos + 3]]));
      for (cut, rows) in [(sos, false), (header_end, true), (data.len() - 2, true)] {
        assert!(
          matches!(
            decode_jpeg(&data[..cut], 8, false),
            Ok(JpegDecode::Incomplete { rows_started }) if rows_started == rows
          ),
          "{name} cut at {cut}"
        );
      }
      assert!(matches!(
        decode_jpeg(data, 8, false),
        Ok(JpegDecode::Frame(_))
      ));
    }
  }

  #[test]
  fn info_reads_the_header() {
    let data = fixture!("420-37x9.jpg");
    let info = jpeg_info(data, 3, true).unwrap().unwrap();
    assert_eq!((info.width, info.height), (37, 9));
    assert_eq!((info.output_width, info.output_height), (14, 4));
    assert!(jpeg_info(data, 0, true).is_err());
    assert!(jpeg_info(data, 9, true).is_err());
    // A 12-bit header reads; like Blink, the decode is what fails.
    assert!(matches!(
      jpeg_info(fixture!("12-bit.jpg"), 8, true),
      Ok(Some(_))
    ));
    // The header ends at the first SOS marker.
    let sos = data.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    assert!(matches!(jpeg_info(&data[..sos + 4], 8, true), Ok(None)));
    assert!(matches!(jpeg_info(&data[..sos + 14], 8, true), Ok(Some(_))));
  }
}
