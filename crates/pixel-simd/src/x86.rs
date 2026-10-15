//! x86_64: SSE2 (every x86_64 CPU) and SSSE3 (pshufb). Each kernel does whole vectors from the
//! start and returns how many bytes or pixels it did; the scalar code does the rest.

use core::arch::x86_64::*;

use crate::simd::zip_chunks;
use crate::{Alpha, Be16, Bitfield, Opaque, Palette};

/// The first 16 bytes of `src`.
#[inline]
#[target_feature(enable = "sse2")]
pub(crate) fn load(src: &[u8]) -> __m128i {
  let src = &src[..16];
  // SAFETY: 16 bytes; loadu takes any alignment.
  unsafe { _mm_loadu_si128(src.as_ptr().cast()) }
}

/// `v` to the first 16 bytes of `dst`.
#[inline]
#[target_feature(enable = "sse2")]
pub(crate) fn store(dst: &mut [u8], v: __m128i) {
  let dst = &mut dst[..16];
  // SAFETY: 16 bytes; storeu takes any alignment.
  unsafe { _mm_storeu_si128(dst.as_mut_ptr().cast(), v) }
}

#[inline]
#[target_feature(enable = "sse2")]
fn opaque() -> __m128i {
  _mm_set1_epi32(0xFF00_0000u32 as i32)
}

/// Bytes 0 and 2 of each 4-byte pixel swapped.
#[inline]
#[target_feature(enable = "sse2")]
fn swap_sse2(v: __m128i) -> __m128i {
  let rb = _mm_and_si128(v, _mm_set1_epi32(0x00FF_00FF));
  let br = _mm_shufflehi_epi16::<0xB1>(_mm_shufflelo_epi16::<0xB1>(rb));
  _mm_or_si128(br, _mm_and_si128(v, _mm_set1_epi32(0xFF00_FF00u32 as i32)))
}

/// The 4th byte of each pixel written by `alpha`.
#[inline]
#[target_feature(enable = "sse2")]
fn set_alpha(v: __m128i, alpha: Alpha) -> __m128i {
  match alpha {
    Alpha::Keep => v,
    Alpha::Opaque => _mm_or_si128(v, opaque()),
    Alpha::Key(key) => {
      let first = _mm_and_si128(v, _mm_set1_epi32(0xFF));
      let keyed = _mm_cmpeq_epi32(first, _mm_set1_epi32(key.into()));
      _mm_or_si128(
        _mm_andnot_si128(opaque(), v),
        _mm_andnot_si128(keyed, opaque()),
      )
    }
  }
}

#[target_feature(enable = "sse2")]
pub(crate) fn edit_bgra_sse2(pixels: &mut [u8], swap: bool, alpha: Alpha) -> usize {
  let (vectors, _) = pixels.as_chunks_mut::<16>();
  for v in vectors.iter_mut() {
    let x = load(v);
    store(v, set_alpha(if swap { swap_sse2(x) } else { x }, alpha));
  }
  16 * vectors.len()
}

#[target_feature(enable = "ssse3")]
pub(crate) fn edit_bgra_ssse3(pixels: &mut [u8], swap: bool, alpha: Alpha) -> usize {
  let order = _mm_setr_epi8(2, 1, 0, 3, 6, 5, 4, 7, 10, 9, 8, 11, 14, 13, 12, 15);
  let (vectors, _) = pixels.as_chunks_mut::<16>();
  for v in vectors.iter_mut() {
    let x = load(v);
    store(
      v,
      set_alpha(if swap { _mm_shuffle_epi8(x, order) } else { x }, alpha),
    );
  }
  16 * vectors.len()
}

/// [R, G, B, x] pixels to [B, G, R, 255].
#[inline]
#[target_feature(enable = "sse2")]
fn rgbx_to_bgr255(v: __m128i) -> __m128i {
  _mm_or_si128(swap_sse2(v), opaque())
}

/// Bytes 0..12 of `v`, 4 packed 3-byte pixels, as 4 pixels [p0, p1, p2, x].
#[inline]
#[target_feature(enable = "sse2")]
fn spread24(v: __m128i) -> __m128i {
  let b = _mm_srli_si128::<3>(v);
  let c = _mm_srli_si128::<6>(v);
  let d = _mm_srli_si128::<9>(v);
  _mm_unpacklo_epi64(_mm_unpacklo_epi32(v, b), _mm_unpacklo_epi32(c, d))
}

