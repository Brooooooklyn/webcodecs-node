//! Every SIMD path this CPU runs against the scalar code. Each SIMD call runs on copies of its
//! buffers fenced by an inaccessible page (Linux and macOS: a read or write past either end
//! crashes the test) and by canary bytes (everywhere: they must stay, and changing them must not
//! change the output).

use crate::isa::{Isa, Level};
use crate::webp::{Plane, Yuva420, yuva420_to_bgra_with};
use crate::*;

/// splitmix64
struct Rng(u64);

impl Rng {
  fn next(&mut self) -> u64 {
    self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = self.0;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
  }

  fn below(&mut self, n: usize) -> usize {
    (self.next() % n as u64) as usize
  }

  /// `len` bytes of one of the patterns that reach saturation, edges and repeats.
  fn bytes(&mut self, len: usize) -> Vec<u8> {
    let (pattern, a, b) = (self.below(7), self.next() as u8, self.next() as u8);
    (0..len)
      .map(|i| match pattern {
        0 => 0,
        1 => 255,
        2 => [0, 255][self.below(2)],
        3 => [0, 1, 127, 128, 254, 255, a][self.below(7)],
        4 => [a, b][i % 2],
        5 => {
          if self.below(16) == 0 {
            self.next() as u8
          } else {
            a
          }
        }
        _ => self.next() as u8,
      })
      .collect()
  }
}

/// The SIMD levels of this CPU, and the scalar one.
fn levels() -> (Isa, Vec<Isa>) {
  let mut isas = Isa::supported();
  let scalar = isas.next().unwrap();
  assert_eq!(scalar.level(), Level::Scalar);
  let simd: Vec<Isa> = isas.collect();
  eprintln!("SIMD paths: {simd:?}");
  (scalar, simd)
}

/// Pixel counts: every count up to past the widest block (64 pixels) and its tails, then
/// around larger powers of two.
fn widths() -> impl Iterator<Item = usize> {
  (0..=140).chain([
    255, 256, 257, 511, 512, 513, 1023, 1024, 1025, 4000, 4001, 4099,
  ])
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod pages {
  //! Memory with inaccessible pages (64 KiB, a multiple of any page size) around a body.
  use core::ffi::c_void;

  unsafe extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64)
    -> *mut c_void;
    fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
  }
  const GUARD: usize = 1 << 16;
  const PROT_NONE: i32 = 0;
  const PROT_READ_WRITE: i32 = 1 | 2;
  const MAP_PRIVATE: i32 = 2;
  #[cfg(target_os = "macos")]
  const MAP_ANON: i32 = 0x1000;
  #[cfg(target_os = "linux")]
  const MAP_ANON: i32 = 0x20;

  pub(super) struct Pages {
    map: *mut u8,
    map_len: usize,
    body: *mut u8,
    body_len: usize,
  }

  impl Pages {
    /// At least `len` bytes between two inaccessible pages.
    pub(super) fn new(len: usize) -> Pages {
      let body_len = len.div_ceil(GUARD).max(1) * GUARD;
      let map_len = body_len + 3 * GUARD;
      // SAFETY: a new private mapping; both guards and the body lie inside it.
      unsafe {
        let map = mmap(
          core::ptr::null_mut(),
          map_len,
          PROT_READ_WRITE,
          MAP_PRIVATE | MAP_ANON,
          -1,
          0,
        );
        assert_ne!(map as isize, -1, "mmap");
        let map = map.cast::<u8>();
        let start = map.add(map.align_offset(GUARD));
        assert_eq!(mprotect(start.cast(), GUARD, PROT_NONE), 0);
        let body = start.add(GUARD);
        assert_eq!(mprotect(body.add(body_len).cast(), GUARD, PROT_NONE), 0);
        Pages {
          map,
          map_len,
          body,
          body_len,
        }
      }
    }

    pub(super) fn body(&mut self) -> &mut [u8] {
      // SAFETY: readable and writable, owned by `self`.
      unsafe { core::slice::from_raw_parts_mut(self.body, self.body_len) }
    }
  }

  impl Drop for Pages {
    fn drop(&mut self) {
      // SAFETY: the mapping made in `new`.
      unsafe { munmap(self.map.cast(), self.map_len) };
    }
  }
}

/// Where a buffer sits in its fence.
#[derive(Clone, Copy)]
enum Side {
  /// Its first byte right after the fence, or `n` canary bytes after it.
  Head(usize),
  /// Its last byte right before the fence, or `n` canary bytes before it.
  Tail(usize),
}

/// A copy of some bytes, fenced (see the module comment).
struct Fenced {
  #[cfg(any(target_os = "linux", target_os = "macos"))]
  memory: pages::Pages,
  #[cfg(not(any(target_os = "linux", target_os = "macos")))]
  memory: Vec<u8>,
  start: usize,
  len: usize,
  canary: u8,
}

const CANARIES: usize = 64;

impl Fenced {
  fn new(data: &[u8], side: Side, canary: u8) -> Fenced {
    let len = data.len();
    let room = len + 2 * CANARIES + 16;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let mut memory = pages::Pages::new(room);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let mut memory = vec![0; room];
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let body = memory.body();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let body = &mut memory[..];
    body.fill(canary);
    let start = match side {
      Side::Head(n) => n,
      Side::Tail(n) => body.len() - n - len,
    };
    body[start..start + len].copy_from_slice(data);
    Fenced {
      memory,
      start,
      len,
      canary,
    }
  }

  fn body(&mut self) -> &mut [u8] {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    return self.memory.body();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return &mut self.memory[..];
  }

  fn bytes(&mut self) -> &mut [u8] {
    let (start, len) = (self.start, self.len);
    &mut self.body()[start..start + len]
  }

  /// The bytes as 16-bit words (`fenced_at` with `align` 2).
  fn words(&mut self) -> &mut [u16] {
    // SAFETY: any two bytes are a u16.
    let (head, words, tail) = unsafe { self.bytes().align_to_mut::<u16>() };
    assert!(
      head.is_empty() && tail.is_empty(),
      "words at an odd address"
    );
    words
  }

  /// The bytes, after checking that every canary is still there.
  fn into_bytes(mut self) -> Vec<u8> {
    let (start, len, canary) = (self.start, self.len, self.canary);
    let body = self.body();
    let outside = body[..start].iter().chain(&body[start + len..]);
    assert!(
      outside.into_iter().all(|&b| b == canary),
      "a canary changed"
    );
    body[start..start + len].to_vec()
  }
}

/// Runs `kernel` on fenced copies of `bufs`: against the fence at the start, against it at the
/// end, and a few bytes off it, each time with other canaries. Every run must leave the same
/// bytes, which it returns.
fn fenced(bufs: &[&[u8]], rng: &mut Rng, kernel: impl FnMut(&mut [Fenced])) -> Vec<Vec<u8>> {
  fenced_at(bufs, 1, rng, kernel)
}

