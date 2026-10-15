//! The rows of a decoded AVIF's YUV(A) to BGRA, the way Chrome 155 converts them: CrabbyAvif
//! 3da6b895 rgb::Image::convert_from_yuv (BGRA, automatic chroma upsampling, unpremultiplied
//! output) on the C rows of libyuv 2dd42573 that its find_conversion_function picks, or
//! CrabbyAvif's f32 rgb_impl for the matrices libyuv has no constants for. Chrome's NEON, SSSE3
//! and AVX2 rows give the same bytes. The caller picks the rows for a frame.
//!
//! Samples deeper than 8 bits come in 16-bit words. One change from libyuv: chroma that is
//! upsampled is clamped to the bit depth first (AV1 decoders never output more).
//!
//! The matrix, upsampling, Convert16To8 and f32 rows have SIMD paths that give the bytes of their
//! scalar code: each does whole vectors from the start and the scalar code does the rest, and the
//! edge pixels of a row. The f32 rows of YCgCo-Re or limited-range alpha, the alpha rows and
//! ARGBUnattenuate have only their scalar code.

use std::ops::Range;

use crate::isa::{Isa, Level};

#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  all(target_arch = "wasm32", target_feature = "simd128")
))]
mod simd;
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod wasm;
#[cfg(target_arch = "x86_64")]
mod x86;

/// The matrices libyuv has constants for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
  Bt601,
  Bt709,
  Bt2020,
}

/// libyuv YuvConstants (row_common.cc, LIBYUV_UNLIMITED_* off): YG, YB, UB, UG, VG, VR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constants([i32; 6]);

/// By matrix, limited range first: I601, JPEG, H709, F709, 2020, V2020.
const CONSTANTS: [[i32; 6]; 6] = [
  [18997, -1160, 128, 25, 52, 102],
  [16320, 32, 113, 22, 46, 90],
  [18997, -1160, 128, 14, 34, 115],
  [16320, 32, 119, 12, 30, 101],
  [19003, -1160, 128, 12, 42, 107],
  [16320, 32, 120, 11, 37, 94],
];

impl Constants {
  pub fn new(matrix: Matrix, full_range: bool) -> Constants {
    Constants(CONSTANTS[2 * matrix as usize + usize::from(full_range)])
  }
}

/// libyuv's integer YUV to RGB of one pixel (the YuvPixel math after its input stage): luma
/// expanded to 16 bits, 8-bit chroma; B, G, R.
#[inline(always)]
fn yuv_pixel(y16: u16, u: u8, v: u8, k: &Constants) -> [u8; 3] {
  let [yg, yb, ub, ug, vg, vr] = k.0;
  let y1 = ((u32::from(y16) * yg as u32) >> 16) as i32;
  let (u, v) = (i32::from(u), i32::from(v));
  [
    clamp6(y1 + u * ub - (ub * 128 - yb)),
    clamp6(y1 + (ug * 128 + vg * 128 + yb) - (u * ug + v * vg)),
    clamp6(y1 + v * vr - (vr * 128 - yb)),
  ]
}

fn clamp6(z: i32) -> u8 {
  if z < 0 { 0 } else { (z >> 6).min(255) as u8 }
}

/// libyuv I444AlphaToARGBRow_C (I444ToARGBRow_C without alpha): 8-bit Y, U and V, one of each
/// per pixel, and alpha (None: 255) to the BGRA pixels of `dst`.
pub fn yuv8_to_bgra(y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, k: &Constants, dst: &mut [u8]) {
  yuv8_to_bgra_with(Isa::best(), y, u, v, a, k, dst)
}

