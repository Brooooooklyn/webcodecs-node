//! PNG and APNG as Chrome 155 reads and draws them.
//!
//! Chrome decodes PNG with Skia's SkPngRustCodec over the png crate 0.18.1, and Blink's
//! SkiaImageDecoderBase draws each frame on the frame Skia says it needs. `parse_with` reads the
//! chunks the way the png crate's StreamingDecoder does and lists the frames the way Skia counts
//! them; `PngLayout` draws them:
//!
//! - the header is ready at the header of the first IDAT chunk (the png crate's read_info); an
//!   error before that fails the image
//! - frame 0 is the IDAT image, unless an acTL comes with no fcTL before IDAT; every later frame
//!   counts at the header of its first fdAT chunk, up to acTL's frame count
//! - a frame's data ends at the header of the next chunk that is not one of its data chunks
//! - after the header, an error ends the frame list; a frame whose data holds the error fails
//! - a bad CRC on a critical chunk (IDAT, PLTE, ...) is an error; an ancillary chunk with a bad
//!   CRC (fcTL, tRNS, ...) is skipped, but fdAT data is used whatever its CRC
//! - a frame starts from a clear canvas or from the frame it requires (Skia's
//!   setAlphaAndRequiredFrame); blending over is Skia's blendRow, in floats, unpremultiplied
//!
//! Frames are unpremultiplied BGRA. `PngFrame::has_alpha` says whether Chrome labels the frame
//! BGRA (true) or BGRX (false).

use crate::webcodecs::image_hints::{ImageHeader, PixelHint};
use std::collections::HashMap;

const SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
/// Skia's and Blink's limit on each side (SkPngRustCodec CreateEncodedInfo)
const MAX_SIDE: u32 = 1_000_000;

/// What a frame does to the canvas once it has been shown (APNG dispose_op)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dispose {
  None,
  Background,
  Previous,
}

/// How a frame goes onto the canvas (APNG blend_op). Skia draws frame 0 as `Source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Blend {
  Source,
  Over,
}

/// Rect with exclusive right and bottom edges
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rect {
  pub(crate) x0: u32,
  pub(crate) y0: u32,
  pub(crate) x1: u32,
  pub(crate) y1: u32,
}

impl Rect {
  fn is_empty(self) -> bool {
    self.x0 >= self.x1 || self.y0 >= self.y1
  }

  /// SkIRect::contains: false when either rect is empty
  fn contains(self, r: Rect) -> bool {
    !r.is_empty()
      && !self.is_empty()
      && self.x0 <= r.x0
      && self.y0 <= r.y0
      && self.x1 >= r.x1
      && self.y1 >= r.y1
  }

  fn width(self) -> u32 {
    self.x1 - self.x0
  }

  fn height(self) -> u32 {
    self.y1 - self.y0
  }
}

/// Where a frame's data stands
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DataState {
  /// All its data chunks are here, and the header of the chunk after them
  Complete,
  /// More of it can still come
  Incomplete,
  /// It holds an error (Chrome's decode of it fails)
  Failed,
}

/// How often an animation plays, as Skia reports it
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Repetition {
  /// A still image (Blink kAnimationNone)
  None,
  /// An animation that plays once (Blink kAnimationLoopOnce)
  Once,
  /// Forever (Blink kAnimationLoopInfinite)
  Infinite,
  /// This many times after the first play
  Count(u32),
}

/// One frame of the list
#[derive(Clone, Debug)]
pub(crate) struct PngFrame {
  pub(crate) rect: Rect,
  /// Skia's frame duration, in milliseconds
  pub(crate) duration_ms: u32,
  pub(crate) dispose: Dispose,
  pub(crate) blend: Blend,
  /// Chrome labels the frame BGRA when true, BGRX when false
  pub(crate) has_alpha: bool,
  /// The frame this one is drawn on (Skia's fRequiredFrame), None for a clear canvas
  pub(crate) required: Option<usize>,
  /// Where its compressed data is: the data of its IDAT chunks, or of its fdAT chunks after
  /// their sequence numbers
  data: Vec<(usize, usize)>,
  pub(crate) state: DataState,
}

/// A PNG's header and frame list, read so far
#[derive(Clone, Debug)]
pub(crate) struct PngLayout {
  pub(crate) width: u32,
  pub(crate) height: u32,
  /// What the header says about alpha and pixels (see `image_hints`)
  pub(crate) header: ImageHeader,
  pub(crate) repetition: Repetition,
  pub(crate) frames: Vec<PngFrame>,
  /// IHDR's data
  ihdr: [u8; 13],
  /// The chunks before the image data that a still PNG of one frame keeps: PLTE, the tRNS the
  /// png crate takes, and the colour chunks FFmpeg reads (gAMA, cHRM, sRGB, cICP). Not iCCP:
  /// FFmpeg fails an image whose iCCP it cannot read, Chrome does not. With an iCCP, FFmpeg
  /// ignores gAMA, cHRM and sRGB, so they are left out too.
  kept: Vec<(usize, usize)>,
}

/// The CRC checks made so far, by chunk offset. The data only grows, so each chunk is checked
/// once, also when the data is read again for every chunk that arrives.
#[derive(Debug, Default)]
pub(crate) struct CrcMemo(HashMap<usize, bool>);

impl CrcMemo {
  fn check(&mut self, at: usize, bytes: &[u8], crc: &[u8]) -> bool {
    *self
      .0
      .entry(at)
      .or_insert_with(|| crc32(bytes) == u32::from_be_bytes([crc[0], crc[1], crc[2], crc[3]]))
  }
}

/// What the data so far says
#[derive(Clone, Debug)]
pub(crate) enum PngParse {
  /// The header has not ended yet
  Incomplete,
  /// Chrome fails the image (`tracks.ready` rejects)
  Bad,
  Ready(PngLayout),
}

/// fcTL, as the png crate keeps it
#[derive(Clone, Copy, Debug)]
struct Fctl {
  rect: Rect,
  delay_num: u16,
  delay_den: u16,
  dispose: Dispose,
  blend: Blend,
}

impl Fctl {
  /// Skia's duration (SkPngRustCodec, through the png crate's FrameControl)
  fn duration_ms(self) -> u32 {
    let (num, den) = (u32::from(self.delay_num), u32::from(self.delay_den));
    // a 0 denominator means 1/100 s
    (num * 1000).checked_div(den).unwrap_or(num * 10)
  }
}

/// The png crate's view of tRNS: the chunk it takes, and for a 1, 2 or 4-bit gray image the
/// gray value it compares the samples with
#[derive(Clone, Copy, Debug)]
struct Trns {
  pixels: PixelHint,
}

struct Ihdr {
  width: u32,
  height: u32,
  depth: u8,
  color_type: u8,
  data: [u8; 13],
}

/// The frame whose data is being read: an index into `frames`, or None for the IDAT image when
/// it is not part of the animation (or a frame past acTL's count)
#[derive(Clone, Copy, Debug)]
struct Current {
  frame: Option<usize>,
}

