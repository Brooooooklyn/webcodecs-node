//! Timings of every path this CPU runs, for `cargo run --release -p pixel-simd --example bench`:
//! each kernel on a 4000x3000 image, row by row like the addon, best of 10 runs.

use std::time::Instant;

use crate::avif::{Constants, Matrix, RgbImpl, Transform};
use crate::isa::Isa;
use crate::webp::{Plane, Yuva420, yuva420_to_bgra_with};
use crate::*;

const WIDTH: usize = 4000;
const HEIGHT: usize = 3000;

/// The best time of 10 runs of `f`, in ms.
fn best(mut f: impl FnMut()) -> f64 {
  (0..10)
    .map(|_| {
      let start = Instant::now();
      f();
      start.elapsed().as_secs_f64() * 1e3
    })
    .fold(f64::INFINITY, f64::min)
}

/// Row `r` of `width` samples of plane `p`, the planes WIDTH x HEIGHT samples apart in
/// `samples`.
fn row_of<T>(samples: &[T], p: usize, r: usize, width: usize) -> &[T] {
  &samples[p * WIDTH * HEIGHT + r * width..][..width]
}

/// `f` on each row of `size`-byte pixels to BGRA.
fn rows(src: &[u8], size: usize, dst: &mut [u8], mut f: impl FnMut(&[u8], &mut [u8])) {
  for (s, d) in src.chunks(WIDTH * size).zip(dst.chunks_mut(WIDTH * 4)) {
    f(s, d);
  }
}

