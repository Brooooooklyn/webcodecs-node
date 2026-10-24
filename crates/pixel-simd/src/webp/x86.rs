// libwebp's notice, for the code this file ports from it (the files named below):
// Copyright 2011 Google Inc. All Rights Reserved.
// Copyright 2014 Google Inc. All Rights Reserved.
// Use of this source code is governed by a BSD-style license that can be found in
// licenses/libwebp-COPYING.txt. An additional intellectual property rights grant can be found
// in licenses/libwebp-PATENTS.txt.

//! x86_64: libwebp's src/dsp/upsampling_sse2.c (UPSAMPLE_32PIXELS) and src/dsp/yuv_sse2.c
//! (ConvertYUV444ToRGB_SSE2, PackAndStore4_SSE2), and the same widened to AVX2, 64 pixels per
//! block. Every AVX2 operation works on each byte or 16-bit lane like its SSE2 one; only the
//! interleaving needs a cross-lane permute.

use core::arch::x86_64::*;

use super::rows::{Pair, row_pair};
use crate::x86::{load, store};

/// The first 8 bytes of `src`, in the low half.
#[inline]
#[target_feature(enable = "sse2")]
fn load8(src: &[u8]) -> __m128i {
  let src = &src[..8];
  // SAFETY: 8 bytes; loadl takes any alignment.
  unsafe { _mm_loadl_epi64(src.as_ptr().cast()) }
}

/// GET_M: (k + in + 1) / 2 - ((ij & (s ^ t)) | (k ^ in)) & 1
#[inline]
#[target_feature(enable = "sse2")]
fn get_m(k: __m128i, ij: __m128i, inp: __m128i, st: __m128i) -> __m128i {
  let fix = _mm_or_si128(_mm_and_si128(ij, st), _mm_xor_si128(k, inp));
  _mm_sub_epi8(_mm_avg_epu8(k, inp), _mm_and_si128(fix, _mm_set1_epi8(1)))
}

/// PACK_AND_STORE: 32 samples, (9a + 3b + 3c + d + 8) / 16 and (3a + 9b + c + 3d + 8) / 16
/// interleaved.
#[inline]
#[target_feature(enable = "sse2")]
fn pack_and_store(a: __m128i, b: __m128i, da: __m128i, db: __m128i, out: &mut [u8]) {
  let (ta, tb) = (_mm_avg_epu8(a, da), _mm_avg_epu8(b, db));
  store(out, _mm_unpacklo_epi8(ta, tb));
  store(&mut out[16..], _mm_unpackhi_epi8(ta, tb));
}

/// UPSAMPLE_32PIXELS: 17 samples of the chroma row above (`r1`) and below (`r2`) to 32 for the
/// top row in out[..32] and 32 for the bottom row in out[32..].
#[inline]
#[target_feature(enable = "sse2")]
fn upsample32(r1: &[u8], r2: &[u8], out: &mut [u8; 64]) {
  let (a, b, c, d) = (load(r1), load(&r1[1..]), load(r2), load(&r2[1..]));
  let (s, t) = (_mm_avg_epu8(a, d), _mm_avg_epu8(b, c));
  let (st, ad, bc) = (
    _mm_xor_si128(s, t),
    _mm_xor_si128(a, d),
    _mm_xor_si128(b, c),
  );
  let fix = _mm_and_si128(_mm_or_si128(_mm_or_si128(ad, bc), st), _mm_set1_epi8(1));
  let k = _mm_sub_epi8(_mm_avg_epu8(s, t), fix); // (a + b + c + d) / 4
  let diag1 = get_m(k, bc, t, st); // (a + 3b + 3c + d) / 8
  let diag2 = get_m(k, ad, s, st); // (3a + b + c + 3d) / 8
  pack_and_store(a, b, diag1, diag2, &mut out[..32]);
  pack_and_store(c, d, diag2, diag1, &mut out[32..]);
}