/// Read the data so far like the png crate (StreamingDecoder, Reader::read_info and
/// next_frame_info) and list the frames like SkPngRustCodec.
#[cfg(test)]
pub(crate) fn parse(data: &[u8]) -> PngParse {
  parse_with(data, &mut CrcMemo::default())
}

/// `parse`, with the CRC checks made before
pub(crate) fn parse_with(data: &[u8], memo: &mut CrcMemo) -> PngParse {
  // The signature is checked as two 4-byte words.
  if data.len() < 4 {
    return PngParse::Incomplete;
  }
  if data[..4] != SIGNATURE[..4] {
    return PngParse::Bad;
  }
  if data.len() < 8 {
    return PngParse::Incomplete;
  }
  if data[4..8] != SIGNATURE[4..] {
    return PngParse::Bad;
  }

  let mut ihdr: Option<Ihdr> = None;
  let mut palette = false;
  let mut trns: Option<Trns> = None;
  let mut actl: Option<(u32, u32)> = None;
  let mut fctl: Option<Fctl> = None;
  let mut seq: Option<u32> = None;
  let mut have_idat = false;
  let mut ready_for_idat = true;
  let mut ready_for_fdat = false;
  // The type of the data chunks being read (IDAT or fdAT)
  let mut data_type: Option<[u8; 4]> = None;
  let mut kept: Vec<(usize, usize)> = Vec::new();
  let mut iccp = false;
  // Whether a cICP and a bKGD chunk were stored (the png crate checks only the first one)
  let mut cicp = false;
  let mut bkgd = false;
  // Set once read_info ends: the header is ready
  let mut layout: Option<PngLayout> = None;
  let mut current = Current { frame: None };
  // Frames Skia lists at most (getRawFrameCount)
  let mut raw_count = 1usize;

  // `pos` and `body` never pass `data.len()` + 8, so only a chunk's length added to them can
  // pass usize (a 32-bit build): `span` adds it
  let mut pos = 8usize;
  // length, then type
  while let Some(header) = span(data, pos, 8) {
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let kind: [u8; 4] = [header[4], header[5], header[6], header[7]];
    if ihdr.is_none() && &kind != b"IHDR" {
      return end(layout);
    }
    // A chunk of another type ends the image data (ImageDataFlushed)
    if let Some(t) = data_type
      && t != kind
    {
      data_type = None;
      ready_for_idat = false;
      ready_for_fdat = false;
      if let (Some(layout), Some(i)) = (layout.as_mut(), current.frame)
        && layout.frames[i].state == DataState::Incomplete
      {
        layout.frames[i].state = DataState::Complete;
      }
      current.frame = None;
    }
    let body = pos + 8;
    match &kind {
      b"IDAT" | b"fdAT" if data_type == Some(kind) => {
        // one more data chunk of the frame
        if &kind == b"fdAT" {
          if length < 4 {
            return fail_current(layout, current);
          }
          let Some(number) = span(data, body, 4) else {
            break;
          };
          let number = u32::from_be_bytes([number[0], number[1], number[2], number[3]]);
          match seq {
            Some(s) if s.checked_add(1) == Some(number) => seq = Some(number),
            _ => return fail_current(layout, current),
          }
        }
      }
      b"fdAT" => {
        if !ready_for_fdat || length < 4 {
          return fail_current(layout, current);
        }
        data_type = Some(kind);
        // read_until_image_data stops here: the frame of the last fcTL counts
        let Some(layout) = layout.as_mut() else {
          // fdAT can only follow IDAT (ready_for_fdat), so the header is ready
          return PngParse::Bad;
        };
        let Some(fc) = fctl else {
          return end(Some(layout.clone()));
        };
        current.frame = (layout.frames.len() < raw_count).then(|| {
          layout.frames.push(frame_from(fc, layout.frames.len()));
          layout.frames.len() - 1
        });
        let Some(number) = span(data, body, 4) else {
          break;
        };
        let number = u32::from_be_bytes([number[0], number[1], number[2], number[3]]);
        match seq {
          Some(s) if s.checked_add(1) == Some(number) => seq = Some(number),
          // Skia counted the frame; decoding it fails, and nothing after it is read
          Some(_) => return fail_current(Some(layout.clone()), current),
          None => return fail_current(Some(layout.clone()), current),
        }
      }
      b"IDAT" => {
        if !ready_for_idat {
          return fail_current(layout, current);
        }
        have_idat = true;
        data_type = Some(kind);
        // read_info ends at the first IDAT header: Skia makes the codec here
        let Some(info) = ihdr.as_ref() else {
          return PngParse::Bad;
        };
        if layout.is_none() {
          if iccp {
            kept.retain(|&(at, _)| !matches!(&data[at + 4..at + 8], b"gAMA" | b"cHRM" | b"sRGB"));
          }
          let made = make_layout(info, palette, trns, actl, fctl, &kept);
          let Some(made) = made else {
            return PngParse::Bad;
          };
          raw_count = made.1;
          layout = Some(made.0);
          // Frame 0 is the IDAT image, unless acTL came without an fcTL before IDAT
          current.frame = layout
            .as_ref()
            .filter(|layout| !layout.frames.is_empty())
            .map(|_| 0);
        }
      }
      _ => {
        let accepted = match chunk_length_rule(&kind, length) {
          Rule::Error => return end(layout),
          rule => rule,
        };
        // length, type, data, CRC
        let Some(chunk) = span(data, pos, 12 + u64::from(length)) else {
          break;
        };
        let total = chunk.len();
        let critical = kind[0] & 0x20 == 0;
        if !memo.check(pos, &chunk[4..total - 4], &chunk[total - 4..]) {
          if critical {
            return end(layout);
          }
          // an ancillary chunk with a bad CRC is skipped
          pos += total;
          continue;
        }
        if accepted == Rule::Process {
          let payload = &chunk[8..total - 4];
          match &kind {
            b"IHDR" => {
              if ihdr.is_some() {
                return end(layout);
              }
              match read_ihdr(payload) {
                Some(info) => ihdr = Some(info),
                None => return end(layout),
              }
            }
            b"PLTE" => {
              if palette {
                return end(layout);
              }
              palette = true;
              if layout.is_none() {
                kept.push((pos, pos + total));
              }
            }
            b"IEND" => {
              return end(layout);
            }
            b"acTL" => {
              let frames = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
              let plays = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
              // ignored: after IDAT, a second one, 0 frames, more than 2^31 - 1 of either
              if !have_idat
                && actl.is_none()
                && frames != 0
                && frames <= 0x7FFF_FFFF
                && plays <= 0x7FFF_FFFF
              {
                actl = Some((frames, plays));
              }
            }
            b"fcTL" => {
              let number = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
              let expected = match seq {
                Some(s) => s.checked_add(1),
                None => Some(0),
              };
              if expected != Some(number) {
                return end(layout);
              }
              seq = Some(number);
              ready_for_fdat = have_idat;
              let Some(info) = ihdr.as_ref() else {
                return end(layout);
              };
              match read_fctl(payload, info, have_idat) {
                Some(fc) => fctl = Some(fc),
                None => return end(layout),
              }
            }
            b"tRNS" => {
              if let Some(info) = ihdr.as_ref()
                && trns.is_none()
                && let Some(t) = read_trns(payload, info, palette, have_idat)
              {
                trns = Some(t);
                if layout.is_none() {
                  kept.push((pos, pos + total));
                }
              }
            }
            // parse_cicp: the first one before PLTE and IDAT with a full-range flag other than 0
            // or 1, or matrix coefficients other than 0, fails the decoder (an io error, which is
            // not a benign error of an ancillary chunk)
            b"cICP" if !cicp && !palette && !have_idat => {
              if payload[3] > 1 || payload[2] != 0 {
                return end(layout);
              }
              cicp = true;
              kept.push((pos, pos + total));
            }
            b"gAMA" | b"cHRM" | b"sRGB" | b"cICP" if layout.is_none() => {
              kept.push((pos, pos + total));
            }
            // parse_bkgd: the first one before IDAT fails the decoder in an indexed image before
            // PLTE (an io error); one of the wrong size is a benign error, and not stored
            b"bKGD" if !bkgd && !have_idat => {
              let expected = match ihdr.as_ref().map(|info| info.color_type) {
                Some(3) if !palette => return end(layout),
                Some(3) => 1,
                Some(0 | 4) => 2,
                _ => 6,
              };
              bkgd = payload.len() == expected;
            }
            b"iCCP" if layout.is_none() => iccp = true,
            _ => {}
          }
        }
        pos += total;
        continue;
      }
    }
    // data chunk: its data, then its CRC
    let skip = if &kind == b"fdAT" { 4 } else { 0 };
    let start = body + skip;
    // None: the data ends past any `data` there can be
    let stop = end_of(body, u64::from(length));
    let available = stop.map_or(data.len(), |stop| data.len().min(stop));
    if let (Some(layout), Some(i)) = (layout.as_mut(), current.frame)
      && available > start
    {
      layout.frames[i].data.push((start, available));
    }
    let Some((stop, crc)) = stop.and_then(|stop| Some((stop, span(data, stop, 4)?))) else {
      break;
    };
    // IDAT is critical: a bad CRC fails its frame. fdAT is ancillary: its CRC is not checked.
    if &kind == b"IDAT" && !memo.check(pos, &data[pos + 4..stop], crc) {
      return fail_current(layout, current);
    }
    pos = stop + 4;
  }
  match layout {
    Some(layout) => PngParse::Ready(layout),
    None => PngParse::Incomplete,
  }
}