#[target_feature(enable = "sse2")]
pub(crate) fn be16_to_bgra_sse2(src: &[u8], dst: &mut [u8], layout: Be16, alpha: bool) -> usize {
  // The high byte of a big-endian sample is the low byte of a little-endian 16-bit lane.
  let high = |s: &[u8]| _mm_and_si128(load(s), _mm_set1_epi16(0xFF));
  match layout {
    Be16::Gray => {
      let ff = _mm_set1_epi8(-1);
      16 * zip_chunks::<32, 64>(src, dst, |s, d| {
        let h = _mm_packus_epi16(high(s), high(&s[16..]));
        let (hh, ha) = (_mm_unpacklo_epi8(h, h), _mm_unpacklo_epi8(h, ff));
        store(d, _mm_unpacklo_epi16(hh, ha));
        store(&mut d[16..], _mm_unpackhi_epi16(hh, ha));
        let (hh, ha) = (_mm_unpackhi_epi8(h, h), _mm_unpackhi_epi8(h, ff));
        store(&mut d[32..], _mm_unpacklo_epi16(hh, ha));
        store(&mut d[48..], _mm_unpackhi_epi16(hh, ha));
      })
    }
    Be16::GrayAlpha => {
      let a = if alpha { _mm_setzero_si128() } else { opaque() };
      4 * zip_chunks::<16, 16>(src, dst, |s, d| {
        let v = load(s);
        let g = _mm_and_si128(v, _mm_set1_epi32(0xFF));
        let ggg = _mm_or_si128(
          _mm_or_si128(g, _mm_slli_epi32::<8>(g)),
          _mm_slli_epi32::<16>(g),
        );
        let va = _mm_and_si128(_mm_slli_epi32::<8>(v), opaque());
        store(d, _mm_or_si128(ggg, _mm_or_si128(va, a)));
      })
    }
    Be16::Rgb => {
      8 * zip_chunks::<48, 32>(src, dst, |s, d| {
        let p0 = _mm_packus_epi16(high(s), high(&s[16..]));
        let p1 = _mm_packus_epi16(high(&s[32..]), _mm_setzero_si128());
        let p4 = _mm_or_si128(_mm_srli_si128::<12>(p0), _mm_slli_si128::<4>(p1));
        store(d, rgbx_to_bgr255(spread24(p0)));
        store(&mut d[16..], rgbx_to_bgr255(spread24(p4)));
      })
    }
    Be16::Rgba => {
      4 * zip_chunks::<32, 16>(src, dst, |s, d| {
        let v = swap_sse2(_mm_packus_epi16(high(s), high(&s[16..])));
        store(d, if alpha { v } else { _mm_or_si128(v, opaque()) });
      })
    }
  }
}

