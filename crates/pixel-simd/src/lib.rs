//! The pixel loops of the addon's image decoding (PNG, BMP, lossy WebP and AVIF to BGRA, with
//! Chrome's bytes). Each has SIMD paths that give exactly the bytes of its scalar code, on every
//! input (for the AVIF rows, `avif` lists the few with only their scalar code):
//!
//! - x86_64: SSE2 (part of the target), plus SSSE3 and AVX2 when the CPU has them
//! - aarch64: NEON (part of the target)
//! - wasm32: simd128, when built with `-C target-feature=+simd128`
//! - anything else, armv7 included (stable Rust has no NEON intrinsics there): the scalar code
//!
//! The scalar code is the reference: the tests run every path this CPU has against it.

pub mod avif;
mod isa;
#[cfg(target_arch = "aarch64")]
mod neon;
mod scalar;
#[cfg(any(
  target_arch = "x86_64",
  target_arch = "aarch64",
  all(target_arch = "wasm32", target_feature = "simd128")
))]
mod simd;
#[cfg(test)]
mod tests;
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod wasm;
mod webp;
#[cfg(target_arch = "x86_64")]
mod x86;

#[doc(hidden)]
pub mod bench;

use isa::{Isa, Level};
pub use webp::{Plane, Yuva420, yuva420_to_bgra};

/// What `edit_bgra` writes to the 4th byte of each pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alpha {
  /// Nothing.
  Keep,
  /// 255.
  Opaque,
  /// 0 where the 1st byte (after the swap) is the key, else 255.
  Key(u8),
}

/// Edit packed 4-byte pixels in place: swap their 1st and 3rd bytes (RGBA to BGRA) when `swap`,
/// then write their 4th byte by `alpha`.
pub fn edit_bgra(pixels: &mut [u8], swap: bool, alpha: Alpha) {
  edit_bgra_with(Isa::best(), pixels, swap, alpha)
}

fn edit_bgra_with(isa: Isa, pixels: &mut [u8], swap: bool, alpha: Alpha) {
  // SAFETY (every SIMD arm here and below): an `Isa` is only made for a level this CPU runs.
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 => unsafe { x86::edit_bgra_sse2(pixels, swap, alpha) },
    #[cfg(target_arch = "x86_64")]
    Level::Ssse3 | Level::Avx2 => unsafe { x86::edit_bgra_ssse3(pixels, swap, alpha) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::edit_bgra(pixels, swap, alpha) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::edit_bgra(pixels, swap, alpha),
  };
  scalar::edit_bgra(&mut pixels[done..], swap, alpha);
}

/// The layout of a PNG pixel with 16-bit big-endian samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Be16 {
  Gray,
  GrayAlpha,
  Rgb,
  Rgba,
}

impl Be16 {
  /// Bytes per pixel.
  pub fn size(self) -> usize {
    match self {
      Be16::Gray => 2,
      Be16::GrayAlpha => 4,
      Be16::Rgb => 6,
      Be16::Rgba => 8,
    }
  }
}

/// BGRA pixels from PNG pixels with 16-bit big-endian samples: the high byte of each sample, like
/// Skia. `dst` holds the pixels and `src` at least as many; their 4th byte is 255 without `alpha`.
pub fn be16_to_bgra(src: &[u8], dst: &mut [u8], layout: Be16, alpha: bool) {
  be16_to_bgra_with(Isa::best(), src, dst, layout, alpha)
}

fn be16_to_bgra_with(isa: Isa, src: &[u8], dst: &mut [u8], layout: Be16, alpha: bool) {
  let size = layout.size();
  assert!(
    src.len() / size >= dst.len() / 4,
    "be16_to_bgra: short source"
  );
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 => unsafe { x86::be16_to_bgra_sse2(src, dst, layout, alpha) },
    #[cfg(target_arch = "x86_64")]
    Level::Ssse3 | Level::Avx2 => unsafe { x86::be16_to_bgra_ssse3(src, dst, layout, alpha) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::be16_to_bgra(src, dst, layout, alpha) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::be16_to_bgra(src, dst, layout, alpha),
  };
  scalar::be16_to_bgra(&src[done * size..], &mut dst[done * 4..], layout, alpha);
}

/// One field of a BMP bitfield mask, as the image crate reads it: `len` bits (at most 8) from bit
/// `shift`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bitfield {
  pub shift: u32,
  pub len: u32,
}