/// `at + len`, or None when it passes usize: then no `data` reaches it
fn end_of(at: usize, len: u64) -> Option<usize> {
  usize::try_from(u64::try_from(at).ok()?.checked_add(len)?).ok()
}

/// The `len` bytes of `data` from `at`, or None when they are not all here
fn span(data: &[u8], at: usize, len: u64) -> Option<&[u8]> {
  data.get(at..end_of(at, len)?)
}

/// The parse stops at an error. Before the header, Chrome fails the image; after it, the frame
/// list ends there.
fn end(layout: Option<PngLayout>) -> PngParse {
  match layout {
    Some(layout) => PngParse::Ready(layout),
    None => PngParse::Bad,
  }
}

/// An error in the data of the frame being read: it fails, and the frame list ends
fn fail_current(mut layout: Option<PngLayout>, current: Current) -> PngParse {
  if let (Some(layout), Some(i)) = (layout.as_mut(), current.frame) {
    layout.frames[i].state = DataState::Failed;
  }
  end(layout)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
  /// read and parse it
  Process,
  /// read it, then skip it (an unknown ancillary chunk, or a known one of a wrong length)
  Skip,
  /// a fatal error
  Error,
}

/// The png crate's StreamingDecoder::start_chunk
fn chunk_length_rule(kind: &[u8; 4], length: u32) -> Rule {
  let range = match kind {
    b"IHDR" => 13..=13,
    b"PLTE" => 3..=768,
    b"IEND" => 0..=0,
    b"sBIT" => 1..=4,
    b"tRNS" => 1..=256,
    b"pHYs" => 9..=9,
    b"gAMA" => 4..=4,
    b"acTL" => 8..=8,
    b"fcTL" => 26..=26,
    b"cHRM" => 32..=32,
    b"sRGB" => 1..=1,
    b"cICP" => 4..=4,
    b"mDCV" => 24..=24,
    b"cLLI" => 8..=8,
    b"bKGD" => 1..=6,
    b"eXIf" | b"iCCP" | b"tEXt" | b"zTXt" | b"iTXt" => 0..=u32::MAX >> 1,
    // an unknown critical chunk is an error, an unknown ancillary one is skipped
    _ if kind[0] & 0x20 == 0 => return Rule::Error,
    _ => return Rule::Skip,
  };
  if range.contains(&length) {
    Rule::Process
  } else if matches!(kind, b"IHDR" | b"PLTE" | b"IEND" | b"fcTL") {
    Rule::Error
  } else {
    Rule::Skip
  }
}

/// The png crate's parse_ihdr
fn read_ihdr(payload: &[u8]) -> Option<Ihdr> {
  let width = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
  let height = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
  let (depth, color_type) = (payload[8], payload[9]);
  if width == 0 || height == 0 {
    return None;
  }
  let depth_ok = match color_type {
    0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
    3 => matches!(depth, 1 | 2 | 4 | 8),
    2 | 4 | 6 => matches!(depth, 8 | 16),
    _ => return None,
  };
  // compression and filter method 0, interlace 0 or 1
  if !depth_ok || payload[10] != 0 || payload[11] != 0 || payload[12] > 1 {
    return None;
  }
  let mut data = [0; 13];
  data.copy_from_slice(&payload[..13]);
  Some(Ihdr {
    width,
    height,
    depth,
    color_type,
    data,
  })
}

/// The png crate's parse_fctl (after the sequence number): None is an error
fn read_fctl(payload: &[u8], info: &Ihdr, have_idat: bool) -> Option<Fctl> {
  let be32 =
    |i: usize| u32::from_be_bytes([payload[i], payload[i + 1], payload[i + 2], payload[i + 3]]);
  let (width, height, x, y) = (be32(4), be32(8), be32(12), be32(16));
  let delay_num = u16::from_be_bytes([payload[20], payload[21]]);
  let delay_den = u16::from_be_bytes([payload[22], payload[23]]);
  let dispose = match payload[24] {
    0 => Dispose::None,
    1 => Dispose::Background,
    2 => Dispose::Previous,
    _ => return None,
  };
  let blend = match payload[25] {
    0 => Blend::Source,
    1 => Blend::Over,
    _ => return None,
  };
  // validate: a frame inside the image
  if width == 0 || height == 0 {
    return None;
  }
  if Some(width) > info.width.checked_sub(x) || Some(height) > info.height.checked_sub(y) {
    return None;
  }
  // validate_default_image: an fcTL before IDAT describes the whole image
  if !have_idat && (x != 0 || y != 0 || width != info.width || height != info.height) {
    return None;
  }
  Some(Fctl {
    rect: Rect {
      x0: x,
      y0: y,
      x1: x + width,
      y1: y + height,
    },
    delay_num,
    delay_den,
    dispose,
    blend,
  })
}

