//! BMP decoder with Chrome 155's pixels.
//!
//! Chrome decodes BMP with the `image` crate 0.25.10 in lenient mode, patched by
//! Chromium (third_party/rust/chromium_crates_io/patches/image 0001 to 0003), through
//! skia's SkBmpRustCodec. This file follows the same rules:
//!
//! - headers of 12, 40, 52, 56, 108 and 124 bytes, and OS/2 headers of 16 to 64 bytes
//! - 1, 2, 4 and 8-bit palettes (an index past the palette is black), RLE4, RLE8,
//!   OS/2 RLE24 (skipped pixels are black), 16 and 32-bit bitfields, 24-bit and 32-bit
//! - a channel of n bits is scaled to 8 bits with round(v * 255 / (2^n - 1)); masks
//!   wider than 8 bits keep their top 8 bits
//! - alpha comes from an alpha mask: bitfields with a V3 to V5 header or
//!   BI_ALPHABITFIELDS, or 32-bit BI_RGB with a V4 or V5 header
//! - errors in the headers, masks, palette or ICC profile range are an image Chrome
//!   fails to open; short or bad pixel data fails the decode
//!
//! Output is unpremultiplied BGRA. `has_alpha` says whether Chrome labels the image
//! BGRA (true) or BGRX (false, the X bytes are 255).

use std::fmt;

/// A malformed or unsupported BMP
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BmpError(&'static str);

impl fmt::Display for BmpError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "BMP: {}", self.0)
  }
}

impl std::error::Error for BmpError {}

/// The data ends early. While more data can come, a stream is not failed by it.
pub const TRUNCATED: BmpError = BmpError("truncated data");

const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;
const BI_RLE4: u32 = 2;
const BI_BITFIELDS: u32 = 3;
const BI_JPEG: u32 = 4;
const BI_ALPHABITFIELDS: u32 = 6;

/// A color channel inside a little-endian pixel value
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Bitfield {
  shift: u32,
  len: u32,
}

impl Bitfield {
  const fn new(shift: u32, len: u32) -> Self {
    Bitfield { shift, len }
  }

  fn from_mask(mask: u32, max_len: u32) -> Result<Self, BmpError> {
    if mask == 0 {
      return Ok(Bitfield::default());
    }
    let shift = mask.trailing_zeros();
    let len = (!(mask >> shift)).trailing_zeros();
    if len != mask.count_ones() {
      return Err(BmpError("non-contiguous bitfield mask"));
    }
    if len + shift > max_len {
      return Err(BmpError("bitfield mask too wide"));
    }
    // keep the top 8 bits of a wider channel
    Ok(match len {
      9.. => Bitfield::new(shift + len - 8, 8),
      _ => Bitfield::new(shift, len),
    })
  }

  /// The channel scaled to 8 bits, round(v * 255 / (2^len - 1)), without a division
  fn read(self, pixel: u32) -> u8 {
    let (mul, add) = SCALE[self.len as usize];
    let v = (pixel >> self.shift) & ((1 << self.len) - 1);
    ((v * mul + add) >> 6) as u8
  }
}

/// (mul, add) for each channel length: round(v * 255 / (2^len - 1)) == (v * mul + add) >> 6
/// for every v of len bits (0 for len 0)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rle {
  Rle4,
  Rle8,
  Rle24,
}

#[derive(Clone, Copy, Debug)]
enum Kind {
  /// 1, 2, 4 or 8 bits per palette index
  Palette(u32),
  /// 2, 3 or 4 bytes per pixel. Masks are red, green, blue, alpha.
  Packed {
    bytes: usize,
    masks: [Bitfield; 4],
  },
  Rle(Rle),
}

