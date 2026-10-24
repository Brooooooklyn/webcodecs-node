//! aarch64 NEON (part of every aarch64 target). Each kernel does whole vectors from the start and
//! returns how many bytes or pixels it did; the scalar code does the rest.

use core::arch::aarch64::*;

use crate::simd::zip_chunks;
use crate::{Alpha, Be16, Bitfield};

/// The first 16 bytes of `src`.
#[inline]
#[target_feature(enable = "neon")]
pub(crate) fn load(src: &[u8]) -> uint8x16_t {
  let src = &src[..16];
  // SAFETY: 16 bytes; vld1q takes any alignment.
  unsafe { vld1q_u8(src.as_ptr()) }
}

/// `v` to the first 16 bytes of `dst`.
#[inline]
#[target_feature(enable = "neon")]
pub(crate) fn store(dst: &mut [u8], v: uint8x16_t) {
  let dst = &mut dst[..16];
  // SAFETY: 16 bytes; vst1q takes any alignment.
  unsafe { vst1q_u8(dst.as_mut_ptr(), v) }
}

/// `v` with 255 in the 4th byte of each 4-byte pixel.
#[inline]
#[target_feature(enable = "neon")]
fn opaque(v: uint8x16_t) -> uint8x16_t {
  vreinterpretq_u8_u32(vorrq_u32(vreinterpretq_u32_u8(v), vdupq_n_u32(0xFF00_0000)))
}

/// The 4th byte of each pixel written by `alpha`.
#[inline]
#[target_feature(enable = "neon")]
fn set_alpha(v: uint8x16_t, alpha: Alpha) -> uint8x16_t {
  match alpha {
    Alpha::Keep => v,
    Alpha::Opaque => opaque(v),
    Alpha::Key(key) => {
      let (w, a) = (vreinterpretq_u32_u8(v), vdupq_n_u32(0xFF00_0000));
      let keyed = vceqq_u32(vandq_u32(w, vdupq_n_u32(0xFF)), vdupq_n_u32(key.into()));
      vreinterpretq_u8_u32(vorrq_u32(vbicq_u32(w, a), vbicq_u32(a, keyed)))
    }
  }
}

#[target_feature(enable = "neon")]
pub(crate) fn edit_bgra(pixels: &mut [u8], swap: bool, alpha: Alpha) -> usize {
  let order = load(&[2, 1, 0, 3, 6, 5, 4, 7, 10, 9, 8, 11, 14, 13, 12, 15]);
  let (vectors, _) = pixels.as_chunks_mut::<16>();
  for v in vectors.iter_mut() {
    let x = load(v);
    store(
      v,
      set_alpha(if swap { vqtbl1q_u8(x, order) } else { x }, alpha),
    );
  }
  16 * vectors.len()
}

#[target_feature(enable = "neon")]
pub(crate) fn be16_to_bgra(src: &[u8], dst: &mut [u8], layout: Be16, alpha: bool) -> usize {
  // vqtbl gives 0 for index 255, which `a` then sets to 255 where there is no alpha. The high
  // byte of a big-endian sample comes first.
  let a = |v| if alpha { v } else { opaque(v) };
  match layout {
    Be16::Gray => {
      let lo = load(&[0, 0, 0, 255, 2, 2, 2, 255, 4, 4, 4, 255, 6, 6, 6, 255]);
      let hi = load(&[
        8, 8, 8, 255, 10, 10, 10, 255, 12, 12, 12, 255, 14, 14, 14, 255,
      ]);
      8 * zip_chunks::<16, 32>(src, dst, |s, d| {
        let v = load(s);
        store(d, opaque(vqtbl1q_u8(v, lo)));
        store(&mut d[16..], opaque(vqtbl1q_u8(v, hi)));
      })
    }
    Be16::GrayAlpha => {
      let order = load(&[0, 0, 0, 2, 4, 4, 4, 6, 8, 8, 8, 10, 12, 12, 12, 14]);
      4 * zip_chunks::<16, 16>(src, dst, |s, d| store(d, a(vqtbl1q_u8(load(s), order))))
    }
    Be16::Rgb => {
      // two tables over 32-byte windows: pixels at bytes 0..24, and at bytes 8..32
      let lo = load(&[
        4, 2, 0, 255, 10, 8, 6, 255, 16, 14, 12, 255, 22, 20, 18, 255,
      ]);
      let hi = load(&[
        12, 10, 8, 255, 18, 16, 14, 255, 24, 22, 20, 255, 30, 28, 26, 255,
      ]);
      8 * zip_chunks::<48, 32>(src, dst, |s, d| {
        let (q0, q1, q2) = (load(s), load(&s[16..]), load(&s[32..]));
        store(d, opaque(vqtbl2q_u8(uint8x16x2_t(q0, q1), lo)));
        store(&mut d[16..], opaque(vqtbl2q_u8(uint8x16x2_t(q1, q2), hi)));
      })
    }
    Be16::Rgba => {
      let order = load(&[4, 2, 0, 6, 12, 10, 8, 14, 20, 18, 16, 22, 28, 26, 24, 30]);
      4 * zip_chunks::<32, 16>(src, dst, |s, d| {
        store(
          d,
          a(vqtbl2q_u8(uint8x16x2_t(load(s), load(&s[16..])), order)),
        );
      })
    }
  }
}