pub(crate) fn yuv8_to_bgra_with(
  isa: Isa,
  y: &[u8],
  u: &[u8],
  v: &[u8],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) {
  // SAFETY (every SIMD arm here and below): an `Isa` is only made for a level this CPU runs.
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::yuv8_sse2(y, u, v, a, k, dst) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::yuv8_avx2(y, u, v, a, k, dst) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::yuv8(y, u, v, a, k, dst) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::yuv8(y, u, v, a, k, dst),
  };
  let a = a.map(|a| &a[done..]);
  let (y, u, v) = (&y[done..], &u[done..], &v[done..]);
  for (i, px) in dst[4 * done..]
    .as_chunks_mut::<4>()
    .0
    .iter_mut()
    .enumerate()
  {
    let [b, g, r] = yuv_pixel(u16::from(y[i]) * 0x0101, u[i], v[i], k);
    *px = [b, g, r, a.map_or(255, |a| a[i])];
  }
}

/// libyuv I410AlphaToARGBRow_C: 10-bit Y, U and V, one of each per pixel.
pub fn yuv10_to_bgra(
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) {
  yuv10_to_bgra_with(Isa::best(), y, u, v, a, k, dst)
}

pub(crate) fn yuv10_to_bgra_with(
  isa: Isa,
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) {
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::yuv10_sse2(y, u, v, a, k, dst) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::yuv10_avx2(y, u, v, a, k, dst) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::yuv10(y, u, v, a, k, dst) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::yuv10(y, u, v, a, k, dst),
  };
  let (y, u, v) = (&y[done..], &u[done..], &v[done..]);
  let dst = &mut dst[4 * done..];
  for (i, px) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
    *px = pixel16(y[i], u[i], v[i], 10, k);
  }
  put_alpha(a.map(|a| &a[done..]), dst);
}

/// libyuv I212ToARGBRow_C as I012ToARGBMatrix runs it: 12-bit Y, and U and V with one sample
/// for each pair of pixels (and the last pixel of an odd width).
pub fn yuv12_to_bgra(
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) {
  yuv12_to_bgra_with(Isa::best(), y, u, v, a, k, dst)
}

pub(crate) fn yuv12_to_bgra_with(
  isa: Isa,
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) {
  // an even count of pixels
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::yuv12_sse2(y, u, v, a, k, dst) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::yuv12_avx2(y, u, v, a, k, dst) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::yuv12(y, u, v, a, k, dst) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::yuv12(y, u, v, a, k, dst),
  };
  let (y, u, v) = (&y[done..], &u[done / 2..], &v[done / 2..]);
  let dst = &mut dst[4 * done..];
  let (pairs, last) = dst.as_chunks_mut::<4>().0.as_chunks_mut::<2>();
  for (c, pair) in pairs.iter_mut().enumerate() {
    pair[0] = pixel16(y[2 * c], u[c], v[c], 12, k);
    pair[1] = pixel16(y[2 * c + 1], u[c], v[c], 12, k);
  }
  let c = pairs.len();
  if let [px] = last {
    *px = pixel16(y[2 * c], u[c], v[c], 12, k);
  }
  put_alpha(a.map(|a| &a[done..]), dst);
}

/// libyuv YuvPixel10 and YuvPixel12: a `depth`-bit pixel, alpha 255. Luma expands in 16 bits
/// (y << 6 | y >> 4 for 10 bits, which wraps above the depth), chroma saturates at 8 bits.
#[inline(always)]
fn pixel16(y: u16, u: u16, v: u16, depth: u32, k: &Constants) -> [u8; 4] {
  let y16 = (u32::from(y) << (16 - depth) | u32::from(y) >> (2 * depth - 16)) as u16;
  let chroma = |c: u16| (c >> (depth - 8)).min(255) as u8;
  let [b, g, r] = yuv_pixel(y16, chroma(u), chroma(v), k);
  [b, g, r, 255]
}

/// libyuv I400ToARGBRow_C: 8-bit gray (U = V = 128) and alpha (None: 255) to BGRA.
pub fn gray8_to_bgra(y: &[u8], a: Option<&[u8]>, k: &Constants, dst: &mut [u8]) {
  gray8_to_bgra_with(Isa::best(), y, a, k, dst)
}