/// 8 pixels: y, u and v from the first 8 bytes of each (and alpha) to 32 bytes of BGRA
/// (ConvertYUV444ToRGB_SSE2 and PackAndStore4_SSE2).
#[inline]
#[target_feature(enable = "sse2")]
fn convert8(y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, dst: &mut [u8]) {
  // the samples << 8, so that mulhi gives MultHi
  let high = |s| _mm_unpacklo_epi8(_mm_setzero_si128(), load8(s));
  let (y, u, v) = (high(y), high(u), high(v));
  let y1 = _mm_mulhi_epu16(y, _mm_set1_epi16(19077));
  let r = _mm_add_epi16(
    _mm_sub_epi16(y1, _mm_set1_epi16(14234)),
    _mm_mulhi_epu16(v, _mm_set1_epi16(26149)),
  );
  let g = _mm_sub_epi16(
    _mm_add_epi16(y1, _mm_set1_epi16(8708)),
    _mm_add_epi16(
      _mm_mulhi_epu16(u, _mm_set1_epi16(6419)),
      _mm_mulhi_epu16(v, _mm_set1_epi16(13320)),
    ),
  );
  let b = _mm_mulhi_epu16(u, _mm_set1_epi16(33050u16 as i16)); // unsigned arithmetic only
  let b = _mm_subs_epu16(_mm_adds_epu16(b, y1), _mm_set1_epi16(17685));
  let (r, g, b) = (
    _mm_srai_epi16::<6>(r),
    _mm_srai_epi16::<6>(g),
    _mm_srli_epi16::<6>(b),
  );
  let a = match a {
    Some(a) => _mm_unpacklo_epi8(load8(a), _mm_setzero_si128()),
    None => _mm_set1_epi16(255),
  };
  let (br, ga) = (_mm_packus_epi16(b, r), _mm_packus_epi16(g, a));
  let (bg, ra) = (_mm_unpacklo_epi8(br, ga), _mm_unpackhi_epi8(br, ga));
  store(dst, _mm_unpacklo_epi16(bg, ra));
  store(&mut dst[16..], _mm_unpackhi_epi16(bg, ra));
}

/// 32 pixels of each row of a pair.
#[inline]
#[target_feature(enable = "sse2")]
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

#[target_feature(enable = "sse2")]
pub(super) fn pair_sse2(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  row_pair(p, top, bottom, 32, |p, top, bottom| block32(p, top, bottom));
}

/// The first 32 bytes of `src`.
#[inline]
#[target_feature(enable = "avx2")]
fn load32(src: &[u8]) -> __m256i {
  let src = &src[..32];
  // SAFETY: 32 bytes; loadu takes any alignment.
  unsafe { _mm256_loadu_si256(src.as_ptr().cast()) }
}

/// `v` to the first 32 bytes of `dst`.
#[inline]
#[target_feature(enable = "avx2")]
fn store32(dst: &mut [u8], v: __m256i) {
  let dst = &mut dst[..32];
  // SAFETY: 32 bytes; storeu takes any alignment.
  unsafe { _mm256_storeu_si256(dst.as_mut_ptr().cast(), v) }
}

/// `get_m` on 32 bytes.
#[inline]
#[target_feature(enable = "avx2")]
fn get_m_avx2(k: __m256i, ij: __m256i, inp: __m256i, st: __m256i) -> __m256i {
  let fix = _mm256_or_si256(_mm256_and_si256(ij, st), _mm256_xor_si256(k, inp));
  _mm256_sub_epi8(
    _mm256_avg_epu8(k, inp),
    _mm256_and_si256(fix, _mm256_set1_epi8(1)),
  )
}

/// `pack_and_store` on 64 samples.
#[inline]
#[target_feature(enable = "avx2")]
fn pack_and_store_avx2(a: __m256i, b: __m256i, da: __m256i, db: __m256i, out: &mut [u8]) {
  let (ta, tb) = (_mm256_avg_epu8(a, da), _mm256_avg_epu8(b, db));
  // unpack works in each 128-bit half: samples 0..8 and 16..24, then 8..16 and 24..32
  let (lo, hi) = (_mm256_unpacklo_epi8(ta, tb), _mm256_unpackhi_epi8(ta, tb));
  store32(out, _mm256_permute2x128_si256::<0x20>(lo, hi));
  store32(&mut out[32..], _mm256_permute2x128_si256::<0x31>(lo, hi));
}

