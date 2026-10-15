//! aarch64 NEON (part of every aarch64 target). Each kernel does whole vectors from the start and
//! returns how many bytes or pixels it did; the scalar code does the rest.

use core::arch::aarch64::*;

use crate::simd::zip_chunks;
use crate::{Alpha, Be16, Bitfield, Opaque, Palette};

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

#[target_feature(enable = "neon")]
pub(crate) fn opaque_to_bgra(src: &[u8], dst: &mut [u8], layout: Opaque) -> usize {
  match layout {
    Opaque::Bgr => {
      // 16 pixels from 3 vectors, through tables over 1 or 2 of them: pixels 0 to 3 in the 1st,
      // 4 to 7 in the 1st and 2nd, 8 to 11 in the 2nd and 3rd, 12 to 15 in the 3rd. vqtbl gives
      // 0 for index 255, which `opaque` then sets to 255.
      let order = [
        [0, 1, 2, 255, 3, 4, 5, 255, 6, 7, 8, 255, 9, 10, 11, 255],
        [
          12, 13, 14, 255, 15, 16, 17, 255, 18, 19, 20, 255, 21, 22, 23, 255,
        ],
        [
          8, 9, 10, 255, 11, 12, 13, 255, 14, 15, 16, 255, 17, 18, 19, 255,
        ],
        [4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255, 13, 14, 15, 255],
      ]
      .map(|order| load(&order));
      16 * zip_chunks::<48, 64>(src, dst, |s, d| {
        let (q0, q1, q2) = (load(s), load(&s[16..]), load(&s[32..]));
        store(d, opaque(vqtbl1q_u8(q0, order[0])));
        store(
          &mut d[16..],
          opaque(vqtbl2q_u8(uint8x16x2_t(q0, q1), order[1])),
        );
        store(
          &mut d[32..],
          opaque(vqtbl2q_u8(uint8x16x2_t(q1, q2), order[2])),
        );
        store(&mut d[48..], opaque(vqtbl1q_u8(q2, order[3])));
      })
    }
    Opaque::Bgrx => 4 * zip_chunks::<16, 16>(src, dst, |s, d| store(d, opaque(load(s)))),
    Opaque::Xbgr => {
      4 * zip_chunks::<16, 16>(src, dst, |s, d| {
        let bgr = vshrq_n_u32::<8>(vreinterpretq_u32_u8(load(s)));
        store(d, opaque(vreinterpretq_u8_u32(bgr)));
      })
    }
  }
}

/// The B, G and R planes of a palette's first 16 * `T` colors (`T`: 1 to 4, or 8).
struct Planes<const T: usize>([[uint8x16_t; T]; 3]);

impl<const T: usize> Planes<T> {
  #[inline]
  #[target_feature(enable = "neon")]
  fn new(palette: &Palette) -> Planes<T> {
    Planes(
      palette
        .planes
        .each_ref()
        .map(|plane| core::array::from_fn(|i| load(&plane[16 * i..]))),
    )
  }

  /// Plane `c` at 16 indexes; 0 past the plane.
  #[inline]
  #[target_feature(enable = "neon")]
  fn get(&self, c: usize, index: uint8x16_t) -> uint8x16_t {
    let t = &self.0[c];
    match T {
      1 => vqtbl1q_u8(t[0], index),
      2 => vqtbl2q_u8(uint8x16x2_t(t[0], t[1]), index),
      3 => vqtbl3q_u8(uint8x16x3_t(t[0], t[1], t[2]), index),
      4 => vqtbl4q_u8(uint8x16x4_t(t[0], t[1], t[2], t[3]), index),
      // then colors 64 to 127, where the index minus 64 is under 64
      _ => vqtbx4q_u8(
        vqtbl4q_u8(uint8x16x4_t(t[0], t[1], t[2], t[3]), index),
        uint8x16x4_t(t[4], t[5], t[6], t[7]),
        vsubq_u8(index, vdupq_n_u8(64)),
      ),
    }
  }

