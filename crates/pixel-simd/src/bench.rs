//! Timings of every path this CPU runs, for `cargo run --release -p pixel-simd --example bench`:
//! each kernel on a 4000x3000 image, row by row like the addon, best of 10 runs.

use std::time::Instant;

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
    time("webp yuva420", &mut |dst| {
      yuva420_to_bgra_with(isa, &webp, dst, WIDTH * 4)
    });
  }
}