/// (M, A) by field length: the image crate's 8-bit value of an n-bit field v is (v * M + A) >> 6.
/// Up to 6 bits that is round(v * 255 / (2^n - 1)) (its tables), for 7 bits (v << 1) | (v >> 6),
/// for 8 bits v. The tests check every (n, v). v * M + A < 2^14, so it fits a 16-bit lane.
const SCALE: [(u32, u32); 9] = [
  (0, 0),
  (16320, 0),
  (5440, 0),
  (2336, 0),
  (1088, 0),
  (527, 23),
  (259, 33),
  (129, 0),
  (64, 0),
];

impl Bitfield {
  /// None for a mask the image crate rejects: not contiguous, or past `bits`. A field wider
  /// than 8 bits keeps its top 8.
  pub fn from_mask(mask: u32, bits: u32) -> Option<Bitfield> {
    if mask == 0 {
      return Some(Bitfield::default());
    }
    let shift = mask.trailing_zeros();
    let len = (!(mask >> shift)).trailing_zeros();
    if len != mask.count_ones() || len + shift > bits {
      return None;
    }
    Some(match len {
      9.. => Bitfield {
        shift: shift + len - 8,
        len: 8,
      },
      _ => Bitfield { shift, len },
    })
  }

  /// The field of `word` scaled to 8 bits: round(v * 255 / (2^len - 1)) for up to 6 bits
  /// (the image crate's tables), bit replication for 7 bits.
  pub fn read(self, word: u32) -> u8 {
    let (mul, add) = SCALE[self.len as usize];
    let v = (word >> self.shift) & ((1 << self.len) - 1);
    ((v * mul + add) >> 6) as u8
  }
}

/// BGRA pixels from a BMP's little-endian 16-bit words, with the R, G, B and A `fields` read
/// like `Bitfield::read`. `dst` holds the pixels and `src` at least as many; their 4th byte is 255
/// without `alpha`. Each field must lie in the word (as `Bitfield::from_mask(mask, 16)` gives).
pub fn bitfields16_to_bgra(src: &[u8], dst: &mut [u8], fields: [Bitfield; 4], alpha: bool) {
  bitfields_to_bgra_with(Isa::best(), src, dst, 2, fields, alpha)
}

/// The same for 32-bit words (fields as `Bitfield::from_mask(mask, 32)` gives).
pub fn bitfields32_to_bgra(src: &[u8], dst: &mut [u8], fields: [Bitfield; 4], alpha: bool) {
  bitfields_to_bgra_with(Isa::best(), src, dst, 4, fields, alpha)
}

fn bitfields_to_bgra_with(
  isa: Isa,
  src: &[u8],
  dst: &mut [u8],
  size: usize,
  fields: [Bitfield; 4],
  alpha: bool,
) {
  let bits = 8 * size as u32;
  assert!(
    fields
      .iter()
      .all(|f| f.len <= 8 && f.shift < bits && f.shift + f.len <= bits),
    "bitfields_to_bgra: a field outside the word"
  );
  assert!(
    src.len() / size >= dst.len() / 4,
    "bitfields_to_bgra: short source"
  );
  let done = match (isa.level(), size) {
    (Level::Scalar, _) => 0,
    #[cfg(target_arch = "x86_64")]
    (_, 2) => unsafe { x86::bitfields16_to_bgra_sse2(src, dst, fields, alpha) },
    #[cfg(target_arch = "x86_64")]
    _ => unsafe { x86::bitfields32_to_bgra_sse2(src, dst, fields, alpha) },
    #[cfg(target_arch = "aarch64")]
    (Level::Neon, 2) => unsafe { neon::bitfields16_to_bgra(src, dst, fields, alpha) },
    #[cfg(target_arch = "aarch64")]
    (Level::Neon, _) => unsafe { neon::bitfields32_to_bgra(src, dst, fields, alpha) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    (Level::Simd128, 2) => wasm::bitfields16_to_bgra(src, dst, fields, alpha),
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    (Level::Simd128, _) => wasm::bitfields32_to_bgra(src, dst, fields, alpha),
  };
  scalar::bitfields_to_bgra(
    &src[done * size..],
    &mut dst[done * 4..],
    size,
    fields,
    alpha,
  );
}

/// The layout of an opaque BMP pixel whose B, G and R are whole bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opaque {
  /// B, G, R (24-bit)
  Bgr,
  /// B, G, R and an unused byte (32-bit BI_RGB)
  Bgrx,
  /// An unused byte, then B, G, R (32-bit masks 0xFF000000, 0xFF0000 and 0xFF00, no alpha)
  Xbgr,
}

