//! x86_64 rows of `avif`: SSE2 (every x86_64 CPU) and AVX2. Each does whole vectors from the
//! start and returns how many pixels (sample pairs for the upsampling) it did; the scalar code
//! does the rest. The AVX2 rows do the operations of the SSE2 rows on twice the lanes, and
//! interleave in each 128-bit half before they put the halves in order.

use core::arch::x86_64::*;

use super::simd::Rgb;
use super::{Constants, RgbImpl, Sample, Samples};
use crate::x86::{load, store};

/// The first 8 words of `src`.
#[inline]
#[target_feature(enable = "sse2")]
fn load16(src: &[u16]) -> __m128i {
  let src = &src[..8];
  // SAFETY: 16 bytes; loadu takes any alignment.
  unsafe { _mm_loadu_si128(src.as_ptr().cast()) }
}

/// `v` to the first 8 words of `dst`.
#[inline]
#[target_feature(enable = "sse2")]
fn store16(dst: &mut [u16], v: __m128i) {
  let dst = &mut dst[..8];
  // SAFETY: 16 bytes; storeu takes any alignment.
  unsafe { _mm_storeu_si128(dst.as_mut_ptr().cast(), v) }
}

/// The unsigned min of the 16-bit lanes of `v` and `max` (SSE2 has no pminuw).
#[inline]
#[target_feature(enable = "sse2")]
fn min16(v: __m128i, max: __m128i) -> __m128i {
  _mm_sub_epi16(v, _mm_subs_epu16(v, max))
}

/// 3v in 16-bit lanes
#[inline]
#[target_feature(enable = "sse2")]
fn times3(v: __m128i) -> __m128i {
  _mm_add_epi16(_mm_add_epi16(v, v), v)
}

/// `Constants::lanes` in 16-bit lanes
struct K {
  yg: __m128i,
  ub: __m128i,
  ug: __m128i,
  vg: __m128i,
  vr: __m128i,
  bb: __m128i,
  bg: __m128i,
  br: __m128i,
}

