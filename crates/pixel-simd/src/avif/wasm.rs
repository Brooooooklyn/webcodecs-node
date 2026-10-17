//! wasm32 simd128 rows of `avif` (the SSE2 rows of `x86`; simd128 has no 16-bit multiply-high,
//! so (y16 * YG) >> 16 takes the high halves of 32-bit products). Each does whole vectors from
//! the start and returns how many pixels (sample pairs for the upsampling) it did; the scalar
//! code does the rest.

use core::arch::wasm32::*;

use super::simd::Rgb;
use super::{Constants, RgbImpl, Sample, Samples};
use crate::wasm::{load, store};

/// The first 8 words of `src`.
#[inline]
fn load16(src: &[u16]) -> v128 {
  let src = &src[..8];
  // SAFETY: 16 bytes; v128_load takes any alignment.
  unsafe { v128_load(src.as_ptr().cast()) }
}

/// `v` to the first 8 words of `dst`.
#[inline]
fn store16(dst: &mut [u16], v: v128) {
  let dst = &mut dst[..8];
  // SAFETY: 16 bytes; v128_store takes any alignment.
  unsafe { v128_store(dst.as_mut_ptr().cast(), v) }
}

/// 3v in 16-bit lanes
#[inline]
fn times3(v: v128) -> v128 {
  i16x8_add(i16x8_add(v, v), v)
}

/// `Constants::lanes` in 16-bit lanes
#[derive(Clone, Copy)]
struct K {
  yg: v128,
  ub: v128,
  ug: v128,
  vg: v128,
  vr: v128,
  bb: v128,
  bg: v128,
  br: v128,
}

impl K {
  #[inline]
  fn new(k: &Constants) -> K {
    let [yg, ub, ug, vg, vr, bb, bg, br] = k.lanes().map(|c| u16x8_splat(c));
    K {
      yg,
      ub,
      ug,
      vg,
      vr,
      bb,
      bg,
      br,
    }
  }
}

/// libyuv's math for 8 pixels, luma expanded to 16 bits and 8-bit chroma in 16-bit lanes, to B,
/// G and R in 16-bit lanes: sub_sat clamps at 0, and they are at most 806, which the narrowing
/// clamps to 255.
#[inline]
fn bgr8(y16: v128, u: v128, v: v128, k: K) -> [v128; 3] {
  let (lo, hi) = (
    u32x4_extmul_low_u16x8(y16, k.yg),
    u32x4_extmul_high_u16x8(y16, k.yg),
  );
  // the high halves of the products
  let y1 = i16x8_shuffle::<1, 3, 5, 7, 9, 11, 13, 15>(lo, hi);
  let b = u16x8_sub_sat(i16x8_add(y1, i16x8_mul(u, k.ub)), k.bb);
  let uv = i16x8_add(i16x8_mul(u, k.ug), i16x8_mul(v, k.vg));
  let g = u16x8_sub_sat(i16x8_add(y1, k.bg), uv);
  let r = u16x8_sub_sat(i16x8_add(y1, i16x8_mul(v, k.vr)), k.br);
  [b, g, r].map(|c| u16x8_shr(c, 6))
}

/// 16 pixels, luma expanded to 16 bits (pixels 0..8, then 8..16) and 16 bytes of U, V and A, to
/// the first 64 bytes of `dst`.
#[inline]
fn bgra16(y16: [v128; 2], u: v128, v: v128, a: v128, k: K, dst: &mut [u8]) {
  let lo = bgr8(
    y16[0],
    u16x8_extend_low_u8x16(u),
    u16x8_extend_low_u8x16(v),
    k,
  );
  let hi = bgr8(
    y16[1],
    u16x8_extend_high_u8x16(u),
    u16x8_extend_high_u8x16(v),
    k,
  );
  let [b, g, r] = [0, 1, 2].map(|c| u8x16_narrow_i16x8(lo[c], hi[c]));
  let zip_lo = |x, y| i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(x, y);
  let zip_hi =
    |x, y| i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(x, y);
  for (half, bg, ra) in [
    (0, zip_lo(b, g), zip_lo(r, a)),
    (32, zip_hi(b, g), zip_hi(r, a)),
  ] {
    store(
      &mut dst[half..],
      i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(bg, ra),
    );
    store(
      &mut dst[half + 16..],
      i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(bg, ra),
    );
  }
}

