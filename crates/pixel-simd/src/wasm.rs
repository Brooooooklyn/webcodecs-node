//! wasm32 simd128 (in a build made with `-C target-feature=+simd128`: wasm has no run-time
//! check). Each kernel does whole vectors from the start and returns how many bytes or pixels it
//! did; the scalar code does the rest.

use core::arch::wasm32::*;

use crate::simd::zip_chunks;
use crate::{Alpha, Be16, Bitfield, Opaque, Palette};

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

pub(crate) fn opaque_to_bgra(src: &[u8], dst: &mut [u8], layout: Opaque) -> usize {
  match layout {
    // 4 pixels from the first 12 bytes of each load and 255 (byte 0 of the second vector); the
    // last load ends with the 48 bytes
    Opaque::Bgr => {
      16 * zip_chunks::<48, 64>(src, dst, |s, d| {
        let ff = u8x16_splat(255);
        for (i, at) in [0, 12, 24].into_iter().enumerate() {
          let v = load(&s[at..]);
          let v = i8x16_shuffle::<0, 1, 2, 16, 3, 4, 5, 16, 6, 7, 8, 16, 9, 10, 11, 16>(v, ff);
          store(&mut d[16 * i..], v);
        }
        let v = load(&s[32..]);
        let v = i8x16_shuffle::<4, 5, 6, 16, 7, 8, 9, 16, 10, 11, 12, 16, 13, 14, 15, 16>(v, ff);
        store(&mut d[48..], v);
      })
    }
    Opaque::Bgrx => 4 * zip_chunks::<16, 16>(src, dst, |s, d| store(d, opaque(load(s)))),
    // a shuffle: wasm engines are slower at the 32-bit shift
    Opaque::Xbgr => {
      4 * zip_chunks::<16, 16>(src, dst, |s, d| {
        let v = load(s);
        let v = i8x16_shuffle::<1, 2, 3, 0, 5, 6, 7, 4, 9, 10, 11, 8, 13, 14, 15, 12>(v, v);
        store(d, opaque(v));
      })
    }
  }
}

/// The B, G and R planes of a palette's first 16 * `T` colors.
struct Planes<const T: usize>([[v128; T]; 3]);

impl<const T: usize> Planes<T> {
  #[inline]
  fn new(palette: &Palette) -> Planes<T> {
    Planes(
      palette
        .planes
        .each_ref()
        .map(|plane| core::array::from_fn(|i| load(&plane[16 * i..]))),
    )
  }

  /// The colors of 16 indexes, as BGRA pixels at the start of `dst`; black past the planes.
  #[inline]
  fn store(&self, dst: &mut [u8], index: v128) {
    let mut bgr = [u8x16_splat(0); 3];
    for k in 0..T {
      // swizzle gives 0 for 16 or more: the indexes past 16k + 15, and (wrapping) under 16k
      let i = u8x16_sub(index, u8x16_splat((16 * k) as u8));
      for (c, plane) in bgr.iter_mut().zip(&self.0) {
        *c = v128_or(*c, u8x16_swizzle(plane[k], i));
      }
    }
    let [b, g, r] = bgr;
    let a = u8x16_splat(255);
    let bg = [
      i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(b, g),
      i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(b, g),
    ];
    let ra = [
      i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(r, a),
      i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(r, a),
    ];
    for (i, (bg, ra)) in bg.into_iter().zip(ra).enumerate() {
      store(
        &mut dst[32 * i..],
        i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(bg, ra),
      );
      store(
        &mut dst[32 * i + 16..],
        i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(bg, ra),
      );
    }
  }
}

/// The largest palette of 8-bit indexes that the lookups below take.
pub(crate) const PALETTE_COLORS: usize = 64;

pub(crate) fn palette_to_bgra(src: &[u8], dst: &mut [u8], bits: u32, palette: &Palette) -> usize {
  // 1, 2 and 4-bit indexes are under 16: one register per plane
  match bits {
    1 => {
      let p = Planes::<1>::new(palette);
      let bit = u8x16(128, 64, 32, 16, 8, 4, 2, 1, 128, 64, 32, 16, 8, 4, 2, 1);
      128
        * zip_chunks::<16, 512>(src, dst, |s, d| {
          let x = load(s);
          for i in 0..8 {
            // bytes 2i and 2i + 1, 8 times each, to 16 indexes of 0 or 1
            let (lo, hi) = (2 * i, 2 * i + 1);
            let pair = u8x16_swizzle(
              x,
              u8x16(
                lo, lo, lo, lo, lo, lo, lo, lo, hi, hi, hi, hi, hi, hi, hi, hi,
              ),
            );
            let set = u8x16_eq(v128_and(pair, bit), bit);
            p.store(&mut d[64 * usize::from(i)..], v128_and(set, u8x16_splat(1)));
          }
        })
    }
    2 => {
      let p = Planes::<1>::new(palette);
      64 * zip_chunks::<16, 256>(src, dst, |s, d| {
        let (x, three) = (load(s), u8x16_splat(3));
        let a = u8x16_shr(x, 6);
        let b = v128_and(u8x16_shr(x, 4), three);
        let c = v128_and(u8x16_shr(x, 2), three);
        let e = v128_and(x, three);
        // a, b, c, e of each byte in turn
        let ab = [
          i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(a, b),
          i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(a, b),
        ];
        let ce = [
          i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(c, e),
          i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(c, e),
        ];
        for (i, (ab, ce)) in ab.into_iter().zip(ce).enumerate() {
          p.store(
            &mut d[128 * i..],
            i16x8_shuffle::<0, 8, 1, 9, 2, 10, 3, 11>(ab, ce),
          );
          p.store(
            &mut d[128 * i + 64..],
            i16x8_shuffle::<4, 12, 5, 13, 6, 14, 7, 15>(ab, ce),
          );
        }
      })
    }
    4 => {
      let p = Planes::<1>::new(palette);
      32 * zip_chunks::<16, 128>(src, dst, |s, d| {
        let x = load(s);
        let (high, low) = (u8x16_shr(x, 4), v128_and(x, u8x16_splat(15)));
        p.store(
          d,
          i8x16_shuffle::<0, 16, 1, 17, 2, 18, 3, 19, 4, 20, 5, 21, 6, 22, 7, 23>(high, low),
        );
        p.store(
          &mut d[64..],
          i8x16_shuffle::<8, 24, 9, 25, 10, 26, 11, 27, 12, 28, 13, 29, 14, 30, 15, 31>(high, low),
        );
      })
    }
    // A swizzle per 16 colors: past 64 colors the scalar code is faster.
    _ => match palette.len {
      0..=16 => indexes::<1>(src, dst, palette),
      17..=32 => indexes::<2>(src, dst, palette),
      33..=48 => indexes::<3>(src, dst, palette),
      49..=PALETTE_COLORS => indexes::<4>(src, dst, palette),
      _ => 0,
    },
  }
}

/// 8-bit indexes, with a palette of at most 16 * `T` colors.
#[inline]
fn indexes<const T: usize>(src: &[u8], dst: &mut [u8], palette: &Palette) -> usize {
  let p = Planes::<T>::new(palette);
  16 * zip_chunks::<16, 64>(src, dst, |s, d| p.store(d, load(s)))
}