pub(crate) fn gray8_to_bgra_with(
  isa: Isa,
  y: &[u8],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) {
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::gray8_sse2(y, a, k, dst) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::gray8_avx2(y, a, k, dst) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::gray8(y, a, k, dst) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::gray8(y, a, k, dst),
  };
  let (y, a) = (&y[done..], a.map(|a| &a[done..]));
  for (i, px) in dst[4 * done..]
    .as_chunks_mut::<4>()
    .0
    .iter_mut()
    .enumerate()
  {
    let [b, g, r] = yuv_pixel(u16::from(y[i]) * 0x0101, 128, 128, k);
    *px = [b, g, r, a.map_or(255, |a| a[i])];
  }
}

/// The A bytes of the BGRA pixels of `dst` from `a`, over the 255 that `pixel16` stores. The
/// 16-bit rows take alpha in a pass of its own: with the Option in their loops, they do not
/// vectorize.
fn put_alpha(a: Option<&[u8]>, dst: &mut [u8]) {
  if let Some(a) = a {
    for (i, px) in dst.as_chunks_mut::<4>().0.iter_mut().enumerate() {
      px[3] = a[i];
    }
  }
}

/// libyuv Convert16To8Row_C: samples shifted right by `shift` (truncating, below 16), 255 at
/// most.
pub fn convert16_to8(src: &[u16], shift: u32, dst: &mut [u8]) {
  convert16_to8_with(Isa::best(), src, shift, dst)
}

pub(crate) fn convert16_to8_with(isa: Isa, src: &[u16], shift: u32, dst: &mut [u8]) {
  assert!(shift < 16, "convert16_to8: shift {shift}");
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::convert16_to8_sse2(src, shift, dst) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::convert16_to8_avx2(src, shift, dst) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::convert16_to8(src, shift, dst) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::convert16_to8(src, shift, dst),
  };
  let src = &src[done..];
  for (i, d) in dst[done..].iter_mut().enumerate() {
    *d = (src[i] >> shift).min(255) as u8;
  }
}

/// The chroma rows that 4:2:0 output row `row` of `height` (4:2:2 and 4:4:4 when not
/// `subsampled`) takes its chroma from: the nearer one and the other one, which libyuv and
/// rgb_impl weight 3:1. The first row and the last row of an even height use the nearer row
/// alone (twice).
pub fn chroma_rows(row: usize, height: usize, subsampled: bool) -> [usize; 2] {
  if !subsampled {
    return [row; 2];
  }
  [row / 2, far(row, height)]
}

/// The other chroma sample (row or column) for sample `i` of `n`: the nearer one at the edges.
fn far(i: usize, n: usize) -> usize {
  if i == 0 || (i == n - 1 && i % 2 == 1) {
    i / 2
  } else if i % 2 == 1 {
    i / 2 + 1
  } else {
    i / 2 - 1
  }
}

/// libyuv ScaleRowUp2_Bilinear_Any_C for one output row of `dst.len()` samples from the chroma
/// rows of `chroma_rows`: 3:1 between the rows, then 3:1 between the columns, with libyuv's edge
/// pixels. With `far` == `near` it is ScaleRowUp2_Linear_Any_C.
pub fn upsample8(near: &[u8], far: &[u8], dst: &mut [u8]) {
  upsample8_with(Isa::best(), near, far, dst)
}

pub(crate) fn upsample8_with(isa: Isa, near: &[u8], far: &[u8], dst: &mut [u8]) {
  // pairs of output samples
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::upsample8_sse2(near, far, dst) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::upsample8_avx2(near, far, dst) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::upsample8(near, far, dst) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::upsample8(near, far, dst),
  };
  let w = dst.len();
  upsample(
    |x| near[x].into(),
    |x| far[x].into(),
    w,
    done,
    |i, s| dst[i] = s as u8,
  );
}

/// The same for samples of `depth` bits (9 to 12) in 16-bit words; larger samples count as
/// (1 << depth) - 1.
pub fn upsample16(near: &[u16], far: &[u16], depth: u32, dst: &mut [u16]) {
  upsample16_with(Isa::best(), near, far, depth, dst)
}

