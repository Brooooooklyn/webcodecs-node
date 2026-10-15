//! Decoded AVIF YUV(A) to 8-bit BGRA, byte for byte as Chrome 155 converts it: CrabbyAvif
//! 3da6b895 rgb::Image::convert_from_yuv (BGRA, automatic chroma upsampling, unpremultiplied
//! output) on the C rows of libyuv 2dd42573, or CrabbyAvif's f32 rgb_impl for the matrices
//! libyuv has no constants for. This picks the rows (`pixel_simd::avif`) and feeds them the
//! frame's planes.

use crate::codec::{CodecError, Frame};
use crate::ffi::accessors::ffpixfmt_describe;
use crate::ffi::error::{AVERROR_EINVAL, AVERROR_ENOSYS};
use crate::ffi::{AVColorRange, AVPixelFormat, FFmpegError};
use pixel_simd::avif::{self, Constants, Matrix, RgbImpl, Transform};

/// A frame that `avif_to_bgra` reads: 8, 10 or 12-bit little-endian planar YUV 4:2:0, 4:2:2 or
/// 4:4:4, gbrp or gray, as AV1 decoders output them. Planes 0, 1, 2 hold Y, U, V (G, B, R for
/// gbrp).
#[derive(Clone, Copy)]
struct Layout {
  depth: u32,
  /// log2 of the chroma subsampling, across and down
  hs: usize,
  vs: usize,
  gray: bool,
}

impl Layout {
  fn of(frame: &Frame) -> Option<Layout> {
    // libavutil 61: yuv420p, yuv422p, yuv444p, gray, yuvj420p, yuvj422p, yuvj444p, yuv420p10le,
    // yuv422p10le, yuv444p10le, gbrp, gbrp10le, yuv420p12le, yuv422p12le, yuv444p12le,
    // gbrp12le, gray12le, gray10le
    const FORMATS: [i32; 18] = [
      0, 4, 5, 8, 12, 13, 14, 62, 64, 68, 71, 75, 123, 127, 131, 135, 166, 168,
    ];
    let format = frame.format_raw();
    let mut d = [0; 5];
    if !FORMATS.contains(&format) || unsafe { ffpixfmt_describe(format, d.as_mut_ptr()) } < 0 {
      return None;
    }
    let layout = Layout {
      depth: d[4] as u32,
      hs: d[2] as usize,
      vs: d[3] as usize,
      gray: d[1] == 1,
    };
    // `row` reads 16-bit samples in place
    let planes = if layout.gray { 1 } else { 3 };
    let aligned = |p| {
      layout.depth == 8 || (frame.data(p) as usize | frame.linesize(p) as usize).is_multiple_of(2)
    };
    (0..planes)
      .all(|p| !frame.data(p).is_null() && aligned(p))
      .then_some(layout)
  }
}