/// A bitfield's shift (negative: vshl shifts right), mask, M and A in each 16-bit lane.
struct Field16 {
  shift: int16x8_t,
  mask: uint16x8_t,
  mul: uint16x8_t,
  add: uint16x8_t,
}

impl Field16 {
  #[inline]
  #[target_feature(enable = "neon")]
  fn new(f: Bitfield) -> Field16 {
    let (shift, mask, mul, add) = f.scale();
    Field16 {
      shift: vdupq_n_s16(-(shift as i16)),
      mask: vdupq_n_u16(mask as u16),
      mul: vdupq_n_u16(mul as u16),
      add: vdupq_n_u16(add as u16),
    }
  }

  /// `Bitfield::read` of 8 words.
  #[inline]
  #[target_feature(enable = "neon")]
  fn read(&self, w: uint16x8_t) -> uint16x8_t {
    let v = vandq_u16(vshlq_u16(w, self.shift), self.mask);
    vshrq_n_u16::<6>(vmlaq_u16(self.add, v, self.mul))
  }
}

/// The same in 32-bit lanes.
struct Field32 {
  shift: int32x4_t,
  mask: uint32x4_t,
  mul: uint32x4_t,
  add: uint32x4_t,
}

impl Field32 {
  #[inline]
  #[target_feature(enable = "neon")]
  fn new(f: Bitfield) -> Field32 {
    let (shift, mask, mul, add) = f.scale();
    Field32 {
      shift: vdupq_n_s32(-(shift as i32)),
      mask: vdupq_n_u32(mask),
      mul: vdupq_n_u32(mul),
      add: vdupq_n_u32(add),
    }
  }

  /// `Bitfield::read` of 4 words.
  #[inline]
  #[target_feature(enable = "neon")]
  fn read(&self, w: uint32x4_t) -> uint32x4_t {
    let v = vandq_u32(vshlq_u32(w, self.shift), self.mask);
    vshrq_n_u32::<6>(vmlaq_u32(self.add, v, self.mul))
  }
}

#[target_feature(enable = "neon")]
pub(crate) fn bitfields16_to_bgra(
  src: &[u8],
  dst: &mut [u8],
  fields: [Bitfield; 4],
  alpha: bool,
) -> usize {
  let [r, g, b, a] = fields.map(|f| Field16::new(f));
  8 * zip_chunks::<16, 32>(src, dst, |s, d| {
    let w = vreinterpretq_u16_u8(load(s));
    // B | G << 8 and R | A << 8 in each 16-bit lane, then the two interleaved
    let bg = vsliq_n_u16::<8>(b.read(w), g.read(w));
    let ra = if alpha {
      vsliq_n_u16::<8>(r.read(w), a.read(w))
    } else {
      vorrq_u16(r.read(w), vdupq_n_u16(0xFF00))
    };
    store(d, vreinterpretq_u8_u16(vzip1q_u16(bg, ra)));
    store(&mut d[16..], vreinterpretq_u8_u16(vzip2q_u16(bg, ra)));
  })
}

#[target_feature(enable = "neon")]
pub(crate) fn bitfields32_to_bgra(
  src: &[u8],
  dst: &mut [u8],
  fields: [Bitfield; 4],
  alpha: bool,
) -> usize {
  let [r, g, b, a] = fields.map(|f| Field32::new(f));
  4 * zip_chunks::<16, 16>(src, dst, |s, d| {
    let w = vreinterpretq_u32_u8(load(s));
    let bgr = vsliq_n_u32::<16>(vsliq_n_u32::<8>(b.read(w), g.read(w)), r.read(w));
    let bgra = if alpha {
      vsliq_n_u32::<24>(bgr, a.read(w))
    } else {
      vorrq_u32(bgr, vdupq_n_u32(0xFF00_0000))
    };
    store(d, vreinterpretq_u8_u32(bgra));
  })
}