/// `fenced` with each buffer at a multiple of `align` bytes from the fence.
fn fenced_at(
  bufs: &[&[u8]],
  align: usize,
  rng: &mut Rng,
  mut kernel: impl FnMut(&mut [Fenced]),
) -> Vec<Vec<u8>> {
  let off = align * (1 + rng.below(15));
  let runs = [
    (Side::Head(0), 0x5A),
    (Side::Tail(0), 0xA5),
    (Side::Head(off), 0x00),
  ];
  let mut first: Option<Vec<Vec<u8>>> = None;
  for (side, canary) in runs {
    let mut fenced: Vec<Fenced> = bufs.iter().map(|b| Fenced::new(b, side, canary)).collect();
    kernel(&mut fenced);
    let out: Vec<Vec<u8>> = fenced.into_iter().map(Fenced::into_bytes).collect();
    match &first {
      Some(first) => assert_eq!(first, &out, "the canaries changed the output"),
      None => first = Some(out),
    }
  }
  first.unwrap()
}

#[test]
fn bitfield_scale_is_the_image_crates_rounding() {
  for len in 0..=8u32 {
    let max = (1u32 << len) - 1;
    for v in 0..=max {
      // the per-pixel code before the (M, A) table
      let want = match len {
        0 => 0,
        7 => ((v << 1) | (v >> 6)) as u8,
        8 => v as u8,
        _ => ((v * 255 + max / 2) / max) as u8,
      };
      assert_eq!(
        Bitfield { shift: 0, len }.read(v),
        want,
        "len {len} value {v}"
      );
      let (mul, add) = SCALE[len as usize];
      assert!(v * mul + add < 1 << 14);
    }
  }
}

#[test]
fn edit_matches_scalar() {
  let (scalar, simd) = levels();
  let mut rng = Rng(1);
  for px in widths() {
    for swap in [false, true] {
      let key = [0, 255, rng.next() as u8][rng.below(3)];
      for alpha in [Alpha::Keep, Alpha::Opaque, Alpha::Key(key)] {
        let pixels = rng.bytes(4 * px);
        let mut want = pixels.clone();
        edit_bgra_with(scalar, &mut want, swap, alpha);
        for &isa in &simd {
          let got = fenced(&[&pixels], &mut rng, |b| {
            edit_bgra_with(isa, b[0].bytes(), swap, alpha)
          });
          assert_eq!(got[0], want, "{isa:?} {px} px swap {swap} {alpha:?}");
        }
      }
    }
  }
}

#[test]
fn be16_matches_scalar() {
  let (scalar, simd) = levels();
  let mut rng = Rng(2);
  for px in widths() {
    for layout in [Be16::Gray, Be16::GrayAlpha, Be16::Rgb, Be16::Rgba] {
      for alpha in [false, true] {
        let extra = rng.below(3);
        let src = rng.bytes(px * layout.size() + extra);
        let dst = rng.bytes(4 * px);
        let mut want = dst.clone();
        be16_to_bgra_with(scalar, &src, &mut want, layout, alpha);
        for &isa in &simd {
          let got = fenced(&[&src, &dst], &mut rng, |b| {
            let (src, dst) = b.split_at_mut(1);
            be16_to_bgra_with(isa, src[0].bytes(), dst[0].bytes(), layout, alpha)
          });
          assert_eq!(got[1], want, "{isa:?} {px} px {layout:?} alpha {alpha}");
        }
      }
    }
  }
}

/// Every field `Bitfield::from_mask` gives for `bits`-bit words.
fn all_fields(bits: u32) -> Vec<Bitfield> {
  let mut fields = vec![Bitfield::default()];
  for len in 1..=bits {
    for shift in 0..=bits - len {
      let mask = (u32::MAX >> (32 - len)) << shift;
      fields.push(Bitfield::from_mask(mask, bits).unwrap());
    }
  }
  fields
}

#[test]
fn bitfields_match_scalar() {
  let (scalar, simd) = levels();
  let mut rng = Rng(3);
  for size in [2, 4] {
    let fields = all_fields(8 * size as u32);
    // each field as R over every 16-bit word, or over every value with random other bits
    for &field in &fields {
      let words: Vec<u32> = if size == 2 {
        (0..=0xFFFF).collect()
      } else {
        (0..1 << field.len)
          .map(|v| (rng.next() as u32 & !(0xFF << field.shift)) | v << field.shift)
          .collect()
      };
      let src: Vec<u8> = words
        .iter()
        .flat_map(|w| w.to_le_bytes()[..size].to_vec())
        .collect();
      let others = [0; 3].map(|_| fields[rng.below(fields.len())]);
      let fields = [field, others[0], others[1], others[2]];
      for alpha in [false, true] {
        let mut want = vec![0; 4 * words.len()];
        bitfields_to_bgra_with(scalar, &src, &mut want, size, fields, alpha);
        for &isa in &simd {
          let mut got = vec![0; want.len()];
          bitfields_to_bgra_with(isa, &src, &mut got, size, fields, alpha);
          assert_eq!(
            got, want,
            "{isa:?} {size}-byte words {fields:?} alpha {alpha}"
          );
        }
      }
    }
    // random fields and words, fenced
    for px in widths() {
      let fields = [0; 4].map(|_| fields[rng.below(fields.len())]);
      let alpha = rng.below(2) == 0;
      let extra = rng.below(3);
      let src = rng.bytes(px * size + extra);
      let dst = rng.bytes(4 * px);
      let mut want = dst.clone();
      bitfields_to_bgra_with(scalar, &src, &mut want, size, fields, alpha);
      for &isa in &simd {
        let got = fenced(&[&src, &dst], &mut rng, |b| {
          let (src, dst) = b.split_at_mut(1);
          bitfields_to_bgra_with(isa, src[0].bytes(), dst[0].bytes(), size, fields, alpha)
        });
        assert_eq!(got[1], want, "{isa:?} {px} px {fields:?} alpha {alpha}");
      }
    }
  }
}

/// A frame's planes, with `pad` bytes past each row.
struct Frame {
  width: usize,
  height: usize,
  planes: [Vec<u8>; 4],
  strides: [usize; 4],
}

impl Frame {
  fn random(rng: &mut Rng, width: usize, height: usize) -> Frame {
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let strides = [width, cw, cw, width].map(|w| w + [0, 1, 7, 33][rng.below(4)]);
    let rows = [height, ch, ch, height];
    let planes = [0, 1, 2, 3].map(|i| rng.bytes(strides[i] * rows[i]));
    Frame {
      width,
      height,
      planes,
      strides,
    }
  }

  fn yuva<'a>(&self, planes: [&'a [u8]; 4], alpha: bool) -> Yuva420<'a> {
    let plane = |i: usize| Plane {
      data: planes[i],
      stride: self.strides[i],
    };
    Yuva420 {
      width: self.width,
      height: self.height,
      y: plane(0),
      u: plane(1),
      v: plane(2),
      a: alpha.then(|| plane(3)),
    }
  }
}