/// Convert a decoded AVIF colour frame and its alpha frame (same size and depth, or None) into
/// `dst`, a BGRA frame of the same size; without alpha the X byte is 255. `cicp` (matrix,
/// primaries, full range) is what Chrome uses (the item's nclx, else the sequence header),
/// `chroma_position` the av1C chroma_sample_position and `premultiplied` the 'prem' link. Fails
/// with AVERROR(ENOSYS) where CrabbyAvif returns not_implemented, which makes Chrome fail the
/// decode, and with AVERROR(EINVAL) for frames it does not read.
pub(crate) fn avif_to_bgra(
  color: &Frame,
  alpha: Option<&Frame>,
  premultiplied: bool,
  (mut matrix, mut primaries, full_range): (i32, i32, bool),
  chroma_position: i32,
  dst: &mut Frame,
) -> Result<(), CodecError> {
  let fail = |code| Err(CodecError::from(FFmpegError::from_code(code)));
  let Some(l) = Layout::of(color) else {
    return fail(AVERROR_EINVAL);
  };
  let (width, height) = (color.width(), color.height());
  let same_size = |f: &Frame| f.width() == width && f.height() == height;
  if width as i32 <= 0
    || height as i32 <= 0
    || dst.format() != AVPixelFormat::Bgra
    || dst.data(0).is_null()
    || !same_size(dst)
    || alpha.is_some_and(|a| !same_size(a) || Layout::of(a).is_none_or(|al| al.depth != l.depth))
  {
    return fail(AVERROR_EINVAL);
  }
  // CrabbyAvif reads unknown matrix and primaries values as unspecified (2).
  if !(0..=17).contains(&matrix) || matrix == 15 {
    matrix = 2;
  }
  if !(0..=22).contains(&primaries) || primaries == 3 || (13..22).contains(&primaries) {
    primaries = 2;
  }
  // convert_from_yuv's not_implemented cases. YCgCo-Re / -Ro must drop exactly 2 / 1 bits.
  let ycgco_r = |bits| !full_range || l.depth - bits != 8;
  if matches!(matrix, 3 | 10 | 11 | 13 | 14)
    || (matrix == 8 && !full_range)
    || (matrix == 16 && ycgco_r(2))
    || (matrix == 17 && ycgco_r(1))
    || (matrix == 0 && l.hs == 1)
  {
    return fail(AVERROR_ENOSYS);
  }
  let alpha_limited = alpha.is_some_and(|a| a.color_range() == AVColorRange::Mpeg);
  // libyuv.rs find_constants (4:0:0 identity counts as BT.601)
  let libyuv_matrix = match (matrix, primaries) {
    (1, _) | (12, 1 | 2) => Some(Matrix::Bt709),
    (2 | 5 | 6, _) | (12, 5 | 6) => Some(Matrix::Bt601),
    (0, _) if l.gray => Some(Matrix::Bt601),
    (9, _) | (12, 9) => Some(Matrix::Bt2020),
    _ => None,
  };
  if let Some(m) = libyuv_matrix {
    let k = Constants::new(m, full_range);
    libyuv(color, alpha, alpha_limited, &k, l, premultiplied, dst);
    return Ok(());
  }
  let transform = match matrix {
    0 => Transform::Identity,
    8 => Transform::YCgCo,
    16 => Transform::YCgCoRe,
    // SMPTE 240M; chroma-derived uses the primaries' (y_coeffs)
    _ => {
      let (kr, kb) = match (matrix, primaries) {
        (4, _) => (0.30, 0.11),
        (12, 4) => (0.299, 0.1146),
        (12, 8) => (0.2536, 0.06808),
        (12, 10) => (0.0, 0.0),
        (12, 11) => (0.2095, 0.0689),
        (12, 12) => (0.229, 0.0793),
        (12, 22) => (0.2318, 0.096),
        (12, 7) | (7, _) => (0.212, 0.087),
        _ => (0.2126, 0.0722),
      };
      Transform::Coefficients { kr, kb }
    }
  };
  // yuv_to_rgb_fast (4:4:4 or 4:0:0 with coefficients, the 8-bit full-range identity copy,
  // which equals the f32 result) leaves 'prem' to ARGBUnattenuate. yuv_to_rgb_any undoes it
  // per pixel in f32, and needs centred chroma to upsample.
  let fast = l.hs == 0
    && match transform {
      Transform::Coefficients { .. } => true,
      Transform::Identity => l.depth == 8 && full_range,
      _ => false,
    };
  if !fast && (l.hs == 1 || l.gray) && chroma_position != 0 {
    return fail(AVERROR_ENOSYS);
  }
  let p = RgbImpl {
    transform,
    depth: l.depth,
    full_range,
    subsampled: l.hs == 1,
    alpha_limited,
    unpremultiply: premultiplied && !fast,
  };
  let unattenuate = alpha.is_some() && premultiplied && fast;
  if l.depth == 8 {
    rgb_impl::<u8>(color, alpha, &p, l, unattenuate, dst);
  } else {
    rgb_impl::<u16>(color, alpha, &p, l, unattenuate, dst);
  }
  Ok(())
}