/// `upsample32` on 33 samples of each chroma row: 64 for the top row in out[..64] and 64 for
/// the bottom row in out[64..].
#[inline]
#[target_feature(enable = "avx2")]
fn upsample64(r1: &[u8], r2: &[u8], out: &mut [u8; 128]) {
  let (a, b, c, d) = (load32(r1), load32(&r1[1..]), load32(r2), load32(&r2[1..]));
  let (s, t) = (_mm256_avg_epu8(a, d), _mm256_avg_epu8(b, c));
  let (st, ad, bc) = (
    _mm256_xor_si256(s, t),
    _mm256_xor_si256(a, d),
    _mm256_xor_si256(b, c),
  );
  let fix = _mm256_and_si256(
    _mm256_or_si256(_mm256_or_si256(ad, bc), st),
    _mm256_set1_epi8(1),
  );
  let k = _mm256_sub_epi8(_mm256_avg_epu8(s, t), fix);
  let diag1 = get_m_avx2(k, bc, t, st);
  let diag2 = get_m_avx2(k, ad, s, st);
  pack_and_store_avx2(a, b, diag1, diag2, &mut out[..64]);
  pack_and_store_avx2(c, d, diag2, diag1, &mut out[64..]);
}

/// `convert8` on 16 pixels: 64 bytes of BGRA.
#[inline]
#[target_feature(enable = "avx2")]
fn convert16(y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, dst: &mut [u8]) {
  let high = |s| _mm256_slli_epi16::<8>(_mm256_cvtepu8_epi16(load(s)));
  let (y, u, v) = (high(y), high(u), high(v));
  let y1 = _mm256_mulhi_epu16(y, _mm256_set1_epi16(19077));
  let r = _mm256_add_epi16(
    _mm256_sub_epi16(y1, _mm256_set1_epi16(14234)),
    _mm256_mulhi_epu16(v, _mm256_set1_epi16(26149)),
  );
  let g = _mm256_sub_epi16(
    _mm256_add_epi16(y1, _mm256_set1_epi16(8708)),
    _mm256_add_epi16(
      _mm256_mulhi_epu16(u, _mm256_set1_epi16(6419)),
      _mm256_mulhi_epu16(v, _mm256_set1_epi16(13320)),
    ),
  );
  let b = _mm256_mulhi_epu16(u, _mm256_set1_epi16(33050u16 as i16));
  let b = _mm256_subs_epu16(_mm256_adds_epu16(b, y1), _mm256_set1_epi16(17685));
  let (r, g, b) = (
    _mm256_srai_epi16::<6>(r),
    _mm256_srai_epi16::<6>(g),
    _mm256_srli_epi16::<6>(b),
  );
  let a = match a {
    Some(a) => _mm256_cvtepu8_epi16(load(a)),
    None => _mm256_set1_epi16(255),
  };
  let (br, ga) = (_mm256_packus_epi16(b, r), _mm256_packus_epi16(g, a));
  let (bg, ra) = (_mm256_unpacklo_epi8(br, ga), _mm256_unpackhi_epi8(br, ga));
  // pixels 0..4 and 8..12, then 4..8 and 12..16
  let (lo, hi) = (_mm256_unpacklo_epi16(bg, ra), _mm256_unpackhi_epi16(bg, ra));
  store32(dst, _mm256_permute2x128_si256::<0x20>(lo, hi));
  store32(&mut dst[32..], _mm256_permute2x128_si256::<0x31>(lo, hi));
}

/// 64 pixels of each row of a pair.
#[inline]
#[target_feature(enable = "avx2")]
fn block64(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  let (mut u, mut v) = ([0; 128], [0; 128]);
  upsample64(p.top_u, p.cur_u, &mut u);
  upsample64(p.top_v, p.cur_v, &mut v);
  let convert64 = |y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, dst: &mut [u8]| {
    for i in (0..64).step_by(16) {
      convert16(
        &y[i..],
        &u[i..],
        &v[i..],
        a.map(|a| &a[i..]),
        &mut dst[4 * i..],
      );
    }
  };
  convert64(p.top_y, &u[..64], &v[..64], p.top_a, top);
  if let (Some(y), Some(bottom)) = (p.bot_y, bottom) {
    convert64(y, &u[64..], &v[64..], p.bot_a, bottom);
  }
}

#[target_feature(enable = "avx2")]
pub(super) fn pair_avx2(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  row_pair(p, top, bottom, 64, |p, top, bottom| block64(p, top, bottom));
}