/// The png crate's parse_trns: the chunk it takes, or None when it ignores it (too short for
/// the color type, a palette one before PLTE or after IDAT, a gray-alpha or RGBA image)
fn read_trns(payload: &[u8], info: &Ihdr, palette: bool, have_idat: bool) -> Option<Trns> {
  let ok = match info.color_type {
    0 => payload.len() >= 2,
    2 => payload.len() >= 6,
    3 => palette && !have_idat,
    _ => false,
  };
  if !ok {
    return None;
  }
  // The png crate compares each 1, 2 or 4-bit gray sample with the low byte of the value.
  let pixels = match info.depth {
    1 | 2 | 4 if info.color_type == 0 => {
      let (key, max) = (payload[1], (1u8 << info.depth) - 1);
      PixelHint::GrayKey((key <= max).then(|| key * (255 / max)))
    }
    _ => PixelHint::None,
  };
  Some(Trns { pixels })
}

/// SkPngRustCodec::MakeFromStream and its constructor: the layout and Skia's raw frame count,
/// or None when Chrome fails the image
fn make_layout(
  info: &Ihdr,
  palette: bool,
  trns: Option<Trns>,
  actl: Option<(u32, u32)>,
  fctl: Option<Fctl>,
  kept: &[(usize, usize)],
) -> Option<(PngLayout, usize)> {
  let _ = palette;
  // Skia's and Blink's size limits (CreateEncodedInfo, SkiaImageDecoderBase::SetSize,
  // ImageDecoder::SetSize: 4 bytes a pixel must fit an int32)
  if info.width > MAX_SIDE
    || info.height > MAX_SIDE
    || u64::from(info.width) * u64::from(info.height) * 4 > i32::MAX as u64
  {
    return None;
  }
  // IsValidFctlIfAny: an fcTL before IDAT describes the whole image (read_fctl checks it)
  let alpha = info.color_type & 4 != 0 || trns.is_some();
  let header = ImageHeader {
    alpha,
    pixels: match trns {
      Some(t) if info.color_type & 4 == 0 => t.pixels,
      _ => PixelHint::None,
    },
  };
  let (repetition, raw_count) = match actl {
    None => (Repetition::None, 1),
    Some((frames, plays)) => {
      let repetition = if frames <= 1 {
        Repetition::None
      } else if plays == 0 {
        Repetition::Infinite
      } else if plays == 1 {
        Repetition::Once
      } else {
        Repetition::Count(plays - 1)
      };
      (repetition, frames as usize)
    }
  };
  let mut layout = PngLayout {
    width: info.width,
    height: info.height,
    header,
    repetition,
    frames: Vec::new(),
    ihdr: info.data,
    kept: kept.to_vec(),
  };
  let idat_not_part = actl.is_some() && fctl.is_none();
  if !idat_not_part {
    let fc = match (actl, fctl) {
      (Some(_), Some(fc)) => fc,
      // the whole image, drawn as Source
      _ => Fctl {
        rect: Rect {
          x0: 0,
          y0: 0,
          x1: info.width,
          y1: info.height,
        },
        delay_num: 0,
        delay_den: 0,
        dispose: Dispose::None,
        blend: Blend::Source,
      },
    };
    let mut frame = frame_from(fc, 0);
    if actl.is_none() {
      frame.duration_ms = 0;
    }
    layout.frames.push(frame);
  }
  Some((layout, raw_count))
}

fn frame_from(fc: Fctl, index: usize) -> PngFrame {
  PngFrame {
    rect: fc.rect,
    duration_ms: fc.duration_ms(),
    dispose: fc.dispose,
    // Skia draws frame 0 as Source: the two are the same there
    blend: if index == 0 { Blend::Source } else { fc.blend },
    has_alpha: false,
    required: None,
    data: Vec::new(),
    state: DataState::Incomplete,
  }
}

impl PngLayout {
  /// Fill in each frame's alpha and required frame (Skia's setAlphaAndRequiredFrame, run as
  /// each frame is listed)
  pub(crate) fn resolve(&mut self) {
    let screen = Rect {
      x0: 0,
      y0: 0,
      x1: self.width,
      y1: self.height,
    };
    for i in 0..self.frames.len() {
      let (has_alpha, required) =
        alpha_and_required_frame(&self.frames, i, screen, self.header.alpha);
      self.frames[i].has_alpha = has_alpha;
      self.frames[i].required = required;
    }
  }

  /// Frame `i` as a still PNG of its own: IHDR with the frame's size, the kept chunks, its data
  /// as one IDAT chunk, and IEND. FFmpeg decodes it the way the png crate decodes the frame.
  pub(crate) fn still_png(&self, data: &[u8], i: usize) -> Vec<u8> {
    let frame = &self.frames[i];
    let size: usize = frame.data.iter().map(|(a, b)| b - a).sum();
    let mut out = Vec::with_capacity(8 + 25 + size + 12 + 64);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = self.ihdr;
    ihdr[..4].copy_from_slice(&frame.rect.width().to_be_bytes());
    ihdr[4..8].copy_from_slice(&frame.rect.height().to_be_bytes());
    write_chunk(&mut out, b"IHDR", &[&ihdr]);
    for &(a, b) in &self.kept {
      out.extend_from_slice(&data[a..b]);
    }
    let parts: Vec<&[u8]> = frame.data.iter().map(|&(a, b)| &data[a..b]).collect();
    write_chunk(&mut out, b"IDAT", &parts);
    write_chunk(&mut out, b"IEND", &[]);
    out
  }

  /// The canvas frame `i` starts from (Blink SkiaImageDecoderBase::Decode, SkCodec
  /// handleFrameIndex): clear when it requires no frame; else a copy of `base`, the canvas of
  /// `base_index` (the required frame, or a later decoded frame that is not restore-previous),
  /// with the required frame's rect cleared when that frame restores the background and is the
  /// base.
  pub(crate) fn start(&self, i: usize, base: Option<(usize, &[u8])>) -> Vec<u8> {
    let size = self.width as usize * self.height as usize * 4;
    let frame = &self.frames[i];
    let (Some(required), Some((base_index, base))) = (frame.required, base) else {
      return vec![0; size];
    };
    let mut canvas = base.to_vec();
    let prior = &self.frames[required];
    if base_index == required && prior.dispose == Dispose::Background {
      let stride = self.width as usize * 4;
      let r = prior.rect;
      for y in r.y0..r.y1 {
        let row = y as usize * stride;
        canvas[row + r.x0 as usize * 4..row + r.x1 as usize * 4].fill(0);
      }
    }
    canvas
  }