#[target_feature(enable = "ssse3")]
pub(crate) fn be16_to_bgra_ssse3(src: &[u8], dst: &mut [u8], layout: Be16, alpha: bool) -> usize {
  // pshufb: -1 gives 0, which `a` then sets to 255 where there is no alpha
  let a = if alpha && matches!(layout, Be16::GrayAlpha | Be16::Rgba) {
    _mm_setzero_si128()
  } else {
    opaque()
  };
  match layout {
    Be16::Gray => {
      let lo = _mm_setr_epi8(0, 0, 0, -1, 2, 2, 2, -1, 4, 4, 4, -1, 6, 6, 6, -1);
      let hi = _mm_setr_epi8(8, 8, 8, -1, 10, 10, 10, -1, 12, 12, 12, -1, 14, 14, 14, -1);
      8 * zip_chunks::<16, 32>(src, dst, |s, d| {
        let v = load(s);
        store(d, _mm_or_si128(_mm_shuffle_epi8(v, lo), a));
        store(&mut d[16..], _mm_or_si128(_mm_shuffle_epi8(v, hi), a));
      })
    }
    Be16::GrayAlpha => {
      let order = _mm_setr_epi8(0, 0, 0, 2, 4, 4, 4, 6, 8, 8, 8, 10, 12, 12, 12, 14);
      4 * zip_chunks::<16, 16>(src, dst, |s, d| {
        store(d, _mm_or_si128(_mm_shuffle_epi8(load(s), order), a));
      })
    }
    Be16::Rgb => {
      let lo = _mm_setr_epi8(4, 2, 0, -1, 10, 8, 6, -1, -1, -1, -1, -1, -1, -1, -1, -1);
      let hi = _mm_setr_epi8(-1, -1, -1, -1, -1, -1, -1, -1, 8, 6, 4, -1, 14, 12, 10, -1);
      4 * zip_chunks::<24, 16>(src, dst, |s, d| {
        let v = _mm_or_si128(
          _mm_shuffle_epi8(load(s), lo),
          _mm_shuffle_epi8(load(&s[8..]), hi),
        );
        store(d, _mm_or_si128(v, a));
      })
    }
    Be16::Rgba => {
      let lo = _mm_setr_epi8(4, 2, 0, 6, 12, 10, 8, 14, -1, -1, -1, -1, -1, -1, -1, -1);
      let hi = _mm_setr_epi8(-1, -1, -1, -1, -1, -1, -1, -1, 4, 2, 0, 6, 12, 10, 8, 14);
      4 * zip_chunks::<32, 16>(src, dst, |s, d| {
        let v = _mm_or_si128(
          _mm_shuffle_epi8(load(s), lo),
          _mm_shuffle_epi8(load(&s[16..]), hi),
        );
        store(d, _mm_or_si128(v, a));
      })
    }
  }
}

/// A bitfield's shift, mask, M and A (see `Bitfield::scale`), the last three in each lane.
struct Field {
  shift: __m128i,
  mask: __m128i,
  mul: __m128i,
  add: __m128i,
}

impl Field {
  /// For 16-bit lanes.
  #[inline]
  #[target_feature(enable = "sse2")]
  fn lanes16(f: Bitfield) -> Field {
    let (shift, mask, mul, add) = f.scale();
    Field {
      shift: _mm_cvtsi32_si128(shift as i32),
      mask: _mm_set1_epi16(mask as i16),
      mul: _mm_set1_epi16(mul as i16),
      add: _mm_set1_epi16(add as i16),
    }
  }

  /// For 32-bit lanes.
  #[inline]
  #[target_feature(enable = "sse2")]
  fn lanes32(f: Bitfield) -> Field {
    let (shift, mask, mul, add) = f.scale();
    Field {
      shift: _mm_cvtsi32_si128(shift as i32),
      mask: _mm_set1_epi32(mask as i32),
      mul: _mm_set1_epi32(mul as i32),
      add: _mm_set1_epi32(add as i32),
    }
  }

  /// `Bitfield::read` of 8 16-bit words.
  #[inline]
  #[target_feature(enable = "sse2")]
  fn read16(&self, w: __m128i) -> __m128i {
    let v = _mm_and_si128(_mm_srl_epi16(w, self.shift), self.mask);
    _mm_srli_epi16::<6>(_mm_add_epi16(_mm_mullo_epi16(v, self.mul), self.add))
  }

  /// `Bitfield::read` of 4 32-bit words. v * M < 2^14 sits in the low 16 bits of each lane and
  /// the high 16 bits are 0 * 0, so the 16-bit multiply is exact.
  #[inline]
  #[target_feature(enable = "sse2")]
  fn read32(&self, w: __m128i) -> __m128i {
    let v = _mm_and_si128(_mm_srl_epi32(w, self.shift), self.mask);
    _mm_srli_epi32::<6>(_mm_add_epi32(_mm_mullo_epi16(v, self.mul), self.add))
  }
}

#[target_feature(enable = "sse2")]
pub(crate) fn bitfields16_to_bgra_sse2(
  src: &[u8],
  dst: &mut [u8],
  fields: [Bitfield; 4],
  alpha: bool,
) -> usize {
  let [r, g, b, a] = fields.map(|f| Field::lanes16(f));
  8 * zip_chunks::<16, 32>(src, dst, |s, d| {
    let w = load(s);
    let aa = if alpha {
      a.read16(w)
    } else {
      _mm_set1_epi16(0xFF)
    };
    let bg = _mm_or_si128(b.read16(w), _mm_slli_epi16::<8>(g.read16(w)));
    let ra = _mm_or_si128(r.read16(w), _mm_slli_epi16::<8>(aa));
    store(d, _mm_unpacklo_epi16(bg, ra));
    store(&mut d[16..], _mm_unpackhi_epi16(bg, ra));
  })
}

