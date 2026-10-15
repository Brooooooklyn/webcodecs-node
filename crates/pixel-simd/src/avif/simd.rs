//! What the SIMD paths of `avif` share.

use super::{Constants, RgbImpl, Transform};

impl Constants {
  /// libyuv's constants as its SIMD rows hold them, each positive and 16 bits: YG, UB, UG, VG,
  /// VR, then the biases of B, G and R: UB * 128 - YB, (UG + VG) * 128 + YB, VR * 128 - YB. With
  /// y1 = (y16 * YG) >> 16 <= 19002 the sums of `yuv_pixel` fit 16 bits (at most 51642), and a
  /// saturating subtraction of the bias gives its clamp at 0.
  pub(super) fn lanes(self) -> [u16; 8] {
    let [yg, yb, ub, ug, vg, vr] = self.0;
    [
      yg,
      ub,
      ug,
      vg,
      vr,
      ub * 128 - yb,
      ug * 128 + vg * 128 + yb,
      vr * 128 - yb,
    ]
    .map(|c| c as u16)
  }
}

/// How a SIMD path of `rgb_impl_row` makes R, G and B from Y, U and V (a gray row: R = G = B =
/// Y).
#[derive(Clone, Copy, Debug)]
pub(super) enum Rgb {
  Identity,
  YCgCo,
  /// rgb_impl's matrix, from 2 (1 - Kr) for R, 2 (1 - Kb) for B, Kr (1 - Kr) and Kb (1 - Kb) for
  /// G, and Kg: the factors the scalar code computes first.
  Matrix {
    r: f32,
    b: f32,
    gr: f32,
    gb: f32,
    g: f32,
  },
}

impl RgbImpl {
  /// How the SIMD paths make R, G and B for a row with or without `alpha`; None where only the
  /// scalar code goes: YCgCo-Re, limited-range alpha.
  pub(super) fn rgb(&self, alpha: bool) -> Option<Rgb> {
    if alpha && self.alpha_limited {
      return None;
    }
    Some(match self.transform {
      Transform::Identity => Rgb::Identity,
      Transform::YCgCo => Rgb::YCgCo,
      Transform::YCgCoRe => return None,
      Transform::Coefficients { kr, kb } => Rgb::Matrix {
        r: 2.0 * (1.0 - kr),
        b: 2.0 * (1.0 - kb),
        gr: kr * (1.0 - kr),
        gb: kb * (1.0 - kb),
        g: 1.0 - kr - kb,
      },
    })
  }
}
