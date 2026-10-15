//! What PNG, GIF and BMP headers declare, read without FFmpeg.
//!
//! Chrome 155 decodes these images with Skia (the png crate, Wuffs and the image crate) and
//! labels each frame BGRA or BGRX by what its header declares, not by its pixels. Some of it
//! also changes bytes that FFmpeg decodes differently (see `PixelHint`).

use pixel_simd::Bitfield;

/// What a header says about pixels that FFmpeg decodes differently from Chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PixelHint {
  #[default]
  None,
  /// A 1, 2 or 4-bit gray PNG with a tRNS chunk: its transparent gray value scaled to 8 bits,
  /// or None when no sample can have it. FFmpeg drops it.
  GrayKey(Option<u8>),
  /// A 16 or 32-bit BMP whose pixels are bitfields: its R, G, B and A fields (5-5-5 for a
  /// 16-bit one without masks).
  Bitfields([Bitfield; 4]),
}

/// A PNG or BMP header: whether Chrome decodes the image with alpha, and its `PixelHint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageHeader {
  pub(crate) alpha: bool,
  pub(crate) pixels: PixelHint,
}

/// The png crate's view of a PNG header (Skia's PNG decoder in Chrome). Alpha: color type 4 or
/// 6, or a tRNS chunk the png crate accepts before IDAT, even when every entry is 255. It
/// ignores a tRNS chunk outside 1..=256 bytes, a gray one under 2 bytes, an RGB one under 6
/// bytes and a palette one before PLTE; then a later tRNS chunk can still count. None until the
/// header ends (IDAT).
pub(crate) fn png_header(data: &[u8]) -> Option<ImageHeader> {
  const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
  if data.len() < 33 || &data[..8] != SIGNATURE || &data[12..16] != b"IHDR" {
    return None;
  }
  let (depth, color_type) = (data[24], data[25]);
  if color_type & 4 != 0 {
    return Some(ImageHeader {
      alpha: true,
      pixels: PixelHint::None,
    });
  }
  let mut palette = false;
  let mut pos = 8usize;
  while let Some(header) = data.get(pos..pos.checked_add(8)?) {
    // length, type, data, CRC
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    match &header[4..8] {
      b"IDAT" | b"IEND" => {
        return Some(ImageHeader {
          alpha: false,
          pixels: PixelHint::None,
        });
      }
      b"PLTE" => palette = true,
      b"tRNS"
        if (1..=256).contains(&length)
          && match color_type {
            0 => length >= 2,
            2 => length >= 6,
            _ => palette,
          } =>
      {
        // The png crate compares each 1, 2 or 4-bit gray sample with the low byte of the value.
        let pixels = match depth {
          1 | 2 | 4 if color_type == 0 => {
            let (key, max) = (*data.get(pos + 9)?, (1u8 << depth) - 1);
            PixelHint::GrayKey((key <= max).then(|| key * (255 / max)))
          }
          _ => PixelHint::None,
        };
        return Some(ImageHeader {
          alpha: true,
          pixels,
        });
      }
      _ => {}
    }
    pos = pos.checked_add(12)?.checked_add(length)?;
  }
  None
}

/// The fields of a 16-bit BI_RGB pixel.
const RGB555: [Bitfield; 4] = [
  Bitfield { shift: 10, len: 5 },
  Bitfield { shift: 5, len: 5 },
  Bitfield { shift: 0, len: 5 },
  Bitfield { shift: 0, len: 0 },
];