/// Every (y, u, v), with alpha: constant chroma planes make every upsampled sample the constant.
#[test]
fn webp_converts_every_yuv_like_scalar() {
  let (scalar, simd) = levels();
  let (width, height) = (256, 3);
  let luma: Vec<u8> = (0..=255)
    .chain((0..=255).rev())
    .chain((0..=255).map(|y| y ^ 0x5A))
    .collect();
  let alpha: Vec<u8> = (0..width * height).map(|i| (i * 7 + 3) as u8).collect();
  let (mut want, mut got) = (vec![0; 4 * width * height], vec![0; 4 * width * height]);
  for u in 0..=255 {
    for v in 0..=255 {
      let (u, v) = ([u; 256], [v; 256]);
      let src = Yuva420 {
        width,
        height,
        y: Plane {
          data: &luma,
          stride: width,
        },
        u: Plane {
          data: &u,
          stride: 128,
        },
        v: Plane {
          data: &v,
          stride: 128,
        },
        a: Some(Plane {
          data: &alpha,
          stride: width,
        }),
      };
      yuva420_to_bgra_with(scalar, &src, &mut want, 4 * width);
      for &isa in &simd {
        yuva420_to_bgra_with(isa, &src, &mut got, 4 * width);
        assert_eq!(got, want, "{isa:?} u {} v {}", u[0], v[0]);
      }
    }
  }
}

#[test]
fn webp_frames_match_scalar() {
  let (scalar, simd) = levels();
  let mut rng = Rng(4);
  let sizes = (1..=70)
    .flat_map(|w| (1..=4).map(move |h| (w, h)))
    .chain((1..=9).flat_map(|w| (5..=40).map(move |h| (w, h))))
    .chain([
      (127, 5),
      (128, 6),
      (129, 7),
      (191, 3),
      (193, 2),
      (1023, 9),
      (4000, 3),
      (4001, 4),
    ]);
  for (width, height) in sizes {
    let frame = Frame::random(&mut rng, width, height);
    for alpha in [false, true] {
      let stride = 4 * width + [0, 4, 3][rng.below(3)];
      let dst = rng.bytes(stride * (height - 1) + 4 * width);
      let mut want = dst.clone();
      let planes = frame.planes.each_ref().map(|p| &p[..]);
      yuva420_to_bgra_with(scalar, &frame.yuva(planes, alpha), &mut want, stride);
      for &isa in &simd {
        let mut bufs: Vec<&[u8]> = planes.to_vec();
        bufs.push(&dst);
        let got = fenced(&bufs, &mut rng, |b| {
          let (planes, dst) = b.split_at_mut(4);
          let [y, u, v, a] = planes else { unreachable!() };
          let src = frame.yuva([y.bytes(), u.bytes(), v.bytes(), a.bytes()], alpha);
          yuva420_to_bgra_with(isa, &src, dst[0].bytes(), stride)
        });
        assert_eq!(got[4], want, "{isa:?} {width}x{height} alpha {alpha}");
      }
    }
  }
}

/// de Bruijn B(256, 2), with its first byte again at the end: every byte pair once as
/// (s[i], s[i + 1]).
fn debruijn() -> Vec<u8> {
  fn fkm(t: usize, p: usize, a: &mut [usize; 3], out: &mut Vec<u8>) {
    if t > 2 {
      if 2 % p == 0 {
        out.extend(a[1..=p].iter().map(|&x| x as u8));
      }
      return;
    }
    a[t] = a[t - p];
    fkm(t + 1, p, a, out);
    for j in a[t - p] + 1..256 {
      a[t] = j;
      fkm(t + 1, t, a, out);
    }
  }
  let mut out = Vec::with_capacity(65537);
  fkm(1, 1, &mut [0; 3], &mut out);
  out.push(out[0]);
  let mut seen = vec![false; 65536];
  for pair in out.windows(2) {
    seen[usize::from(pair[0]) << 8 | usize::from(pair[1])] = true;
  }
  assert!(out.len() == 65537 && seen.iter().all(|&s| s));
  out
}

/// Every chroma quad (top left, top, left, current: 2^32) through each path's whole conversion.
/// For each (p, q), a frame of 131073 x 3 pixels with chroma rows p, q, p, q, ... and de Bruijn:
/// its row pair (rows 1 and 2) gives every quad of p and q to both rows, nearer each in turn. U
/// and V are the same plane, and each pixel's luma is one under which any other U changes B and
/// any other V changes R, so equal bytes mean equal chroma. Run with
/// `cargo test --release -p pixel-simd -- --ignored`.
#[test]
#[ignore = "2^32 chroma quads per path: run with --release -- --ignored"]
fn webp_upsamples_every_chroma_like_scalar() {
  use crate::webp::{chroma_neighbours, upsample};
  let (scalar, simd) = levels();
  if simd.is_empty() {
    return;
  }
  let bgra = |samples: [u8; 3]| {
    let plane = |i: usize| Plane {
      data: &samples[i..=i],
      stride: 1,
    };
    let src = Yuva420 {
      width: 1,
      height: 1,
      y: plane(0),
      u: plane(1),
      v: plane(2),
      a: None,
    };
    let mut px = [0; 4];
    yuva420_to_bgra_with(scalar, &src, &mut px, 4);
    px
  };
  let luma_for: Vec<u8> = (0..=255)
    .map(|c| {
      (0..=255)
        .find(|&y| {
          let [b, _, r, _] = bgra([y, c, c]);
          (0..=255)
            .filter(|&o| o != c)
            .all(|o| bgra([y, o, c])[0] != b && bgra([y, c, o])[2] != r)
        })
        .expect("a luma that shows the chroma")
    })
    .collect();
  let debruijn = debruijn();
  let cw = debruijn.len();
  let (width, height) = (2 * cw - 1, 3);
  let cols: Vec<_> = (0..width).map(|col| chroma_neighbours(col, cw)).collect();
  // wasm32-wasip1 has no threads: there the test thread does all of it
  let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
  let check = |t: usize| {
    let mut chroma = [vec![0; cw], debruijn.clone()].concat();
    let mut luma = vec![0; width * height];
    let (mut want, mut got) = (vec![0; 4 * width * height], vec![0; 4 * width * height]);
    for pq in (t..65536).step_by(threads) {
      for (i, x) in chroma[..cw].iter_mut().enumerate() {
        *x = [(pq >> 8) as u8, pq as u8][i % 2];
      }
      for (row, luma) in luma.chunks_mut(width).enumerate() {
        let (near, far) = chroma_neighbours(row, 2);
        let rows = [&chroma[near * cw..], &chroma[far * cw..]];
        for (y, &cols) in luma.iter_mut().zip(&cols) {
          *y = luma_for[upsample(rows, cols) as usize];
        }
      }
      let plane = |data, stride| Plane { data, stride };
      let src = Yuva420 {
        width,
        height,
        y: plane(&luma[..], width),
        u: plane(&chroma[..], cw),
        v: plane(&chroma[..], cw),
        a: None,
      };
      yuva420_to_bgra_with(scalar, &src, &mut want, 4 * width);
      for &isa in &simd {
        yuva420_to_bgra_with(isa, &src, &mut got, 4 * width);
        if let Some(i) = got.iter().zip(&want).position(|(g, w)| g != w) {
          panic!("{isa:?} p {} q {}: byte {i}", pq >> 8, pq & 255);
        }
      }
    }
  };
  std::thread::scope(|s| {
    for t in 1..threads {
      s.spawn(move || check(t));
    }
    check(0);
  });
}

/// FNV-1a 64 of `bytes`, after `hash`.
fn fnv(hash: u64, bytes: &[u8]) -> u64 {
  bytes.iter().fold(hash, |h, &b| {
    (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3)
  })
}

const FNV: u64 = 0xCBF2_9CE4_8422_2325;

