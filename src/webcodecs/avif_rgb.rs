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

/// The C converter this replaces (accessors.c `ffframe_avif_to_rgb`, as of 43f5871) gives the
/// same bytes and errors. Chroma that is upsampled differs only above the bit depth, where the
/// rows clamp it; the frames here keep it in range.
#[cfg(test)]
mod against_c {
  use super::*;
  use crate::ffi::accessors::{
    ffframe_avif_to_rgb, ffframe_set_color_range, ffframe_set_format, ffframe_set_height,
    ffframe_set_width,
  };
  use crate::ffi::avutil::{av_frame_get_buffer, av_get_pix_fmt};
  use std::os::raw::c_int;

  /// splitmix64
  struct Rng(u64);

  impl Rng {
    fn next(&mut self) -> u64 {
      self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
      let mut z = self.0;
      z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
      z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
      z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
      (self.next() % n as u64) as usize
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
      items[self.below(items.len())]
    }
  }

  fn pix_fmt(name: &str) -> c_int {
    let name = std::ffi::CString::new(name).unwrap();
    let raw = unsafe { av_get_pix_fmt(name.as_ptr()) };
    assert!(raw >= 0, "{name:?}");
    raw
  }

  /// (depth, log2 chroma height, planes) of a format, by its descriptor
  fn facts(format: c_int) -> (u32, usize, usize) {
    let mut d = [0; 5];
    assert!(unsafe { ffpixfmt_describe(format, d.as_mut_ptr()) } >= 0);
    (d[4] as u32, d[3] as usize, d[1].max(1) as usize)
  }

  /// A `w` x `h` frame of `format` whose rows are `pad` samples longer than `w` (align 1, so
  /// strides vary), every byte random. 16-bit samples are masked to the bit depth unless the
  /// plane is in `noise` (planes 0..3; bit 3: the whole buffer of an alpha frame).
  fn frame(rng: &mut Rng, format: c_int, w: usize, h: usize, pad: usize, noise: u32) -> Frame {
    let mut frame = Frame::new().unwrap();
    unsafe {
      ffframe_set_width(frame.as_mut_ptr(), (w + pad) as c_int);
      ffframe_set_height(frame.as_mut_ptr(), h as c_int);
      ffframe_set_format(frame.as_mut_ptr(), format);
      assert_eq!(av_frame_get_buffer(frame.as_mut_ptr(), 1), 0, "{format}");
    }
    let (depth, vs, _) = facts(format);
    for p in 0..4 {
      if frame.data(p).is_null() {
        break;
      }
      let rows = if p == 1 || p == 2 { (h + vs) >> vs } else { h };
      let len = frame.linesize(p) as usize * rows;
      let data = unsafe { std::slice::from_raw_parts_mut(frame.data_mut(p), len) };
      data.iter_mut().for_each(|b| *b = rng.next() as u8);
      if depth > 8 && noise & (1 << p) == 0 {
        let mask = ((1u16 << depth) - 1).to_le_bytes();
        for (i, b) in data.iter_mut().enumerate() {
          *b &= mask[i % 2];
        }
      }
    }
    frame.set_dimensions(w as u32, h as u32);
    frame
  }

