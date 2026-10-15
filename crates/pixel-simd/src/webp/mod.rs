//! A lossy WebP's YUV planes to BGRA, the way libwebp decodes them for Chrome.
//!
//! The scalar code goes pixel by pixel. The SIMD paths go by row pairs like libwebp b43b2caa (the
//! revision Chromium 155 builds; BSD-3-Clause), whose code they port with the alpha row written
//! in the same pass:
//!
//! - x86_64: src/dsp/upsampling_sse2.c and src/dsp/yuv_sse2.c (what Chrome runs on x64), and the
//!   same SSE2 code widened to AVX2 (libwebp has no AVX2 for this)
//! - aarch64: src/dsp/upsampling_neon.c (what Chrome runs on arm64), 16 chroma samples per block
//!   instead of 8
//! - wasm32: the SSE2 code in simd128

use crate::isa::{Isa, Level};

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  all(target_arch = "wasm32", target_feature = "simd128")
))]
mod rows;
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod wasm;
#[cfg(target_arch = "x86_64")]
mod x86;

/// A plane of samples; row `r` starts at `data[r * stride]`.
#[derive(Clone, Copy, Debug)]
pub struct Plane<'a> {
  pub data: &'a [u8],
  pub stride: usize,
}

impl<'a> Plane<'a> {
  /// Row `r`, to the end of the plane.
  fn row(self, r: usize) -> &'a [u8] {
    &self.data[r * self.stride..]
  }
}

/// A lossy WebP's planes, as libwebp decodes them (FFmpeg's yuv420p or yuva420p): chroma planes
/// of ceil(width / 2) x ceil(height / 2) samples, and alpha, or None for 255.
#[derive(Clone, Copy, Debug)]
pub struct Yuva420<'a> {
  pub width: usize,
  pub height: usize,
  pub y: Plane<'a>,
  pub u: Plane<'a>,
  pub v: Plane<'a>,
  pub a: Option<Plane<'a>>,
}

/// `src` to BGRA rows of `stride` bytes in `dst`, never premultiplied. Each pixel takes libwebp's
/// fancy upsampling of the chroma (upsampling.c UPSAMPLE_FUNC over the row pairs of io_dec.c
/// EmitFancyRGB) and its YUV to RGB math (yuv.h VP8YuvToBgr).
pub fn yuva420_to_bgra(src: &Yuva420, dst: &mut [u8], stride: usize) {
  yuva420_to_bgra_with(Isa::best(), src, dst, stride)
}

pub(crate) fn yuva420_to_bgra_with(isa: Isa, src: &Yuva420, dst: &mut [u8], stride: usize) {
  // SAFETY (every SIMD arm): an `Isa` is only made for a level this CPU runs.
  match isa.level() {
    Level::Scalar => scalar(src, dst, stride),
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => rows::fancy(src, dst, stride, |p, top, bottom| unsafe {
      x86::pair_sse2(p, top, bottom)
    }),
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => rows::fancy(src, dst, stride, |p, top, bottom| unsafe {
      x86::pair_avx2(p, top, bottom)
    }),
    #[cfg(target_arch = "aarch64")]
    Level::Neon => rows::fancy(src, dst, stride, |p, top, bottom| unsafe {
      neon::pair(p, top, bottom)
    }),
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => rows::fancy(src, dst, stride, wasm::pair),
  }
}

fn scalar(src: &Yuva420, dst: &mut [u8], stride: usize) {
  let (width, height) = (src.width, src.height);
  let cols: Vec<_> = (0..width)
    .map(|col| chroma_neighbours(col, width.div_ceil(2)))
    .collect();
  for row in 0..height {
    let line = &mut dst[row * stride..][..width * 4];
    let (near, far) = chroma_neighbours(row, height.div_ceil(2));
    let [u, v] = [src.u, src.v].map(|plane| [plane.row(near), plane.row(far)]);
    let (luma, alpha) = (src.y.row(row), src.a.map(|a| a.row(row)));
    let pixels = line.as_chunks_mut::<4>().0.iter_mut();
    for (col, (px, &cols)) in pixels.zip(&cols).enumerate() {
      let [u, v] = [u, v].map(|rows| upsample(rows, cols));
      *px = bgra(luma[col], u, v, alpha.map_or(255, |alpha| alpha[col]));
    }
  }
}

/// The chroma sample nearest to sample `i` of a 4:2:0 row or column, and the next nearest (on the
/// other side of `i`), out of `count`. At an edge there is none: the nearest is used again.
pub(crate) fn chroma_neighbours(i: usize, count: usize) -> (usize, usize) {
  let near = i / 2;
  let far = if i % 2 == 1 {
    near + 1
  } else {
    near.wrapping_sub(1)
  };
  (near, if far < count { far } else { near })
}

/// libwebp's fancy upsampling: a pixel's chroma from the 4 samples around it, in the nearest chroma
/// row and the next nearest, at the nearest column and the next nearest (`cols`). They are weighted
/// 9:3:3:1 and rounded in libwebp's two steps. At an edge column libwebp weights the nearest column
/// of the two rows 3:1 instead, which gives the same value.
pub(crate) fn upsample([near_row, far_row]: [&[u8]; 2], (near, far): (usize, usize)) -> i32 {
  let nearest = i32::from(near_row[near]);
  let diagonal = (nearest
    + 3 * i32::from(near_row[far])
    + 3 * i32::from(far_row[near])
    + i32::from(far_row[far])
    + 8)
    >> 3;
  (diagonal + nearest) >> 1
}

/// libwebp yuv.h VP8YuvToBgr, and alpha `a`.
fn bgra(y: u8, u: i32, v: i32, a: u8) -> [u8; 4] {
  let y = mult_hi(i32::from(y), 19077);
  [
    clip8(y + mult_hi(u, 33050) - 17685),
    clip8(y - mult_hi(u, 6419) - mult_hi(v, 13320) + 8708),
    clip8(y + mult_hi(v, 26149) - 14234),
    a,
  ]
}

/// libwebp yuv.h MultHi: the fixed-point product, with 6 fraction bits left for `clip8`.
fn mult_hi(v: i32, coeff: i32) -> i32 {
  (v * coeff) >> 8
}

/// libwebp yuv.h VP8Clip8
fn clip8(v: i32) -> u8 {
  (v >> 6).clamp(0, 255) as u8
}