pub(crate) fn upsample16_with(isa: Isa, near: &[u16], far: &[u16], depth: u32, dst: &mut [u16]) {
  assert!((9..=12).contains(&depth), "upsample16: depth {depth}");
  let max = (1 << depth) - 1;
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::upsample16_sse2(near, far, max as u16, dst) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::upsample16_avx2(near, far, max as u16, dst) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::upsample16(near, far, max as u16, dst) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::upsample16(near, far, max as u16, dst),
  };
  let w = dst.len();
  upsample(
    |x| u32::from(near[x]).min(max),
    |x| u32::from(far[x]).min(max),
    w,
    done,
    |i, s| dst[i] = s as u16,
  );
}

/// The upsampling of `w` samples, the pairs between the edge ones from pair `from` on.
fn upsample(
  near: impl Fn(usize) -> u32,
  far: impl Fn(usize) -> u32,
  w: usize,
  from: usize,
  mut put: impl FnMut(usize, u32),
) {
  if w == 0 {
    return;
  }
  put(0, (3 * near(0) + far(0) + 2) >> 2);
  for x in from..((w - 1) & !1) / 2 {
    let (n0, n1, f0, f1) = (near(x), near(x + 1), far(x), far(x + 1));
    put(2 * x + 1, (n0 * 9 + n1 * 3 + f0 * 3 + f1 + 8) >> 4);
    put(2 * x + 2, (n0 * 3 + n1 * 9 + f0 + f1 * 3 + 8) >> 4);
  }
  let x = (w - 1) / 2;
  put(w - 1, (3 * near(x) + far(x) + 2) >> 2);
}

/// CrabbyAvif's 8-bit alpha from `depth`-bit alpha samples: made full range first when
/// `limited` (alpha_to_full_range), then shifted down to 8 bits (truncating, like libyuv's YUVA
/// rows and Convert16To8), or with `scale` rescaled in f32 (import_alpha_from).
pub fn alpha_to_8bit<T: Copy + Into<u32>>(
  src: &[T],
  depth: u32,
  limited: bool,
  scale: bool,
  dst: &mut [u8],
) {
  for (i, d) in dst.iter_mut().enumerate() {
    let a = alpha(src[i], depth, limited);
    *d = if scale {
      scale_alpha(a, depth)
    } else {
      (a >> (depth - 8)).min(255) as u8
    };
  }
}

/// An alpha sample, made full range when `limited`
fn alpha<T: Into<u32>>(sample: T, depth: u32, limited: bool) -> u32 {
  let a = sample.into();
  if !limited {
    return a;
  }
  let (lo, span, max) = (16 << (depth - 8), 219 << (depth - 8), (1 << depth) - 1);
  (((a as i32 - lo) * max + span / 2) / span).clamp(0, max) as u32
}

/// import_alpha_from: an alpha sample rescaled to 8 bits in f32
fn scale_alpha(a: u32, depth: u32) -> u8 {
  ((0.5 + a as f32 / ((1 << depth) - 1) as f32 * 255.0) as u32).min(255) as u8
}

/// libyuv ARGBUnattenuateRow_C on BGRA pixels, in place. Its fixed_invtbl8 holds 0x10000 / a,
/// except 0 for a = 0, 0xffff for a = 1 and 0x100 for a = 255.
pub fn unattenuate(pixels: &mut [u8]) {
  for px in pixels.as_chunks_mut::<4>().0 {
    let a = u32::from(px[3]);
    let ia = match a {
      0 => 0,
      1 => 0xffff,
      255 => 0x100,
      _ => 0x10000 / a,
    };
    for c in &mut px[..3] {
      *c = ((u32::from(*c) * 0x101 * ia) >> 16).min(255) as u8;
    }
  }
}