pub fn run() {
  let mut seed = 1u64;
  let src: Vec<u8> = (0..WIDTH * HEIGHT * 8)
    .map(|_| {
      seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
      (seed >> 56) as u8
    })
    .collect();
  let mut dst = vec![0; WIDTH * HEIGHT * 4];
  let rgb565 = [0xF800, 0x7E0, 0x1F, 0].map(|m| Bitfield::from_mask(m, 16).unwrap());
  let argb2101010 =
    [0x3FF00000, 0xFFC00, 0x3FF, 0xC0000000].map(|m| Bitfield::from_mask(m, 32).unwrap());
  let (cw, ch) = (WIDTH / 2, HEIGHT / 2);
  let plane = |at: usize, stride: usize| Plane {
    data: &src[at..],
    stride,
  };
  let webp = Yuva420 {
    width: WIDTH,
    height: HEIGHT,
    y: plane(0, WIDTH),
    u: plane(WIDTH * HEIGHT, cw),
    v: plane(WIDTH * HEIGHT + cw * ch, cw),
    a: Some(plane(2 * WIDTH * HEIGHT, WIDTH)),
  };
  // 10-bit samples, and the 16-bit rows of the upsampling
  let words: Vec<u16> = src
    .chunks(2)
    .map(|b| u16::from_le_bytes([b[0], b[1]]) & 1023)
    .collect();
  let mut up16 = vec![0; WIDTH * HEIGHT];
  let k = Constants::new(Matrix::Bt709, false);
  let f32_420 = RgbImpl {
    transform: Transform::Coefficients { kr: 0.30, kb: 0.11 },
    depth: 8,
    full_range: false,
    subsampled: true,
    alpha_limited: false,
    unpremultiply: true,
  };
  let f32_444 = RgbImpl {
    transform: Transform::Identity,
    depth: 10,
    full_range: true,
    subsampled: false,
    alpha_limited: false,
    unpremultiply: false,
  };
  println!("{WIDTH}x{HEIGHT}, best of 10, ms");
  for isa in Isa::supported() {
    let mut time = |name: &str, f: &mut dyn FnMut(&mut [u8])| {
      println!(
        "{name:<22} {:<8} {:>7.2}",
        format!("{:?}", isa.level()),
        best(|| f(&mut dst))
      );
    };
    time("edit swap+opaque", &mut |dst| {
      for row in dst.chunks_mut(WIDTH * 4) {
        edit_bgra_with(isa, row, true, Alpha::Opaque);
      }
    });
    time("edit gray key", &mut |dst| {
      for row in dst.chunks_mut(WIDTH * 4) {
        edit_bgra_with(isa, row, false, Alpha::Key(7));
      }
    });
    for layout in [Be16::Gray, Be16::GrayAlpha, Be16::Rgb, Be16::Rgba] {
      time(&format!("png16 {layout:?}"), &mut |dst| {
        rows(&src, layout.size(), dst, |s, d| {
          be16_to_bgra_with(isa, s, d, layout, true)
        })
      });
    }
    time("bmp16 565", &mut |dst| {
      rows(&src, 2, dst, |s, d| {
        bitfields_to_bgra_with(isa, s, d, 2, rgb565, false)
      })
    });
    time("bmp32 2-10-10-10", &mut |dst| {
      rows(&src, 4, dst, |s, d| {
        bitfields_to_bgra_with(isa, s, d, 4, argb2101010, true)
      })
    });
    for layout in [Opaque::Bgr, Opaque::Bgrx, Opaque::Xbgr] {
      time(&format!("bmp {layout:?}"), &mut |dst| {
        rows(&src, layout.size(), dst, |s, d| {
          opaque_to_bgra_with(isa, s, d, layout)
        })
      });
    }
    for (bits, len) in [
      (1, 2),
      (2, 4),
      (4, 16),
      (8, 16),
      (8, 32),
      (8, 64),
      (8, 128),
      (8, 256),
    ] {
      let palette = Palette::new(src.as_chunks().0[..len].iter().copied());
      time(&format!("bmp {bits}-bit {len} colors"), &mut |dst| {
        let row = WIDTH * bits / 8;
        for (s, d) in src.chunks(row).zip(dst.chunks_mut(WIDTH * 4)) {
          palette_to_bgra_with(isa, s, d, bits as u32, &palette)
        }
      });
    }
    time("webp yuva420", &mut |dst| {
      yuva420_to_bgra_with(isa, &webp, dst, WIDTH * 4)
    });
    time("avif up8", &mut |dst| {
      for (r, d) in dst.chunks_mut(WIDTH).take(HEIGHT).enumerate() {
        avif::upsample8_with(
          isa,
          row_of(&src, 0, r / 2, cw),
          row_of(&src, 1, r / 2, cw),
          d,
        );
      }
    });
    time("avif up16 10-bit", &mut |_| {
      for (r, d) in up16.chunks_mut(WIDTH).enumerate() {
        let [near, far] = [0, 1].map(|p| row_of(&words, p, r / 2, cw));
        avif::upsample16_with(isa, near, far, 10, d);
      }
    });
    time("avif yuv8+a", &mut |dst| {
      for (r, d) in dst.chunks_mut(4 * WIDTH).enumerate() {
        let [y, u, v, a] = [0, 1, 2, 3].map(|p| row_of(&src, p, r, WIDTH));
        avif::yuv8_to_bgra_with(isa, y, u, v, Some(a), &k, d);
      }
    });
    time("avif gray8", &mut |dst| {
      for (r, d) in dst.chunks_mut(4 * WIDTH).enumerate() {
        avif::gray8_to_bgra_with(isa, row_of(&src, 0, r, WIDTH), None, &k, d);
      }
    });
    time("avif yuv10+a", &mut |dst| {
      for (r, d) in dst.chunks_mut(4 * WIDTH).enumerate() {
        let [y, u, v] = [0, 1, 2].map(|p| row_of(&words, p, r, WIDTH));
        let a = row_of(&src, 3, r, WIDTH);
        avif::yuv10_to_bgra_with(isa, y, u, v, Some(a), &k, d);
      }
    });
    time("avif yuv12", &mut |dst| {
      for (r, d) in dst.chunks_mut(4 * WIDTH).enumerate() {
        let [u, v] = [1, 2].map(|p| row_of(&words, p, r, cw));
        avif::yuv12_to_bgra_with(isa, row_of(&words, 0, r, WIDTH), u, v, None, &k, d);
      }
    });
    time("avif 16to8", &mut |dst| {
      for (r, d) in dst.chunks_mut(WIDTH).take(HEIGHT).enumerate() {
        avif::convert16_to8_with(isa, row_of(&words, 0, r, WIDTH), 2, d);
      }
    });
    time("avif f32 420+a prem", &mut |dst| {
      for (r, d) in dst.chunks_mut(4 * WIDTH).enumerate() {
        let rows = [r / 2, (r / 2 + 1).min(ch - 1)];
        let uv = [1, 2].map(|p| rows.map(|c| row_of(&src, p, c, cw)));
        let [y, a] = [0, 3].map(|p| row_of(&src, p, r, WIDTH));
        avif::rgb_impl_row_with(isa, &f32_420, y, Some(uv), Some(a), d);
      }
    });
    time("avif f32 444 10-bit", &mut |dst| {
      for (r, d) in dst.chunks_mut(4 * WIDTH).enumerate() {
        let uv = [1, 2].map(|p| [row_of(&words, p, r, WIDTH); 2]);
        avif::rgb_impl_row_with(
          isa,
          &f32_444,
          row_of(&words, 0, r, WIDTH),
          Some(uv),
          None,
          d,
        );
      }
    });
  }
}
