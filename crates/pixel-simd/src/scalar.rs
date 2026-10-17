//! The scalar code: the reference of every SIMD path, and what each runs past its last whole
//! vector.

use crate::{Alpha, Be16, Bitfield, Opaque, Palette};

pub(crate) fn edit_bgra(pixels: &mut [u8], swap: bool, alpha: Alpha) {
  for px in pixels.as_chunks_mut::<4>().0 {
    if swap {
      px.swap(0, 2);
    }
    match alpha {
      Alpha::Keep => {}
      Alpha::Opaque => px[3] = 255,
      Alpha::Key(key) => px[3] = if px[0] == key { 0 } else { 255 },
    }
  }
}

pub(crate) fn be16_to_bgra(src: &[u8], dst: &mut [u8], layout: Be16, alpha: bool) {
  // big-endian samples: the high byte comes first
  match layout {
    Be16::Gray => unpack(src, dst, 2, alpha, |p| [p[0], p[0], p[0], 255]),
    Be16::GrayAlpha => unpack(src, dst, 4, alpha, |p| [p[0], p[0], p[0], p[2]]),
    Be16::Rgb => unpack(src, dst, 6, alpha, |p| [p[4], p[2], p[0], 255]),
    Be16::Rgba => unpack(src, dst, 8, alpha, |p| [p[4], p[2], p[0], p[6]]),
  }
}

/// `size`-byte little-endian words (2 or 4), each field read like the image crate does.
pub(crate) fn bitfields_to_bgra(
  src: &[u8],
  dst: &mut [u8],
  size: usize,
  fields: [Bitfield; 4],
  alpha: bool,
) {
  // a loop for each size, with a constant word load
  match size {
    2 => words_to_bgra::<2>(src, dst, fields, alpha),
    _ => words_to_bgra::<4>(src, dst, fields, alpha),
  }
}

fn words_to_bgra<const N: usize>(
  src: &[u8],
  dst: &mut [u8],
  [r, g, b, a]: [Bitfield; 4],
  alpha: bool,
) {
  unpack(src, dst, N, alpha, |p| {
    let mut le = [0; 4];
    le[..N].copy_from_slice(p);
    let word = u32::from_le_bytes(le);
    [b.read(word), g.read(word), r.read(word), a.read(word)]
  })
}

pub(crate) fn opaque_to_bgra(src: &[u8], dst: &mut [u8], layout: Opaque) {
  match layout {
    Opaque::Bgr => unpack(src, dst, 3, false, |p| [p[0], p[1], p[2], 255]),
    Opaque::Bgrx => unpack(src, dst, 4, false, |p| [p[0], p[1], p[2], 255]),
    Opaque::Xbgr => unpack(src, dst, 4, false, |p| [p[1], p[2], p[3], 255]),
  }
}

pub(crate) fn palette_to_bgra(src: &[u8], dst: &mut [u8], bits: u32, palette: &Palette) {
  // a loop for each size, with constant shifts
  match bits {
    1 => indexes_to_bgra::<1>(src, dst, palette),
    2 => indexes_to_bgra::<2>(src, dst, palette),
    4 => indexes_to_bgra::<4>(src, dst, palette),
    _ => indexes_to_bgra::<8>(src, dst, palette),
  }
}

fn indexes_to_bgra<const BITS: u32>(src: &[u8], dst: &mut [u8], palette: &Palette) {
  let pixels = dst.as_chunks_mut::<4>().0.chunks_mut((8 / BITS) as usize);
  for (pixels, &byte) in pixels.zip(src) {
    for (k, px) in pixels.iter_mut().enumerate() {
      *px = palette.color((byte << (BITS * k as u32)) >> (8 - BITS));
    }
  }
}

/// BGRA pixels from the packed `size`-byte pixels of `src`: `read` gives the B, G, R and A of
/// each, and A is 255 without `alpha`.
fn unpack(src: &[u8], dst: &mut [u8], size: usize, alpha: bool, read: impl Fn(&[u8]) -> [u8; 4]) {
  for (p, px) in src.chunks_exact(size).zip(dst.as_chunks_mut::<4>().0) {
    let [b, g, r, a] = read(p);
    *px = [b, g, r, if alpha { a } else { 255 }];
  }
}