pub(super) fn yuv8(
  y: &[u8],
  u: &[u8],
  v: &[u8],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) -> usize {
  let k = K::new(k);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let yv = load(&y[i..]);
    // y * 0x0101
    let y16 = [u16x8_extend_low_u8x16(yv), u16x8_extend_high_u8x16(yv)]
      .map(|y| v128_or(y, i16x8_shl(y, 8)));
    let a = a.map_or(u8x16_splat(255), |a| load(&a[i..]));
    bgra16(y16, load(&u[i..]), load(&v[i..]), a, k, d);
  }
  16 * blocks.len()
}

pub(super) fn yuv10(
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) -> usize {
  let k = K::new(k);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    // y << 6 | y >> 4, which wraps like the scalar code
    let y16 =
      [load16(&y[i..]), load16(&y[i + 8..])].map(|y| v128_or(i16x8_shl(y, 6), u16x8_shr(y, 4)));
    // min(c >> 2, 255): c >> 2 is a positive i16, which the narrowing clamps
    let c8 = |c: &[u16]| {
      let half = |at: usize| u16x8_shr(load16(&c[at..]), 2);
      u8x16_narrow_i16x8(half(i), half(i + 8))
    };
    let a = a.map_or(u8x16_splat(255), |a| load(&a[i..]));
    bgra16(y16, c8(u), c8(v), a, k, d);
  }
  16 * blocks.len()
}

pub(super) fn yuv12(
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) -> usize {
  let k = K::new(k);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let y16 =
      [load16(&y[i..]), load16(&y[i + 8..])].map(|y| v128_or(i16x8_shl(y, 4), u16x8_shr(y, 8)));
    // min(c >> 4, 255) of 8 samples, each for 2 pixels
    let c8 = |c: &[u16]| {
      let c = u16x8_shr(load16(&c[i / 2..]), 4);
      let c = u8x16_narrow_i16x8(c, c);
      i8x16_shuffle::<0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7>(c, c)
    };
    let a = a.map_or(u8x16_splat(255), |a| load(&a[i..]));
    bgra16(y16, c8(u), c8(v), a, k, d);
  }
  16 * blocks.len()
}

pub(super) fn convert16_to8(src: &[u16], shift: u32, dst: &mut [u8]) -> usize {
  let max = u16x8_splat(255);
  let ((src, _), (dst, _)) = (src.as_chunks::<16>(), dst.as_chunks_mut::<16>());
  for (s, d) in src.iter().zip(&mut *dst) {
    let [lo, hi] = [&s[..8], &s[8..]].map(|s| u16x8_min(u16x8_shr(load16(s), shift), max));
    store(d, u8x16_narrow_i16x8(lo, hi));
  }
  16 * src.len().min(dst.len())
}

/// The sample pairs between the edges of `dst` (see `upsample`), 16 at a time: pair x is
/// samples 2x + 1 and 2x + 2, from samples x and x + 1 of the chroma rows.
pub(super) fn upsample8(near: &[u8], far: &[u8], dst: &mut [u8]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 16;
  if blocks == 0 {
    return 0;
  }
  let eight = u16x8_splat(8);
  // 8 pairs from samples in 16-bit lanes: (3p + q + 8) >> 4 and (p + 3q + 8) >> 4 in the low
  // and high byte of each lane, with p = 3a + c and q = 3b + d (at most 1020)
  let pairs = |a, b, c, d| {
    let (p, q) = (i16x8_add(times3(a), c), i16x8_add(times3(b), d));
    let odd = u16x8_shr(i16x8_add(i16x8_add(times3(p), q), eight), 4);
    let even = u16x8_shr(i16x8_add(i16x8_add(times3(q), p), eight), 4);
    v128_or(odd, i16x8_shl(even, 8))
  };
  // sliced once: block k reads samples 16k..16k + 17 and writes bytes 32k + 1..32k + 33
  let (near, far) = (&near[..16 * blocks + 1], &far[..16 * blocks + 1]);
  let (out, _) = dst[1..].as_chunks_mut::<32>();
  for (x, out) in (0..).step_by(16).zip(out) {
    let (n, f) = (&near[x..][..17], &far[x..][..17]);
    let [a, b, c, d] = [&n[..16], &n[1..], &f[..16], &f[1..]].map(load);
    let lo = |s| u16x8_extend_low_u8x16(s);
    let hi = |s| u16x8_extend_high_u8x16(s);
    store(out, pairs(lo(a), lo(b), lo(c), lo(d)));
    store(&mut out[16..], pairs(hi(a), hi(b), hi(c), hi(d)));
  }
  16 * blocks
}