/// How CrabbyAvif's rgb_impl makes R, G and B.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transform {
  /// A matrix with these Kr and Kb (Kg = 1 - Kr - Kb).
  Coefficients {
    kr: f32,
    kb: f32,
  },
  /// G, B, R are Y, U, V.
  Identity,
  YCgCo,
  /// YCgCo-Re (H.273 (62)-(65)) on Cg and Co rounded back to integers.
  YCgCoRe,
}

/// A frame's settings for `rgb_impl_row`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RgbImpl {
  pub transform: Transform,
  /// Bits per sample: 8, 10 or 12.
  pub depth: u32,
  pub full_range: bool,
  /// One chroma sample for every 2 pixels of a row (4:2:0, 4:2:2): bilinear, 9:3:3:1.
  pub subsampled: bool,
  /// The alpha samples are limited range.
  pub alpha_limited: bool,
  /// Undo premultiplied alpha in f32 (yuv_to_rgb_any; yuv_to_rgb_fast leaves it to
  /// `unattenuate`).
  pub unpremultiply: bool,
}

/// The numbers of rgb_impl's unorm: the largest sample, as f32, and the offset and range of Y
/// and of U and V.
#[derive(Clone, Copy, Debug)]
struct Ranges {
  max: u32,
  maxf: f32,
  by: f32,
  ry: f32,
  bc: f32,
  rc: f32,
}

impl RgbImpl {
  fn ranges(&self) -> Ranges {
    let depth = self.depth;
    let max = (1u32 << depth) - 1;
    let maxf = max as f32;
    let (by, ry) = if self.full_range {
      (0.0, maxf)
    } else {
      ((16 << (depth - 8)) as f32, (219 << (depth - 8)) as f32)
    };
    let (bc, rc) = match self.transform {
      Transform::Identity => (by, ry),
      _ if self.full_range => ((1 << (depth - 1)) as f32, maxf),
      _ => ((1 << (depth - 1)) as f32, (224 << (depth - 8)) as f32),
    };
    Ranges {
      max,
      maxf,
      by,
      ry,
      bc,
      rc,
    }
  }
}

/// The samples of the rows of `rgb_impl_row`: 8 bits, or deeper in 16-bit words.
pub trait Sample: Copy + Into<u32> {
  /// The row as one of the two widths, for the SIMD paths.
  fn samples(row: &[Self]) -> Samples<'_>;
}

/// A row of `Sample`s.
#[derive(Clone, Copy, Debug)]
pub enum Samples<'a> {
  U8(&'a [u8]),
  U16(&'a [u16]),
}

impl Sample for u8 {
  fn samples(row: &[u8]) -> Samples<'_> {
    Samples::U8(row)
  }
}

impl Sample for u16 {
  fn samples(row: &[u16]) -> Samples<'_> {
    Samples::U16(row)
  }
}

/// CrabbyAvif rgb_impl in f32 for one row of BGRA pixels in `dst`, from the samples of `y`, of
/// the `chroma_rows` of U and V (None for gray: R = G = B = Y) and of alpha (None: 255).
pub fn rgb_impl_row<T: Sample>(
  p: &RgbImpl,
  y: &[T],
  uv: Option<[[&[T]; 2]; 2]>,
  a: Option<&[T]>,
  dst: &mut [u8],
) {
  rgb_impl_row_with(Isa::best(), p, y, uv, a, dst)
}