/// The 6 libyuv constant sets, in the order of their table.
fn constant_sets() -> Vec<avif::Constants> {
  use avif::Matrix::*;
  [Bt601, Bt709, Bt2020]
    .into_iter()
    .flat_map(|m| [false, true].map(|full| avif::Constants::new(m, full)))
    .collect()
}

/// `len` 16-bit samples: one of the byte patterns of `Rng::bytes` in pairs, masked to `depth`
/// bits half the time.
fn words(rng: &mut Rng, len: usize, depth: u32) -> Vec<u16> {
  let mask = if rng.below(2) == 0 {
    (1 << depth) - 1
  } else {
    u16::MAX
  };
  let bytes = rng.bytes(2 * len);
  bytes
    .chunks(2)
    .map(|b| u16::from_le_bytes([b[0], b[1]]) & mask)
    .collect()
}

// The AVIF rows pin their bytes here: each digest is that of the C converter they port
// (src/ffi/accessors.c at 43f5871: rc_yuv_matrix and the rc_avif_* rows, with upsampled chroma
// clamped to the bit depth) on the same inputs.

/// Every 8-bit (y, u, v), 4:0:0 and the 10 and 12-bit rows, with every constant set
#[test]
fn avif_matrix_rows_give_the_c_bytes() {
  let sets = constant_sets();
  let mut rng = Rng(20);
  let (y, u): (Vec<u8>, Vec<u8>) = (0..65536).map(|i| (i as u8, (i >> 8) as u8)).unzip();
  let mut out = vec![0; 4 * 65536];
  let mut hash = FNV;
  for (s, k) in sets.iter().enumerate() {
    for v in 0..=255 {
      let a: Vec<u8> = (0..65536).map(|i| (i * 7 + v) as u8).collect();
      let alpha = ((s + v) % 2 == 1).then_some(&a[..]);
      avif::yuv8_to_bgra(&y, &u, &[v as u8; 65536], alpha, k, &mut out);
      hash = fnv(hash, &out);
    }
    let a: Vec<u8> = (0..256).map(|i| (i * 13) as u8).collect();
    avif::gray8_to_bgra(
      &y[..256],
      (s % 2 == 1).then_some(&a[..]),
      k,
      &mut out[..1024],
    );
    hash = fnv(hash, &out[..1024]);
  }
  assert_eq!(hash, 0xf904_195f_356b_bf8e, "8-bit");
  // 10-bit: every y, u in steps of 3, v in steps of 101, then 16-bit noise
  let mut hash = FNV;
  let y: Vec<u16> = (0..1024).collect();
  let mut out = vec![0; 4 * 1024];
  for (s, k) in sets.iter().enumerate() {
    for v in (0..1024).step_by(101) {
      for u in (0..1024).step_by(3) {
        let a: Vec<u8> = (0..1024).map(|i| (i * 7 + u) as u8).collect();
        let alpha = ((s + u) % 2 == 1).then_some(&a[..]);
        avif::yuv10_to_bgra(&y, &[u as u16; 1024], &[v; 1024], alpha, k, &mut out);
        hash = fnv(hash, &out);
      }
    }
    for _ in 0..64 {
      let [y, u, v] = [0; 3].map(|_| words(&mut rng, 1024, 10));
      let a = rng.bytes(1024);
      avif::yuv10_to_bgra(&y, &u, &v, Some(&a), k, &mut out);
      hash = fnv(hash, &out);
    }
  }
  assert_eq!(hash, 0xfa46_08f5_0801_046a, "10-bit");
  // 12-bit, chroma for every 2 pixels
  let mut hash = FNV;
  for k in &sets {
    for w in (1..=100).chain([4097]) {
      let y = words(&mut rng, w, 12);
      let [u, v] = [0; 2].map(|_| words(&mut rng, w.div_ceil(2), 12));
      let a = rng.bytes(w);
      let alpha = (rng.below(2) == 0).then_some(&a[..]);
      let mut out = vec![0; 4 * w];
      avif::yuv12_to_bgra(&y, &u, &v, alpha, k, &mut out);
      hash = fnv(hash, &out);
    }
  }
  assert_eq!(hash, 0x52a2_bbe0_03fd_9bb8, "12-bit");
}

/// The chroma rows and upsampling: rows of random widths, every pair of 8-bit samples next to
/// 64 others, and 16-bit samples above the bit depth
#[test]
fn avif_upsampling_gives_the_c_bytes() {
  let mut rng = Rng(21);
  let mut hash = FNV;
  for sub in [false, true] {
    for h in 1..=300 {
      for j in 0..h {
        let rows = avif::chroma_rows(j, h, sub).map(|r| r as u32);
        hash = fnv(
          hash,
          &[rows[0].to_le_bytes(), rows[1].to_le_bytes()].concat(),
        );
      }
    }
  }
  assert_eq!(hash, 0x9904_fa25_cb57_e121, "rows");
  let mut hash = FNV;
  for w in widths().chain([4097]) {
    let [near, far] = [0; 2].map(|_| rng.bytes(w.div_ceil(2)));
    let mut out = vec![0; w];
    avif::upsample8(&near, &far, &mut out);
    hash = fnv(hash, &out);
  }
  let near = debruijn();
  let mut out = vec![0; 2 * 65536];
  for p in 0..64u8 {
    let far: Vec<u8> = (0..65537).map(|i| [p * 4, 255 - p * 3][i % 2]).collect();
    avif::upsample8(&near, &far, &mut out);
    hash = fnv(hash, &out);
  }
  assert_eq!(hash, 0x2ab8_4a57_9e0a_eb40, "8-bit");
  let mut hash = FNV;
  for depth in 9..=12 {
    for w in widths().chain([4097]) {
      let [near, far] = [0; 2].map(|_| words(&mut rng, w.div_ceil(2), depth));
      let mut out = vec![0; w];
      avif::upsample16(&near, &far, depth, &mut out);
      let bytes: Vec<u8> = out.iter().flat_map(|s| s.to_le_bytes()).collect();
      hash = fnv(hash, &bytes);
    }
  }
  assert_eq!(hash, 0x44a6_bdaa_4831_9973, "16-bit");
}

/// Every 16-bit sample shifted to 8 bits, every alpha sample, every premultiplied pixel
#[test]
fn avif_sample_rows_give_the_c_bytes() {
  let all: Vec<u16> = (0..=u16::MAX).collect();
  let mut out = vec![0; 65536];
  let mut hash = FNV;
  for shift in [2, 4] {
    avif::convert16_to8(&all, shift, &mut out);
    hash = fnv(hash, &out);
  }
  assert_eq!(hash, 0xc9c6_9dce_281a_6925, "Convert16To8");
  let mut hash = FNV;
  for limited in [false, true] {
    let bytes: Vec<u8> = (0..=255).collect();
    avif::alpha_to_8bit(&bytes, 8, limited, false, &mut out[..256]);
    hash = fnv(hash, &out[..256]);
    for depth in [10, 12] {
      for scale in [false, true] {
        avif::alpha_to_8bit(&all, depth, limited, scale, &mut out);
        hash = fnv(hash, &out);
      }
    }
  }
  assert_eq!(hash, 0xc01c_9bd8_43da_b5db, "alpha");
  let mut pixels: Vec<u8> = (0..65536)
    .flat_map(|i| {
      let (c, a) = (i as u8, (i >> 8) as u8);
      [c, c ^ 0x5A, 255 - c, a]
    })
    .collect();
  avif::unattenuate(&mut pixels);
  assert_eq!(fnv(FNV, &pixels), 0xfee6_55fb_0fee_4b55, "unattenuate");
}

