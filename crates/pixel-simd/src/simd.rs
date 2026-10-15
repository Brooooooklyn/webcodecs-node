//! What the SIMD paths share (a build without one, such as armv7, has none of this).

use crate::{Bitfield, SCALE};

impl Bitfield {
  /// (shift, mask, M, A): `read` is (((word >> shift) & mask) * M + A) >> 6.
  pub(crate) fn scale(self) -> (u32, u32, u32, u32) {
    let (mul, add) = SCALE[self.len as usize];
    (self.shift, (1 << self.len) - 1, mul, add)
  }
}

/// Runs `f` on the `S`-byte chunks of `src` and the `D`-byte chunks of `dst` side by side, as
/// many as both have, and returns how many that is.
#[inline(always)]
pub(crate) fn zip_chunks<const S: usize, const D: usize>(
  src: &[u8],
  dst: &mut [u8],
  mut f: impl FnMut(&[u8; S], &mut [u8; D]),
) -> usize {
  let (src, _) = src.as_chunks::<S>();
  let (dst, _) = dst.as_chunks_mut::<D>();
  let n = src.len().min(dst.len());
  for (s, d) in src.iter().zip(dst) {
    f(s, d);
  }
  n
}