impl K {
  #[inline]
  #[target_feature(enable = "sse2")]
  fn new(k: &Constants) -> K {
    let [yg, ub, ug, vg, vr, bb, bg, br] = k.lanes().map(|c| _mm_set1_epi16(c as i16));
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
/// G and R in 16-bit lanes: subs clamps at 0, and they are at most 806, which packus clamps to
/// 255.
#[inline]
#[target_feature(enable = "sse2")]
fn bgr8(y16: __m128i, u: __m128i, v: __m128i, k: &K) -> [__m128i; 3] {
  let y1 = _mm_mulhi_epu16(y16, k.yg);
  let b = _mm_subs_epu16(_mm_add_epi16(y1, _mm_mullo_epi16(u, k.ub)), k.bb);
  let uv = _mm_add_epi16(_mm_mullo_epi16(u, k.ug), _mm_mullo_epi16(v, k.vg));
  let g = _mm_subs_epu16(_mm_add_epi16(y1, k.bg), uv);
  let r = _mm_subs_epu16(_mm_add_epi16(y1, _mm_mullo_epi16(v, k.vr)), k.br);
  [b, g, r].map(|c| _mm_srli_epi16::<6>(c))
}

/// 16 pixels, luma expanded to 16 bits (pixels 0..8, then 8..16) and 16 bytes of U, V and A, to
/// the first 64 bytes of `dst`.
#[inline]
#[target_feature(enable = "sse2")]
fn bgra16(y16: [__m128i; 2], u: __m128i, v: __m128i, a: __m128i, k: &K, dst: &mut [u8]) {
  let z = _mm_setzero_si128();
  let lo = bgr8(y16[0], _mm_unpacklo_epi8(u, z), _mm_unpacklo_epi8(v, z), k);
  let hi = bgr8(y16[1], _mm_unpackhi_epi8(u, z), _mm_unpackhi_epi8(v, z), k);
  let [b, g, r] = [0, 1, 2].map(|c| _mm_packus_epi16(lo[c], hi[c]));
  let (bg, ra) = (_mm_unpacklo_epi8(b, g), _mm_unpacklo_epi8(r, a));
  store(dst, _mm_unpacklo_epi16(bg, ra));
  store(&mut dst[16..], _mm_unpackhi_epi16(bg, ra));
  let (bg, ra) = (_mm_unpackhi_epi8(b, g), _mm_unpackhi_epi8(r, a));
  store(&mut dst[32..], _mm_unpacklo_epi16(bg, ra));
  store(&mut dst[48..], _mm_unpackhi_epi16(bg, ra));
}

#[target_feature(enable = "sse2")]
pub(super) fn yuv8_sse2(
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
    let y16 = [_mm_unpacklo_epi8(yv, yv), _mm_unpackhi_epi8(yv, yv)];
    let a = a.map_or(_mm_set1_epi8(-1), |a| load(&a[i..]));
    bgra16(y16, load(&u[i..]), load(&v[i..]), a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "sse2")]
pub(super) fn gray8_sse2(y: &[u8], a: Option<&[u8]>, k: &Constants, dst: &mut [u8]) -> usize {
  let k = K::new(k);
  let c = _mm_set1_epi8(128u8 as i8);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let yv = load(&y[i..]);
    let y16 = [_mm_unpacklo_epi8(yv, yv), _mm_unpackhi_epi8(yv, yv)];
    let a = a.map_or(_mm_set1_epi8(-1), |a| load(&a[i..]));
    bgra16(y16, c, c, a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "sse2")]
pub(super) fn yuv10_sse2(
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
      .map(|y| _mm_or_si128(_mm_slli_epi16::<6>(y), _mm_srli_epi16::<4>(y)));
    // min(c >> 2, 255): c >> 2 is a positive i16, which packus clamps
    let c8 = |c: &[u16]| {
      let half = |at: usize| _mm_srli_epi16::<2>(load16(&c[at..]));
      _mm_packus_epi16(half(i), half(i + 8))
    };
    let a = a.map_or(_mm_set1_epi8(-1), |a| load(&a[i..]));
    bgra16(y16, c8(u), c8(v), a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "sse2")]
pub(super) fn yuv12_sse2(
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
      .map(|y| _mm_or_si128(_mm_slli_epi16::<4>(y), _mm_srli_epi16::<8>(y)));
    // min(c >> 4, 255) of 8 samples, each for 2 pixels
    let c8 = |c: &[u16]| {
      let c = _mm_packus_epi16(
        _mm_srli_epi16::<4>(load16(&c[i / 2..])),
        _mm_setzero_si128(),
      );
      _mm_unpacklo_epi8(c, c)
    };
    let a = a.map_or(_mm_set1_epi8(-1), |a| load(&a[i..]));
    bgra16(y16, c8(u), c8(v), a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "sse2")]
pub(super) fn convert16_to8_sse2(src: &[u16], shift: u32, dst: &mut [u8]) -> usize {
  let (shift, max) = (_mm_cvtsi32_si128(shift as i32), _mm_set1_epi16(255));
  let ((src, _), (dst, _)) = (src.as_chunks::<16>(), dst.as_chunks_mut::<16>());
  for (s, d) in src.iter().zip(&mut *dst) {
    let [lo, hi] = [&s[..8], &s[8..]].map(|s| min16(_mm_srl_epi16(load16(s), shift), max));
    store(d, _mm_packus_epi16(lo, hi));
  }
  16 * src.len().min(dst.len())
}

/// The sample pairs between the edges of `dst` (see `upsample`), 16 at a time: pair x is
/// samples 2x + 1 and 2x + 2, from samples x and x + 1 of the chroma rows.
#[target_feature(enable = "sse2")]
pub(super) fn upsample8_sse2(near: &[u8], far: &[u8], dst: &mut [u8]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 16;
  if blocks == 0 {
    return 0;
  }
  let (z, eight) = (_mm_setzero_si128(), _mm_set1_epi16(8));
  // 8 pairs from samples in 16-bit lanes: (3p + q + 8) >> 4 and (p + 3q + 8) >> 4 in the low
  // and high byte of each lane, with p = 3a + c and q = 3b + d (at most 1020)
  let pairs = |a, b, c, d| {
    let (p, q) = (_mm_add_epi16(times3(a), c), _mm_add_epi16(times3(b), d));
    let odd = _mm_srli_epi16::<4>(_mm_add_epi16(_mm_add_epi16(times3(p), q), eight));
    let even = _mm_srli_epi16::<4>(_mm_add_epi16(_mm_add_epi16(times3(q), p), eight));
    _mm_or_si128(odd, _mm_slli_epi16::<8>(even))
  };
  // sliced once: block k reads samples 16k..16k + 17 and writes bytes 32k + 1..32k + 33
  let (near, far) = (&near[..16 * blocks + 1], &far[..16 * blocks + 1]);
  let (out, _) = dst[1..].as_chunks_mut::<32>();
  for (x, out) in (0..).step_by(16).zip(out) {
    let (n, f) = (&near[x..][..17], &far[x..][..17]);
    let [a, b, c, d] = [&n[..16], &n[1..], &f[..16], &f[1..]].map(|s| load(s));
    let lo = |s| _mm_unpacklo_epi8(s, z);
    let hi = |s| _mm_unpackhi_epi8(s, z);
    store(out, pairs(lo(a), lo(b), lo(c), lo(d)));
    store(&mut out[16..], pairs(hi(a), hi(b), hi(c), hi(d)));
  }
  16 * blocks
}

/// `upsample8_sse2` on samples of at most `max` (4095), 8 pairs at a time.
#[target_feature(enable = "sse2")]
pub(super) fn upsample16_sse2(near: &[u16], far: &[u16], max: u16, dst: &mut [u16]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 8;
  let (max, eight) = (_mm_set1_epi16(max as i16), _mm_set1_epi16(8));
  for x in (0..blocks).map(|n| 8 * n) {
    let [a, b, c, d] =
      [&near[x..], &near[x + 1..], &far[x..], &far[x + 1..]].map(|s| min16(load16(s), max));
    // p = 3a + c and q = 3b + d, at most 16380; 3p + q + 8 at most 65528
    let (p, q) = (_mm_add_epi16(times3(a), c), _mm_add_epi16(times3(b), d));
    let odd = _mm_srli_epi16::<4>(_mm_add_epi16(_mm_add_epi16(times3(p), q), eight));
    let even = _mm_srli_epi16::<4>(_mm_add_epi16(_mm_add_epi16(times3(q), p), eight));
    store16(&mut dst[2 * x + 1..], _mm_unpacklo_epi16(odd, even));
    store16(&mut dst[2 * x + 9..], _mm_unpackhi_epi16(odd, even));
  }
  8 * blocks
}

/// 4 samples of `row` from `at`, in 16-bit lanes 0..4.
#[inline]
#[target_feature(enable = "sse2")]
fn words4<T: Sample>(row: &[T], at: usize) -> __m128i {
  match T::samples(row) {
    Samples::U8(row) => {
      let s: [u8; 4] = row[at..at + 4].try_into().unwrap();
      _mm_unpacklo_epi8(
        _mm_cvtsi32_si128(i32::from_le_bytes(s)),
        _mm_setzero_si128(),
      )
    }
    Samples::U16(row) => {
      let s = &row[at..at + 4];
      // SAFETY: 8 bytes; loadl takes any alignment.
      unsafe { _mm_loadl_epi64(s.as_ptr().cast()) }
    }
  }
}

/// The pixels of `rgb_impl_row` from `start` on (even), 4 at a time and short of the right edge.
/// Each lane does the scalar code's f32 operations in its order (no fused multiply-add), with
/// maxps and minps in the operand order that keeps or drops NaN like the scalar code.
#[target_feature(enable = "sse2")]
pub(super) fn rgb_impl_sse2<T: Sample>(
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
  let (max, z) = (_mm_set1_epi16(rg.max as i16), _mm_setzero_si128());
  let [maxf, by, ry, bc, rc] = [rg.maxf, rg.by, rg.ry, rg.bc, rg.rc].map(|x| _mm_set1_ps(x));
  let [zero, half, one, k255] = [0.0, 0.5, 1.0, 255.0].map(|x| _mm_set1_ps(x));
  let widen = |w| _mm_cvtepi32_ps(_mm_unpacklo_epi16(w, z));
  // (min(s, max) as f32 - base) / range
  let unorm = |row: &[T], at: usize, base, range| {
    _mm_div_ps(_mm_sub_ps(widen(min16(words4(row, at), max)), base), range)
  };
  // f32::clamp(0.0, 1.0): maxps and minps give their second operand where one is NaN
  let unit = |c| _mm_min_ps(one, _mm_max_ps(zero, c));
  // (0.5 + c * 255.0) as u8: 0..=255, or NaN, which cvttps makes 0x80000000 (low byte 0)
  let q = |c| {
    let c = _mm_cvttps_epi32(_mm_add_ps(half, _mm_mul_ps(c, k255)));
    _mm_and_si128(c, _mm_set1_epi32(0xFF))
  };
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
          let nearer = |s| _mm_shuffle_ps::<0b10_10_01_01>(s, s);
          let other = |s| _mm_shuffle_ps::<0b11_01_10_00>(s, s);
          // 9, 3, 3, 1 (/16), summed in the scalar code's order
          let sum = _mm_add_ps(
            _mm_mul_ps(nearer(n), _mm_set1_ps(9.0 / 16.0)),
            _mm_mul_ps(other(n), _mm_set1_ps(3.0 / 16.0)),
          );
          let sum = _mm_add_ps(sum, _mm_mul_ps(nearer(f), _mm_set1_ps(3.0 / 16.0)));
          _mm_add_ps(sum, _mm_mul_ps(other(f), _mm_set1_ps(1.0 / 16.0)))
        });
        match rgb {
          Rgb::Identity => [v, yf, u],
          Rgb::YCgCo => {
            let t = _mm_sub_ps(yf, u);
            [_mm_add_ps(t, v), _mm_add_ps(yf, u), _mm_sub_ps(t, v)]
          }
          Rgb::Matrix { r, b, gr, gb, g } => {
            let uv = _mm_add_ps(
              _mm_mul_ps(v, _mm_set1_ps(gr)),
              _mm_mul_ps(u, _mm_set1_ps(gb)),
            );
            let g = _mm_div_ps(_mm_mul_ps(uv, _mm_set1_ps(2.0)), _mm_set1_ps(g));
            [
              _mm_add_ps(yf, _mm_mul_ps(v, _mm_set1_ps(r))),
              _mm_sub_ps(yf, g),
              _mm_add_ps(yf, _mm_mul_ps(u, _mm_set1_ps(b))),
            ]
          }
        }
      }
    };
    let [mut r, mut g, mut b] = [r, g, b].map(unit);
    let aw = a.map(|a| words4(a, i));
    let alpha = match aw {
      None => _mm_set1_epi32(255),
      // `as u8` keeps the low byte, which the shift by 24 does too
      Some(aw) if p.depth == 8 => _mm_unpacklo_epi16(aw, z),
      // scale_alpha: min((0.5 + a / max * 255) as u32, 255), the min taken before the
      // truncation
      Some(aw) => {
        let x = _mm_add_ps(half, _mm_mul_ps(_mm_div_ps(widen(aw), maxf), k255));
        _mm_cvttps_epi32(_mm_min_ps(x, k255))
      }
    };
    if let Some(aw) = aw
      && p.unpremultiply
    {
      let ac = unit(_mm_div_ps(widen(min16(aw, max)), maxf));
      let (clear, part) = (_mm_cmpeq_ps(ac, zero), _mm_cmplt_ps(ac, one));
      // 0 where ac is 0, else min(c / ac, 1) where ac < 1 (minps gives its second operand, 1,
      // for NaN, as f32::min does)
      let un = |c| {
        let x = _mm_min_ps(_mm_div_ps(c, ac), one);
        let c = _mm_or_ps(_mm_and_ps(part, x), _mm_andnot_ps(part, c));
        _mm_andnot_ps(clear, c)
      };
      [r, g, b] = [r, g, b].map(un);
    }
    let bg = _mm_or_si128(q(b), _mm_slli_epi32::<8>(q(g)));
    let ra = _mm_or_si128(_mm_slli_epi32::<16>(q(r)), _mm_slli_epi32::<24>(alpha));
    store(&mut dst[4 * i..], _mm_or_si128(bg, ra));
    i += 4;
  }
  i
}