/// The rows of a frame for rgb_impl: Y, the U and V planes, alpha
struct RgbFrame<T> {
  width: usize,
  height: usize,
  /// chroma subsampling across and down (log2), and no chroma
  hs: usize,
  vs: usize,
  gray: bool,
  planes: [Vec<T>; 4],
}

impl<T: avif::Sample> RgbFrame<T> {
  fn bgra(&self, p: &avif::RgbImpl, alpha: bool) -> Vec<u8> {
    let (w, h) = (self.width, self.height);
    let cw = (w + self.hs) >> self.hs;
    let mut out = vec![0; 4 * w * h];
    for (j, out) in out.chunks_mut(4 * w).enumerate() {
      let rows = avif::chroma_rows(j, h, self.vs == 1);
      let uv = (!self.gray).then(|| [1, 2].map(|i| rows.map(|r| &self.planes[i][r * cw..][..cw])));
      let a = alpha.then(|| &self.planes[3][j * w..][..w]);
      avif::rgb_impl_row(p, &self.planes[0][j * w..][..w], uv, a, out);
    }
    out
  }
}

/// The f32 rows: every transform the converter uses, depth, layout, range and alpha, on frames of
/// random sizes and samples
#[test]
fn avif_rgb_impl_gives_the_c_bytes() {
  use avif::Transform::*;
  let transforms = [
    Coefficients {
      kr: 0.212,
      kb: 0.087,
    },
    Coefficients { kr: 0.30, kb: 0.11 },
    Coefficients {
      kr: 0.299,
      kb: 0.1146,
    },
    Coefficients {
      kr: 0.2536,
      kb: 0.06808,
    },
    Coefficients { kr: 0.0, kb: 0.0 },
    Coefficients {
      kr: 0.2095,
      kb: 0.0689,
    },
    Coefficients {
      kr: 0.229,
      kb: 0.0793,
    },
    Coefficients {
      kr: 0.2318,
      kb: 0.096,
    },
    Coefficients {
      kr: 0.2126,
      kb: 0.0722,
    },
    Identity,
    YCgCo,
    YCgCoRe,
  ];
  let mut rng = Rng(22);
  let mut hash = FNV;
  for depth in [8, 10, 12] {
    for (hs, vs, gray) in [(0, 0, false), (1, 0, false), (1, 1, false), (0, 0, true)] {
      for transform in transforms {
        for full_range in [false, true] {
          for alpha in [None, Some(false), Some(true)] {
            for unpremultiply in [false, true] {
              // what the converter runs: rgb_impl undoes 'prem' only off the fast path
              let coefficients = matches!(transform, Coefficients { .. });
              let fast =
                hs == 0 && (coefficients || (transform == Identity && depth == 8 && full_range));
              if (transform == Identity && (hs == 1 || gray))
                || (transform == YCgCo && !full_range)
                || (transform == YCgCoRe && !(full_range && depth == 10))
                || (unpremultiply && (alpha.is_none() || fast))
              {
                continue;
              }
              let p = avif::RgbImpl {
                transform,
                depth,
                full_range,
                subsampled: hs == 1,
                alpha_limited: alpha == Some(true),
                unpremultiply,
              };
              let (w, h) = if rng.below(50) == 0 {
                (4097, 2)
              } else {
                (1 + rng.below(40), 1 + rng.below(6))
              };
              let (cw, ch) = ((w + hs) >> hs, (h + vs) >> vs);
              let planes = [(w, h), (cw, ch), (cw, ch), (w, h)]
                .map(|(pw, ph)| words(&mut rng, pw * ph, depth));
              let out = if depth == 8 {
                let frame = RgbFrame {
                  width: w,
                  height: h,
                  hs,
                  vs,
                  gray,
                  planes: planes.map(|p| p.iter().map(|&s| s as u8).collect()),
                };
                frame.bgra(&p, alpha.is_some())
              } else {
                let frame = RgbFrame {
                  width: w,
                  height: h,
                  hs,
                  vs,
                  gray,
                  planes,
                };
                frame.bgra(&p, alpha.is_some())
              };
              hash = fnv(hash, &out);
            }
          }
        }
      }
    }
  }
  assert_eq!(hash, 0xa369_cf47_29aa_18b4, "frames");
  // 4:4:4 grids: every 8-bit (y, u) with v in steps of 17, and for YCgCo-Re every 10-bit y with
  // u and v in steps of 31
  let mut hash = FNV;
  for transform in transforms {
    for full_range in [false, true] {
      if matches!(transform, YCgCo | YCgCoRe) && !full_range {
        continue;
      }
      let depth = if transform == YCgCoRe { 10 } else { 8 };
      let p = avif::RgbImpl {
        transform,
        depth,
        full_range,
        subsampled: false,
        alpha_limited: false,
        unpremultiply: false,
      };
      let out = if depth == 8 {
        let (width, height) = (65536, 16);
        let samples = |f: fn(usize) -> usize| (0..width * height).map(|i| f(i) as u8).collect();
        let planes = [
          samples(|i| i),
          samples(|i| i >> 8),
          samples(|i| i / 65536 * 17),
          vec![],
        ];
        let frame = RgbFrame {
          width,
          height,
          hs: 0,
          vs: 0,
          gray: false,
          planes,
        };
        frame.bgra(&p, false)
      } else {
        let (width, height) = (1024, 34 * 34);
        let samples = |f: fn(usize) -> usize| (0..width * height).map(|i| f(i) as u16).collect();
        let planes = [
          samples(|i| i % 1024),
          samples(|i| i / 1024 % 34 * 31),
          samples(|i| i / 1024 / 34 * 31),
          vec![],
        ];
        let frame = RgbFrame {
          width,
          height,
          hs: 0,
          vs: 0,
          gray: false,
          planes,
        };
        frame.bgra(&p, false)
      };
      hash = fnv(hash, &out);
    }
  }
  assert_eq!(hash, 0x0116_cbe4_4b99_c4d8, "grids");
}

// The AVIF rows: each SIMD path against the scalar code.

impl Rng {
  /// `n`, or a few more: a row that runs on into its stride.
  fn past(&mut self, n: usize) -> usize {
    n + [0, 1, 7, 33][self.below(4)]
  }
}

/// Pixel counts for the AVIF rows: every count up to past the widest vectors and their tails,
/// then long rows.
fn row_widths() -> impl Iterator<Item = usize> {
  (0..=300).chain([4095, 4096, 4097, 8191])
}

/// The bytes of 16-bit words, as memory holds them.
fn word_bytes(words: &[u16]) -> Vec<u8> {
  words.iter().flat_map(|w| w.to_ne_bytes()).collect()
}

/// Panics at the first byte of `got` that is not `want`'s.
#[track_caller]
fn assert_bytes(got: &[u8], want: &[u8], what: impl std::fmt::Display) {
  assert_eq!(got.len(), want.len(), "{what}: length");
  if let Some(i) = got.iter().zip(want).position(|(g, w)| g != w) {
    panic!("{what}: byte {i} is {} instead of {}", got[i], want[i]);
  }
}