const RGB555: [Bitfield; 4] = [
  Bitfield::new(10, 5),
  Bitfield::new(5, 5),
  Bitfield::new(0, 5),
  Bitfield::new(0, 0),
];
const RGB888: [Bitfield; 4] = [
  Bitfield::new(16, 8),
  Bitfield::new(8, 8),
  Bitfield::new(0, 8),
  Bitfield::new(0, 0),
];
const ARGB8888: [Bitfield; 4] = [
  Bitfield::new(16, 8),
  Bitfield::new(8, 8),
  Bitfield::new(0, 8),
  Bitfield::new(24, 8),
];

/// A BMP's size and pixel format, read the way the patched `image` crate reads them
pub struct BmpDecoder<'a> {
  data: &'a [u8],
  width: u32,
  height: u32,
  top_down: bool,
  data_offset: u64,
  kind: Kind,
  /// BGRA, padded with black
  palette: [[u8; 4]; 256],
}

fn bytes(data: &[u8], at: u64, len: usize) -> Result<&[u8], BmpError> {
  let at = usize::try_from(at).map_err(|_| TRUNCATED)?;
  data.get(at..at.saturating_add(len)).ok_or(TRUNCATED)
}

fn u32_at(data: &[u8], at: u64) -> Result<u32, BmpError> {
  let b = bytes(data, at, 4)?;
  Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl<'a> BmpDecoder<'a> {
  /// Reads the headers, masks and palette. An error here is an image Chrome fails to
  /// open (ImageDecoder's `tracks.ready` rejects).
  pub fn new(data: &'a [u8]) -> Result<Self, BmpError> {
    let file_header = bytes(data, 0, 14)?;
    if &file_header[..2] != b"BM" {
      return Err(BmpError("bad signature"));
    }
    let data_offset = u64::from(u32_at(data, 10)?);
    let header_size = u32_at(data, 14)?;
    let header_end = 14 + u64::from(header_size);
    let core = header_size == 12;
    let os2 = match header_size {
      12 | 40 | 52 | 56 | 108 | 124 => false,
      16..=64 if header_size % 4 == 0 || header_size == 42 || header_size == 46 => true,
      _ => return Err(BmpError("unknown header size")),
    };

    let (width, height, top_down, bit_count, compression, colors_used);
    if core {
      let h = bytes(data, 18, 8)?;
      width = i64::from(u16::from_le_bytes([h[0], h[1]]));
      height = i64::from(u16::from_le_bytes([h[2], h[3]]));
      top_down = false;
      bit_count = u16::from_le_bytes([h[6], h[7]]);
      if !matches!(bit_count, 1 | 4 | 8 | 24) {
        return Err(BmpError("unsupported bit count"));
      }
      compression = BI_RGB;
      colors_used = 0;
    } else {
      // An OS/2 header shorter than 40 bytes reads as zeros past its end. A longer
      // OS/2 header is read whole; a Windows header only up to its first 40 bytes.
      let read = (header_size as usize - 4).min(36);
      let mut h = [0; 36];
      h[..read].copy_from_slice(bytes(data, 18, read)?);
      if os2 {
        bytes(data, 18, header_size as usize - 4)?;
      }
      let w = i32::from_le_bytes([h[0], h[1], h[2], h[3]]);
      let mut hh = i32::from_le_bytes([h[4], h[5], h[6], h[7]]);
      if !(0..=0xFFFF).contains(&w) || hh > 0xFFFF || hh == i32::MIN {
        return Err(BmpError("bad dimensions"));
      }
      top_down = hh < 0;
      hh = hh.abs();
      width = i64::from(w);
      height = i64::from(hh);
      bit_count = u16::from_le_bytes([h[10], h[11]]);
      compression = u32::from_le_bytes([h[12], h[13], h[14], h[15]]);
      colors_used = u32::from_le_bytes([h[28], h[29], h[30], h[31]]);
    }
    if width == 0 || height == 0 {
      return Err(BmpError("empty image"));
    }

    let packed = |bytes, masks| Kind::Packed { bytes, masks };
    let mut kind = match (compression, bit_count) {
      (BI_RGB, 1 | 2 | 4 | 8) => Kind::Palette(u32::from(bit_count)),
      (BI_RGB, 16) => packed(2, RGB555),
      (BI_RGB, 24) => packed(3, RGB888),
      (BI_RGB, 32) => packed(4, RGB888),
      (BI_RLE8, 8) => Kind::Rle(Rle::Rle8),
      (BI_RLE4, 4) => Kind::Rle(Rle::Rle4),
      (BI_BITFIELDS | BI_ALPHABITFIELDS, 16 | 32) => {
        // Masks follow the 40-byte part of the header (inside V2 to V5 headers) or
        // the whole OS/2 header. The alpha mask is read with a V3 to V5 header, or
        // with BI_ALPHABITFIELDS in a Windows header.
        let at = if os2 { header_end } else { 54 };
        let alpha = !os2 && (header_size >= 56 || compression == BI_ALPHABITFIELDS);
        let raw = bytes(data, at, if alpha { 16 } else { 12 })?;
        let mut masks = [Bitfield::default(); 4];
        for (mask, &le) in masks.iter_mut().zip(raw.as_chunks().0) {
          *mask = Bitfield::from_mask(u32::from_le_bytes(le), u32::from(bit_count))?;
        }
        packed(usize::from(bit_count / 8), masks)
      }
      (BI_JPEG, 24) if os2 => Kind::Rle(Rle::Rle24),
      _ => return Err(BmpError("unsupported compression or bit count")),
    };
    // 32-bit BI_RGB with a V4 or V5 header has alpha when the alpha mask is set
    if compression == BI_RGB && bit_count == 32 && header_size >= 108 && u32_at(data, 66)? != 0 {
      kind = packed(4, ARGB8888);
    }
    // A V5 header with an embedded ICC profile must hold the profile in the file
    if header_size == 124 {
      bytes(data, 18, 120)?;
      let (offset, size) = (u32_at(data, 126)?, u32_at(data, 130)?);
      if u32_at(data, 70)? == 0x4D42_4544 && offset != 0 && size != 0 {
        bytes(data, 14 + u64::from(offset), size as usize)?;
      }
    }

    let mut palette = [[0, 0, 0, 255]; 256];
    if matches!(kind, Kind::Palette(_) | Kind::Rle(Rle::Rle4 | Rle::Rle8)) {
      let entry = if core { 3 } else { 4 };
      let count = match colors_used {
        0 => 1 << bit_count,
        n => (n as usize).min(1 << bit_count),
      };
      let table = bytes(data, header_end, count * entry)?;
      for (color, bgr) in palette.iter_mut().zip(table.chunks_exact(entry)) {
        *color = [bgr[0], bgr[1], bgr[2], 255];
      }
    }

    // Blink ImageDecoder::SetSize
    if width > 1_000_000 || height > 1_000_000 || width * height * 4 > i64::from(i32::MAX) {
      return Err(BmpError("image too large"));
    }
    Ok(BmpDecoder {
      data,
      width: width as u32,
      height: height as u32,
      top_down,
      data_offset,
      kind,
      palette,
    })
  }

  pub fn width(&self) -> u32 {
    self.width
  }

  pub fn height(&self) -> u32 {
    self.height
  }

  /// Chrome labels the image BGRA when true, BGRX when false
  pub fn has_alpha(&self) -> bool {
    matches!(self.kind, Kind::Packed { masks, .. } if masks[3].len != 0)
  }

  /// Decodes the pixels as unpremultiplied BGRA, top row first. An error is a decode
  /// Chrome fails (ImageDecoder's `decode()` rejects).
  pub fn decode(&self) -> Result<Vec<u8>, BmpError> {
    match self.kind {
      Kind::Palette(1) => self.decode_rows(1, |src, dst| palette_row::<1>(src, dst, &self.palette)),
      Kind::Palette(2) => self.decode_rows(2, |src, dst| palette_row::<2>(src, dst, &self.palette)),
      Kind::Palette(4) => self.decode_rows(4, |src, dst| palette_row::<4>(src, dst, &self.palette)),
      Kind::Palette(_) => self.decode_rows(8, |src, dst| palette_row::<8>(src, dst, &self.palette)),
      Kind::Packed { bytes: 2, masks } => {
        self.decode_rows(16, |src, dst| packed_row::<2>(src, dst, masks))
      }
      Kind::Packed { bytes: 3, masks } => {
        self.decode_rows(24, |src, dst| packed_row::<3>(src, dst, masks))
      }
      Kind::Packed { masks, .. } => {
        self.decode_rows(32, |src, dst| packed_row::<4>(src, dst, masks))
      }
      Kind::Rle(rle) => self.decode_rle(rle),
    }
  }

  /// Uncompressed rows of `bits` per pixel, each padded to 4 bytes. `row` converts one.
  fn decode_rows(
    &self,
    bits: usize,
    row: impl Fn(&[u8], &mut [[u8; 4]]),
  ) -> Result<Vec<u8>, BmpError> {
    let (width, height) = (self.width as usize, self.height as usize);
    let stride = (bits * width).div_ceil(32) * 4;
    // every row must be in the file
    let rows = bytes(self.data, self.data_offset, stride * height)?;
    let mut out = room_for(width * height * 4)?;
    // output rows top first, each zeroed just before it is written
    for y in 0..height {
      let i = if self.top_down { y } else { height - 1 - y };
      let at = out.len();
      out.resize(at + width * 4, 0);
      row(&rows[i * stride..][..stride], out[at..].as_chunks_mut().0);
    }
    Ok(out)
  }

  /// RLE4, RLE8 and RLE24 (image crate BmpDecoder::read_rle_data)
  fn decode_rle(&self, rle: Rle) -> Result<Vec<u8>, BmpError> {
    let (width, height) = (self.width as usize, self.height as usize);
    let mut pixels = room_for(width * height * 4)?;
    pixels.resize(width * height * 4, 0);
    let mut out = RleRows {
      pixels,
      width,
      row: 0,
      x: 0,
    };
    // output rows in file order
    let mut rows = (0..height).map(|i| if self.top_down { i } else { height - 1 - i });
    let mut src = Source {
      data: self.data,
      pos: self.data_offset,
    };
    let color = |index: u8| self.palette[usize::from(index)];
    let nibble = |pair: u8, i: usize| {
      if i.is_multiple_of(2) {
        pair >> 4
      } else {
        pair & 15
      }
    };
    while let Some(row) = rows.next() {
      out.start(row);
      // pixels counted in this row, which can pass the row end
      let mut count = 0;
      loop {
        match src.byte()? {
          0 => match src.byte()? {
            // end of row
            0 => {
              out.fill();
              break;
            }
            // end of image
            1 => {
              out.fill();
              for row in rows {
                out.start(row);
                out.fill();
              }
              return Ok(out.pixels);
            }
            // move right and down; skipped pixels are black
            2 => {
              let (dx, dy) = (usize::from(src.byte()?), src.byte()?);
              if dy > 0 {
                out.fill();
                for _ in 1..dy {
                  out.start(rows.next().ok_or(CORRUPT)?);
                  out.fill();
                }
                out.start(rows.next().ok_or(CORRUPT)?);
                out.black(count)?;
              }
              out.black(dx)?;
              count += dx;
            }
            // absolute run, padded to 2 bytes
            n => {
              let n = usize::from(n);
              match rle {
                Rle::Rle8 => {
                  let run = src.take(n + n % 2)?;
                  for &index in &run[..n] {
                    out.put(color(index))?;
                  }
                }
                Rle::Rle4 => {
                  let len = n.div_ceil(2);
                  let run = src.take(len + len % 2)?;
                  for i in 0..n {
                    out.put(color(nibble(run[i / 2], i)))?;
                  }
                }
                Rle::Rle24 => {
                  let run = src.take(n * 3 + n % 2)?;
                  for &[b, g, r] in run.as_chunks().0.iter().take(out.left()) {
                    out.put([b, g, r, 255])?;
                  }
                }
              }
              count += n;
            }
          },
          // encoded run; RLE8 and RLE24 runs stop at the row end
          n => {
            let n = usize::from(n);
            match rle {
              Rle::Rle8 => {
                let c = color(src.byte()?);
                for _ in 0..n.min(out.left()) {
                  out.put(c)?;
                }
              }
              Rle::Rle4 => {
                let pair = src.byte()?;
                for i in 0..n {
                  out.put(color(nibble(pair, i)))?;
                }
              }
              Rle::Rle24 => {
                let bgr = src.take(3)?;
                for _ in 0..n.min(out.left()) {
                  out.put([bgr[0], bgr[1], bgr[2], 255])?;
                }
              }
            }
            count += n;
          }
        }
      }
    }
    Ok(out.pixels)
  }
}

/// An empty buffer with room for `size` bytes, or an error when the memory is not there (Blink
/// fails the decode when it cannot allocate the frame)
fn room_for(size: usize) -> Result<Vec<u8>, BmpError> {
  let mut buffer = Vec::new();
  buffer
    .try_reserve_exact(size)
    .map_err(|_| BmpError("out of memory"))?;
  Ok(buffer)
}

/// One row of `BITS`-bit palette indexes (1, 2, 4 or 8), the first pixel in the high bits
fn palette_row<const BITS: u32>(src: &[u8], dst: &mut [[u8; 4]], palette: &[[u8; 4]; 256]) {
  for (pixels, &byte) in dst.chunks_mut((8 / BITS) as usize).zip(src) {
    for (k, px) in pixels.iter_mut().enumerate() {
      let index = (byte << (BITS * k as u32)) >> (8 - BITS);
      *px = palette[usize::from(index)];
    }
  }
}

/// One row of little-endian `N`-byte pixels, each channel read through its bitfield. Without an
/// alpha bitfield the alpha is 255.
fn packed_row<const N: usize>(src: &[u8], dst: &mut [[u8; 4]], [r, g, b, a]: [Bitfield; 4]) {
  let alpha = a.len != 0;
  for (px, pixel) in dst.iter_mut().zip(src.as_chunks::<N>().0) {
    let mut le = [0; 4];
    le[..N].copy_from_slice(pixel);
    let v = u32::from_le_bytes(le);
    *px = [
      b.read(v),
      g.read(v),
      r.read(v),
      if alpha { a.read(v) } else { 255 },
    ];
  }
}

const CORRUPT: BmpError = BmpError("corrupt RLE data");

struct Source<'a> {
  data: &'a [u8],
  pos: u64,
}