/// libyuv I4xx(Alpha)ToARGBMatrix(Filter) as CrabbyAvif picks them (find_conversion_function):
/// 10-bit rows (I010 / I210 bilinear, I410), 12-bit 4:2:0 with nearest chroma (I012), and
/// 8-bit rows for the rest, after a truncating shift to 8 bits.
fn libyuv(
  c: &Frame,
  a: Option<&Frame>,
  alpha_limited: bool,
  k: &Constants,
  l: Layout,
  premultiplied: bool,
  dst: &mut Frame,
) {
  let (w, h) = (c.width() as usize, c.height() as usize);
  let cw = (w + l.hs) >> l.hs;
  let rows_depth = match l.depth {
    10 if !l.gray => 10,
    12 if l.vs == 1 => 12,
    _ => 8,
  };
  let shift = l.depth - rows_depth;
  // Alpha goes through the YUVA row (truncated to 8 bits), except with the I400 and I012 rows:
  // then import_alpha_from rescales it.
  let scale = l.depth > 8 && (l.gray || rows_depth == 12);
  let (mut a8, mut y8, mut uv8, mut rows8) = (
    vec![0; w],
    vec![0; w],
    [vec![0; w], vec![0; w]],
    [vec![0; cw], vec![0; cw]],
  );
  let mut uv16 = [vec![0; w], vec![0; w]];
  for j in 0..h {
    let [near, far] = avif::chroma_rows(j, h, l.vs == 1);
    // SAFETY (every row): `Layout::of` checked the planes of `c` and `a`, `avif_to_bgra` the
    // size of `dst`.
    let alpha = match a {
      Some(a) if l.depth == 8 => {
        let src = unsafe { row::<u8>(a, 0, j, w) };
        avif::alpha_to_8bit(src, 8, alpha_limited, false, &mut a8);
        Some(&a8[..])
      }
      Some(a) => {
        let src = unsafe { row::<u16>(a, 0, j, w) };
        avif::alpha_to_8bit(src, l.depth, alpha_limited, scale, &mut a8);
        Some(&a8[..])
      }
      None => None,
    };
    let out = unsafe { row_mut(dst, j, w) };
    match rows_depth {
      10 => {
        let y = unsafe { row(c, 0, j, w) };
        let [ub, vb] = &mut uv16;
        let (u, v) = if l.hs == 0 {
          unsafe { (row(c, 1, near, w), row(c, 2, near, w)) }
        } else {
          for (p, buf) in [(1, &mut *ub), (2, &mut *vb)] {
            let [n, f] = [near, far].map(|r| unsafe { row(c, p, r, cw) });
            avif::upsample16(n, f, 10, buf);
          }
          (&ub[..], &vb[..])
        };
        avif::yuv10_to_bgra(y, u, v, alpha, k, out);
      }
      12 => {
        let [y, u, v] =
          [(0, j, w), (1, near, cw), (2, near, cw)].map(|(p, r, n)| unsafe { row(c, p, r, n) });
        avif::yuv12_to_bgra(y, u, v, alpha, k, out);
      }
      _ => {
        let y = unsafe { load8(c, 0, j, w, shift, &mut y8) };
        if l.gray {
          avif::gray8_to_bgra(y, alpha, k, out);
        } else {
          let [ub, vb] = &mut uv8;
          let (u, v) = if l.hs == 0 {
            unsafe {
              (
                load8(c, 1, near, w, shift, ub),
                load8(c, 2, near, w, shift, vb),
              )
            }
          } else {
            let [nb, fb] = &mut rows8;
            for (p, buf) in [(1, &mut *ub), (2, &mut *vb)] {
              let (n, f) = unsafe {
                (
                  load8(c, p, near, cw, shift, nb),
                  load8(c, p, far, cw, shift, fb),
                )
              };
              avif::upsample8(n, f, buf);
            }
            (&ub[..], &vb[..])
          };
          avif::yuv8_to_bgra(y, u, v, alpha, k, out);
        }
      }
    }
    if a.is_some() && premultiplied {
      avif::unattenuate(out);
    }
  }
}