  struct Run<'a> {
    color: &'a Frame,
    alpha: Option<&'a Frame>,
    premultiplied: bool,
    cicp: (c_int, c_int, bool),
    csp: c_int,
    dst: (c_int, u32, u32),
  }

  /// Both converters on the same input and the same random destination: the same result code,
  /// and the same bytes in the whole destination buffer. Returns the code.
  fn check(rng: &mut Rng, run: &Run, what: &dyn Fn() -> String) -> c_int {
    let (format, w, h) = run.dst;
    let seed = rng.next();
    let mut dsts = [0, 1].map(|_| frame(&mut Rng(seed), format, w as usize, h as usize, 0, 0));
    let (matrix, primaries, full) = run.cicp;
    let c = unsafe {
      ffframe_avif_to_rgb(
        run.color.as_ptr(),
        run.alpha.map_or(std::ptr::null(), Frame::as_ptr),
        run.premultiplied as c_int,
        matrix,
        primaries,
        full as c_int,
        run.csp,
        dsts[0].as_mut_ptr(),
      )
    };
    let rust = avif_to_bgra(
      run.color,
      run.alpha,
      run.premultiplied,
      run.cicp,
      run.csp,
      &mut dsts[1],
    )
    .map_or_else(
      |e| match e {
        CodecError::Ffmpeg(e) => e.code,
        e => panic!("{e}"),
      },
      |()| 0,
    );
    assert_eq!(rust, c, "result: {}", what());
    let len = dsts[0].linesize(0) as usize * h as usize;
    let [a, b] = dsts
      .each_ref()
      .map(|d| unsafe { std::slice::from_raw_parts(d.data(0), len) });
    if a != b {
      let i = a.iter().zip(b).position(|(a, b)| a != b).unwrap();
      let stride = dsts[0].linesize(0) as usize;
      panic!(
        "bytes differ at row {} byte {} (C {} Rust {}): {}",
        i / stride,
        i % stride,
        a[i],
        b[i],
        what()
      );
    }
    c
  }

  const COLOR: [&str; 18] = [
    "yuv420p",
    "yuv422p",
    "yuv444p",
    "gray",
    "yuvj420p",
    "yuvj422p",
    "yuvj444p",
    "yuv420p10le",
    "yuv422p10le",
    "yuv444p10le",
    "gray10le",
    "yuv420p12le",
    "yuv422p12le",
    "yuv444p12le",
    "gray12le",
    "gbrp",
    "gbrp10le",
    "gbrp12le",
  ];
  const REJECTED: [&str; 12] = [
    "nv12",
    "yuva420p",
    "yuv440p",
    "gray9le",
    "yuv420p16le",
    "p010le",
    "rgb24",
    "yuv420p10be",
    "gbrap",
    "yuv411p",
    "gray16le",
    "bgra",
  ];
  const MATRICES: [c_int; 21] = [
    -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 255,
  ];
  const PRIMARIES: [c_int; 27] = [
    -1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 255,
    1000,
  ];

  /// A random conversion of a `w` x `h` frame of `color` with `matrix` and its outcome
  #[allow(clippy::too_many_arguments)]
  fn random(
    rng: &mut Rng,
    color: &str,
    w: usize,
    h: usize,
    matrix: c_int,
    primaries: c_int,
    alpha: usize,
    premultiplied: bool,
    full: bool,
  ) -> c_int {
    let format = pix_fmt(color);
    let (depth, vs, planes) = facts(format);
    let hs_10 = depth == 10 && color.contains("42");
    let noise = rng.next() as u32 & if hs_10 { 1 } else { 7 };
    let pad = rng.below(41);
    let color_frame = frame(rng, format, w, h, pad, noise);
    // alpha: none, full range, limited range, unspecified; sometimes a YUV frame, or one that
    // does not match
    let alpha_format = match (depth, rng.below(8)) {
      (_, 0) => pix_fmt(rng.pick(&REJECTED)),
      (_, 1) => pix_fmt(rng.pick(&COLOR)),
      (8, 2) => pix_fmt("yuv420p"),
      (10, 2) => pix_fmt("yuv444p10le"),
      (12, 2) => pix_fmt("yuv422p12le"),
      (8, _) => pix_fmt("gray"),
      (10, _) => pix_fmt("gray10le"),
      _ => pix_fmt("gray12le"),
    };
    let (aw, ah) = match rng.below(40) {
      0 => (w + 1, h),
      1 => (w, h + 1),
      _ => (w, h),
    };
    let alpha_frame = (alpha > 0).then(|| {
      let (pad, noise) = (rng.below(41), rng.next() as u32 & 1);
      let mut a = frame(rng, alpha_format, aw, ah, pad, noise);
      unsafe { ffframe_set_color_range(a.as_mut_ptr(), [0, 2, 1][alpha - 1]) };
      a
    });
    let dst = match rng.below(60) {
      0 => (pix_fmt("rgba"), w as u32, h as u32),
      1 => (pix_fmt("bgra"), w as u32 + 1, h as u32),
      _ => (pix_fmt("bgra"), w as u32, h as u32),
    };
    let run = Run {
      color: &color_frame,
      alpha: alpha_frame.as_ref(),
      premultiplied,
      cicp: (matrix, primaries, full),
      csp: rng.below(4) as c_int,
      dst,
    };
    let what = || {
      format!(
        "{color} {w}x{h} (planes {planes}, vs {vs}, noise {noise:b}) alpha {alpha} {:?} {aw}x{ah} prem {premultiplied} cicp {:?} csp {} dst {:?}",
        alpha_format, run.cicp, run.csp, run.dst
      )
    };
    check(rng, &run, &what)
  }

  /// Every pixel format FFmpeg has: the same ones are refused
  #[test]
  fn refuses_the_same_formats() {
    let mut rng = Rng(1);
    let mut accepted = vec![];
    for format in -1..300 {
      let mut color = Frame::new().unwrap();
      unsafe {
        ffframe_set_width(color.as_mut_ptr(), 6);
        ffframe_set_height(color.as_mut_ptr(), 5);
        ffframe_set_format(color.as_mut_ptr(), format);
        if av_frame_get_buffer(color.as_mut_ptr(), 0) == 0 {
          // zeros: in range at any depth
          let vs = facts(format).1;
          for p in 0..4 {
            let rows = if p == 1 || p == 2 { (5 + vs) >> vs } else { 5 };
            let len = color.linesize(p) as usize * rows;
            let data = color.data_mut(p);
            if !data.is_null() {
              std::ptr::write_bytes(data, 0, len);
            }
          }
        }
      }
      let run = Run {
        color: &color,
        alpha: None,
        premultiplied: false,
        cicp: (1, 1, true),
        csp: 0,
        dst: (pix_fmt("bgra"), 6, 5),
      };
      if check(&mut rng, &run, &|| format!("format {format}")) == 0 {
        accepted.push(format);
      }
    }
    let names: Vec<c_int> = COLOR.iter().map(|&n| pix_fmt(n)).collect();
    accepted.sort();
    let mut want = names.clone();
    want.sort();
    assert_eq!(accepted, want);
  }

  /// Every size from 1x1 to 67x67, each with a random conversion
  #[test]
  fn sizes() {
    let mut rng = Rng(2);
    for w in 1..=67 {
      for h in 1..=67 {
        let color = if rng.below(30) == 0 {
          rng.pick(&REJECTED)
        } else {
          rng.pick(&COLOR)
        };
        let matrix = rng.pick(&MATRICES);
        let primaries = rng.pick(&PRIMARIES);
        let (alpha, prem, full) = (rng.below(4), rng.below(2) == 0, rng.below(2) == 0);
        if REJECTED.contains(&color) {
          // only the result: the frame has other planes
          continue;
        }
        random(&mut rng, color, w, h, matrix, primaries, alpha, prem, full);
      }
    }
  }

  /// Every format, matrix (and primaries for chroma-derived ones), range, alpha and 'prem', at
  /// random sizes, then wide frames
  #[test]
  fn conversions() {
    let mut rng = Rng(3);
    let (mut ok, mut failed) = (0, 0);
    for color in COLOR {
      for matrix in MATRICES {
        let primaries: &[c_int] = if matrix == 12 { &PRIMARIES } else { &[1, 3] };
        for &primaries in primaries {
          for full in [false, true] {
            for alpha in 0..4 {
              for prem in [false, true] {
                let (w, h) = (1 + rng.below(67), 1 + rng.below(67));
                match random(&mut rng, color, w, h, matrix, primaries, alpha, prem, full) {
                  0 => ok += 1,
                  _ => failed += 1,
                }
              }
            }
          }
        }
      }
    }
    for color in COLOR {
      for w in [4095, 4096, 4097] {
        for h in 1..=3 {
          for _ in 0..4 {
            let matrix = rng.pick(&MATRICES);
            let primaries = rng.pick(&PRIMARIES);
            let (alpha, prem, full) = (rng.below(4), rng.below(2) == 0, rng.below(2) == 0);
            random(&mut rng, color, w, h, matrix, primaries, alpha, prem, full);
          }
        }
      }
    }
    eprintln!("{ok} converted, {failed} refused");
    assert!(ok > failed);
  }

  /// A frame of `format` whose planes hold `sample(plane, x, y)`
  fn filled(
    format: &str,
    w: usize,
    h: usize,
    sample: impl Fn(usize, usize, usize) -> u16,
  ) -> Frame {
    let format = pix_fmt(format);
    let mut frame = frame(&mut Rng(0), format, w, h, 0, 0);
    let (depth, _, planes) = facts(format);
    for p in 0..planes {
      for y in 0..h {
        let row = unsafe {
          frame
            .data_mut(p)
            .offset(y as isize * frame.linesize(p) as isize)
        };
        for x in 0..w {
          let s = sample(p, x, y);
          unsafe {
            if depth > 8 {
              row.cast::<u16>().add(x).write_unaligned(s);
            } else {
              row.add(x).write(s as u8);
            }
          }
        }
      }
    }
    frame
  }

  /// Every CICP that converts, without a not_implemented: (matrix, primaries)
  const CICP: [(c_int, c_int); 20] = [
    (0, 1),
    (1, 1),
    (2, 1),
    (4, 1),
    (5, 1),
    (6, 1),
    (7, 1),
    (8, 1),
    (9, 1),
    (12, 0),
    (12, 1),
    (12, 4),
    (12, 5),
    (12, 7),
    (12, 8),
    (12, 9),
    (12, 10),
    (12, 11),
    (12, 12),
    (12, 22),
  ];

  /// Every 8-bit (y, u, v) in one 4:4:4 frame, through every matrix (libyuv rows and rgb_impl)
  #[test]
  fn every_8bit_yuv() {
    let mut rng = Rng(4);
    let color = filled("yuv444p", 4096, 4096, |p, x, y| {
      ((y * 4096 + x) >> (8 * p)) as u16 & 255
    });
    let alpha = filled("gray", 4096, 4096, |_, x, y| {
      ((x * 7 + y * 13) & 255) as u16
    });
    for (i, (matrix, primaries)) in CICP.into_iter().enumerate() {
      for full in [false, true] {
        if matrix == 8 && !full {
          continue;
        }
        let run = Run {
          color: &color,
          alpha: (i % 2 == 1).then_some(&alpha),
          premultiplied: i % 4 == 3,
          cicp: (matrix, primaries, full),
          csp: 0,
          dst: (pix_fmt("bgra"), 4096, 4096),
        };
        assert_eq!(check(&mut rng, &run, &|| format!("{:?}", run.cicp)), 0);
      }
    }
  }

  /// The 10-bit (y, u, v) with v a multiple of `step` in 4:4:4 frames, through the 6 libyuv
  /// matrices (I410)
  fn ten_bit_yuv(step: usize) {
    let mut rng = Rng(5);
    let alpha = filled("gray10le", 1024, 1024, |_, x, y| {
      ((x * 7 + y * 13) & 1023) as u16
    });
    for v in (0..1024).step_by(step) {
      let color = filled("yuv444p10le", 1024, 1024, |p, x, y| [x, y, v][p] as u16);
      for (i, matrix) in [1, 2, 9].into_iter().enumerate() {
        for full in [false, true] {
          let run = Run {
            color: &color,
            alpha: (v % 2 == 1).then_some(&alpha),
            premultiplied: i == 1,
            cicp: (matrix, 1, full),
            csp: 0,
            dst: (pix_fmt("bgra"), 1024, 1024),
          };
          assert_eq!(
            check(&mut rng, &run, &|| format!("v {v} {:?}", run.cicp)),
            0
          );
        }
      }
    }
  }

  #[test]
  fn some_10bit_yuv() {
    ten_bit_yuv(13);
  }

  /// All 2^30: `cargo test --release --lib every_10bit_yuv -- --ignored`
  #[test]
  #[ignore = "2^30 (y, u, v) x 6 matrices: run with --release -- --ignored"]
  fn every_10bit_yuv() {
    ten_bit_yuv(1);
  }
}
