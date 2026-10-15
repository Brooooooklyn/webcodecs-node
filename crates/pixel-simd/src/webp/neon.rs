// libwebp's notice, for the code this file ports from it (the files named below):
// Copyright 2011 Google Inc. All Rights Reserved.
// Use of this source code is governed by a BSD-style license that can be found in
// licenses/libwebp-COPYING.txt. An additional intellectual property rights grant can be found
// in licenses/libwebp-PATENTS.txt.

//! aarch64: libwebp's src/dsp/upsampling_neon.c (UPSAMPLE_16PIXELS, CONVERT8, STORE_Bgra) on q
//! registers, 16 chroma samples per block instead of 8, each lane computed as upstream.

use core::arch::aarch64::*;

use super::rows::{Pair, row_pair};
use crate::neon::load;

/// `a` and `b` interleaved to the first 32 bytes of `dst`.
#[inline]
#[target_feature(enable = "neon")]
fn store2(dst: &mut [u8], a: uint8x16_t, b: uint8x16_t) {
  let dst = &mut dst[..32];
  // SAFETY: 32 bytes; vst2q takes any alignment.
  unsafe { vst2q_u8(dst.as_mut_ptr(), uint8x16x2_t(a, b)) }
}

/// UPSAMPLE_16PIXELS on 16 samples: 17 samples of the chroma row above (`r1`) and below (`r2`) to
/// 32 for the top row in out[..32] and 32 for the bottom row in out[32..].
#[inline]
#[target_feature(enable = "neon")]
fn upsample32(r1: &[u8], r2: &[u8], out: &mut [u8; 64]) {
  let (a, b, c, d) = (load(r1), load(&r1[1..]), load(r2), load(&r2[1..]));
  let (ad_l, ad_h) = (
    vaddl_u8(vget_low_u8(a), vget_low_u8(d)),
    vaddl_high_u8(a, d),
  );
  let (bc_l, bc_h) = (
    vaddl_u8(vget_low_u8(b), vget_low_u8(c)),
    vaddl_high_u8(b, c),
  );
  let (abcd_l, abcd_h) = (vaddq_u16(ad_l, bc_l), vaddq_u16(ad_h, bc_h));
  // 3a + b + c + 3d and a + 3b + 3c + d, / 8
  let al = |abcd, ad| vaddq_u16(abcd, vshlq_n_u16::<1>(ad));
  let diag2 = vshrn_high_n_u16::<3>(vshrn_n_u16::<3>(al(abcd_l, ad_l)), al(abcd_h, ad_h));
  let diag1 = vshrn_high_n_u16::<3>(vshrn_n_u16::<3>(al(abcd_l, bc_l)), al(abcd_h, bc_h));
  store2(&mut out[..32], vrhaddq_u8(a, diag1), vrhaddq_u8(b, diag2));
  store2(&mut out[32..], vrhaddq_u8(c, diag2), vrhaddq_u8(d, diag1));
}

/// CONVERT8 before the final shift, on 8 samples << 7: vqdmulh then gives MultHi. 33050 does
/// not fit, so B takes u * 282 and adds u << 7 (u * 32768 >> 8).
#[inline]
#[target_feature(enable = "neon")]
fn rgb(y: uint16x8_t, u: uint16x8_t, v: uint16x8_t) -> [int16x8_t; 3] {
  let (y, u, v) = (
    vreinterpretq_s16_u16(y),
    vreinterpretq_s16_u16(u),
    vreinterpretq_s16_u16(v),
  );
  let y1 = vqdmulhq_n_s16(y, 19077);
  let r = vqaddq_s16(
    vqdmulhq_n_s16(v, 26149),
    vqaddq_s16(y1, vdupq_n_s16(-14234)),
  );
  let g = vqsubq_s16(
    vqaddq_s16(y1, vdupq_n_s16(8708)),
    vqaddq_s16(vqdmulhq_n_s16(u, 6419), vqdmulhq_n_s16(v, 13320)),
  );
  let b = vqaddq_s16(
    vqaddq_s16(vqdmulhq_n_s16(u, 282), vqaddq_s16(y1, vdupq_n_s16(-17685))),
    u,
  );
  [r, g, b]
}

/// 16 pixels: y, u and v from the first 16 bytes of each (and alpha) to 64 bytes of BGRA.
#[inline]
#[target_feature(enable = "neon")]
fn convert16(y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, dst: &mut [u8]) {
  let (y, u, v) = (load(y), load(u), load(v));
  let low = rgb(
    vshll_n_u8::<7>(vget_low_u8(y)),
    vshll_n_u8::<7>(vget_low_u8(u)),
    vshll_n_u8::<7>(vget_low_u8(v)),
  );
  let high = rgb(
    vshll_high_n_u8::<7>(y),
    vshll_high_n_u8::<7>(u),
    vshll_high_n_u8::<7>(v),
  );
  let [r, g, b] = [0, 1, 2].map(|i| vqshrun_high_n_s16::<6>(vqshrun_n_s16::<6>(low[i]), high[i]));
  let a = match a {
    Some(a) => load(a),
    None => vdupq_n_u8(255),
  };
  let dst = &mut dst[..64];
  // SAFETY: 64 bytes; vst4q takes any alignment.
  unsafe { vst4q_u8(dst.as_mut_ptr(), uint8x16x4_t(b, g, r, a)) }
}

/// 32 pixels of each row of a pair.
#[inline]
#[target_feature(enable = "neon")]
fn block32(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  let (mut u, mut v) = ([0; 64], [0; 64]);
  upsample32(p.top_u, p.cur_u, &mut u);
  upsample32(p.top_v, p.cur_v, &mut v);
  let convert32 = |y: &[u8], u: &[u8], v: &[u8], a: Option<&[u8]>, dst: &mut [u8]| {
    convert16(y, u, v, a, dst);
    convert16(
      &y[16..],
      &u[16..],
      &v[16..],
      a.map(|a| &a[16..]),
      &mut dst[64..],
    );
  };
  convert32(p.top_y, &u[..32], &v[..32], p.top_a, top);
  if let (Some(y), Some(bottom)) = (p.bot_y, bottom) {
    convert32(y, &u[32..], &v[32..], p.bot_a, bottom);
  }
}

#[target_feature(enable = "neon")]
pub(super) fn pair(p: &Pair, top: &mut [u8], bottom: Option<&mut [u8]>) {
  row_pair(p, top, bottom, 32, |p, top, bottom| block32(p, top, bottom));
}