/// `upsample8` on samples of at most `max` (4095), 8 pairs at a time.
pub(super) fn upsample16(near: &[u16], far: &[u16], max: u16, dst: &mut [u16]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 8;
  let (max, eight) = (u16x8_splat(max), u16x8_splat(8));
  for x in (0..blocks).map(|n| 8 * n) {
    let [a, b, c, d] =
      [&near[x..], &near[x + 1..], &far[x..], &far[x + 1..]].map(|s| u16x8_min(load16(s), max));
    // p = 3a + c and q = 3b + d, at most 16380; 3p + q + 8 at most 65528
    let (p, q) = (i16x8_add(times3(a), c), i16x8_add(times3(b), d));
    let odd = u16x8_shr(i16x8_add(i16x8_add(times3(p), q), eight), 4);
    let even = u16x8_shr(i16x8_add(i16x8_add(times3(q), p), eight), 4);
    store16(
      &mut dst[2 * x + 1..],
      i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(odd, even),
    );
    store16(
      &mut dst[2 * x + 9..],
      i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(odd, even),
    );
  }
  8 * blocks
}

/// 4 samples of `row` from `at`, in 16-bit lanes 0..4.
#[inline]
fn words4<T: Sample>(row: &[T], at: usize) -> v128 {
  match T::samples(row) {
    Samples::U8(row) => {
      let s = &row[at..at + 4];
      // SAFETY: 4 bytes; the load takes any alignment.
      u16x8_extend_low_u8x16(unsafe { v128_load32_zero(s.as_ptr().cast()) })
    }
    Samples::U16(row) => {
      let s = &row[at..at + 4];
      // SAFETY: 8 bytes; the load takes any alignment.
      unsafe { v128_load64_zero(s.as_ptr().cast()) }
    }
  }
}