  /// Draw frame `i` on `canvas`. `pixels`: the frame's own BGRA rows, `stride` bytes apart.
  pub(crate) fn draw(&self, i: usize, canvas: &mut [u8], pixels: &[u8], stride: usize) {
    let frame = &self.frames[i];
    let r = frame.rect;
    let canvas_stride = self.width as usize * 4;
    let row_bytes = r.width() as usize * 4;
    for y in 0..r.height() as usize {
      let src = &pixels[y * stride..y * stride + row_bytes];
      let at = (r.y0 as usize + y) * canvas_stride + r.x0 as usize * 4;
      let dst = &mut canvas[at..at + row_bytes];
      // Blending needs an unpremultiplied destination: a frame Chrome decodes as opaque
      // (has_alpha false) blends with no premul steps, which leaves an opaque source as it is.
      if frame.blend == Blend::Source || !frame.has_alpha {
        dst.copy_from_slice(src);
      } else {
        for (d, s) in dst
          .as_chunks_mut::<4>()
          .0
          .iter_mut()
          .zip(src.as_chunks::<4>().0)
        {
          *d = blend_over(*d, *s);
        }
      }
    }
  }
}

/// Skia's blendRow for one unpremultiplied pixel (SkRasterPipeline highp: load_8888,
/// premul_dst, load_8888, premul, srcover, unpremul, store_8888), with fused multiply-adds as
/// on arm64 and AVX2. The channel order does not matter.
pub(crate) fn blend_over(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
  const ONE_255: f32 = 1.0 / 255.0;
  let f = |v: u8| f32::from(v) * ONE_255;
  let (sa, da) = (f(src[3]), f(dst[3]));
  let inv = 1.0 - sa;
  let a = da.mul_add(inv, sa);
  let scale = 1.0 / a;
  let scale = if scale < f32::INFINITY { scale } else { 0.0 };
  let unorm = |v: f32| v.mul_add(255.0, 0.0).clamp(0.0, 255.0).round_ties_even() as u8;
  let mut out = [0; 4];
  for c in 0..3 {
    let s = f(src[c]) * sa;
    let d = f(dst[c]) * da;
    out[c] = unorm(d.mul_add(inv, s) * scale);
  }
  out[3] = unorm(a);
  out
}

/// Skia's SkFrameHolder::setAlphaAndRequiredFrame (src/codec/SkCodec.cpp) for frame `i`:
/// whether it has alpha, and the frame it is drawn on. The branch names are Skia's. Every
/// PNG frame reports the image's alpha (SkPngRustCodec's PngFrame).
fn alpha_and_required_frame(
  frames: &[PngFrame],
  i: usize,
  screen: Rect,
  reports_alpha: bool,
) -> (bool, Option<usize>) {
  let frame = &frames[i];
  let rect = frame.rect;
  // IND1: the first frame
  if i == 0 {
    return (reports_alpha || rect != screen, None);
  }
  let blend_with_prev = frame.blend == Blend::Over;
  // IND2
  if (!reports_alpha || !blend_with_prev) && rect == screen {
    return (reports_alpha, None);
  }
  let mut prev = i - 1;
  while frames[prev].dispose == Dispose::Previous {
    // IND3
    if prev == 0 {
      return (true, None);
    }
    prev -= 1;
  }
  let clear_prev = frames[prev].dispose == Dispose::Background;
  let mut prev_rect = frames[prev].rect;
  // IND4
  if clear_prev && (prev_rect == screen || frames[prev].required.is_none()) {
    return (true, None);
  }
  // DEP5
  if reports_alpha && blend_with_prev {
    return (frames[prev].has_alpha || clear_prev, Some(prev));
  }
  while rect.contains(prev_rect) {
    // IND6
    let Some(required) = frames[prev].required else {
      return (true, None);
    };
    prev = required;
    prev_rect = frames[prev].rect;
  }
  // DEP7
  if frames[prev].dispose == Dispose::Background {
    return (true, Some(prev));
  }
  (
    frames[prev].has_alpha || (reports_alpha && !blend_with_prev),
    Some(prev),
  )
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], parts: &[&[u8]]) {
  let length: usize = parts.iter().map(|p| p.len()).sum();
  out.extend_from_slice(&(length as u32).to_be_bytes());
  let start = out.len();
  out.extend_from_slice(kind);
  for part in parts {
    out.extend_from_slice(part);
  }
  let crc = crc32(&out[start..]);
  out.extend_from_slice(&crc.to_be_bytes());
}

const CRC_TABLE: [u32; 256] = {
  let mut table = [0u32; 256];
  let mut n = 0;
  while n < 256 {
    let mut c = n as u32;
    let mut k = 0;
    while k < 8 {
      c = if c & 1 != 0 {
        0xedb8_8320 ^ (c >> 1)
      } else {
        c >> 1
      };
      k += 1;
    }
    table[n] = c;
    n += 1;
  }
  table
};

/// The CRC-32 of PNG chunks
pub(crate) fn crc32(bytes: &[u8]) -> u32 {
  let mut c = 0xffff_ffffu32;
  for &b in bytes {
    c = CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
  }
  c ^ 0xffff_ffff
}

#[cfg(test)]
mod tests {
  use super::*;