/// A 10 or 12-bit matrix row
type Row16 = fn(Isa, &[u16], &[u16], &[u16], Option<&[u8]>, &avif::Constants, &mut [u8]);

/// Every 8-bit (y, u, v) and gray y, and every 16-bit y, u and v of the 10 and 12-bit rows
/// (within their depth and above it), with every constant set
#[test]
fn avif_matrix_rows_match_scalar() {
  let (scalar, simd) = levels();
  let n = 65536;
  let (y, u): (Vec<u8>, Vec<u8>) = (0..n).map(|i| (i as u8, (i >> 8) as u8)).unzip();
  let a: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
  let (mut want, mut got) = (vec![0; 4 * n], vec![0; 4 * n]);
  let y16: Vec<u16> = (0..=u16::MAX).collect();
  for k in &constant_sets() {
    for v in 0..=255 {
      let alpha = (v % 2 == 1).then_some(&a[..]);
      let v = [v; 65536];
      avif::yuv8_to_bgra_with(scalar, &y, &u, &v, alpha, k, &mut want);
      for &isa in &simd {
        avif::yuv8_to_bgra_with(isa, &y, &u, &v, alpha, k, &mut got);
        assert_bytes(&got, &want, format_args!("{isa:?} {k:?} v {}", v[0]));
      }
    }
    for alpha in [None, Some(&a[..256])] {
      avif::gray8_to_bgra_with(scalar, &y[..256], alpha, k, &mut want[..1024]);
      for &isa in &simd {
        avif::gray8_to_bgra_with(isa, &y[..256], alpha, k, &mut got[..1024]);
        assert_bytes(
          &got[..1024],
          &want[..1024],
          format_args!("{isa:?} {k:?} gray"),
        );
      }
    }
    // each row has every y, and U and V in other orders: every value of the depth, or of u16
    for (depth, row) in [
      (10, avif::yuv10_to_bgra_with as Row16),
      (12, avif::yuv12_to_bgra_with),
    ] {
      let chroma = if depth == 10 { n } else { n / 2 };
      for r in 0..8 {
        let mask = if r < 4 { (1 << depth) - 1 } else { u16::MAX };
        let order = |m: usize, c: usize| -> Vec<u16> {
          (0..chroma)
            .map(|i| ((i * (2 * m + 1) + c) as u16) & mask)
            .collect()
        };
        let (u, v) = (order(r, 1234 * r), order(r + 2, 4321 * r));
        let alpha = (r % 2 == 1).then_some(&a[..]);
        row(scalar, &y16, &u, &v, alpha, k, &mut want);
        for &isa in &simd {
          row(isa, &y16, &u, &v, alpha, k, &mut got);
          assert_bytes(&got, &want, format_args!("{isa:?} {k:?} {depth}-bit {r}"));
        }
      }
    }
  }
}

/// The matrix rows on rows of every width, fenced: samples of every pattern, rows that run on
/// past the pixels, alpha or none, a random constant set
#[test]
fn avif_matrix_rows_fenced_match_scalar() {
  let (scalar, simd) = levels();
  let sets = constant_sets();
  let mut rng = Rng(30);
  for w in row_widths() {
    let k = sets[rng.below(6)];
    let alpha = rng.below(2) == 0;
    let a = {
      let n = rng.past(w);
      rng.bytes(n)
    };
    let dst = rng.bytes(4 * w);
    let a_row = alpha.then_some(&a[..]);
    let [y, u, v] = [0; 3].map(|_| {
      let n = rng.past(w);
      rng.bytes(n)
    });
    let (mut want, mut want_gray) = (dst.clone(), dst.clone());
    avif::yuv8_to_bgra_with(scalar, &y, &u, &v, a_row, &k, &mut want);
    avif::gray8_to_bgra_with(scalar, &y, a_row, &k, &mut want_gray);
    for &isa in &simd {
      let got = fenced(&[&y, &u, &v, &a, &dst, &dst], &mut rng, |b| {
        let [y, u, v, a, d, d_gray] = b else {
          unreachable!()
        };
        let a = if alpha { Some(&*a.bytes()) } else { None };
        avif::yuv8_to_bgra_with(isa, y.bytes(), u.bytes(), v.bytes(), a, &k, d.bytes());
        avif::gray8_to_bgra_with(isa, y.bytes(), a, &k, d_gray.bytes());
      });
      assert_bytes(&got[4], &want, format_args!("{isa:?} 8-bit {w} px"));
      assert_bytes(&got[5], &want_gray, format_args!("{isa:?} gray {w} px"));
    }
    for (depth, row) in [
      (10, avif::yuv10_to_bgra_with as Row16),
      (12, avif::yuv12_to_bgra_with),
    ] {
      let chroma = if depth == 10 { w } else { w.div_ceil(2) };
      let y = {
        let n = rng.past(w);
        words(&mut rng, n, depth)
      };
      let [u, v] = [0; 2].map(|_| {
        let n = rng.past(chroma);
        words(&mut rng, n, depth)
      });
      let mut want = dst.clone();
      row(scalar, &y, &u, &v, a_row, &k, &mut want);
      for &isa in &simd {
        let planes = [&y, &u, &v].map(|p| word_bytes(p));
        let got = fenced_at(
          &[&planes[0], &planes[1], &planes[2], &a, &dst],
          2,
          &mut rng,
          |b| {
            let [y, u, v, a, d] = b else { unreachable!() };
            let a = if alpha { Some(&*a.bytes()) } else { None };
            row(isa, y.words(), u.words(), v.words(), a, &k, d.bytes());
          },
        );
        assert_bytes(&got[4], &want, format_args!("{isa:?} {depth}-bit {w} px"));
      }
    }
  }
}