impl<'a> Source<'a> {
  fn take(&mut self, n: usize) -> Result<&'a [u8], BmpError> {
    let b = bytes(self.data, self.pos, n)?;
    self.pos += n as u64;
    Ok(b)
  }

  fn byte(&mut self) -> Result<u8, BmpError> {
    Ok(self.take(1)?[0])
  }
}

/// RLE output: the row being written and the next pixel in it
struct RleRows {
  pixels: Vec<u8>,
  width: usize,
  row: usize,
  x: usize,
}

impl RleRows {
  fn start(&mut self, row: usize) {
    self.row = row;
    self.x = 0;
  }

  fn left(&self) -> usize {
    self.width - self.x
  }

  /// Writes one pixel; past the row end is corrupt data
  fn put(&mut self, color: [u8; 4]) -> Result<(), BmpError> {
    if self.x >= self.width {
      return Err(CORRUPT);
    }
    let at = (self.row * self.width + self.x) * 4;
    self.pixels[at..at + 4].copy_from_slice(&color);
    self.x += 1;
    Ok(())
  }

  fn black(&mut self, n: usize) -> Result<(), BmpError> {
    (0..n).try_for_each(|_| self.put([0, 0, 0, 255]))
  }

  /// Black up to the row end
  fn fill(&mut self) {
    let _ = self.black(self.left());
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::webcodecs::chrome_cases::{self, Expected, fnv1a};

  #[test]
  fn pixels_match_chrome() {
    for case in chrome_cases::cases("bmp/") {
      let name = &case.name;
      let bmp = match (BmpDecoder::new(&case.data), &case.frames[..]) {
        (Err(_), [Expected::OpenError]) => continue,
        (Ok(_), [Expected::OpenError]) => panic!("{name}: Chrome fails to open it"),
        (Err(e), _) => panic!("{name}: {e}"),
        (Ok(bmp), _) => bmp,
      };
      match (bmp.decode(), &case.frames[..]) {
        (Err(_), [Expected::Error]) => {}
        (
          Ok(pixels),
          &[
            Expected::Frame {
              alpha,
              width,
              height,
              hash,
            },
          ],
        ) => {
          assert_eq!(bmp.has_alpha(), alpha, "{name}");
          assert_eq!((bmp.width(), bmp.height()), (width, height), "{name}");
          assert_eq!(fnv1a(&pixels), hash, "{name} pixels");
        }
        (pixels, expected) => panic!("{name}: {:?} vs {expected:?}", pixels.err()),
      }
    }
  }

  #[test]
  fn broken_files_do_not_panic() {
    for case in chrome_cases::cases("bmp/") {
      for data in chrome_cases::broken_copies(&case.data) {
        let Ok(bmp) = BmpDecoder::new(&data) else {
          continue;
        };
        // keep the test small; the size limit itself is Blink's (4 * w * h < 2^31)
        if bmp.width() * bmp.height() > 1 << 20 {
          continue;
        }
        if let Ok(pixels) = bmp.decode() {
          assert_eq!(pixels.len(), (bmp.width() * bmp.height() * 4) as usize);
        }
      }
    }
  }

  #[test]
  fn missing_rows_fail_before_allocating() {
    // 65535 x 8192 passes Blink's size limit; the 2 GiB output is never allocated
    let mut data = b"BM".to_vec();
    data.extend([0; 8]);
    data.extend(54u32.to_le_bytes());
    data.extend(40u32.to_le_bytes());
    data.extend(65535i32.to_le_bytes());
    data.extend(8192i32.to_le_bytes());
    data.extend([1, 0, 24, 0]);
    data.extend([0; 24]);
    let bmp = BmpDecoder::new(&data).unwrap();
    assert_eq!(bmp.decode(), Err(TRUNCATED));
  }

  #[test]
  fn channel_scaling() {
    // image crate tables: 1 bit v*255, 2 bits v*85, 3 to 6 bits rounded, 7 bits
    // (v << 1) | (v >> 6), 8 bits v
    let scale = |len, v| Bitfield::new(0, len).read(v);
    assert_eq!((scale(1, 1), scale(2, 2)), (255, 170));
    assert_eq!(
      (scale(3, 2), scale(4, 7), scale(5, 3), scale(6, 11)),
      (73, 119, 25, 45)
    );
    assert!((0..128).all(|v| u32::from(scale(7, v)) == (v << 1) | (v >> 6)));
    // every value of every length: the rounded division that SCALE replaces
    for len in 1..=8 {
      let max = (1 << len) - 1;
      assert!((0..=max).all(|v| u32::from(scale(len, v)) == (v * 255 + max / 2) / max));
    }
    assert_eq!(Bitfield::from_mask(0xFFC0, 16), Ok(Bitfield::new(8, 8)));
    assert!(Bitfield::from_mask(0x1F0000, 16).is_err());
    assert!(Bitfield::from_mask(0xF801, 16).is_err());
  }
}
