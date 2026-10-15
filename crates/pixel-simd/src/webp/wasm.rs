//! wasm32 simd128: the SSE2 code of `x86` (libwebp has no wasm code; its Emscripten build runs
//! the SSE2 files). simd128 has no 16-bit unsigned multiply-high, so MultHi of a sample x <= 255
//! is x * (c >> 8) + ((x * (c & 255)) >> 8), every product under 2^16.

use core::arch::wasm32::*;

use super::rows::{Pair, row_pair};
use crate::wasm::{load, store};

/// GET_M: (k + in + 1) / 2 - ((ij & (s ^ t)) | (k ^ in)) & 1
#[inline]
fn get_m(k: v128, ij: v128, inp: v128, st: v128) -> v128 {
  let fix = v128_or(v128_and(ij, st), v128_xor(k, inp));
  i8x16_sub(u8x16_avgr(k, inp), v128_and(fix, u8x16_splat(1)))
}

/// PACK_AND_STORE: 32 samples, (9a + 3b + 3c + d + 8) / 16 and (3a + 9b + c + 3d + 8) / 16
/// interleaved.
#[inline]
fn pack_and_store(a: v128, b: v128, da: v128, db: v128, out: &mut [u8]) {
  let (ta, tb) = (u8x16_avgr(a, da), u8x16_avgr(b, db));
  store(
    out,
    i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(ta, tb),
  );
  store(
    &mut out[16..],
    i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(ta, tb),
  );
}

/// UPSAMPLE_32PIXELS: 17 samples of the chroma row above (`r1`) and below (`r2`) to 32 for the
/// top row in out[..32] and 32 for the bottom row in out[32..].
#[inline]
pub(super) fn upsample32(r1: &[u8], r2: &[u8], out: &mut [u8; 64]) {
  let (a, b, c, d) = (load(r1), load(&r1[1..]), load(r2), load(&r2[1..]));
  let (s, t) = (u8x16_avgr(a, d), u8x16_avgr(b, c));
  let (st, ad, bc) = (v128_xor(s, t), v128_xor(a, d), v128_xor(b, c));
  let fix = v128_and(v128_or(v128_or(ad, bc), st), u8x16_splat(1));
  let k = i8x16_sub(u8x16_avgr(s, t), fix); // (a + b + c + d) / 4
  let diag1 = get_m(k, bc, t, st); // (a + 3b + 3c + d) / 8
  let diag2 = get_m(k, ad, s, st); // (3a + b + c + 3d) / 8
  pack_and_store(a, b, diag1, diag2, &mut out[..32]);
  pack_and_store(c, d, diag2, diag1, &mut out[32..]);
}

/// The first 8 bytes of `src`, each widened to 16 bits.
#[inline]
fn widen(src: &[u8]) -> v128 {
  let src = &src[..8];
  // SAFETY: 8 bytes; the load takes any alignment.
  unsafe { u16x8_load_extend_u8x8(src.as_ptr()) }
}

/// MultHi(x, C) of 8 samples x <= 255.
#[inline]
fn mult_hi<const C: u16>(x: v128) -> v128 {
  let high = i16x8_mul(x, u16x8_splat(C >> 8));
  i16x8_add(high, u16x8_shr(i16x8_mul(x, u16x8_splat(C & 255)), 8))
}

/// 8 pixels: y, u and v from the first 8 bytes of each (and alpha) to 32 bytes of BGRA.
#[inline]
fn convert8(y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, dst: &mut [u8]) {
  let (y, u, v) = (widen(y), widen(u), widen(v));
  let y1 = mult_hi::<19077>(y);
  let r = i16x8_add(i16x8_sub(y1, i16x8_splat(14234)), mult_hi::<26149>(v));
  let g = i16x8_sub(
    i16x8_add(y1, i16x8_splat(8708)),
    i16x8_add(mult_hi::<6419>(u), mult_hi::<13320>(v)),
  );
  let b = u16x8_sub_sat(u16x8_add_sat(mult_hi::<33050>(u), y1), u16x8_splat(17685));
  let (r, g, b) = (i16x8_shr(r, 6), i16x8_shr(g, 6), u16x8_shr(b, 6));
  let a = a.map_or(u16x8_splat(255), widen);
  let (br, ga) = (u8x16_narrow_i16x8(b, r), u8x16_narrow_i16x8(g, a));
  let bg = i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(br, ga);
  let ra = i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(br, ga);
  store(dst, i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(bg, ra));
  store(
    &mut dst[16..],
    i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(bg, ra),
  );
}

/// 32 pixels of each row of a pair.
fn block32(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  let (mut u, mut v) = ([0; 64], [0; 64]);
  upsample32(p.top_u, p.cur_u, &mut u);
  upsample32(p.top_v, p.cur_v, &mut v);
  let convert32 = |y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, dst: &mut [u8]| {
    for i in (0..32).step_by(8) {
      convert8(
        &y[i..],
        &u[i..],
        &v[i..],
        a.map(|a| &a[i..]),
        &mut dst[4 * i..],
      );
    }
  };
  convert32(p.top_y, &u[..32], &v[..32], p.top_a, top);
  if let (Some(y), Some(bottom)) = (p.bot_y, bottom) {
    convert32(y, &u[32..], &v[32..], p.bot_a, bottom);
  }
}

pub(super) fn pair(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  row_pair(p, top, bottom, 32, block32);
}