/// The first 32 bytes of `src`.
#[inline]
#[target_feature(enable = "avx2")]
fn load32(src: &[u8]) -> __m256i {
  let src = &src[..32];
  // SAFETY: 32 bytes; loadu takes any alignment.
  unsafe { _mm256_loadu_si256(src.as_ptr().cast()) }
}

/// The first 16 words of `src`.
#[inline]
#[target_feature(enable = "avx2")]
fn load32w(src: &[u16]) -> __m256i {
  let src = &src[..16];
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

/// `v` to the first 16 words of `dst`.
#[inline]
#[target_feature(enable = "avx2")]
fn store32w(dst: &mut [u16], v: __m256i) {
  let dst = &mut dst[..16];
  // SAFETY: 32 bytes; storeu takes any alignment.
  unsafe { _mm256_storeu_si256(dst.as_mut_ptr().cast(), v) }
}

/// `times3` on 16 lanes
#[inline]
#[target_feature(enable = "avx2")]
fn times3_avx2(v: __m256i) -> __m256i {
  _mm256_add_epi16(_mm256_add_epi16(v, v), v)
}

/// `K` in 16 lanes
struct K2 {
  yg: __m256i,
  ub: __m256i,
  ug: __m256i,
  vg: __m256i,
  vr: __m256i,
  bb: __m256i,
  bg: __m256i,
  br: __m256i,
}

impl K2 {
  #[inline]
  #[target_feature(enable = "avx2")]
  fn new(k: &Constants) -> K2 {
    let [yg, ub, ug, vg, vr, bb, bg, br] = k.lanes().map(|c| _mm256_set1_epi16(c as i16));
    K2 {
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

/// `bgr8` and `bgra16` on 16 pixels in 16-bit lanes: luma expanded to 16 bits, and 8-bit U, V
/// and A.
#[inline]
#[target_feature(enable = "avx2")]
fn bgra16_avx2(y16: __m256i, u: __m256i, v: __m256i, a: __m256i, k: &K2, dst: &mut [u8]) {
  let y1 = _mm256_mulhi_epu16(y16, k.yg);
  let b = _mm256_subs_epu16(_mm256_add_epi16(y1, _mm256_mullo_epi16(u, k.ub)), k.bb);
  let uv = _mm256_add_epi16(_mm256_mullo_epi16(u, k.ug), _mm256_mullo_epi16(v, k.vg));
  let g = _mm256_subs_epu16(_mm256_add_epi16(y1, k.bg), uv);
  let r = _mm256_subs_epu16(_mm256_add_epi16(y1, _mm256_mullo_epi16(v, k.vr)), k.br);
  let [b, g, r] = [b, g, r].map(|c| _mm256_srli_epi16::<6>(c));
  // in each 128-bit half (pixels 0..8, then 8..16): bytes B and G, R and A, then B R and G A
  // interleaved, then BGRA of pixels 0..4 and 4..8
  let (bg, ra) = (_mm256_packus_epi16(b, g), _mm256_packus_epi16(r, a));
  let (br, ga) = (_mm256_unpacklo_epi8(bg, ra), _mm256_unpackhi_epi8(bg, ra));
  let (lo, hi) = (_mm256_unpacklo_epi8(br, ga), _mm256_unpackhi_epi8(br, ga));
  store32(dst, _mm256_permute2x128_si256::<0x20>(lo, hi));
  store32(&mut dst[32..], _mm256_permute2x128_si256::<0x31>(lo, hi));
}

#[target_feature(enable = "avx2")]
pub(super) fn yuv8_avx2(
  y: &[u8],
  u: &[u8],
  v: &[u8],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) -> usize {
  let k = K2::new(k);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let bytes = |s: &[u8]| _mm256_cvtepu8_epi16(load(&s[i..]));
    let yv = bytes(y);
    // y * 0x0101
    let y16 = _mm256_or_si256(yv, _mm256_slli_epi16::<8>(yv));
    let a = a.map_or(_mm256_set1_epi16(255), bytes);
    bgra16_avx2(y16, bytes(u), bytes(v), a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "avx2")]
pub(super) fn gray8_avx2(y: &[u8], a: Option<&[u8]>, k: &Constants, dst: &mut [u8]) -> usize {
  let k = K2::new(k);
  let c = _mm256_set1_epi16(128);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let bytes = |s: &[u8]| _mm256_cvtepu8_epi16(load(&s[i..]));
    let yv = bytes(y);
    let y16 = _mm256_or_si256(yv, _mm256_slli_epi16::<8>(yv));
    let a = a.map_or(_mm256_set1_epi16(255), bytes);
    bgra16_avx2(y16, c, c, a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "avx2")]
pub(super) fn yuv10_avx2(
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) -> usize {
  let k = K2::new(k);
  let max = _mm256_set1_epi16(255);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let yv = load32w(&y[i..]);
    // y << 6 | y >> 4, which wraps like the scalar code
    let y16 = _mm256_or_si256(_mm256_slli_epi16::<6>(yv), _mm256_srli_epi16::<4>(yv));
    // min(c >> 2, 255)
    let c8 = |c: &[u16]| _mm256_min_epu16(_mm256_srli_epi16::<2>(load32w(&c[i..])), max);
    let a = a.map_or(max, |a| _mm256_cvtepu8_epi16(load(&a[i..])));
    bgra16_avx2(y16, c8(u), c8(v), a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "avx2")]
pub(super) fn yuv12_avx2(
  y: &[u16],
  u: &[u16],
  v: &[u16],
  a: Option<&[u8]>,
  k: &Constants,
  dst: &mut [u8],
) -> usize {
  let k = K2::new(k);
  let (blocks, _) = dst.as_chunks_mut::<64>();
  for (n, d) in blocks.iter_mut().enumerate() {
    let i = 16 * n;
    let yv = load32w(&y[i..]);
    let y16 = _mm256_or_si256(_mm256_slli_epi16::<4>(yv), _mm256_srli_epi16::<8>(yv));
    // min(c >> 4, 255) of 8 samples, each in the two 16-bit halves of a 32-bit lane: one for
    // each pixel of a pair
    let c8 = |c: &[u16]| {
      let c = _mm_min_epu16(
        _mm_srli_epi16::<4>(load16(&c[i / 2..])),
        _mm_set1_epi16(255),
      );
      let c = _mm256_cvtepu16_epi32(c);
      _mm256_or_si256(c, _mm256_slli_epi32::<16>(c))
    };
    let a = a.map_or(_mm256_set1_epi16(255), |a| {
      _mm256_cvtepu8_epi16(load(&a[i..]))
    });
    bgra16_avx2(y16, c8(u), c8(v), a, &k, d);
  }
  16 * blocks.len()
}

#[target_feature(enable = "avx2")]
pub(super) fn convert16_to8_avx2(src: &[u16], shift: u32, dst: &mut [u8]) -> usize {
  let (shift, max) = (_mm_cvtsi32_si128(shift as i32), _mm256_set1_epi16(255));
  let ((src, _), (dst, _)) = (src.as_chunks::<32>(), dst.as_chunks_mut::<32>());
  for (s, d) in src.iter().zip(&mut *dst) {
    let [lo, hi] =
      [&s[..16], &s[16..]].map(|s| _mm256_min_epu16(_mm256_srl_epi16(load32w(s), shift), max));
    // packus works in each 128-bit half: samples 0..8, 16..24, 8..16, 24..32
    let packed = _mm256_packus_epi16(lo, hi);
    store32(d, _mm256_permute4x64_epi64::<0b11_01_10_00>(packed));
  }
  32 * src.len().min(dst.len())
}

/// `upsample8_sse2`, 32 pairs at a time.
#[target_feature(enable = "avx2")]
pub(super) fn upsample8_avx2(near: &[u8], far: &[u8], dst: &mut [u8]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 32;
  if blocks == 0 {
    return 0;
  }
  let (z, eight) = (_mm256_setzero_si256(), _mm256_set1_epi16(8));
  let pairs = |a, b, c, d| {
    let (p, q) = (
      _mm256_add_epi16(times3_avx2(a), c),
      _mm256_add_epi16(times3_avx2(b), d),
    );
    let odd = _mm256_add_epi16(_mm256_add_epi16(times3_avx2(p), q), eight);
    let even = _mm256_add_epi16(_mm256_add_epi16(times3_avx2(q), p), eight);
    _mm256_or_si256(
      _mm256_srli_epi16::<4>(odd),
      _mm256_slli_epi16::<8>(_mm256_srli_epi16::<4>(even)),
    )
  };
  // sliced once: block k reads samples 32k..32k + 33 and writes bytes 64k + 1..64k + 65
  let (near, far) = (&near[..32 * blocks + 1], &far[..32 * blocks + 1]);
  let (out, _) = dst[1..].as_chunks_mut::<64>();
  for (x, out) in (0..).step_by(32).zip(out) {
    let (n, f) = (&near[x..][..33], &far[x..][..33]);
    let [a, b, c, d] = [&n[..32], &n[1..], &f[..32], &f[1..]].map(|s| load32(s));
    let lo = |s| _mm256_unpacklo_epi8(s, z);
    let hi = |s| _mm256_unpackhi_epi8(s, z);
    // pairs x..x + 8 and x + 16..x + 24, then x + 8..x + 16 and x + 24..x + 32
    let (l, h) = (
      pairs(lo(a), lo(b), lo(c), lo(d)),
      pairs(hi(a), hi(b), hi(c), hi(d)),
    );
    store32(out, _mm256_permute2x128_si256::<0x20>(l, h));
    store32(&mut out[32..], _mm256_permute2x128_si256::<0x31>(l, h));
  }
  32 * blocks
}

/// `upsample16_sse2`, 16 pairs at a time.
#[target_feature(enable = "avx2")]
pub(super) fn upsample16_avx2(near: &[u16], far: &[u16], max: u16, dst: &mut [u16]) -> usize {
  let blocks = dst.len().saturating_sub(1) / 2 / 16;
  let (max, eight) = (_mm256_set1_epi16(max as i16), _mm256_set1_epi16(8));
  for x in (0..blocks).map(|n| 16 * n) {
    let [a, b, c, d] = [&near[x..], &near[x + 1..], &far[x..], &far[x + 1..]]
      .map(|s| _mm256_min_epu16(load32w(s), max));
    let (p, q) = (
      _mm256_add_epi16(times3_avx2(a), c),
      _mm256_add_epi16(times3_avx2(b), d),
    );
    let odd = _mm256_add_epi16(_mm256_add_epi16(times3_avx2(p), q), eight);
    let even = _mm256_add_epi16(_mm256_add_epi16(times3_avx2(q), p), eight);
    let [odd, even] = [odd, even].map(|s| _mm256_srli_epi16::<4>(s));
    // pairs x..x + 4 and x + 8..x + 12, then x + 4..x + 8 and x + 12..x + 16
    let (l, h) = (
      _mm256_unpacklo_epi16(odd, even),
      _mm256_unpackhi_epi16(odd, even),
    );
    store32w(
      &mut dst[2 * x + 1..],
      _mm256_permute2x128_si256::<0x20>(l, h),
    );
    store32w(
      &mut dst[2 * x + 17..],
      _mm256_permute2x128_si256::<0x31>(l, h),
    );
  }
  16 * blocks
}

/// 8 samples of `row` from `at`, in 16-bit lanes.
#[inline]
#[target_feature(enable = "avx2")]
fn words8<T: Sample>(row: &[T], at: usize) -> __m128i {
  match T::samples(row) {
    Samples::U8(row) => {
      let s: [u8; 8] = row[at..at + 8].try_into().unwrap();
      _mm_cvtepu8_epi16(_mm_cvtsi64_si128(i64::from_le_bytes(s)))
    }
    Samples::U16(row) => load16(&row[at..]),
  }
}

/// `rgb_impl_sse2` on 8 pixels at a time.
#[target_feature(enable = "avx2")]
pub(super) fn rgb_impl_avx2<T: Sample>(
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
  let max = _mm_set1_epi16(rg.max as i16);
  let [maxf, by, ry, bc, rc] = [rg.maxf, rg.by, rg.ry, rg.bc, rg.rc].map(|x| _mm256_set1_ps(x));
  let [zero, half, one, k255] = [0.0, 0.5, 1.0, 255.0].map(|x| _mm256_set1_ps(x));
  let widen = |w| _mm256_cvtepi32_ps(_mm256_cvtepu16_epi32(w));
  let unorm = |row: &[T], at: usize, base, range| {
    _mm256_div_ps(
      _mm256_sub_ps(widen(_mm_min_epu16(words8(row, at), max)), base),
      range,
    )
  };
  let unit = |c| _mm256_min_ps(one, _mm256_max_ps(zero, c));
  let q = |c| {
    let c = _mm256_cvttps_epi32(_mm256_add_ps(half, _mm256_mul_ps(c, k255)));
    _mm256_and_si256(c, _mm256_set1_epi32(0xFF))
  };
  let w = dst.len() / 4;
  // With subsampled chroma, pixels i..i + 8 read samples i / 2 - 1..i / 2 + 7 of each chroma row,
  // 2 past the ones they take, and the last pixel of an even width is an edge: 14 pixels from i
  // keep both inside the row.
  let span = if p.subsampled { 14 } else { 8 };
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
          // as in `rgb_impl_sse2`, from k = i / 2 - 1
          let (n, f) = (
            unorm(near, i / 2 - 1, bc, rc),
            unorm(far, i / 2 - 1, bc, rc),
          );
          let nearer = |s| _mm256_permutevar8x32_ps(s, _mm256_setr_epi32(1, 1, 2, 2, 3, 3, 4, 4));
          let other = |s| _mm256_permutevar8x32_ps(s, _mm256_setr_epi32(0, 2, 1, 3, 2, 4, 3, 5));
          let sum = _mm256_add_ps(
            _mm256_mul_ps(nearer(n), _mm256_set1_ps(9.0 / 16.0)),
            _mm256_mul_ps(other(n), _mm256_set1_ps(3.0 / 16.0)),
          );
          let sum = _mm256_add_ps(sum, _mm256_mul_ps(nearer(f), _mm256_set1_ps(3.0 / 16.0)));
          _mm256_add_ps(sum, _mm256_mul_ps(other(f), _mm256_set1_ps(1.0 / 16.0)))
        });
        match rgb {
          Rgb::Identity => [v, yf, u],
          Rgb::YCgCo => {
            let t = _mm256_sub_ps(yf, u);
            [
              _mm256_add_ps(t, v),
              _mm256_add_ps(yf, u),
              _mm256_sub_ps(t, v),
            ]
          }
          Rgb::Matrix { r, b, gr, gb, g } => {
            let uv = _mm256_add_ps(
              _mm256_mul_ps(v, _mm256_set1_ps(gr)),
              _mm256_mul_ps(u, _mm256_set1_ps(gb)),
            );
            let g = _mm256_div_ps(_mm256_mul_ps(uv, _mm256_set1_ps(2.0)), _mm256_set1_ps(g));
            [
              _mm256_add_ps(yf, _mm256_mul_ps(v, _mm256_set1_ps(r))),
              _mm256_sub_ps(yf, g),
              _mm256_add_ps(yf, _mm256_mul_ps(u, _mm256_set1_ps(b))),
            ]
          }
        }
      }
    };
    let [mut r, mut g, mut b] = [r, g, b].map(unit);
    let aw = a.map(|a| words8(a, i));
    let alpha = match aw {
      None => _mm256_set1_epi32(255),
      Some(aw) if p.depth == 8 => _mm256_cvtepu16_epi32(aw),
      Some(aw) => {
        let x = _mm256_add_ps(half, _mm256_mul_ps(_mm256_div_ps(widen(aw), maxf), k255));
        _mm256_cvttps_epi32(_mm256_min_ps(x, k255))
      }
    };
    if let Some(aw) = aw
      && p.unpremultiply
    {
      let ac = unit(_mm256_div_ps(widen(_mm_min_epu16(aw, max)), maxf));
      let clear = _mm256_cmp_ps::<_CMP_EQ_OQ>(ac, zero);
      let part = _mm256_cmp_ps::<_CMP_LT_OQ>(ac, one);
      let un = |c| {
        let x = _mm256_min_ps(_mm256_div_ps(c, ac), one);
        _mm256_andnot_ps(clear, _mm256_blendv_ps(c, x, part))
      };
      [r, g, b] = [r, g, b].map(un);
    }
    let bg = _mm256_or_si256(q(b), _mm256_slli_epi32::<8>(q(g)));
    let ra = _mm256_or_si256(
      _mm256_slli_epi32::<16>(q(r)),
      _mm256_slli_epi32::<24>(alpha),
    );
    store32(&mut dst[4 * i..], _mm256_or_si256(bg, ra));
    i += 8;
  }
  i
}
