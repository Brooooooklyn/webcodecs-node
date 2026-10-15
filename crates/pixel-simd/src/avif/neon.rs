//! aarch64 NEON rows of `avif`. Each does whole vectors from the start and returns how many
//! pixels (sample pairs for the upsampling) it did; the scalar code does the rest.

use core::arch::aarch64::*;

use super::simd::Rgb;
use super::{Constants, RgbImpl, Sample, Samples};
use crate::neon::{load, store};

/// The first 8 words of `src`.
#[inline]
#[target_feature(enable = "neon")]
fn load16(src: &[u16]) -> uint16x8_t {
  let src = &src[..8];
  // SAFETY: 16 bytes; vld1q takes any alignment.
  unsafe { vld1q_u16(src.as_ptr()) }
}

/// `Constants::lanes`, UB to VR in bytes.
struct K {
  yg: uint16x8_t,
  ub: uint8x16_t,
  ug: uint8x16_t,
  vg: uint8x16_t,
  vr: uint8x16_t,
  bb: uint16x8_t,
  bg: uint16x8_t,
  br: uint16x8_t,
}

impl K {
  #[inline]
  #[target_feature(enable = "neon")]
  fn new(k: &Constants) -> K {
    let [yg, ub, ug, vg, vr, bb, bg, br] = k.lanes();
    // UB to VR are below 256
    let byte = |c: u16| vdupq_n_u8(c as u8);
    K {
      yg: vdupq_n_u16(yg),
      ub: byte(ub),
      ug: byte(ug),
      vg: byte(vg),
      vr: byte(vr),
      bb: vdupq_n_u16(bb),
      bg: vdupq_n_u16(bg),
      br: vdupq_n_u16(br),
    }
  }
}

/// libyuv's math for 16 pixels, luma expanded to 16 bits (pixels 0..8, then 8..16) and 8-bit
/// chroma, to B, G and R bytes: vqsub clamps at 0, vqshrn at 255.
#[inline]
#[target_feature(enable = "neon")]
fn bgr16(y16: [uint16x8_t; 2], u: uint8x16_t, v: uint8x16_t, k: &K) -> [uint8x16_t; 3] {
  // (y16 * YG) >> 16: the high halves of the products
  let [y0, y1] = y16.map(|y| {
    let lo = vmull_u16(vget_low_u16(y), vget_low_u16(k.yg));
    let hi = vmull_high_u16(y, k.yg);
    vuzp2q_u16(vreinterpretq_u16_u32(lo), vreinterpretq_u16_u32(hi))
  });
  let (ul, vl) = (vget_low_u8(u), vget_low_u8(v));
  let low = |k: uint8x16_t| vget_low_u8(k);
  let to8 = |lo, hi, bias| {
    vqshrn_high_n_u16::<6>(
      vqshrn_n_u16::<6>(vqsubq_u16(lo, bias)),
      vqsubq_u16(hi, bias),
    )
  };
  let b = to8(
    vmlal_u8(y0, ul, low(k.ub)),
    vmlal_high_u8(y1, u, k.ub),
    k.bb,
  );
  let uv = [
    vmlal_u8(vmull_u8(ul, low(k.ug)), vl, low(k.vg)),
    vmlal_high_u8(vmull_high_u8(u, k.ug), v, k.vg),
  ];
  let g = vqshrn_high_n_u16::<6>(
    vqshrn_n_u16::<6>(vqsubq_u16(vaddq_u16(y0, k.bg), uv[0])),
    vqsubq_u16(vaddq_u16(y1, k.bg), uv[1]),
  );
  let r = to8(
    vmlal_u8(y0, vl, low(k.vr)),
    vmlal_high_u8(y1, v, k.vr),
    k.br,
  );
  [b, g, r]
}