/// CrabbyAvif rgb_impl in f32, for identity above 8-bit full range, YCgCo, YCgCo-Re, FCC,
/// SMPTE 240M and chroma-derived matrices whose primaries libyuv has no constants for. With
/// `unattenuate`, libyuv ARGBUnattenuate undoes premultiplied alpha after it.
fn rgb_impl<T: Copy + Into<u32>>(
  c: &Frame,
  a: Option<&Frame>,
  p: &RgbImpl,
  l: Layout,
  unattenuate: bool,
  dst: &mut Frame,
) {
  let (w, h) = (c.width() as usize, c.height() as usize);
  let cw = (w + l.hs) >> l.hs;
  for j in 0..h {
    let rows = avif::chroma_rows(j, h, l.vs == 1);
    // SAFETY (every row): `Layout::of` checked the planes of `c` and `a`, `avif_to_bgra` the
    // size of `dst`.
    let y = unsafe { row::<T>(c, 0, j, w) };
    let uv =
      (!l.gray).then(|| [1, 2].map(|plane| rows.map(|r| unsafe { row::<T>(c, plane, r, cw) })));
    let alpha = a.map(|a| unsafe { row::<T>(a, 0, j, w) });
    let out = unsafe { row_mut(dst, j, w) };
    avif::rgb_impl_row(p, y, uv, alpha, out);
    if unattenuate {
      avif::unattenuate(out);
    }
  }
}

/// `n` samples of row `r` of plane `p`.
///
/// # Safety
/// `frame` has plane `p` (as `Layout::of` checks: aligned for `T`) with row `r` and at least `n`
/// samples of `T` in it.
unsafe fn row<T>(frame: &Frame, p: usize, r: usize, n: usize) -> &[T] {
  let start = frame
    .data(p)
    .wrapping_offset(r as isize * frame.linesize(p) as isize);
  unsafe { std::slice::from_raw_parts(start.cast(), n) }
}

/// The `n` BGRA pixels of row `r`.
///
/// # Safety
/// `frame` is a BGRA frame with row `r` and at least `n` pixels in it.
unsafe fn row_mut(frame: &mut Frame, r: usize, n: usize) -> &mut [u8] {
  let start = frame
    .data_mut(0)
    .wrapping_offset(r as isize * frame.linesize(0) as isize);
  unsafe { std::slice::from_raw_parts_mut(start, 4 * n) }
}