  /// The colors of 16 indexes, as BGRA pixels at the start of `dst`.
  #[inline]
  #[target_feature(enable = "neon")]
  fn store(&self, dst: &mut [u8], index: uint8x16_t) {
    let [b, g, r] = [0, 1, 2].map(|c| self.get(c, index));
    let a = vdupq_n_u8(255);
    let bg = [vzip1q_u8(b, g), vzip2q_u8(b, g)].map(|v| vreinterpretq_u16_u8(v));
    let ra = [vzip1q_u8(r, a), vzip2q_u8(r, a)].map(|v| vreinterpretq_u16_u8(v));
    for (i, (bg, ra)) in bg.into_iter().zip(ra).enumerate() {
      store(&mut dst[32 * i..], vreinterpretq_u8_u16(vzip1q_u16(bg, ra)));
      store(
        &mut dst[32 * i + 16..],
        vreinterpretq_u8_u16(vzip2q_u16(bg, ra)),
      );
    }
  }
}

#[target_feature(enable = "neon")]
pub(crate) fn palette_to_bgra(src: &[u8], dst: &mut [u8], bits: u32, palette: &Palette) -> usize {
  // 1, 2 and 4-bit indexes are under 16: one register per plane
  match bits {
    1 => {
      let p = Planes::<1>::new(palette);
      let bit = load(&[128, 64, 32, 16, 8, 4, 2, 1, 128, 64, 32, 16, 8, 4, 2, 1]);
      128
        * zip_chunks::<16, 512>(src, dst, |s, d| {
          let x = load(s);
          for i in 0..8 {
            // bytes 2i and 2i + 1, 8 times each, to 16 indexes of 0 or 1
            let pair = vqtbl1q_u8(x, vcombine_u8(vdup_n_u8(2 * i), vdup_n_u8(2 * i + 1)));
            let index = vandq_u8(vtstq_u8(pair, bit), vdupq_n_u8(1));
            p.store(&mut d[64 * usize::from(i)..], index);
          }
        })
    }
    2 => {
      let p = Planes::<1>::new(palette);
      64 * zip_chunks::<16, 256>(src, dst, |s, d| {
        let (x, three) = (load(s), vdupq_n_u8(3));
        let a = vshrq_n_u8::<6>(x);
        let b = vandq_u8(vshrq_n_u8::<4>(x), three);
        let c = vandq_u8(vshrq_n_u8::<2>(x), three);
        let e = vandq_u8(x, three);
        // a, b, c, e of each byte in turn
        let ab = [vzip1q_u8(a, b), vzip2q_u8(a, b)].map(|v| vreinterpretq_u16_u8(v));
        let ce = [vzip1q_u8(c, e), vzip2q_u8(c, e)].map(|v| vreinterpretq_u16_u8(v));
        for (i, (ab, ce)) in ab.into_iter().zip(ce).enumerate() {
          p.store(&mut d[128 * i..], vreinterpretq_u8_u16(vzip1q_u16(ab, ce)));
          p.store(
            &mut d[128 * i + 64..],
            vreinterpretq_u8_u16(vzip2q_u16(ab, ce)),
          );
        }
      })
    }
    4 => {
      let p = Planes::<1>::new(palette);
      32 * zip_chunks::<16, 128>(src, dst, |s, d| {
        let x = load(s);
        let (high, low) = (vshrq_n_u8::<4>(x), vandq_u8(x, vdupq_n_u8(15)));
        p.store(d, vzip1q_u8(high, low));
        p.store(&mut d[64..], vzip2q_u8(high, low));
      })
    }
    // A larger table is slower to look up: past 128 colors the scalar code is faster.
    _ => match palette.len {
      0..=16 => indexes::<1>(src, dst, palette),
      17..=32 => indexes::<2>(src, dst, palette),
      33..=48 => indexes::<3>(src, dst, palette),
      49..=64 => indexes::<4>(src, dst, palette),
      65..=128 => indexes::<8>(src, dst, palette),
      _ => 0,
    },
  }
}

/// 8-bit indexes, with a palette of at most 16 * `T` colors.
#[inline]
#[target_feature(enable = "neon")]
fn indexes<const T: usize>(src: &[u8], dst: &mut [u8], palette: &Palette) -> usize {
  let p = Planes::<T>::new(palette);
  16 * zip_chunks::<16, 64>(src, dst, |s, d| p.store(d, load(s)))
}