/// The pixels of `rgb_impl_row` from `start` on (even), 4 at a time and short of the right edge.
/// Each lane does the scalar code's f32 operations in its order (no fused multiply-add), with
/// pmin and pmax, whose NaN rule is the scalar code's.
pub(super) fn rgb_impl<T: Sample>(
  p: &RgbImpl,
  y: &[T],
  uv: Option<[[&[T]; 2]; 2]>,
  a: Option<&[T]>,
  dst: &mut [u8],
  start: usize,
) -> usize {
  let Some(rgb) = p.rgb(a.is_some()) else {
    return start;
  };
  let rg = p.ranges();
  let max = u16x8_splat(rg.max as u16);
  let [maxf, by, ry, bc, rc] = [rg.maxf, rg.by, rg.ry, rg.bc, rg.rc].map(|x| f32x4_splat(x));
  let [zero, half, one, k255] = [0.0, 0.5, 1.0, 255.0].map(|x| f32x4_splat(x));
  let widen = |w| f32x4_convert_u32x4(u32x4_extend_low_u16x8(w));
  // (min(s, max) as f32 - base) / range
  let unorm = |row: &[T], at: usize, base, range| {
    f32x4_div(
      f32x4_sub(widen(u16x8_min(words4(row, at), max)), base),
      range,
    )
  };
  // f32::clamp(0.0, 1.0): c < 0 ? 0 : c, then 1 < c ? 1 : c
  let unit = |c| f32x4_pmin(f32x4_pmax(c, zero), one);
  // (0.5 + c * 255.0) as u32, which gives 0 for NaN
  let q = |c| u32x4_trunc_sat_f32x4(f32x4_add(half, f32x4_mul(c, k255)));
  let w = dst.len() / 4;
  // with subsampled chroma, the last pixel of an even width is an edge
  let span = if p.subsampled { 5 } else { 4 };
  let mut i = start;
  while i + span <= w {
    let yf = unorm(y, i, by, ry);
    let [r, g, b] = match uv {
      None => [yf; 3],
      Some(uv) => {
        let [u, v] = uv.map(|[near, far]| {
          if !p.subsampled {
            return unorm(near, i, bc, rc);
          }
          // pixels i..i + 4 take samples (k + 1, k), (k + 1, k + 2), (k + 2, k + 1),
          // (k + 2, k + 3) of each row: the nearer and the other one, from k = i / 2 - 1
          let (n, f) = (
            unorm(near, i / 2 - 1, bc, rc),
            unorm(far, i / 2 - 1, bc, rc),
          );
          let nearer = |s| i32x4_shuffle::<1, 1, 2, 2>(s, s);
          let other = |s| i32x4_shuffle::<0, 2, 1, 3>(s, s);
          // 9, 3, 3, 1 (/16), summed in the scalar code's order
          let sum = f32x4_add(
            f32x4_mul(nearer(n), f32x4_splat(9.0 / 16.0)),
            f32x4_mul(other(n), f32x4_splat(3.0 / 16.0)),
          );
          let sum = f32x4_add(sum, f32x4_mul(nearer(f), f32x4_splat(3.0 / 16.0)));
          f32x4_add(sum, f32x4_mul(other(f), f32x4_splat(1.0 / 16.0)))
        });
        match rgb {
          Rgb::Identity => [v, yf, u],
          Rgb::YCgCo => {
            let t = f32x4_sub(yf, u);
            [f32x4_add(t, v), f32x4_add(yf, u), f32x4_sub(t, v)]
          }
          Rgb::Matrix { r, b, gr, gb, g } => {
            let uv = f32x4_add(f32x4_mul(v, f32x4_splat(gr)), f32x4_mul(u, f32x4_splat(gb)));
            let g = f32x4_div(f32x4_mul(uv, f32x4_splat(2.0)), f32x4_splat(g));
            [
              f32x4_add(yf, f32x4_mul(v, f32x4_splat(r))),
              f32x4_sub(yf, g),
              f32x4_add(yf, f32x4_mul(u, f32x4_splat(b))),
            ]
          }
        }
      }
    };
    let [mut r, mut g, mut b] = [r, g, b].map(unit);
    let aw = a.map(|a| words4(a, i));
    let alpha = match aw {
      None => u32x4_splat(255),
      // `as u8` keeps the low byte, which the shift by 24 does too
      Some(aw) if p.depth == 8 => u32x4_extend_low_u16x8(aw),
      // scale_alpha
      Some(aw) => u32x4_min(q(f32x4_div(widen(aw), maxf)), u32x4_splat(255)),
    };
    if let Some(aw) = aw
      && p.unpremultiply
    {
      let ac = unit(f32x4_div(widen(u16x8_min(aw, max)), maxf));
      let (clear, part) = (f32x4_eq(ac, zero), f32x4_lt(ac, one));
      // 0 where ac is 0, else min(c / ac, 1) where ac < 1 (pmin(1, x) gives 1 for NaN, as
      // f32::min does)
      let un = |c| {
        let x = f32x4_pmin(one, f32x4_div(c, ac));
        v128_andnot(v128_bitselect(x, c, part), clear)
      };
      [r, g, b] = [r, g, b].map(un);
    }
    let bg = v128_or(q(b), i32x4_shl(q(g), 8));
    let ra = v128_or(i32x4_shl(q(r), 16), i32x4_shl(alpha, 24));
    store(&mut dst[4 * i..], v128_or(bg, ra));
    i += 4;
  }
  i
}