/// `n` samples of row `r` of plane `p` in 8 bits: the plane's own, or 16-bit samples shifted
/// down by `shift` into `buf` (Convert16To8).
///
/// # Safety
/// As for `row`, with samples of 8 bits when `shift` is 0, else of 16 bits.
unsafe fn load8<'a>(
  frame: &'a Frame,
  p: usize,
  r: usize,
  n: usize,
  shift: u32,
  buf: &'a mut [u8],
) -> &'a [u8] {
  if shift == 0 {
    return unsafe { row(frame, p, r, n) };
  }
  avif::convert16_to8(unsafe { row(frame, p, r, n) }, shift, &mut buf[..n]);
  &buf[..n]
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::ffi::accessors::{ffframe_set_format, ffframe_set_height, ffframe_set_width};
  use crate::ffi::avutil::{av_frame_get_buffer, av_get_pix_fmt};
  use std::os::raw::c_int;

  const ENOSYS: c_int = AVERROR_ENOSYS;

  /// Decoded planes of a small AVIF (FFmpeg's libdav1d) and Chrome 155's copyTo() bytes
  struct Case {
    /// FFmpeg pixel format, width, height
    color: (&'static str, usize, usize),
    /// Hex samples, row after row
    planes: &'static [&'static str],
    /// Pixel format, hex samples, limited range
    alpha: Option<(&'static str, &'static str, bool)>,
    premultiplied: bool,
    /// Matrix, primaries, full range (the item's nclx)
    cicp: (c_int, c_int, bool),
    chrome: &'static str,
  }

  const CASES: &[Case] = &[
    // 8-bit 4:2:0, premultiplied alpha: libyuv bilinear chroma, ARGBUnattenuate
    Case {
      color: ("yuv420p", 5, 4),
      planes: &[
        "1c1d2022242324272a2b2c2e303335333437393b",
        "8e8c8a908e8b",
        "7d7e8175777a",
      ],
      alpha: Some(("gray", "54565a5e615c5e62666966696d71746d6f73777a", false)),
      premultiplied: true,
      cicp: (6, 1, true),
      chrome: "a24c4954a14d4a569d52525a96545a5e8e546161\
               aa5c4e5ca65f4f5ea26056629e645f6695616669\
               b5714e66b0735069ac73576da87361719f736774\
               ba7c546db97c576fb47c5d73ac7d6377a47e6b7a",
    },
    // 10-bit 4:2:2, limited-range alpha: I210 linear chroma, alpha made full range, >> 2
    Case {
      color: ("yuv422p10le", 5, 3),
      planes: &[
        "f101260124022e0239021c0244014b026702740235024e016102a1025c01",
        "6b0135027202790151029302740161026201",
        "38035f026802290354025d0212034202ce01",
      ],
      alpha: Some((
        "gray10le",
        "4d02570267027602820273027c028c029c02a7028f029802a802b802c302",
        true,
      )),
      premultiplied: false,
      cicp: (6, 1, true),
      chrome: "3951ea991d24a39c896ebda0a974ada5bf72b2a8\
               4b5defa42c2ca7a79d78c2abc482b7b0dc80bdb3\
               4f68ecac2e33a1afa880c1b3b69fb4b8106e44bb",
    },
    // 12-bit 4:2:0: I012 nearest chroma, alpha rescaled in f32
    Case {
      color: ("yuv420p12le", 5, 3),
      planes: &[
        "1a073907380433085608c007d807d204d008fb084f08580846054a098509",
        "7904b40567077904b4056707",
        "ed033f0a3c047803c909c703",
      ],
      alpha: Some((
        "gray12le",
        "de06ef0610073b076a07390753078207c0070208d30fd70fdc0fe40fec0f",
        false,
      )),
      premultiplied: false,
      cicp: (6, 1, true),
      chrome: "0db4146d0eb6166f023774714176b47373b42f76\
               17bf1f7318c020750c407e784b80be7c7ebe3980\
               20cd1efc20cd1efd134d7bfd538dbbfd86cc38fe",
    },
    // 12-bit 4:0:0 without alpha: shifted to 8 bits, I400, X = 255
    Case {
      color: ("gray12le", 5, 3),
      planes: &["04089204b608e708f6086f0819056209ba09ee0983084c05ca09420a7105"],
      alpha: None,
      premultiplied: false,
      cicp: (6, 1, true),
      chrome: "808080ff494949ff8b8b8bff8e8e8eff8f8f8fff\
               868686ff515151ff969696ff9b9b9bff9e9e9eff\
               888888ff545454ff9c9c9cffa4a4a4ff575757ff",
    },
    // SMPTE 240M 4:2:0, premultiplied alpha: f32 rgb_impl, f32 unpremultiply
    Case {
      color: ("yuv420p", 5, 4),
      planes: &[
        "1a1c1f2123212326282a2b2d2f3234323436393b",
        "8c8b888e8c8a",
        "7d7e8175777a",
      ],
      alpha: Some(("gray", "54565a5e615c5e62666966696d71746d6f73777a", false)),
      premultiplied: true,
      cicp: (7, 1, true),
      chrome: "914b4154934f465692544e5a8c55545e86565d61\
               9b5a465c9b5e4b5e9a615262946258668f636069\
               a96f4866a7704d69a270526d9f725a719b736174\
               b17a4c6db07c516faa7b5773a77d5e77a37d657a",
    },
    // 10-bit 4:4:4 YCgCo-Re: f32 rgb_impl
    Case {
      color: ("yuv444p10le", 3, 2),
      planes: &[
        "680068006800680068006800",
        "c301c301c301c501c501c501",
        "f401f401f401f401f401f401",
      ],
      alpha: Some(("gray10le", "8a0191019b0191019801a201", false)),
      premultiplied: false,
      cicp: (16, 1, true),
      chrome: "8d4a81628d4a81648d4a81668c4b80648c4b80668c4b8068",
    },
  ];

  fn frame(format: &str, width: usize, height: usize, planes: &[&str]) -> Frame {
    let name = std::ffi::CString::new(format).unwrap();
    let mut frame = Frame::new().unwrap();
    unsafe {
      ffframe_set_width(frame.as_mut_ptr(), width as c_int);
      ffframe_set_height(frame.as_mut_ptr(), height as c_int);
      ffframe_set_format(frame.as_mut_ptr(), av_get_pix_fmt(name.as_ptr()));
      assert_eq!(av_frame_get_buffer(frame.as_mut_ptr(), 0), 0);
    }
    for (p, hex) in planes.iter().enumerate() {
      let rows = if p > 0 && format.starts_with("yuv420") {
        height.div_ceil(2)
      } else {
        height
      };
      let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
      let stride = frame.linesize(p) as usize;
      for (y, row) in bytes.chunks(bytes.len() / rows).enumerate() {
        unsafe {
          let dst = frame.data_mut(p).add(y * stride);
          std::ptr::copy_nonoverlapping(row.as_ptr(), dst, row.len());
        }
      }
    }
    frame
  }

  /// The FFmpeg error code of an error
  fn code(error: CodecError) -> c_int {
    match error {
      CodecError::Ffmpeg(error) => error.code,
      error => panic!("{error}"),
    }
  }

  /// BGRA rows as hex, or the error code
  fn convert(case: &Case, csp: c_int) -> Result<String, c_int> {
    let (format, width, height) = case.color;
    let color = frame(format, width, height, case.planes);
    let alpha = case.alpha.map(|(format, plane, limited)| {
      let mut alpha = frame(format, width, height, &[plane]);
      alpha.set_color_range(if limited {
        AVColorRange::Mpeg
      } else {
        AVColorRange::Jpeg
      });
      alpha
    });
    let mut dst = Frame::new_video(width as u32, height as u32, AVPixelFormat::Bgra).unwrap();
    avif_to_bgra(
      &color,
      alpha.as_ref(),
      case.premultiplied,
      case.cicp,
      csp,
      &mut dst,
    )
    .map_err(code)?;
    let stride = dst.linesize(0) as usize;
    let data = dst.plane_data(0).unwrap();
    Ok((0..height).fold(String::new(), |hex, y| {
      data[y * stride..y * stride + width * 4]
        .iter()
        .fold(hex, |hex, b| hex + &format!("{b:02x}"))
    }))
  }

  #[test]
  fn avif_to_bgra_gives_chromes_bytes() {
    for case in CASES {
      assert_eq!(
        convert(case, 0).as_deref(),
        Ok(case.chrome),
        "{:?}",
        case.color
      );
    }
  }

  #[test]
  fn avif_to_bgra_fails_where_chrome_does() {
    let [libyuv, .., f32_bilinear, ycgco_re] = CASES else {
      unreachable!()
    };
    // Chroma that is not centred: only the f32 upsampling refuses it
    assert!(convert(libyuv, 1).is_ok());
    assert_eq!(convert(f32_bilinear, 2), Err(ENOSYS));
    for (matrix, case) in [
      (14, libyuv),      // ICtCp
      (0, libyuv),       // identity on 4:2:0
      (17, ycgco_re),    // YCgCo-Ro
      (8, f32_bilinear), // YCgCo, at limited range
    ] {
      let full = matrix != 8;
      let case = Case {
        cicp: (matrix, 1, full),
        ..*case
      };
      assert_eq!(convert(&case, 0), Err(ENOSYS), "matrix {matrix}");
    }
  }
}
