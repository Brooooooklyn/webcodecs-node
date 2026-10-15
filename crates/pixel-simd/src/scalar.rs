//! The scalar code: the reference of every SIMD path, and what each runs past its last whole
//! vector.

use crate::{Alpha, Be16, Bitfield};

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

/// `size`-byte little-endian words, each field read like the image crate does.
pub(crate) fn bitfields_to_bgra(
  src: &[u8],
  dst: &mut [u8],
  size: usize,
  [r, g, b, a]: [Bitfield; 4],
  alpha: bool,
) {
  unpack(src, dst, size, alpha, |p| {
    let word = p.iter().rev().fold(0, |word, &v| word << 8 | u32::from(v));
    [b.read(word), g.read(word), r.read(word), a.read(word)]
  })
}

/// BGRA pixels from the packed `size`-byte pixels of `src`: `read` gives the B, G, R and A of
/// each, and A is 255 without `alpha`.
fn unpack(src: &[u8], dst: &mut [u8], size: usize, alpha: bool, read: impl Fn(&[u8]) -> [u8; 4]) {
  for (p, px) in src.chunks_exact(size).zip(dst.as_chunks_mut::<4>().0) {
    let [b, g, r, a] = read(p);
    *px = [b, g, r, if alpha { a } else { 255 }];
  }
}