/// The upsampling: every pair of 8-bit samples next to 64 others; rows of every width, fenced,
/// with one chroma row or two, at every depth with samples above it; and 16-bit rows of the
/// largest sums
#[test]
fn avif_upsampling_matches_scalar() {
  let (scalar, simd) = levels();
  let near = debruijn();
  let (mut want, mut got) = (vec![0; 2 * 65536], vec![0; 2 * 65536]);
  for p in 0..64u8 {
    let far: Vec<u8> = (0..65537).map(|i| [p * 4, 255 - p * 3][i % 2]).collect();
    avif::upsample8_with(scalar, &near, &far, &mut want);
    for &isa in &simd {
      avif::upsample8_with(isa, &near, &far, &mut got);
      assert_bytes(&got, &want, format_args!("{isa:?} far {p}"));
    }
  }
  let mut rng = Rng(31);
  for w in row_widths() {
    let cw = w.div_ceil(2);
    let one_row = rng.below(4) == 0;
    let [near, far] = [0; 2].map(|_| {
      let n = rng.past(cw);
      rng.bytes(n)
    });
    let far = if one_row { near.clone() } else { far };
    let dst = rng.bytes(w);
    let mut want = dst.clone();
    avif::upsample8_with(scalar, &near, &far, &mut want);
    for &isa in &simd {
      let got = fenced(&[&near, &far, &dst], &mut rng, |b| {
        let [n, f, d] = b else { unreachable!() };
        avif::upsample8_with(isa, n.bytes(), f.bytes(), d.bytes())
      });
      assert_bytes(&got[2], &want, format_args!("{isa:?} 8-bit {w}"));
    }
    let depth = 9 + rng.below(4) as u32;
    let [near, far] = [0; 2].map(|_| {
      let n = rng.past(cw);
      words(&mut rng, n, depth)
    });
    let far = if one_row { near.clone() } else { far };
    let dst = words(&mut rng, w, 12);
    let mut want = dst.clone();
    avif::upsample16_with(scalar, &near, &far, depth, &mut want);
    for &isa in &simd {
      let bufs = [&near, &far, &dst].map(|b| word_bytes(b));
      let got = fenced_at(&[&bufs[0], &bufs[1], &bufs[2]], 2, &mut rng, |b| {
        let [n, f, d] = b else { unreachable!() };
        avif::upsample16_with(isa, n.words(), f.words(), depth, d.words())
      });
      assert_bytes(
        &got[2],
        &word_bytes(&want),
        format_args!("{isa:?} {depth}-bit {w}"),
      );
    }
  }
  // every (near, near, far, far) of samples around the largest of each depth
  for depth in 9..=12 {
    let max = (1u16 << depth) - 1;
    let values = [0, 1, max - 1, max, max + 1, 0x8000, u16::MAX];
    let row = |a: u16, b: u16| -> Vec<u16> { (0..300).map(|i| [a, b][i % 2]).collect() };
    let (mut want, mut got) = (vec![0; 599], vec![0; 599]);
    for (a, b, c, d) in values
      .iter()
      .flat_map(|&a| values.map(move |b| (a, b)))
      .flat_map(|(a, b)| {
        values
          .iter()
          .flat_map(move |&c| values.map(move |d| (a, b, c, d)))
      })
    {
      let (near, far) = (row(a, b), row(c, d));
      avif::upsample16_with(scalar, &near, &far, depth, &mut want);
      for &isa in &simd {
        avif::upsample16_with(isa, &near, &far, depth, &mut got);
        assert_eq!(got, want, "{isa:?} {depth}-bit {a} {b} {c} {d}");
      }
    }
  }
}

/// Every (near, near, far, far) of 8-bit samples (2^32) through each path's upsampling: the near
/// row is de Bruijn, which holds every pair of samples, and the far row p, q, p, q, ..., which
/// gives each pair (p, q) or (q, p) by the column's parity. Run with
/// `cargo test --release -p pixel-simd -- --ignored`.
#[test]
#[ignore = "2^32 sample quads per path: run with --release -- --ignored"]
fn avif_upsamples_every_quad_like_scalar() {
  let (scalar, simd) = levels();
  let near = debruijn();
  // wasm32-wasip1 has no threads: there the test thread does all of it
  let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
  let check = |t: usize| {
    let mut far = vec![0; near.len()];
    let (mut want, mut got) = (vec![0; 2 * near.len() - 1], vec![0; 2 * near.len() - 1]);
    for pq in (t..65536).step_by(threads) {
      for (i, x) in far.iter_mut().enumerate() {
        *x = [(pq >> 8) as u8, pq as u8][i % 2];
      }
      avif::upsample8_with(scalar, &near, &far, &mut want);
      for &isa in &simd {
        avif::upsample8_with(isa, &near, &far, &mut got);
        assert_bytes(
          &got,
          &want,
          format_args!("{isa:?} p {} q {}", pq >> 8, pq & 255),
        );
      }
    }
  };
  std::thread::scope(|s| {
    for t in 1..threads {
      s.spawn(move || check(t));
    }
    check(0);
  });
}

/// Every 10-bit (y, u, v) (2^30) with every constant set through each path, alpha in every other
/// row. Run with `cargo test --release -p pixel-simd -- --ignored`.
#[test]
#[ignore = "2^30 pixels per constant set and path: run with --release -- --ignored"]
fn avif_10bit_matrix_every_yuv_like_scalar() {
  let (scalar, simd) = levels();
  let sets = constant_sets();
  let (y, u): (Vec<u16>, Vec<u16>) = (0..1 << 20)
    .map(|i| (i as u16 & 1023, (i >> 10) as u16))
    .unzip();
  let a: Vec<u8> = (0..1 << 20).map(|i| (i * 7 + 3) as u8).collect();
  let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
  let check = |t: usize| {
    let (mut want, mut got) = (vec![0; 4 << 20], vec![0; 4 << 20]);
    for job in (t..6 * 1024).step_by(threads) {
      let (k, v) = (&sets[job / 1024], vec![(job % 1024) as u16; 1 << 20]);
      let alpha = (job % 2 == 1).then_some(&a[..]);
      avif::yuv10_to_bgra_with(scalar, &y, &u, &v, alpha, k, &mut want);
      for &isa in &simd {
        avif::yuv10_to_bgra_with(isa, &y, &u, &v, alpha, k, &mut got);
        assert_bytes(&got, &want, format_args!("{isa:?} {k:?} v {}", v[0]));
      }
    }
  };
  std::thread::scope(|s| {
    for t in 1..threads {
      s.spawn(move || check(t));
    }
    check(0);
  });
}

/// Convert16To8: every sample at every shift, then rows of every width, fenced
#[test]
fn avif_convert16_matches_scalar() {
  let (scalar, simd) = levels();
  let all: Vec<u16> = (0..=u16::MAX).collect();
  let (mut want, mut got) = (vec![0; 65536], vec![0; 65536]);
  for shift in 0..16 {
    avif::convert16_to8_with(scalar, &all, shift, &mut want);
    for &isa in &simd {
      avif::convert16_to8_with(isa, &all, shift, &mut got);
      assert_bytes(&got, &want, format_args!("{isa:?} shift {shift}"));
    }
  }
  let mut rng = Rng(32);
  for w in row_widths() {
    let shift = rng.below(16) as u32;
    let src = {
      let n = rng.past(w);
      words(&mut rng, n, 12)
    };
    let dst = rng.bytes(w);
    let mut want = dst.clone();
    avif::convert16_to8_with(scalar, &src, shift, &mut want);
    for &isa in &simd {
      let got = fenced_at(&[&word_bytes(&src), &dst], 2, &mut rng, |b| {
        let [s, d] = b else { unreachable!() };
        avif::convert16_to8_with(isa, s.words(), shift, d.bytes())
      });
      assert_bytes(&got[1], &want, format_args!("{isa:?} shift {shift} {w}"));
    }
  }
}

/// The samples of the f32 rows, as the fenced buffers hold them
trait Fence: avif::Sample {
  /// `n` samples of one of the patterns of `words` (those of `Rng::bytes` for 8 bits)
  fn random(rng: &mut Rng, n: usize, depth: u32) -> Vec<Self>;
  fn bytes(samples: &[Self]) -> Vec<u8>;
  fn from(fenced: &mut Fenced) -> &[Self];
}

impl Fence for u8 {
  fn random(rng: &mut Rng, n: usize, _: u32) -> Vec<u8> {
    rng.bytes(n)
  }

  fn bytes(samples: &[u8]) -> Vec<u8> {
    samples.to_vec()
  }

  fn from(fenced: &mut Fenced) -> &[u8] {
    fenced.bytes()
  }
}