pub(crate) fn rgb_impl_row_with<T: Sample>(
  isa: Isa,
  p: &RgbImpl,
  y: &[T],
  uv: Option<[[&[T]; 2]; 2]>,
  a: Option<&[T]>,
  dst: &mut [u8],
) {
  assert!(
    matches!(p.depth, 8 | 10 | 12),
    "rgb_impl_row: depth {}",
    p.depth
  );
  let w = dst.len() / 4;
  // The first two pixels of a subsampled row take the scalar code: the first is an edge, and
  // the vectors start at an even pixel.
  let start = if p.subsampled { w.min(2) } else { 0 };
  let done = match isa.level() {
    Level::Scalar => start,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 | Level::Ssse3 => unsafe { x86::rgb_impl_sse2(p, y, uv, a, dst, start) },
    #[cfg(target_arch = "x86_64")]
    Level::Avx2 => unsafe { x86::rgb_impl_avx2(p, y, uv, a, dst, start) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::rgb_impl(p, y, uv, a, dst, start) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::rgb_impl(p, y, uv, a, dst, start),
  };
  rgb_impl(p, y, uv, a, dst, 0..start);
  rgb_impl(p, y, uv, a, dst, done..w);
}

/// `rgb_impl_row` for the pixels in `pixels`.
fn rgb_impl<T: Sample>(
  p: &RgbImpl,
  y: &[T],
  uv: Option<[[&[T]; 2]; 2]>,
  a: Option<&[T]>,
  dst: &mut [u8],
  pixels: Range<usize>,
) {
  let depth = p.depth;
  let Ranges {
    max: mx,
    maxf,
    by,
    ry,
    bc,
    rc,
  } = p.ranges();
  let unorm = |row: &[T], x: usize| (row[x].into().min(mx) as f32 - bc) / rc;
  let unit = |v: f32| v.clamp(0.0, 1.0);
  let w = dst.len() / 4;
  let dst = dst.as_chunks_mut::<4>().0;
  for i in pixels {
    let cy = y[i].into().min(mx);
    let yf = (cy as f32 - by) / ry;
    let (ci, fi) = if p.subsampled {
      (i / 2, far(i, w))
    } else {
      (i, i)
    };
    let [u, v] = uv.map_or([0.5; 2], |uv| {
      uv.map(|[near, far]| {
        let x = unorm(near, ci);
        if !p.subsampled {
          return x;
        }
        // bilinear weights 9, 3, 3, 1 (/16)
        x * (9.0 / 16.0)
          + unorm(near, fi) * (3.0 / 16.0)
          + unorm(far, ci) * (3.0 / 16.0)
          + unorm(far, fi) * (1.0 / 16.0)
      })
    });
    let (mut r, mut g, mut b) = if uv.is_none() {
      (yf, yf, yf)
    } else {
      match p.transform {
        Transform::Identity => (v, yf, u),
        Transform::YCgCo => {
          let t = yf - u;
          (t + v, yf + u, t - v)
        }
        Transform::YCgCoRe => {
          let cg = (0.5 + u * maxf).floor() as i32;
          let co = (0.5 + v * maxf).floor() as i32;
          let t = cy as i32 - (cg >> 1);
          let gi = (t + cg).clamp(0, 255);
          let bi = (t - (co >> 1)).clamp(0, 255);
          let ri = (bi + co).clamp(0, 255);
          (ri as f32 / 255.0, gi as f32 / 255.0, bi as f32 / 255.0)
        }
        Transform::Coefficients { kr, kb } => {
          let kg = 1.0 - kr - kb;
          (
            yf + (2.0 * (1.0 - kr)) * v,
            yf - ((2.0 * ((kr * (1.0 - kr) * v) + (kb * (1.0 - kb) * u))) / kg),
            yf + (2.0 * (1.0 - kb)) * u,
          )
        }
      }
    };
    (r, g, b) = (unit(r), unit(g), unit(b));
    let av = a.map(|a| alpha(a[i], depth, p.alpha_limited));
    if let Some(av) = av
      && p.unpremultiply
    {
      let ac = unit(av.min(mx) as f32 / maxf);
      if ac == 0.0 {
        (r, g, b) = (0.0, 0.0, 0.0);
      } else if ac < 1.0 {
        (r, g, b) = ((r / ac).min(1.0), (g / ac).min(1.0), (b / ac).min(1.0));
      }
    }
    let q = |c: f32| (0.5 + c * 255.0) as u8;
    dst[i] = [
      q(b),
      q(g),
      q(r),
      match av {
        None => 255,
        Some(av) if depth == 8 => av as u8,
        Some(av) => scale_alpha(av, depth),
      },
    ];
  }
}