/// The image crate's view of a BMP header (Skia's BMP decoder in Chrome, with Chromium's
/// patches). Alpha: a non-zero alpha mask in a V3, V4 or V5 header (BI_BITFIELDS) or after the
/// header (BI_ALPHABITFIELDS), or a 32-bit BI_RGB image whose V4 or V5 header has a non-zero
/// alpha mask. Other BMPs are opaque: Chrome ignores the 4th byte of a 32-bit BI_RGB pixel.
/// Err: a bitfield mask the image crate rejects, so Chrome fails the metadata. None for a
/// header without these fields (or not a BMP).
pub(crate) fn bmp_header(data: &[u8]) -> Result<Option<ImageHeader>, ()> {
  let u32_at = |pos: usize| {
    data
      .get(pos..pos + 4)
      .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
  };
  if data.get(..2) != Some(b"BM") {
    return Ok(None);
  }
  // The image crate reads the masks after the first 40 header bytes (inside a V2+ header, or
  // right after a 40-byte one), or after an OS/2 2.x header.
  let (Some(size), Some(bits), Some(compression)) = (
    u32_at(14),
    data.get(28..30).map(|b| u16::from_le_bytes([b[0], b[1]])),
    u32_at(30),
  ) else {
    return Ok(None);
  };
  let masks_at = match size {
    40 | 52 | 56 | 108 | 124 => 54,
    64 => 78,
    _ => return Ok(None),
  };
  let opaque = |pixels| {
    Ok(Some(ImageHeader {
      alpha: false,
      pixels,
    }))
  };
  match (compression, bits) {
    // BI_BITFIELDS, BI_ALPHABITFIELDS
    (3 | 6, 16 | 32) => {
      let alpha_mask = matches!(size, 56 | 108 | 124) || compression == 6;
      let masks = [
        u32_at(masks_at),
        u32_at(masks_at + 4),
        u32_at(masks_at + 8),
        if alpha_mask {
          u32_at(masks_at + 12)
        } else {
          Some(0)
        },
      ];
      let [Some(r), Some(g), Some(b), Some(a)] = masks else {
        return Ok(None);
      };
      let fields = [r, g, b, a].map(|mask| Bitfield::from_mask(mask, bits.into()));
      let [Some(r), Some(g), Some(b), Some(a)] = fields else {
        return Err(());
      };
      Ok(Some(ImageHeader {
        alpha: a.len > 0,
        pixels: PixelHint::Bitfields([r, g, b, a]),
      }))
    }
    // BI_RGB
    (0, 16) => opaque(PixelHint::Bitfields(RGB555)),
    (0, 32) if size >= 108 => match u32_at(66) {
      Some(alpha_mask) => Ok(Some(ImageHeader {
        alpha: alpha_mask != 0,
        pixels: PixelHint::None,
      })),
      None => Ok(None),
    },
    _ => opaque(PixelHint::None),
  }
}

/// A GIF frame as Wuffs (Chrome's GIF decoder) reports it to Skia.
#[derive(Clone, Copy, Debug)]
struct GifFrame {
  rect: Rect,
  /// Whether its graphic control extension declares a transparent index.
  transparent: bool,
  disposal: Disposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposal {
  Keep,
  RestoreBackground,
  RestorePrevious,
}

/// A rectangle from (x0, y0) up to, not including, (x1, y1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
  x0: u32,
  y0: u32,
  x1: u32,
  y1: u32,
}

impl Rect {
  fn is_empty(self) -> bool {
    self.x0 >= self.x1 || self.y0 >= self.y1
  }

  /// The part on the screen, or an empty rectangle (Skia's frame_rect_on_screen).
  fn on(self, screen: Rect) -> Rect {
    let rect = Rect {
      x0: self.x0.max(screen.x0),
      y0: self.y0.max(screen.y0),
      x1: self.x1.min(screen.x1),
      y1: self.y1.min(screen.y1),
    };
    if rect.is_empty() {
      Rect::default()
    } else {
      rect
    }
  }

  /// SkIRect::contains: false when either is empty.
  fn contains(self, other: Rect) -> bool {
    !self.is_empty()
      && !other.is_empty()
      && self.x0 <= other.x0
      && self.y0 <= other.y0
      && self.x1 >= other.x1
      && self.y1 >= other.y1
  }
}