  pub(crate) fn chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    write_chunk(&mut out, kind, &[payload]);
    out
  }

  fn ihdr(w: u32, h: u32, depth: u8, color_type: u8) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&w.to_be_bytes());
    p.extend_from_slice(&h.to_be_bytes());
    p.extend_from_slice(&[depth, color_type, 0, 0, 0]);
    chunk(b"IHDR", &p)
  }

  fn fctl(seq: u32, rect: [u32; 4], delay: (u16, u16), dispose: u8, blend: u8) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&seq.to_be_bytes());
    for v in [rect[2], rect[3], rect[0], rect[1]] {
      p.extend_from_slice(&v.to_be_bytes());
    }
    p.extend_from_slice(&delay.0.to_be_bytes());
    p.extend_from_slice(&delay.1.to_be_bytes());
    p.extend_from_slice(&[dispose, blend]);
    chunk(b"fcTL", &p)
  }

  fn fdat(seq: u32, data: &[u8]) -> Vec<u8> {
    let mut p = seq.to_be_bytes().to_vec();
    p.extend_from_slice(data);
    chunk(b"fdAT", &p)
  }

  fn actl(frames: u32, plays: u32) -> Vec<u8> {
    let mut p = frames.to_be_bytes().to_vec();
    p.extend_from_slice(&plays.to_be_bytes());
    chunk(b"acTL", &p)
  }

  fn file(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut b = SIGNATURE.to_vec();
    chunks.iter().for_each(|c| b.extend_from_slice(c));
    b
  }

  fn ready(data: &[u8]) -> PngLayout {
    match parse(data) {
      PngParse::Ready(mut layout) => {
        layout.resolve();
        layout
      }
      other => panic!("not ready: {other:?}"),
    }
  }

  /// 3 frames: the IDAT image, then 2 fdAT frames
  fn three_frames(plays: u32) -> Vec<u8> {
    file(&[
      ihdr(4, 4, 8, 6),
      actl(3, plays),
      fctl(0, [0, 0, 4, 4], (1, 10), 0, 0),
      chunk(b"IDAT", b"abc"),
      fctl(1, [1, 1, 2, 2], (3, 100), 1, 1),
      fdat(2, b"de"),
      fdat(3, b"f"),
      fctl(4, [0, 0, 4, 4], (0, 0), 0, 1),
      fdat(5, b"gh"),
      chunk(b"IEND", b""),
    ])
  }

  #[test]
  fn the_header_is_ready_at_the_first_idat_header() {
    let data = three_frames(0);
    let idat = data.windows(4).position(|w| w == b"IDAT").unwrap() - 4;
    for cut in 0..idat + 8 {
      assert!(
        matches!(parse(&data[..cut]), PngParse::Incomplete),
        "cut {cut}"
      );
    }
    let layout = ready(&data[..idat + 8]);
    assert_eq!((layout.width, layout.height), (4, 4));
    assert_eq!(layout.repetition, Repetition::Infinite);
    assert_eq!(layout.frames.len(), 1);
    assert_eq!(layout.frames[0].state, DataState::Incomplete);
  }

  #[test]
  fn frames_count_at_their_first_fdat_header_and_end_at_the_next_chunk_header() {
    let data = three_frames(3);
    let starts: Vec<usize> = data
      .windows(4)
      .enumerate()
      .filter(|(_, w)| *w == b"fdAT" || *w == b"fcTL" || *w == b"IEND")
      .map(|(i, _)| i - 4)
      .collect();
    // fcTL(1) at starts[1]: frame 0 ends at its header
    let at = |cut: usize| ready(&data[..cut]);
    assert_eq!(at(starts[1] + 7).frames[0].state, DataState::Incomplete);
    assert_eq!(at(starts[1] + 8).frames[0].state, DataState::Complete);
    // frame 1 counts at the header of its first fdAT (starts[2])
    assert_eq!(at(starts[2] + 7).frames.len(), 1);
    assert_eq!(at(starts[2] + 8).frames.len(), 2);
    // its second fdAT (starts[3]) does not end it; fcTL(4) does
    assert_eq!(at(starts[3] + 8).frames[1].state, DataState::Incomplete);
    assert_eq!(at(starts[4] + 8).frames[1].state, DataState::Complete);
    let layout = at(data.len());
    assert_eq!(layout.frames.len(), 3);
    assert_eq!(layout.repetition, Repetition::Count(2));
    assert!(layout.frames.iter().all(|f| f.state == DataState::Complete));
    assert_eq!(
      layout
        .frames
        .iter()
        .map(|f| f.duration_ms)
        .collect::<Vec<_>>(),
      [100, 30, 0]
    );
    assert_eq!(layout.frames[1].data.len(), 2);
    // the last frame ends at the IEND header
    assert_eq!(at(starts[6] + 7).frames[2].state, DataState::Incomplete);
    assert_eq!(at(starts[6] + 8).frames[2].state, DataState::Complete);
  }

  #[test]
  fn actl_limits_the_frame_count() {
    let mut data = three_frames(0);
    // acTL says 2 frames
    let at = data.windows(4).position(|w| w == b"acTL").unwrap() - 4;
    let fixed = actl(2, 0);
    data.splice(at..at + fixed.len(), fixed);
    assert_eq!(ready(&data).frames.len(), 2);
  }

  #[test]
  fn idat_outside_the_animation_is_not_a_frame() {
    let data = file(&[
      ihdr(4, 4, 8, 2),
      actl(2, 0),
      chunk(b"IDAT", b"abc"),
      fctl(0, [1, 1, 2, 2], (1, 10), 0, 0),
      fdat(1, b"de"),
      fctl(2, [0, 0, 4, 4], (1, 10), 0, 0),
      fdat(3, b"gh"),
      chunk(b"IEND", b""),
    ]);
    let idat = data.windows(4).position(|w| w == b"IDAT").unwrap() - 4;
    let layout = ready(&data[..idat + 8]);
    assert_eq!(layout.frames.len(), 0);
    assert_eq!(layout.repetition, Repetition::Infinite);
    let layout = ready(&data);
    assert_eq!(layout.frames.len(), 2);
    assert_eq!(
      layout.frames[0].rect,
      Rect {
        x0: 1,
        y0: 1,
        x1: 3,
        y1: 3
      }
    );
    // frame 0 is drawn as Source, whatever its fcTL says
    assert_eq!(layout.frames[0].blend, Blend::Source);
  }

  #[test]
  fn errors_before_the_header_fail_the_image() {
    let good = three_frames(0);
    assert!(matches!(parse(b"\x89PNX"), PngParse::Bad));
    assert!(matches!(parse(b"\x89PNG\r\n\x1a\x0b"), PngParse::Bad));
    // a bad IHDR CRC
    let mut bad = good.clone();
    bad[29] ^= 1;
    assert!(matches!(parse(&bad[..33]), PngParse::Bad));
    // a chunk before IHDR
    assert!(matches!(
      parse(&file(&[chunk(b"tEXt", b"a\0b"), ihdr(1, 1, 8, 0)])),
      PngParse::Bad
    ));
    // an unknown critical chunk
    assert!(matches!(
      parse(&file(&[
        ihdr(1, 1, 8, 0),
        chunk(b"ABCD", b""),
        chunk(b"IDAT", b"")
      ])),
      PngParse::Bad
    ));
    // an fcTL before IDAT that is not the whole image
    assert!(matches!(
      parse(&file(&[
        ihdr(4, 4, 8, 0),
        actl(2, 0),
        fctl(0, [0, 0, 3, 4], (1, 1), 0, 0),
        chunk(b"IDAT", b"")
      ])),
      PngParse::Bad
    ));
    // IEND before any IDAT
    assert!(matches!(
      parse(&file(&[ihdr(1, 1, 8, 0), chunk(b"IEND", b"")])),
      PngParse::Bad
    ));
    // too large for Blink
    assert!(matches!(
      parse(&file(&[ihdr(1_000_001, 1, 8, 0), chunk(b"IDAT", b"")])),
      PngParse::Bad
    ));
    // an unknown ancillary chunk, a pHYs with a bad CRC and a tRNS of 257 bytes are skipped
    let mut phys = chunk(b"pHYs", &[0; 9]);
    phys[20] ^= 1;
    let layout = ready(&file(&[
      ihdr(1, 1, 8, 3),
      chunk(b"PLTE", &[0, 0, 0]),
      chunk(b"abCD", b"x"),
      phys,
      chunk(b"tRNS", &[0; 257]),
      chunk(b"IDAT", b""),
    ]));
    assert!(!layout.header.alpha);
  }

  #[test]
  fn a_bad_cicp_or_an_indexed_bkgd_before_plte_fails_the_image_like_the_png_crate() {
    let rgb = |chunks: &[Vec<u8>]| {
      let mut all = vec![ihdr(1, 1, 8, 2)];
      all.extend_from_slice(chunks);
      all.push(chunk(b"IDAT", b""));
      parse(&file(&all))
    };
    let cicp = |p: [u8; 4]| chunk(b"cICP", &p);
    // a full-range flag over 1, or matrix coefficients other than 0: an io error in the png crate
    assert!(matches!(rgb(&[cicp([1, 13, 0, 2])]), PngParse::Bad));
    assert!(matches!(rgb(&[cicp([1, 13, 1, 1])]), PngParse::Bad));
    // benign: good values, another size (rejected unread), a second one, or one after IDAT
    for chunks in [
      vec![cicp([1, 13, 0, 1])],
      vec![cicp([9, 16, 0, 0])],
      vec![chunk(b"cICP", &[1, 13, 1, 2, 0])],
      vec![cicp([1, 13, 0, 1]), cicp([1, 13, 1, 2])],
    ] {
      assert!(matches!(rgb(&chunks), PngParse::Ready(_)));
    }
    let after_idat = file(&[
      ihdr(1, 1, 8, 2),
      chunk(b"IDAT", b""),
      cicp([1, 13, 1, 2]),
      chunk(b"IEND", b""),
    ]);
    assert!(matches!(parse(&after_idat), PngParse::Ready(_)));
    // an indexed image: bKGD before PLTE fails, also of the wrong size; after PLTE it is fine,
    // and so is a bad cICP after PLTE (benign there)
    let indexed = |chunks: &[Vec<u8>]| {
      let mut all = vec![ihdr(1, 1, 8, 3)];
      all.extend_from_slice(chunks);
      all.push(chunk(b"IDAT", b""));
      parse(&file(&all))
    };
    let plte = chunk(b"PLTE", &[0, 0, 0]);
    assert!(matches!(
      indexed(&[chunk(b"bKGD", &[0]), plte.clone()]),
      PngParse::Bad
    ));
    assert!(matches!(
      indexed(&[chunk(b"bKGD", &[0, 0]), plte.clone()]),
      PngParse::Bad
    ));
    assert!(matches!(
      indexed(&[cicp([1, 13, 1, 2]), plte.clone()]),
      PngParse::Bad
    ));
    for chunks in [
      vec![plte.clone(), chunk(b"bKGD", &[0])],
      vec![plte.clone(), cicp([1, 13, 1, 2])],
    ] {
      assert!(matches!(indexed(&chunks), PngParse::Ready(_)));
    }
    // a bKGD of the wrong size in an RGB image is benign
    assert!(matches!(
      rgb(&[chunk(b"bKGD", &[0, 1])]),
      PngParse::Ready(_)
    ));
  }

  #[test]
  fn errors_after_the_header_end_the_frame_list() {
    // a bad fcTL CRC: the fcTL is skipped, and its fdAT is out of order
    let mut data = three_frames(0);
    let second = data
      .windows(4)
      .enumerate()
      .filter(|(_, w)| *w == b"fcTL")
      .nth(2)
      .unwrap()
      .0;
    data[second + 10] ^= 1;
    let layout = ready(&data);
    assert_eq!(layout.frames.len(), 2);
    assert!(layout.frames.iter().all(|f| f.state == DataState::Complete));

    // a bad fdAT CRC changes nothing
    let mut data = three_frames(0);
    let fdat_at = data.windows(4).position(|w| w == b"fdAT").unwrap();
    data[fdat_at + 9] ^= 1;
    let layout = ready(&data);
    assert_eq!(layout.frames.len(), 3);
    assert!(layout.frames.iter().all(|f| f.state == DataState::Complete));

    // a bad IDAT CRC fails frame 0 once the CRC is here, and nothing after it is read
    let mut data = three_frames(0);
    let idat = data.windows(4).position(|w| w == b"IDAT").unwrap();
    data[idat + 4] ^= 1;
    assert_eq!(
      ready(&data[..idat + 10]).frames[0].state,
      DataState::Incomplete
    );
    let layout = ready(&data[..idat + 11]);
    assert_eq!(layout.frames.len(), 1);
    assert_eq!(layout.frames[0].state, DataState::Failed);

    // an fdAT out of order in a frame fails that frame
    let data = file(&[
      ihdr(4, 4, 8, 6),
      actl(3, 0),
      fctl(0, [0, 0, 4, 4], (1, 10), 0, 0),
      chunk(b"IDAT", b"abc"),
      fctl(1, [1, 1, 2, 2], (3, 100), 1, 1),
      fdat(2, b"de"),
      fdat(4, b"f"),
      chunk(b"IEND", b""),
    ]);
    let layout = ready(&data);
    assert_eq!(layout.frames.len(), 2);
    assert_eq!(layout.frames[0].state, DataState::Complete);
    assert_eq!(layout.frames[1].state, DataState::Failed);
  }

  /// A chunk length near 2^32 added to an offset passes usize in a 32-bit build. Such a chunk
  /// waits for data that never comes, as in a 64-bit build; it must not wrap and panic.
  #[test]
  fn a_chunk_length_past_usize_waits_for_its_data() {
    // 41 bytes: the header, then an ancillary chunk of length 0xfffffff8
    let mut skipped = file(&[ihdr(1, 1, 8, 6)]);
    skipped.extend_from_slice(&0xffff_fff8u32.to_be_bytes());
    skipped.extend_from_slice(b"tEXt");
    assert_eq!(skipped.len(), 41);
    assert!(matches!(parse(&skipped), PngParse::Incomplete));
    // an IDAT chunk of that length: its data so far is the frame's
    let mut image = file(&[ihdr(1, 1, 8, 6)]);
    image.extend_from_slice(&0xffff_fff8u32.to_be_bytes());
    image.extend_from_slice(b"IDATabc");
    let layout = ready(&image);
    assert_eq!(layout.frames[0].data, [(41, 44)]);
    assert_eq!(layout.frames[0].state, DataState::Incomplete);
    // what a 32-bit build adds: past usize there, so None; the same sums near usize::MAX here
    let past = |at: u64, len: u64| at + len > u64::from(u32::MAX);
    for (at, len) in [(33u64, 12 + 0xffff_fff8u64), (41, 0xffff_fff8)] {
      assert!(past(at, len));
      assert_eq!(end_of(at as usize, len), usize::try_from(at + len).ok());
      let near = usize::MAX - (u64::from(u32::MAX) - at) as usize;
      assert_eq!(end_of(near, len), None);
      assert!(span(&image, near, len).is_none());
    }
    assert_eq!(end_of(usize::MAX - 4, 4), Some(usize::MAX));
    assert_eq!(end_of(usize::MAX - 4, 5), None);
  }

  #[test]
  fn reads_png_alpha_like_the_png_crate() {
    let plte = chunk(b"PLTE", &[0, 0, 0]);
    let trns = |len: usize| chunk(b"tRNS", &vec![255; len]);
    let text = chunk(b"tEXt", b"a\0b");
    let alpha = |color_type: u8, depth: u8, chunks: &[Vec<u8>]| {
      let mut all = vec![ihdr(1, 1, depth, color_type)];
      all.extend_from_slice(chunks);
      all.push(chunk(b"IDAT", b""));
      ready(&file(&all)).header.alpha
    };
    assert!(!alpha(3, 8, std::slice::from_ref(&plte)));
    // tRNS declares alpha even when every entry is 255
    assert!(alpha(3, 8, &[plte.clone(), text.clone(), trns(1)]));
    assert!(alpha(0, 8, &[trns(2)]));
    assert!(alpha(2, 8, &[trns(6)]));
    assert!(!alpha(2, 8, std::slice::from_ref(&text)));
    assert!(alpha(4, 8, &[]));
    assert!(alpha(6, 8, &[]));
    // ignored: too short for gray or RGB, longer than 256, palette before PLTE
    assert!(!alpha(0, 8, &[trns(1)]));
    assert!(!alpha(2, 8, &[trns(5)]));
    assert!(!alpha(3, 8, &[plte.clone(), trns(257)]));
    assert!(!alpha(3, 8, &[trns(1), plte.clone()]));
    // a later valid tRNS still counts
    assert!(alpha(0, 8, &[trns(1), trns(2)]));
    // a tRNS with a bad CRC is skipped
    let mut bad = trns(2);
    bad[10] ^= 1;
    assert!(!alpha(0, 8, &[bad]));
  }

  #[test]
  fn reads_the_gray_key_of_low_bit_pngs() {
    let key = |depth: u8, value: [u8; 2]| {
      ready(&file(&[
        ihdr(1, 1, depth, 0),
        chunk(b"tRNS", &value),
        chunk(b"IDAT", b""),
      ]))
      .header
      .pixels
    };
    assert_eq!(key(1, [0, 1]), PixelHint::GrayKey(Some(255)));
    assert_eq!(key(2, [0, 2]), PixelHint::GrayKey(Some(170)));
    assert_eq!(key(4, [0, 15]), PixelHint::GrayKey(Some(255)));
    // only the low byte counts; a value no sample can have makes nothing transparent
    assert_eq!(key(2, [1, 1]), PixelHint::GrayKey(Some(85)));
    assert_eq!(key(2, [0, 4]), PixelHint::GrayKey(None));
    // FFmpeg keeps 8-bit keys
    assert_eq!(key(8, [0, 4]), PixelHint::None);
  }

  #[test]
  fn a_frames_still_png_has_its_size_its_data_and_the_kept_chunks() {
    let data = three_frames(0);
    let layout = ready(&data);
    let still = layout.still_png(&data, 1);
    let expected = file(&[
      ihdr(2, 2, 8, 6),
      chunk(b"IDAT", b"def"),
      chunk(b"IEND", b""),
    ]);
    assert_eq!(still, expected);
  }

  #[test]
  fn blend_over_follows_skias_float_pipeline() {
    // an opaque source, and a clear one over a clear destination
    assert_eq!(
      blend_over([1, 2, 3, 4], [10, 20, 30, 255]),
      [10, 20, 30, 255]
    );
    assert_eq!(blend_over([9, 9, 9, 0], [5, 6, 7, 0]), [0, 0, 0, 0]);
    // a clear source keeps the destination
    assert_eq!(
      blend_over([40, 50, 60, 128], [5, 6, 7, 0]),
      [40, 50, 60, 128]
    );
    // half over opaque: (255 * 0.502 + 0 * 0.498) rounds to 128
    assert_eq!(
      blend_over([0, 0, 0, 255], [255, 255, 255, 128]),
      [128, 128, 128, 255]
    );
  }

  fn frame(rect: [u32; 4], dispose: Dispose, blend: Blend) -> PngFrame {
    PngFrame {
      rect: Rect {
        x0: rect[0],
        y0: rect[1],
        x1: rect[0] + rect[2],
        y1: rect[1] + rect[3],
      },
      duration_ms: 0,
      dispose,
      blend,
      has_alpha: false,
      required: None,
      data: Vec::new(),
      state: DataState::Complete,
    }
  }

  fn resolve(mut frames: Vec<PngFrame>, alpha: bool) -> Vec<(bool, Option<usize>)> {
    let screen = Rect {
      x0: 0,
      y0: 0,
      x1: 10,
      y1: 10,
    };
    for i in 0..frames.len() {
      let (a, r) = alpha_and_required_frame(&frames, i, screen, alpha);
      frames[i].has_alpha = a;
      frames[i].required = r;
    }
    frames.iter().map(|f| (f.has_alpha, f.required)).collect()
  }

  #[test]
  fn frame_alpha_follows_each_branch_of_skias_rule() {
    use Blend::*;
    use Dispose::*;
    let full = [0, 0, 10, 10];
    let part = [2, 2, 4, 4];
    // IND1, then IND2 (opaque or Source over the whole screen)
    assert_eq!(
      resolve(
        vec![frame(part, None, Source), frame(full, None, Over)],
        false
      ),
      [(true, Option::None), (false, Option::None)]
    );
    assert_eq!(
      resolve(
        vec![frame(full, None, Source), frame(full, None, Source)],
        true
      ),
      [(true, Option::None), (true, Option::None)]
    );
    // IND3: restore-previous back to frame 0
    assert_eq!(
      resolve(
        vec![frame(full, Previous, Source), frame(part, None, Over)],
        false
      )[1],
      (true, Option::None)
    );
    // IND4: the frame before clears the whole screen
    assert_eq!(
      resolve(
        vec![frame(full, Background, Source), frame(part, None, Over)],
        false
      )[1],
      (true, Option::None)
    );
    // DEP5: alpha and Over
    assert_eq!(
      resolve(
        vec![frame(full, None, Source), frame(part, None, Over)],
        true
      )[1],
      (true, Some(0))
    );
    // DEP7: opaque, on the frame before
    assert_eq!(
      resolve(
        vec![frame(full, None, Source), frame(part, None, Over)],
        false
      )[1],
      (false, Some(0))
    );
    // DEP7 with Source and alpha: the frame's own alpha counts
    assert_eq!(
      resolve(
        vec![frame(full, None, Source), frame(part, None, Source)],
        true
      )[1],
      (true, Some(0))
    );
    // IND6: covering an independent frame
    assert_eq!(
      resolve(
        vec![
          frame(full, None, Source),
          frame([0, 0, 10, 10], Background, Source),
          frame(part, None, Source),
          frame([1, 1, 6, 6], None, Source),
        ],
        false
      )[3],
      (true, Option::None)
    );
  }
}