impl Fence for u16 {
  fn random(rng: &mut Rng, n: usize, depth: u32) -> Vec<u16> {
    words(rng, n, depth)
  }

  fn bytes(samples: &[u16]) -> Vec<u8> {
    word_bytes(samples)
  }

  fn from(fenced: &mut Fenced) -> &[u16] {
    fenced.words()
  }
}

/// The chroma of an f32 row
#[derive(Clone, Copy, Debug, PartialEq)]
enum Chroma {
  Gray,
  /// a sample per pixel, one row
  Full,
  /// a sample per 2 pixels, one row (4:2:2) or two (4:2:0)
  Half {
    rows: usize,
  },
}

/// An f32 row of `w` random pixels (`chroma` must match `p.subsampled`) through each SIMD path
/// and the scalar code, fenced
fn rgb_impl_row_matches<T: Fence>(
  (scalar, simd): (Isa, &[Isa]),
  rng: &mut Rng,
  p: &avif::RgbImpl,
  w: usize,
  chroma: Chroma,
  alpha: bool,
) {
  let cw = if p.subsampled { w.div_ceil(2) } else { w };
  let mut row = |n: usize| {
    let n = rng.past(n);
    T::random(rng, n, p.depth)
  };
  let y = row(w);
  let [un, uf, vn, vf] = [0; 4].map(|_| row(cw));
  let a = row(w);
  let (uf, vf) = if chroma == (Chroma::Half { rows: 2 }) {
    (uf, vf)
  } else {
    (un.clone(), vn.clone())
  };
  let dst = rng.bytes(4 * w);
  let gray = chroma == Chroma::Gray;
  let mut want = dst.clone();
  let uv = (!gray).then_some([[&un[..], &uf[..]], [&vn[..], &vf[..]]]);
  avif::rgb_impl_row_with(scalar, p, &y, uv, alpha.then_some(&a[..]), &mut want);
  let bufs = [&y, &un, &uf, &vn, &vf, &a].map(|b| T::bytes(b));
  for &isa in simd {
    let got = fenced_at(
      &[
        &bufs[0], &bufs[1], &bufs[2], &bufs[3], &bufs[4], &bufs[5], &dst,
      ],
      2,
      rng,
      |b| {
        let [y, un, uf, vn, vf, a, d] = b else {
          unreachable!()
        };
        let a = if alpha { Some(T::from(a)) } else { None };
        let uv = if gray {
          None
        } else {
          Some([[T::from(un), T::from(uf)], [T::from(vn), T::from(vf)]])
        };
        avif::rgb_impl_row_with(isa, p, T::from(y), uv, a, d.bytes());
      },
    );
    assert_bytes(
      &got[6],
      &want,
      format_args!("{isa:?} {p:?} {chroma:?} alpha {alpha} {w} px"),
    );
  }
}

/// The f32 rows: every transform (and Kr, Kb that make infinities and NaN), depth, sample width,
/// chroma, range, alpha and unpremultiply on random rows; rows of every width; and 4:4:4 grids of
/// every 8-bit (y, u) per transform
#[test]
fn avif_rgb_impl_matches_scalar() {
  use avif::Transform::*;
  let (scalar, simd) = levels();
  let paths = (scalar, &simd[..]);
  let transforms = [
    Coefficients {
      kr: 0.212,
      kb: 0.087,
    },
    Coefficients { kr: 0.30, kb: 0.11 },
    Coefficients {
      kr: 0.299,
      kb: 0.1146,
    },
    Coefficients {
      kr: 0.2536,
      kb: 0.06808,
    },
    Coefficients { kr: 0.0, kb: 0.0 },
    Coefficients {
      kr: 0.2095,
      kb: 0.0689,
    },
    Coefficients {
      kr: 0.229,
      kb: 0.0793,
    },
    Coefficients {
      kr: 0.2318,
      kb: 0.096,
    },
    Coefficients {
      kr: 0.2126,
      kb: 0.0722,
    },
    // Kg 0, a NaN, infinities
    Coefficients { kr: 0.5, kb: 0.5 },
    Coefficients {
      kr: f32::NAN,
      kb: 0.1,
    },
    Coefficients {
      kr: 3e38,
      kb: -3e38,
    },
    Identity,
    YCgCo,
    YCgCoRe,
  ];
  let chromas = [
    Chroma::Gray,
    Chroma::Full,
    Chroma::Half { rows: 1 },
    Chroma::Half { rows: 2 },
  ];
  let mut rng = Rng(33);
  for transform in transforms {
    for depth in [8, 10, 12] {
      for chroma in chromas {
        for full_range in [false, true] {
          for alpha in [None, Some(false), Some(true)] {
            for unpremultiply in [false, true] {
              let p = avif::RgbImpl {
                transform,
                depth,
                full_range,
                subsampled: matches!(chroma, Chroma::Half { .. }),
                alpha_limited: alpha == Some(true),
                unpremultiply,
              };
              let w = if rng.below(40) == 0 {
                4097
              } else {
                rng.below(41)
              };
              rgb_impl_row_matches::<u8>(paths, &mut rng, &p, w, chroma, alpha.is_some());
              rgb_impl_row_matches::<u16>(paths, &mut rng, &p, w, chroma, alpha.is_some());
            }
          }
        }
      }
    }
  }
  for w in row_widths() {
    let chroma = chromas[rng.below(4)];
    let p = avif::RgbImpl {
      transform: transforms[rng.below(transforms.len())],
      depth: [8, 10, 12][rng.below(3)],
      full_range: rng.below(2) == 0,
      subsampled: matches!(chroma, Chroma::Half { .. }),
      alpha_limited: rng.below(4) == 0,
      unpremultiply: rng.below(2) == 0,
    };
    let alpha = rng.below(2) == 0;
    rgb_impl_row_matches::<u8>(paths, &mut rng, &p, w, chroma, alpha);
    rgb_impl_row_matches::<u16>(paths, &mut rng, &p, w, chroma, alpha);
  }
  // 4:4:4 rows of every 8-bit (y, u), for v in steps of 17, with alpha that is unpremultiplied
  // in every other one
  let (width, n) = (65536, 16);
  let y: Vec<u8> = (0..width).map(|i| i as u8).collect();
  let u: Vec<u8> = (0..width).map(|i| (i >> 8) as u8).collect();
  let a: Vec<u8> = (0..width).map(|i| (i * 7 + 3) as u8).collect();
  let (mut want, mut got) = (vec![0; 4 * width], vec![0; 4 * width]);
  for transform in transforms {
    for full_range in [false, true] {
      for r in 0..n {
        let p = avif::RgbImpl {
          transform,
          depth: 8,
          full_range,
          subsampled: false,
          alpha_limited: false,
          unpremultiply: r % 2 == 1,
        };
        let v = vec![(r * 17) as u8; width];
        let uv = Some([[&u[..], &u[..]], [&v[..], &v[..]]]);
        let alpha = (r % 4 != 0).then_some(&a[..]);
        avif::rgb_impl_row_with(scalar, &p, &y, uv, alpha, &mut want);
        for &isa in &simd {
          avif::rgb_impl_row_with(isa, &p, &y, uv, alpha, &mut got);
          assert_bytes(&got, &want, format_args!("{isa:?} {p:?} v {}", v[0]));
        }
      }
    }
  }
}