/// Whether Chrome decodes each frame of a GIF with alpha. None when the data is not a GIF.
pub(crate) fn gif_frame_alpha(data: &[u8]) -> Option<Vec<bool>> {
  if !matches!(data.get(..6)?, b"GIF87a" | b"GIF89a") {
    return None;
  }
  let u16_at = |pos: usize| {
    data
      .get(pos..pos + 2)
      .map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])))
  };
  // 3 bytes for each of 2^(n + 1) colors, when the flags have a color table
  let color_table = |flags: u8| (flags & 0x80 != 0).then(|| 3usize << ((flags & 7) + 1));
  let (width, height) = (u16_at(6)?, u16_at(8)?);
  let mut pos = 13 + color_table(*data.get(10)?).unwrap_or(0);
  let mut frames = Vec::new();
  let (mut transparent, mut disposal) = (false, Disposal::Keep);
  loop {
    let next = match data.get(pos) {
      // Extension: a graphic control extension (4 bytes) sets the next frame's transparent
      // index and disposal (Wuffs reads disposal 4 as 3)
      Some(0x21) => {
        if data.get(pos + 1..pos + 3) == Some(&[0xf9, 4]) {
          let flags = data.get(pos + 3).copied().unwrap_or(0);
          transparent = flags & 1 != 0;
          disposal = match (flags >> 2) & 7 {
            2 => Disposal::RestoreBackground,
            3 | 4 => Disposal::RestorePrevious,
            _ => Disposal::Keep,
          };
        }
        skip_sub_blocks(data, pos + 2)
      }
      // Image descriptor: x, y, width, height, flags, then a color table, the LZW code size
      // and the image data
      Some(0x2c) => {
        let (Some(x), Some(y), Some(w), Some(h), Some(&flags)) = (
          u16_at(pos + 1),
          u16_at(pos + 3),
          u16_at(pos + 5),
          u16_at(pos + 7),
          data.get(pos + 9),
        ) else {
          break;
        };
        frames.push(GifFrame {
          rect: Rect {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
          },
          transparent,
          disposal,
        });
        (transparent, disposal) = (false, Disposal::Keep);
        skip_sub_blocks(data, pos + 11 + color_table(flags).unwrap_or(0))
      }
      // trailer, another block, or the end of the data
      _ => None,
    };
    match next {
      Some(next) => pos = next,
      None => break,
    }
  }
  // Wuffs grows the screen to the first frame.
  let screen = frames.first().map_or(
    Rect {
      x1: width,
      y1: height,
      ..Rect::default()
    },
    |first| Rect {
      x1: width.max(first.rect.x1),
      y1: height.max(first.rect.y1),
      ..Rect::default()
    },
  );
  Some(frame_alpha(&frames, screen))
}

/// The position after the data sub-blocks that start at `pos`, if they all fit.
fn skip_sub_blocks(data: &[u8], mut pos: usize) -> Option<usize> {
  loop {
    match *data.get(pos)? {
      0 => return Some(pos + 1),
      size => pos += 1 + usize::from(size),
    }
  }
}

/// Chrome's alpha for each frame: SkFrameHolder::setAlphaAndRequiredFrame (skia
/// src/codec/SkCodec.cpp). A frame is opaque only when it and the frames it is drawn over
/// cover the screen with opaque pixels. A GIF frame always blends over the one before it
/// (Wuffs' overwrite_instead_of_blend is false), so Skia's kSrc case never happens.
fn frame_alpha(frames: &[GifFrame], screen: Rect) -> Vec<bool> {
  // (has alpha, required frame) of each frame so far
  let mut done: Vec<(bool, Option<usize>)> = Vec::with_capacity(frames.len());
  for (i, frame) in frames.iter().enumerate() {
    let next = alpha_and_required_frame(frames, &done, i, frame, screen);
    done.push(next);
  }
  done.into_iter().map(|(alpha, _)| alpha).collect()
}