/// 16 pixels to the first 64 bytes of `dst`.
#[inline]
#[target_feature(enable = "neon")]
fn store_bgra(dst: &mut [u8], [b, g, r]: [uint8x16_t; 3], a: uint8x16_t) {
  let dst = &mut dst[..64];
  // SAFETY: 64 bytes; vst4q takes any alignment.
  unsafe { vst4q_u8(dst.as_mut_ptr(), uint8x16x4_t(b, g, r, a)) }
}

#[target_feature(enable = "neon")]
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
    let y16 = [vzip1q_u8(yv, yv), vzip2q_u8(yv, yv)].map(|y| vreinterpretq_u16_u8(y));
    let bgr = bgr16(y16, load(&u[i..]), load(&v[i..]), &k);
    store_bgra(d, bgr, a.map_or(vdupq_n_u8(255), |a| load(&a[i..])));
  }
  16 * blocks.len()
}

#[target_feature(enable = "neon")]
pub(super) fn gray8(y: &[u8], a: Option<&[u8]>, k: &Constants, dst: &mut [u8]) -> usize {
  let k = K::new(k);
  let c = vdupq_n_u8(128);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let yv = load(&y[i..]);
    let y16 = [vzip1q_u8(yv, yv), vzip2q_u8(yv, yv)].map(|y| vreinterpretq_u16_u8(y));
    store_bgra(
      d,
      bgr16(y16, c, c, &k),
      a.map_or(vdupq_n_u8(255), |a| load(&a[i..])),
    );
  }
  16 * blocks.len()
}

#[target_feature(enable = "neon")]
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
    let y16 = [load16(&y[i..]), load16(&y[i + 8..])]
      .map(|y| vorrq_u16(vshlq_n_u16::<6>(y), vshrq_n_u16::<4>(y)));
    // min(c >> 2, 255)
    let c8 =
      |c: &[u16]| vqshrn_high_n_u16::<2>(vqshrn_n_u16::<2>(load16(&c[i..])), load16(&c[i + 8..]));
    store_bgra(
      d,
      bgr16(y16, c8(u), c8(v), &k),
      a.map_or(vdupq_n_u8(255), |a| load(&a[i..])),
    );
  }
  16 * blocks.len()
}

#[target_feature(enable = "neon")]
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
    let y16 = [load16(&y[i..]), load16(&y[i + 8..])]
      .map(|y| vorrq_u16(vshlq_n_u16::<4>(y), vshrq_n_u16::<8>(y)));
    // min(c >> 4, 255) of 8 samples, each for 2 pixels
    let c8 = |c: &[u16]| {
      let c = vqshrn_n_u16::<4>(load16(&c[i / 2..]));
      vcombine_u8(vzip1_u8(c, c), vzip2_u8(c, c))
    };
    store_bgra(
      d,
      bgr16(y16, c8(u), c8(v), &k),
      a.map_or(vdupq_n_u8(255), |a| load(&a[i..])),
    );
  }
  16 * blocks.len()
}

#[target_feature(enable = "neon")]
pub(super) fn convert16_to8(src: &[u16], shift: u32, dst: &mut [u8]) -> usize {
  // vshl by a negative count shifts right
  let shift = vdupq_n_s16(-(shift as i16));
  let ((src, _), (dst, _)) = (src.as_chunks::<16>(), dst.as_chunks_mut::<16>());
  for (s, d) in src.iter().zip(&mut *dst) {
    let [lo, hi] = [&s[..8], &s[8..]].map(|s| vshlq_u16(load16(s), shift));
    store(d, vqmovn_high_u16(vqmovn_u16(lo), hi));
  }
  16 * src.len().min(dst.len())
}

