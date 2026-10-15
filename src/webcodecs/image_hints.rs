//! What PNG headers declare, read without FFmpeg.
//!
//! Chrome 155 decodes PNG with Skia (the png crate) and labels each frame BGRA or BGRX by what
//! its header declares, not by its pixels. Some of it also changes bytes that FFmpeg decodes
//! differently (see `PixelHint`).

/// What a header says about pixels that FFmpeg decodes differently from Chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PixelHint {
  #[default]
  None,
  /// A 1, 2 or 4-bit gray PNG with a tRNS chunk: its transparent gray value scaled to 8 bits,
  /// or None when no sample can have it. FFmpeg drops it.
  GrayKey(Option<u8>),
}

/// A PNG header: whether Chrome decodes the image with alpha, and its `PixelHint`.
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
}