#[target_feature(enable = "sse2")]
pub(crate) fn bitfields32_to_bgra_sse2(
  src: &[u8],
  dst: &mut [u8],
  fields: [Bitfield; 4],
  alpha: bool,
) -> usize {
  let [r, g, b, a] = fields.map(|f| Field::lanes32(f));
  4 * zip_chunks::<16, 16>(src, dst, |s, d| {
    let w = load(s);
    let aa = if alpha {
      _mm_slli_epi32::<24>(a.read32(w))
    } else {
      opaque()
    };
    let bg = _mm_or_si128(b.read32(w), _mm_slli_epi32::<8>(g.read32(w)));
    store(
      d,
      _mm_or_si128(_mm_or_si128(bg, _mm_slli_epi32::<16>(r.read32(w))), aa),
    );
  })
}

#[target_feature(enable = "sse2")]
pub(crate) fn opaque_to_bgra_sse2(src: &[u8], dst: &mut [u8], layout: Opaque) -> usize {
  match layout {
    // 4 pixels from the first 12 bytes of each load; the last load ends with the 48 bytes
    Opaque::Bgr => {
      16 * zip_chunks::<48, 64>(src, dst, |s, d| {
        let last = _mm_srli_si128::<4>(load(&s[32..]));
        for (i, v) in [load(s), load(&s[12..]), load(&s[24..]), last]
          .into_iter()
          .enumerate()
        {
          store(&mut d[16 * i..], _mm_or_si128(spread24(v), opaque()));
        }
      })
    }
    Opaque::Bgrx => {
      4 * zip_chunks::<16, 16>(src, dst, |s, d| store(d, _mm_or_si128(load(s), opaque())))
    }
    Opaque::Xbgr => {
      4 * zip_chunks::<16, 16>(src, dst, |s, d| {
        store(d, _mm_or_si128(_mm_srli_epi32::<8>(load(s)), opaque()))
      })
    }
  }
}

#[target_feature(enable = "ssse3")]
pub(crate) fn opaque_to_bgra_ssse3(src: &[u8], dst: &mut [u8], layout: Opaque) -> usize {
  if layout != Opaque::Bgr {
    return opaque_to_bgra_sse2(src, dst, layout);
  }
  // pshufb: -1 gives 0, which `opaque` then sets to 255
  let first = _mm_setr_epi8(0, 1, 2, -1, 3, 4, 5, -1, 6, 7, 8, -1, 9, 10, 11, -1);
  let last = _mm_setr_epi8(4, 5, 6, -1, 7, 8, 9, -1, 10, 11, 12, -1, 13, 14, 15, -1);
  16 * zip_chunks::<48, 64>(src, dst, |s, d| {
    for (i, (at, order)) in [(0, first), (12, first), (24, first), (32, last)]
      .into_iter()
      .enumerate()
    {
      let v = _mm_shuffle_epi8(load(&s[at..]), order);
      store(&mut d[16 * i..], _mm_or_si128(v, opaque()));
    }
  })
}

/// The B, G and R planes of a palette's first 16 * `T` colors.
struct Planes<const T: usize>([[__m128i; T]; 3]);

impl<const T: usize> Planes<T> {
  #[inline]
  #[target_feature(enable = "ssse3")]
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
  #[target_feature(enable = "ssse3")]
  fn store(&self, dst: &mut [u8], index: __m128i) {
    let mut bgr = [_mm_setzero_si128(); 3];
    for k in 0..T {
      // pshufb gives 0 where the high bit is set: the indexes 16k to 16k + 15 keep their low 4
      // bits under it, and the others saturate to 128 or more
      let i = _mm_sub_epi8(index, _mm_set1_epi8((16 * k) as i8));
      let i = _mm_adds_epu8(i, _mm_set1_epi8(0x70));
      for (c, plane) in bgr.iter_mut().zip(&self.0) {
        *c = _mm_or_si128(*c, _mm_shuffle_epi8(plane[k], i));
      }
    }
    let [b, g, r] = bgr;
    let a = _mm_set1_epi8(-1);
    let bg = [_mm_unpacklo_epi8(b, g), _mm_unpackhi_epi8(b, g)];
    let ra = [_mm_unpacklo_epi8(r, a), _mm_unpackhi_epi8(r, a)];
    for (i, (bg, ra)) in bg.into_iter().zip(ra).enumerate() {
      store(&mut dst[32 * i..], _mm_unpacklo_epi16(bg, ra));
      store(&mut dst[32 * i + 16..], _mm_unpackhi_epi16(bg, ra));
    }
  }
}