/// The sample pairs between the edges of `dst` (see `upsample`), 16 at a time: pair x is
/// samples 2x + 1 and 2x + 2, from samples x and x + 1 of the chroma rows.
#[target_feature(enable = "neon")]
pub(super) fn upsample8(near: &[u8], far: &[u8], dst: &mut [u8]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 16;
  if blocks == 0 {
    return 0;
  }
  let three = vdupq_n_u8(3);
  // sliced once: block k reads samples 16k..16k + 17 and writes bytes 32k + 1..32k + 33
  let (near, far) = (&near[..16 * blocks + 1], &far[..16 * blocks + 1]);
  let (out, _) = dst[1..].as_chunks_mut::<32>();
  for (x, out) in (0..).step_by(16).zip(out) {
    let (n, f) = (&near[x..][..17], &far[x..][..17]);
    let [a, b, c, d] = [&n[..16], &n[1..], &f[..16], &f[1..]].map(|s| load(s));
    // p = 3a + c and q = 3b + d, at most 1020
    let three_plus = |a: uint8x16_t, c: uint8x16_t| {
      [
        vmlal_u8(vmovl_u8(vget_low_u8(c)), vget_low_u8(a), vget_low_u8(three)),
        vmlal_high_u8(vmovl_high_u8(c), a, three),
      ]
    };
    let (p, q) = (three_plus(a, c), three_plus(b, d));
    // (3p + q + 8) >> 4, at most 255
    let mix = |p: [uint16x8_t; 2], q: [uint16x8_t; 2]| {
      vrshrn_high_n_u16::<4>(
        vrshrn_n_u16::<4>(vmlaq_n_u16(q[0], p[0], 3)),
        vmlaq_n_u16(q[1], p[1], 3),
      )
    };
    // SAFETY: 32 bytes; vst2q takes any alignment.
    unsafe { vst2q_u8(out.as_mut_ptr(), uint8x16x2_t(mix(p, q), mix(q, p))) }
  }
  16 * blocks
}

/// `upsample8` on samples of at most `max` (4095), 8 pairs at a time.
#[target_feature(enable = "neon")]
pub(super) fn upsample16(near: &[u16], far: &[u16], max: u16, dst: &mut [u16]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 8;
  let max = vdupq_n_u16(max);
  for x in (0..blocks).map(|n| 8 * n) {
    let [a, b, c, d] =
      [&near[x..], &near[x + 1..], &far[x..], &far[x + 1..]].map(|s| vminq_u16(load16(s), max));
    // p = 3a + c and q = 3b + d, at most 16380; 3p + q at most 65520, and vrshr rounds in more
    // bits
    let (p, q) = (vmlaq_n_u16(c, a, 3), vmlaq_n_u16(d, b, 3));
    let out = &mut dst[2 * x + 1..][..16];
    let pair = uint16x8x2_t(
      vrshrq_n_u16::<4>(vmlaq_n_u16(q, p, 3)),
      vrshrq_n_u16::<4>(vmlaq_n_u16(p, q, 3)),
    );
    // SAFETY: 16 words; vst2q takes any alignment.
    unsafe { vst2q_u16(out.as_mut_ptr(), pair) }
  }
  8 * blocks
}

/// 4 samples of `row` from `at`, in 16-bit lanes.
#[inline]
#[target_feature(enable = "neon")]
fn words4<T: Sample>(row: &[T], at: usize) -> uint16x4_t {
  match T::samples(row) {
    Samples::U8(row) => {
      let s: [u8; 4] = row[at..at + 4].try_into().unwrap();
      vget_low_u16(vmovl_u8(vcreate_u8(u32::from_le_bytes(s).into())))
    }
    Samples::U16(row) => {
      let s = &row[at..at + 4];
      // SAFETY: 8 bytes; vld1 takes any alignment.
      unsafe { vld1_u16(s.as_ptr()) }
    }
  }
}

