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
fn fenced(bufs: &[&[u8]], rng: &mut Rng, mut kernel: impl FnMut(&mut [Fenced])) -> Vec<Vec<u8>> {
  let off = 1 + rng.below(15);
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