/// The largest palette of 8-bit indexes that the lookups below take.
pub(crate) const PALETTE_COLORS: usize = 32;

#[target_feature(enable = "ssse3")]
pub(crate) fn palette_to_bgra_ssse3(
  src: &[u8],
  dst: &mut [u8],
  bits: u32,
  palette: &Palette,
) -> usize {
  // 1, 2 and 4-bit indexes are under 16: one register per plane
  match bits {
    1 => {
      let p = Planes::<1>::new(palette);
      let bit = _mm_setr_epi8(-128, 64, 32, 16, 8, 4, 2, 1, -128, 64, 32, 16, 8, 4, 2, 1);
      128
        * zip_chunks::<16, 512>(src, dst, |s, d| {
          let x = load(s);
          for i in 0..8 {
            // bytes 2i and 2i + 1, 8 times each, to 16 indexes of 0 or 1
            let bytes = 0x0101_0101_0101_0101 * i;
            let pair = _mm_shuffle_epi8(
              x,
              _mm_set_epi64x(2 * bytes + 0x0101_0101_0101_0101, 2 * bytes),
            );
            let set = _mm_cmpeq_epi8(_mm_and_si128(pair, bit), bit);
            p.store(
              &mut d[64 * i as usize..],
              _mm_and_si128(set, _mm_set1_epi8(1)),
            );
          }
        })
    }
    2 => {
      let p = Planes::<1>::new(palette);
      64 * zip_chunks::<16, 256>(src, dst, |s, d| {
        let (x, three) = (load(s), _mm_set1_epi8(3));
        let a = _mm_and_si128(_mm_srli_epi16::<6>(x), three);
        let b = _mm_and_si128(_mm_srli_epi16::<4>(x), three);
        let c = _mm_and_si128(_mm_srli_epi16::<2>(x), three);
        let e = _mm_and_si128(x, three);
        // a, b, c, e of each byte in turn
        let ab = [_mm_unpacklo_epi8(a, b), _mm_unpackhi_epi8(a, b)];
        let ce = [_mm_unpacklo_epi8(c, e), _mm_unpackhi_epi8(c, e)];
        for (i, (ab, ce)) in ab.into_iter().zip(ce).enumerate() {
          p.store(&mut d[128 * i..], _mm_unpacklo_epi16(ab, ce));
          p.store(&mut d[128 * i + 64..], _mm_unpackhi_epi16(ab, ce));
        }
      })
    }
    4 => {
      let p = Planes::<1>::new(palette);
      32 * zip_chunks::<16, 128>(src, dst, |s, d| {
        let (x, fifteen) = (load(s), _mm_set1_epi8(15));
        let high = _mm_and_si128(_mm_srli_epi16::<4>(x), fifteen);
        let low = _mm_and_si128(x, fifteen);
        p.store(d, _mm_unpacklo_epi8(high, low));
        p.store(&mut d[64..], _mm_unpackhi_epi8(high, low));
      })
    }
    // A pshufb per 16 colors: past 32 colors the scalar code is faster.
    _ => match palette.len {
      0..=16 => indexes::<1>(src, dst, palette),
      17..=PALETTE_COLORS => indexes::<2>(src, dst, palette),
      _ => 0,
    },
  }
}

/// 8-bit indexes, with a palette of at most 16 * `T` colors.
#[inline]
#[target_feature(enable = "ssse3")]
fn indexes<const T: usize>(src: &[u8], dst: &mut [u8], palette: &Palette) -> usize {
  let p = Planes::<T>::new(palette);
  16 * zip_chunks::<16, 64>(src, dst, |s, d| p.store(d, load(s)))
}
