//! wasm32 simd128 (in a build made with `-C target-feature=+simd128`: wasm has no run-time
//! check). Each kernel does whole vectors from the start and returns how many bytes or pixels it
//! did; the scalar code does the rest.

use core::arch::wasm32::*;

use crate::simd::zip_chunks;
use crate::{Alpha, Be16, Bitfield};

/// The first 16 bytes of `src`.
#[inline]
pub(crate) fn load(src: &[u8]) -> v128 {
  let src = &src[..16];
  // SAFETY: 16 bytes; v128_load takes any alignment.
  unsafe { v128_load(src.as_ptr().cast()) }
}

/// `v` to the first 16 bytes of `dst`.
#[inline]
pub(crate) fn store(dst: &mut [u8], v: v128) {
  let dst = &mut dst[..16];
  // SAFETY: 16 bytes; v128_store takes any alignment.
  unsafe { v128_store(dst.as_mut_ptr().cast(), v) }
}

/// `v` with 255 in the 4th byte of each 4-byte pixel.
#[inline]
fn opaque(v: v128) -> v128 {
  v128_or(v, u32x4_splat(0xFF00_0000))
}

/// The 4th byte of each pixel written by `alpha`.
#[inline]
fn set_alpha(v: v128, alpha: Alpha) -> v128 {
  match alpha {
    Alpha::Keep => v,
    Alpha::Opaque => opaque(v),
    Alpha::Key(key) => {
      let a = u32x4_splat(0xFF00_0000);
      let keyed = i32x4_eq(v128_and(v, u32x4_splat(0xFF)), u32x4_splat(key.into()));
      v128_or(v128_andnot(v, a), v128_andnot(a, keyed))
    }
  }
}

pub(crate) fn edit_bgra(pixels: &mut [u8], swap: bool, alpha: Alpha) -> usize {
  let (vectors, _) = pixels.as_chunks_mut::<16>();
  for v in vectors.iter_mut() {
    let x = load(v);
    let x = if swap {
      i8x16_shuffle::<2, 1, 0, 3, 6, 5, 4, 7, 10, 9, 8, 11, 14, 13, 12, 15>(x, x)
    } else {
      x
    };
    store(v, set_alpha(x, alpha));
  }
  16 * vectors.len()
}

pub(crate) fn be16_to_bgra(src: &[u8], dst: &mut [u8], layout: Be16, alpha: bool) -> usize {
  // The high byte of a big-endian sample comes first. Index 16 of a two-vector shuffle is byte 0
  // of the second vector.
  let a = |v| if alpha { v } else { opaque(v) };
  match layout {
    Be16::Gray => {
      8 * zip_chunks::<16, 32>(src, dst, |s, d| {
        let (v, ff) = (load(s), u8x16_splat(255));
        let lo = i8x16_shuffle::<0, 0, 0, 16, 2, 2, 2, 16, 4, 4, 4, 16, 6, 6, 6, 16>(v, ff);
        let hi =
          i8x16_shuffle::<8, 8, 8, 16, 10, 10, 10, 16, 12, 12, 12, 16, 14, 14, 14, 16>(v, ff);
        store(d, lo);
        store(&mut d[16..], hi);
      })
    }
    Be16::GrayAlpha => {
      4 * zip_chunks::<16, 16>(src, dst, |s, d| {
        let v = load(s);
        let v = i8x16_shuffle::<0, 0, 0, 2, 4, 4, 4, 6, 8, 8, 8, 10, 12, 12, 12, 14>(v, v);
        store(d, a(v));
      })
    }
    // pixels 0 and 1 from bytes 0..12, pixels 2 and 3 from bytes 8..24
    Be16::Rgb => {
      4 * zip_chunks::<24, 16>(src, dst, |s, d| {
        let (lo, hi) = (load(s), load(&s[8..]));
        let v = i8x16_shuffle::<4, 2, 0, 0, 10, 8, 6, 0, 24, 22, 20, 0, 30, 28, 26, 0>(lo, hi);
        store(d, opaque(v));
      })
    }
    Be16::Rgba => {
      4 * zip_chunks::<32, 16>(src, dst, |s, d| {
        let (lo, hi) = (load(s), load(&s[16..]));
        let v = i8x16_shuffle::<4, 2, 0, 6, 12, 10, 8, 14, 20, 18, 16, 22, 28, 26, 24, 30>(lo, hi);
        store(d, a(v));
      })
    }
  }
}

/// A bitfield's shift, and its mask, M and A in each lane (see `Bitfield::scale`).
#[derive(Clone, Copy)]
struct Field {
  shift: u32,
  mask: v128,
  mul: v128,
  add: v128,
}

impl Field {
  #[inline]
  fn lanes16(f: Bitfield) -> Field {
    let (shift, mask, mul, add) = f.scale();
    Field {
      shift,
      mask: u16x8_splat(mask as u16),
      mul: u16x8_splat(mul as u16),
      add: u16x8_splat(add as u16),
    }
  }

  #[inline]
  fn lanes32(f: Bitfield) -> Field {
    let (shift, mask, mul, add) = f.scale();
    Field {
      shift,
      mask: u32x4_splat(mask),
      mul: u32x4_splat(mul),
      add: u32x4_splat(add),
    }
  }

  /// `Bitfield::read` of 8 16-bit words.
  #[inline]
  fn read16(self, w: v128) -> v128 {
    let v = v128_and(u16x8_shr(w, self.shift), self.mask);
    u16x8_shr(i16x8_add(i16x8_mul(v, self.mul), self.add), 6)
  }

  /// `Bitfield::read` of 4 32-bit words.
  #[inline]
  fn read32(self, w: v128) -> v128 {
    let v = v128_and(u32x4_shr(w, self.shift), self.mask);
    u32x4_shr(i32x4_add(i32x4_mul(v, self.mul), self.add), 6)
  }
}

pub(crate) fn bitfields16_to_bgra(
  src: &[u8],
  dst: &mut [u8],
  fields: [Bitfield; 4],
  alpha: bool,
) -> usize {
  let [r, g, b, a] = fields.map(Field::lanes16);
  8 * zip_chunks::<16, 32>(src, dst, |s, d| {
    let w = load(s);
    let aa = if alpha {
      a.read16(w)
    } else {
      u16x8_splat(0xFF)
    };
    let bg = v128_or(b.read16(w), i16x8_shl(g.read16(w), 8));
    let ra = v128_or(r.read16(w), i16x8_shl(aa, 8));
    store(d, i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(bg, ra));
    store(
      &mut d[16..],
      i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(bg, ra),
    );
  })
}

pub(crate) fn bitfields32_to_bgra(
  src: &[u8],
  dst: &mut [u8],
  fields: [Bitfield; 4],
  alpha: bool,
) -> usize {
  let [r, g, b, a] = fields.map(Field::lanes32);
  4 * zip_chunks::<16, 16>(src, dst, |s, d| {
    let w = load(s);
    let aa = if alpha {
      i32x4_shl(a.read32(w), 24)
    } else {
      u32x4_splat(0xFF00_0000)
    };
    let bg = v128_or(b.read32(w), i32x4_shl(g.read32(w), 8));
    store(d, v128_or(v128_or(bg, i32x4_shl(r.read32(w), 16)), aa));
  })
}