impl Opaque {
  /// Bytes per pixel.
  pub fn size(self) -> usize {
    match self {
      Opaque::Bgr => 3,
      Opaque::Bgrx | Opaque::Xbgr => 4,
    }
  }
}

/// BGRA pixels from opaque BMP pixels: their B, G and R, and 255. `dst` holds the pixels and `src`
/// at least as many.
pub fn opaque_to_bgra(src: &[u8], dst: &mut [u8], layout: Opaque) {
  opaque_to_bgra_with(Isa::best(), src, dst, layout)
}

fn opaque_to_bgra_with(isa: Isa, src: &[u8], dst: &mut [u8], layout: Opaque) {
  let size = layout.size();
  assert!(
    src.len() / size >= dst.len() / 4,
    "opaque_to_bgra: short source"
  );
  let done = match isa.level() {
    Level::Scalar => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 => unsafe { x86::opaque_to_bgra_sse2(src, dst, layout) },
    #[cfg(target_arch = "x86_64")]
    Level::Ssse3 | Level::Avx2 => unsafe { x86::opaque_to_bgra_ssse3(src, dst, layout) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::opaque_to_bgra(src, dst, layout) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::opaque_to_bgra(src, dst, layout),
  };
  scalar::opaque_to_bgra(&src[done * size..], &mut dst[done * 4..], layout);
}

/// Up to 256 opaque colors for `palette_to_bgra`. An index past them is black, like in a BMP.
#[derive(Clone)]
pub struct Palette {
  len: usize,
  bgra: [[u8; 4]; 256],
  /// B, G and R of each color, 0 past `len`, for the SIMD lookups
  planes: [[u8; 256]; 3],
}

impl Palette {
  /// From B, G, R colors; the ones past 256 are left out.
  pub fn new(colors: impl IntoIterator<Item = [u8; 3]>) -> Palette {
    let mut palette = Palette {
      len: 0,
      bgra: [[0, 0, 0, 255]; 256],
      planes: [[0; 256]; 3],
    };
    for (i, [b, g, r]) in colors.into_iter().take(256).enumerate() {
      palette.bgra[i] = [b, g, r, 255];
      for (plane, v) in palette.planes.iter_mut().zip([b, g, r]) {
        plane[i] = v;
      }
      palette.len = i + 1;
    }
    palette
  }

  /// The BGRA color of `index`.
  pub fn color(&self, index: u8) -> [u8; 4] {
    self.bgra[usize::from(index)]
  }
}

/// BGRA pixels from a BMP row of `bits`-bit palette indexes (1, 2, 4 or 8), the first pixel in
/// the high bits of a byte. `dst` holds the pixels and `src` at least as many.
pub fn palette_to_bgra(src: &[u8], dst: &mut [u8], bits: u32, palette: &Palette) {
  palette_to_bgra_with(Isa::best(), src, dst, bits, palette)
}

fn palette_to_bgra_with(isa: Isa, src: &[u8], dst: &mut [u8], bits: u32, palette: &Palette) {
  assert!(
    matches!(bits, 1 | 2 | 4 | 8),
    "palette_to_bgra: {bits}-bit indexes"
  );
  let per_byte = (8 / bits) as usize;
  assert!(
    src.len() * per_byte >= dst.len() / 4,
    "palette_to_bgra: short source"
  );
  // Each path does whole source bytes, and leaves to the scalar code 8-bit rows whose palette
  // is too large for its lookups to be faster.
  let done = match isa.level() {
    Level::Scalar => 0,
    // SSE2 has no byte lookup (pshufb is SSSE3)
    #[cfg(target_arch = "x86_64")]
    Level::Sse2 => 0,
    #[cfg(target_arch = "x86_64")]
    Level::Ssse3 | Level::Avx2 => unsafe { x86::palette_to_bgra_ssse3(src, dst, bits, palette) },
    #[cfg(target_arch = "aarch64")]
    Level::Neon => unsafe { neon::palette_to_bgra(src, dst, bits, palette) },
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    Level::Simd128 => wasm::palette_to_bgra(src, dst, bits, palette),
  };
  scalar::palette_to_bgra(&src[done / per_byte..], &mut dst[done * 4..], bits, palette);
}