fn alpha_and_required_frame(
  frames: &[GifFrame],
  done: &[(bool, Option<usize>)],
  i: usize,
  frame: &GifFrame,
  screen: Rect,
) -> (bool, Option<usize>) {
  let rect = frame.rect.on(screen);
  // IND1: the first frame
  if i == 0 {
    return (frame.transparent || rect != screen, None);
  }
  // IND2: an opaque frame that fills the screen
  if !frame.transparent && rect == screen {
    return (false, None);
  }
  // IND3: every frame before it is restored away
  let mut prev = i - 1;
  while frames[prev].disposal == Disposal::RestorePrevious {
    if prev == 0 {
      return (true, None);
    }
    prev -= 1;
  }
  let clear_prev = frames[prev].disposal == Disposal::RestoreBackground;
  let mut prev_rect = frames[prev].rect.on(screen);
  // IND4: the frame before is cleared, and it fills the screen or is independent
  if clear_prev && (prev_rect == screen || done[prev].1.is_none()) {
    return (true, None);
  }
  // DEP5: a frame with transparency blends over the one before
  if frame.transparent {
    return (done[prev].0 || clear_prev, Some(prev));
  }
  // IND6: it covers every frame back to an independent one
  while rect.contains(prev_rect) {
    let Some(required) = done[prev].1 else {
      return (true, None);
    };
    prev = required;
    prev_rect = frames[prev].rect.on(screen);
  }
  // DEP7
  (
    frames[prev].disposal == Disposal::RestoreBackground || done[prev].0,
    Some(prev),
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    c.extend_from_slice(&[0; 4]); // CRC is not checked
    c
  }

  fn png(color_type: u8, depth: u8, chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend(chunk(
      b"IHDR",
      &[0, 0, 0, 1, 0, 0, 0, 1, depth, color_type, 0, 0, 0],
    ));
    chunks.iter().for_each(|c| b.extend_from_slice(c));
    b.extend(chunk(b"IDAT", &[]));
    b
  }

  fn alpha(header: Option<ImageHeader>) -> Option<bool> {
    header.map(|h| h.alpha)
  }

  #[test]
  fn reads_png_alpha_like_the_png_crate() {
    let plte = chunk(b"PLTE", &[0, 0, 0]);
    let trns = |len: usize| chunk(b"tRNS", &vec![255; len]);
    let text = chunk(b"tEXt", b"a\0b");
    assert_eq!(
      alpha(png_header(&png(3, 8, std::slice::from_ref(&plte)))),
      Some(false)
    );
    // tRNS declares alpha even when every entry is 255
    assert_eq!(
      alpha(png_header(&png(
        3,
        8,
        &[plte.clone(), text.clone(), trns(1)]
      ))),
      Some(true)
    );
    assert_eq!(alpha(png_header(&png(0, 8, &[trns(2)]))), Some(true));
    assert_eq!(alpha(png_header(&png(2, 8, &[trns(6)]))), Some(true));
    assert_eq!(alpha(png_header(&png(2, 8, &[text]))), Some(false));
    assert_eq!(alpha(png_header(&png(4, 8, &[]))), Some(true));
    assert_eq!(alpha(png_header(&png(6, 8, &[]))), Some(true));
    // ignored: empty, too short for gray or RGB, longer than 256, palette before PLTE
    assert_eq!(
      alpha(png_header(&png(3, 8, &[plte.clone(), trns(0)]))),
      Some(false)
    );
    assert_eq!(alpha(png_header(&png(0, 8, &[trns(1)]))), Some(false));
    assert_eq!(alpha(png_header(&png(2, 8, &[trns(5)]))), Some(false));
    assert_eq!(
      alpha(png_header(&png(3, 8, &[plte.clone(), trns(257)]))),
      Some(false)
    );
    assert_eq!(
      alpha(png_header(&png(3, 8, &[trns(1), plte.clone()]))),
      Some(false)
    );
    // a later valid tRNS still counts; one after IDAT does not
    assert_eq!(
      alpha(png_header(&png(0, 8, &[trns(1), trns(2)]))),
      Some(true)
    );
    let mut after_idat = png(2, 8, &[]);
    after_idat.extend(trns(6));
    assert_eq!(alpha(png_header(&after_idat)), Some(false));
    // no IDAT yet (truncated) or not a PNG
    assert_eq!(png_header(&png(3, 8, &[])[..33]), None);
    assert_eq!(png_header(b"GIF89a"), None);
  }

  #[test]
  fn reads_the_gray_key_of_low_bit_pngs() {
    let key = |depth: u8, value: [u8; 2]| {
      png_header(&png(0, depth, &[chunk(b"tRNS", &value)])).map(|h| h.pixels)
    };
    assert_eq!(key(1, [0, 1]), Some(PixelHint::GrayKey(Some(255))));
    assert_eq!(key(2, [0, 2]), Some(PixelHint::GrayKey(Some(170))));
    assert_eq!(key(4, [0, 15]), Some(PixelHint::GrayKey(Some(255))));
    // only the low byte counts; a value no sample can have makes nothing transparent
    assert_eq!(key(2, [1, 1]), Some(PixelHint::GrayKey(Some(85))));
    assert_eq!(key(2, [0, 4]), Some(PixelHint::GrayKey(None)));
    // FFmpeg keeps 8-bit keys
    assert_eq!(key(8, [0, 4]), Some(PixelHint::None));
  }

  #[test]
  fn reads_bitfields_like_the_image_crate() {
    const LUT5: [u8; 32] = [
      0, 8, 16, 25, 33, 41, 49, 58, 66, 74, 82, 90, 99, 107, 115, 123, 132, 140, 148, 156, 165,
      173, 181, 189, 197, 206, 214, 222, 230, 239, 247, 255,
    ];
    const LUT6: [u8; 64] = [
      0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 45, 49, 53, 57, 61, 65, 69, 73, 77, 81, 85, 89, 93,
      97, 101, 105, 109, 113, 117, 121, 125, 130, 134, 138, 142, 146, 150, 154, 158, 162, 166, 170,
      174, 178, 182, 186, 190, 194, 198, 202, 206, 210, 215, 219, 223, 227, 231, 235, 239, 243,
      247, 251, 255,
    ];
    let read = |mask: u32, v: u32| {
      let field = Bitfield::from_mask(mask, 32).unwrap();
      field.read(v << mask.trailing_zeros())
    };
    for v in 0..64 {
      assert_eq!(read(0x3f << 5, v), LUT6[v as usize]);
    }
    for v in 0..32 {
      assert_eq!(read(0x1f << 10, v), LUT5[v as usize]);
    }
    for v in 0..16 {
      assert_eq!(read(0xf000, v), v as u8 * 17);
    }
    assert_eq!(
      (0..8).map(|v| read(0x7, v)).collect::<Vec<_>>(),
      [0, 36, 73, 109, 146, 182, 219, 255]
    );
    assert_eq!(read(0x3, 2), 170);
    assert_eq!(read(0x8000, 1), 255);
    assert_eq!(read(0x7f, 64), 129);
    assert_eq!(read(0xff00, 0xab), 0xab);
    // wider than 8 bits: the top 8
    assert_eq!(read(0x3ff, 0x2ab), 0xaa);
    assert_eq!(Bitfield::from_mask(0, 16), Some(Bitfield::default()));
    // not contiguous, or past the pixel bits
    assert_eq!(Bitfield::from_mask(0x8001, 16), None);
    assert_eq!(Bitfield::from_mask(0x18000, 16), None);
    assert_eq!(Bitfield::from_mask(0xffff0000, 16), None);
    assert!(Bitfield::from_mask(0xffff0000, 32).is_some());
  }

  /// BMP file and DIB header bytes up to the masks and the alpha mask of a V3+ header.
  fn bmp(size: u32, bits: u16, compression: u32, masks: [u32; 4]) -> Vec<u8> {
    let mut b = vec![0u8; 70];
    b[..2].copy_from_slice(b"BM");
    b[14..18].copy_from_slice(&size.to_le_bytes());
    b[28..30].copy_from_slice(&bits.to_le_bytes());
    b[30..34].copy_from_slice(&compression.to_le_bytes());
    for (i, mask) in masks.iter().enumerate() {
      b[54 + 4 * i..58 + 4 * i].copy_from_slice(&mask.to_le_bytes());
    }
    b
  }

  #[test]
  fn reads_bmp_alpha_like_the_image_crate() {
    let alpha = |data: &[u8]| bmp_header(data).map(|h| h.map(|h| h.alpha));
    const BGRA: [u32; 4] = [0xff0000, 0xff00, 0xff, 0xff000000];
    // BI_BITFIELDS: the alpha mask of a V3, V4 or V5 header
    for size in [56, 108, 124] {
      assert_eq!(alpha(&bmp(size, 32, 3, BGRA)), Ok(Some(true)));
    }
    assert_eq!(
      alpha(&bmp(56, 32, 3, [0xff0000, 0xff00, 0xff, 0])),
      Ok(Some(false))
    );
    // a 40-byte header has no alpha mask (the 4th dword is pixel data), nor has a V2 header
    assert_eq!(alpha(&bmp(40, 32, 3, BGRA)), Ok(Some(false)));
    assert_eq!(alpha(&bmp(52, 32, 3, BGRA)), Ok(Some(false)));
    // BI_ALPHABITFIELDS: the 4th mask after a 40-byte header
    assert_eq!(alpha(&bmp(40, 32, 6, BGRA)), Ok(Some(true)));
    // BI_RGB: only a V4 or V5 header's alpha mask, not a V3 one
    assert_eq!(alpha(&bmp(56, 32, 0, BGRA)), Ok(Some(false)));
    for size in [108, 124] {
      assert_eq!(alpha(&bmp(size, 32, 0, BGRA)), Ok(Some(true)));
      assert_eq!(alpha(&bmp(size, 32, 0, [0; 4])), Ok(Some(false)));
    }
    assert_eq!(alpha(&bmp(40, 32, 0, BGRA)), Ok(Some(false)));
    assert_eq!(alpha(&bmp(40, 24, 0, BGRA)), Ok(Some(false)));
    // 16-bit: the masks are the pixel's fields
    let argb1555 = [0x7c00, 0x3e0, 0x1f, 0x8000];
    let header = bmp_header(&bmp(108, 16, 3, argb1555)).unwrap().unwrap();
    assert!(header.alpha);
    assert_eq!(
      header.pixels,
      PixelHint::Bitfields(argb1555.map(|m| Bitfield::from_mask(m, 16).unwrap()))
    );
    assert_eq!(
      RGB555,
      [0x7c00, 0x3e0, 0x1f, 0].map(|m| Bitfield::from_mask(m, 16).unwrap())
    );
    assert_eq!(
      bmp_header(&bmp(40, 16, 0, [0; 4])),
      Ok(Some(ImageHeader {
        alpha: false,
        pixels: PixelHint::Bitfields(RGB555),
      }))
    );
    // 32-bit: the masks too, even an alpha mask over a color field (FFmpeg reads another byte)
    let alpha_on_blue = [0xff0000, 0xff00, 0xff, 0xff];
    assert_eq!(
      bmp_header(&bmp(108, 32, 3, alpha_on_blue))
        .unwrap()
        .unwrap()
        .pixels,
      PixelHint::Bitfields(alpha_on_blue.map(|m| Bitfield::from_mask(m, 32).unwrap()))
    );
    // a mask the image crate rejects, even one FFmpeg ignores
    assert_eq!(
      alpha(&bmp(56, 16, 3, [0x7c00, 0x3e0, 0x1f, 0x8001])),
      Err(())
    );
    assert_eq!(
      alpha(&bmp(56, 16, 3, [0x7c00, 0x3e0, 0x1f, 0x18000])),
      Err(())
    );
    assert_eq!(
      alpha(&bmp(108, 32, 3, [0xff0000, 0xff00, 0xff, 0xf0f00000])),
      Err(())
    );
    // the core header, a truncated header, or not a BMP
    assert_eq!(alpha(&bmp(12, 24, 0, [0; 4])), Ok(None));
    assert_eq!(alpha(&bmp(56, 32, 3, BGRA)[..60]), Ok(None));
    assert_eq!(alpha(b"GIF89a"), Ok(None));
  }

  fn frame(rect: [u32; 4], transparent: bool, disposal: Disposal) -> GifFrame {
    let [x, y, w, h] = rect;
    GifFrame {
      rect: Rect {
        x0: x,
        y0: y,
        x1: x + w,
        y1: y + h,
      },
      transparent,
      disposal,
    }
  }

  const SCREEN: Rect = Rect {
    x0: 0,
    y0: 0,
    x1: 11,
    y1: 5,
  };
  const FULL: [u32; 4] = [0, 0, 11, 5];
  use Disposal::{Keep, RestoreBackground as Background, RestorePrevious as Previous};

  #[test]
  fn gif_frame_alpha_follows_each_branch_of_skias_rule() {
    let alpha = |frames: &[GifFrame]| frame_alpha(frames, SCREEN);
    // IND1: the first frame has alpha when it has a transparent index or leaves screen pixels
    assert_eq!(alpha(&[frame(FULL, false, Keep)]), [false]);
    assert_eq!(alpha(&[frame(FULL, true, Keep)]), [true]);
    assert_eq!(alpha(&[frame([1, 1, 4, 3], false, Keep)]), [true]);
    // IND2: an opaque frame that fills the screen, after any frame
    assert_eq!(
      alpha(&[frame(FULL, true, Keep), frame(FULL, false, Keep)]),
      [true, false]
    );
    // IND3: every frame before it is restored to the previous image
    assert_eq!(
      alpha(&[
        frame(FULL, false, Previous),
        frame([5, 0, 6, 5], false, Keep)
      ]),
      [false, true]
    );
    // IND4: the frame before is cleared and fills the screen (or is independent)
    assert_eq!(
      alpha(&[
        frame(FULL, false, Background),
        frame([2, 1, 3, 2], false, Keep)
      ]),
      [false, true]
    );
    // DEP5: a frame with a transparent index takes the alpha of the frame below
    assert_eq!(
      alpha(&[frame(FULL, false, Keep), frame([1, 1, 5, 3], true, Keep)]),
      [false, false]
    );
    assert_eq!(
      alpha(&[
        frame([0, 0, 5, 5], false, Keep),
        frame([1, 1, 5, 3], true, Keep)
      ]),
      [true, true]
    );
    // DEP5 after a cleared dependent frame
    assert_eq!(
      alpha(&[
        frame(FULL, false, Keep),
        frame([1, 1, 5, 3], true, Background),
        frame([2, 2, 3, 2], true, Keep),
      ]),
      [false, false, true]
    );
    // IND6: it covers every frame back to an independent one with alpha
    assert_eq!(
      alpha(&[
        frame([1, 1, 4, 3], false, Keep),
        frame([0, 0, 6, 5], false, Keep)
      ]),
      [true, true]
    );
    // DEP7: drawn over an opaque frame, or over a cleared one
    assert_eq!(
      alpha(&[frame(FULL, false, Keep), frame([2, 1, 3, 2], false, Keep)]),
      [false, false]
    );
    assert_eq!(
      alpha(&[
        frame(FULL, false, Keep),
        frame([2, 1, 3, 2], true, Background),
        frame([0, 0, 4, 2], false, Keep),
      ]),
      [false, false, true]
    );
  }

  #[test]
  fn gif_frame_alpha_matches_chrome_on_the_research_animations() {
    let alpha = |frames: &[GifFrame]| frame_alpha(frames, SCREEN);
    // gif_anim_trans.gif: X X A A A
    assert_eq!(
      alpha(&[
        frame(FULL, false, Keep),
        frame([1, 1, 5, 3], true, Background),
        frame(FULL, true, Keep),
        frame([3, 0, 4, 5], false, Previous),
        frame([0, 2, 11, 2], false, Keep),
      ]),
      [false, false, true, true, true]
    );
    // gif_anim_f0trans.gif: A X X
    assert_eq!(
      alpha(&[
        frame(FULL, true, Keep),
        frame(FULL, false, Keep),
        frame(FULL, true, Keep)
      ]),
      [true, false, false]
    );
    // gif_anim_smallf0.gif: A A X
    assert_eq!(
      alpha(&[
        frame([1, 1, 4, 3], false, Background),
        frame([5, 0, 6, 5], false, Keep),
        frame(FULL, false, Keep),
      ]),
      [true, true, false]
    );
    // gif_anim_dispose_bg_opaque.gif: X A
    assert_eq!(
      alpha(&[
        frame(FULL, false, Background),
        frame([2, 1, 3, 2], false, Keep)
      ]),
      [false, true]
    );
  }

  /// A GIF: 11x5 screen, a 2-color global table, and per frame (rect, GCE flags).
  fn gif(screen: (u16, u16), frames: &[([u16; 4], Option<u8>)]) -> Vec<u8> {
    let mut b = b"GIF89a".to_vec();
    b.extend(screen.0.to_le_bytes());
    b.extend(screen.1.to_le_bytes());
    b.extend([0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    for (rect, gce) in frames {
      if let Some(flags) = gce {
        b.extend([0x21, 0xf9, 4, *flags, 0, 0, 0, 0]);
      }
      b.push(0x2c);
      rect.iter().for_each(|v| b.extend(v.to_le_bytes()));
      // no local table; LZW code size 2, one sub-block, terminator
      b.extend([0, 2, 2, 0x4c, 0x01, 0]);
    }
    b.push(0x3b);
    b
  }

  #[test]
  fn walks_gif_blocks_like_wuffs() {
    // a transparent index, and the screen growing to the first frame
    assert_eq!(
      gif_frame_alpha(&gif((11, 5), &[([0, 0, 11, 5], Some(1))])),
      Some(vec![true])
    );
    assert_eq!(
      gif_frame_alpha(&gif((11, 5), &[([0, 0, 11, 5], Some(0))])),
      Some(vec![false])
    );
    assert_eq!(
      gif_frame_alpha(&gif((4, 2), &[([0, 0, 11, 5], None)])),
      Some(vec![false])
    );
    // the GCE applies to the next frame only
    assert_eq!(
      gif_frame_alpha(&gif(
        (11, 5),
        &[([0, 0, 11, 5], Some(1)), ([0, 0, 11, 5], None)]
      )),
      Some(vec![true, false])
    );
    // disposal 4 is "restore previous" too: frame 1 has nothing opaque below it
    for disposal in [3, 4] {
      assert_eq!(
        gif_frame_alpha(&gif(
          (11, 5),
          &[([0, 0, 11, 5], Some(disposal << 2)), ([5, 0, 6, 5], None)]
        )),
        Some(vec![false, true])
      );
    }
    // an application extension between frames, and a truncated last frame, still count
    let mut data = gif((11, 5), &[([0, 0, 11, 5], None)]);
    data.pop();
    data.extend(b"\x21\xff\x0bNETSCAPE2.0\x03\x01\0\0\0");
    data.extend([0x2c, 0, 0, 0, 0, 11, 0, 5, 0, 0, 2, 2]);
    assert_eq!(gif_frame_alpha(&data), Some(vec![false, false]));
    assert_eq!(gif_frame_alpha(b"\x89PNG"), None);
  }
}