/// The pixels of `rgb_impl_row` from `start` on (even), 4 at a time and short of the right edge.
/// Each lane does the scalar code's f32 operations in its order: no fused multiply-add, and the
/// min and max that keep NaN where the scalar code does.
#[target_feature(enable = "neon")]
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
  let max = vdup_n_u16(rg.max as u16);
  let [maxf, by, ry, bc, rc] = [rg.maxf, rg.by, rg.ry, rg.bc, rg.rc].map(|x| vdupq_n_f32(x));
  let (zero, half, one) = (vdupq_n_f32(0.0), vdupq_n_f32(0.5), vdupq_n_f32(1.0));
  // (min(s, max) as f32 - base) / range
  let unorm = |row: &[T], at: usize, base, range| {
    let s = vcvtq_f32_u32(vmovl_u16(vmin_u16(words4(row, at), max)));
    vdivq_f32(vsubq_f32(s, base), range)
  };
  // f32::clamp(0.0, 1.0): vmax and vmin keep NaN
  let unit = |c| vminq_f32(vmaxq_f32(c, zero), one);
  // (0.5 + c * 255.0) as u32, which gives 0 for NaN
  let q = |c| vcvtq_u32_f32(vaddq_f32(half, vmulq_n_f32(c, 255.0)));
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
          let nearer = |s| {
            let s = vextq_f32::<1>(s, s);
            vzip1q_f32(s, s)
          };
          let other = |s| vzip1q_f32(s, vextq_f32::<2>(s, s));
          // 9, 3, 3, 1 (/16), summed in the scalar code's order
          let sum = vaddq_f32(
            vmulq_n_f32(nearer(n), 9.0 / 16.0),
            vmulq_n_f32(other(n), 3.0 / 16.0),
          );
          let sum = vaddq_f32(sum, vmulq_n_f32(nearer(f), 3.0 / 16.0));
          vaddq_f32(sum, vmulq_n_f32(other(f), 1.0 / 16.0))
        });
        match rgb {
          Rgb::Identity => [v, yf, u],
          Rgb::YCgCo => {
            let t = vsubq_f32(yf, u);
            [vaddq_f32(t, v), vaddq_f32(yf, u), vsubq_f32(t, v)]
          }
          Rgb::Matrix { r, b, gr, gb, g } => {
            let uv = vaddq_f32(vmulq_n_f32(v, gr), vmulq_n_f32(u, gb));
            [
              vaddq_f32(yf, vmulq_n_f32(v, r)),
              vsubq_f32(yf, vdivq_f32(vmulq_n_f32(uv, 2.0), vdupq_n_f32(g))),
              vaddq_f32(yf, vmulq_n_f32(u, b)),
            ]
          }
        }
      }
    };
    let [mut r, mut g, mut b] = [r, g, b].map(unit);
    let aw = a.map(|a| words4(a, i));
    let alpha = match aw {
      None => vdupq_n_u32(255),
      // `as u8` keeps the low byte, which vsli does too
      Some(aw) if p.depth == 8 => vmovl_u16(aw),
      // scale_alpha
      Some(aw) => vminq_u32(
        q(vdivq_f32(vcvtq_f32_u32(vmovl_u16(aw)), maxf)),
        vdupq_n_u32(255),
      ),
    };
    if let Some(aw) = aw
      && p.unpremultiply
    {
      let ac = unit(vdivq_f32(vcvtq_f32_u32(vmovl_u16(vmin_u16(aw, max))), maxf));
      let (clear, part) = (vceqq_f32(ac, zero), vcltq_f32(ac, one));
      // 0 where ac is 0, else min(c / ac, 1) where ac < 1 (vminnm, like f32::min, gives 1 for
      // NaN)
      let un = |c| {
        vbslq_f32(
          clear,
          zero,
          vbslq_f32(part, vminnmq_f32(vdivq_f32(c, ac), one), c),
        )
      };
      [r, g, b] = [r, g, b].map(un);
    }
    let bgr = vsliq_n_u32::<16>(vsliq_n_u32::<8>(q(b), q(g)), q(r));
    store(
      &mut dst[4 * i..],
      vreinterpretq_u8_u32(vsliq_n_u32::<24>(bgr, alpha)),
    );
    i += 4;
  }
  i
}
