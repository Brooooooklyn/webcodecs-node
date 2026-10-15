//! GIF decoder with Chrome 155's frames.
//!
//! Chrome decodes GIF with Wuffs v0.3 (skia src/codec/SkWuffsCodec.cpp) and draws
//! each frame on the frame Skia says it needs (SkFrameHolder::setAlphaAndRequiredFrame,
//! Blink SkiaImageDecoderBase::Decode). This file follows the same rules:
//!
//! - the canvas is the logical screen, grown to the first frame's rect
//! - a frame starts from a transparent canvas, or from the frame it requires;
//!   restore-background clears that frame's rect to transparent black
//! - transparent pixels keep the pixel below; an index outside the palette is opaque
//!   black, and so is every pixel of a GIF without a palette
//! - frame rects are clipped to the canvas
//! - errors before the first frame fail the image; later header errors end the frame
//!   list; bad or short pixel data fails that frame and the frames drawn on it
//!
//! Frames are unpremultiplied BGRA. `has_alpha` says whether Chrome labels the frame
//! BGRA (true) or BGRX (false).

use std::fmt;

/// A malformed or unsupported GIF, or a frame Chrome does not decode
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GifError(&'static str);

impl fmt::Display for GifError {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "GIF: {}", self.0)
  }
}

impl std::error::Error for GifError {}

/// The data ends early. While more data can come, a stream is not failed by it.
pub const TRUNCATED: GifError = GifError("truncated data");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Disposal {
  Keep,
  RestoreBackground,
  RestorePrevious,
}

/// Rect with exclusive right and bottom edges
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

  /// The part on the screen, or an empty rect (Skia's frame_rect_on_screen)
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

  /// SkIRect::contains: false when either rect is empty
  fn contains(self, r: Rect) -> bool {
    !r.is_empty()
      && !self.is_empty()
      && self.x0 <= r.x0
      && self.y0 <= r.y0
      && self.x1 >= r.x1
      && self.y1 >= r.y1
  }
}

/// One frame, as far as the frame list needs it
#[derive(Clone, Debug)]
pub struct GifFrameInfo {
  /// Chrome labels the frame BGRA when true, BGRX when false
  pub has_alpha: bool,
  /// Graphic Control delay, in hundredths of a second
  pub delay: u16,
  /// The frame this one is drawn on (Skia's fRequiredFrame), None for a clear canvas
  pub required: Option<usize>,
  /// The frame rect as stored (right and bottom edges are exclusive)
  rect: Rect,
  transparent: Option<u8>,
  disposal: Disposal,
  /// Index of the Image Descriptor's flags byte
  pos: usize,
}

/// A GIF's canvas and frame list, read the way Wuffs reads them
pub struct GifDecoder<'a> {
  data: &'a [u8],
  width: u32,
  height: u32,
  /// Global Color Table as BGRA, padded with opaque black
  palette: [[u8; 4]; 256],
  frames: Vec<GifFrameInfo>,
  /// The loop count of the last NETSCAPE2.0 or ANIMEXTS1.0 extension read
  loops: Option<u16>,
}

struct Reader<'a> {
  data: &'a [u8],
  pos: usize,
}

impl<'a> Reader<'a> {
  fn take(&mut self, n: usize) -> Result<&'a [u8], GifError> {
    let bytes = self
      .data
      .get(self.pos..self.pos.saturating_add(n))
      .ok_or(TRUNCATED)?;
    self.pos += n;
    Ok(bytes)
  }

  fn u8(&mut self) -> Result<u8, GifError> {
    Ok(self.take(1)?[0])
  }

  fn u16(&mut self) -> Result<u32, GifError> {
    let b = self.take(2)?;
    Ok(u32::from(u16::from_le_bytes([b[0], b[1]])))
  }

  /// Skip data sub-blocks up to and including the block terminator
  fn skip_blocks(&mut self) -> Result<(), GifError> {
    loop {
      match self.u8()? {
        0 => return Ok(()),
        n => {
          self.take(usize::from(n))?;
        }
      }
    }
  }

  /// Image Descriptor flags and Local Color Table. Returns the flags and the table.
  fn flags_and_palette(&mut self) -> Result<(u8, Option<&'a [u8]>), GifError> {
    let flags = self.u8()?;
    let table = (flags & 0x80 != 0)
      .then(|| self.take(3 << (1 + (flags & 7))))
      .transpose()?;
    Ok((flags, table))
  }

  /// LZW literal width. Wuffs accepts 0 to 8.
  fn literal_width(&mut self) -> Result<u32, GifError> {
    match self.u8()? {
      lw @ 0..=8 => Ok(u32::from(lw)),
      _ => Err(GifError("bad literal width")),
    }
  }
}

/// Graphic Control values for the next frame
#[derive(Clone, Copy)]
struct Control {
  transparent: Option<u8>,
  disposal: Disposal,
  delay: u16,
}

impl Default for Control {
  fn default() -> Self {
    Control {
      transparent: None,
      disposal: Disposal::Keep,
      delay: 0,
    }
  }
}

/// Application extension (Wuffs decode_ae): reads the loop count of a NETSCAPE2.0 or
/// ANIMEXTS1.0 block into `loops`. The sub-block after their 11-byte id is read on its own,
/// so one of size 0 does not end the extension.
fn read_application(r: &mut Reader, loops: &mut Option<u16>) -> Result<(), GifError> {
  match r.u8()? {
    0 => return Ok(()),
    11 => {
      if matches!(r.take(11)?, b"NETSCAPE2.0" | b"ANIMEXTS1.0") {
        match r.u8()? {
          3 => {
            let id = r.u8()?;
            let count = r.u16()? as u16;
            if id == 1 {
              *loops = Some(count);
            }
          }
          n => {
            r.take(usize::from(n))?;
          }
        }
      }
    }
    n => {
      r.take(usize::from(n))?;
    }
  }
  r.skip_blocks()
}

/// Reads blocks up to the next Image Descriptor rect. None at the trailer, or at any
/// other block type (Wuffs treats it as the trailer).
fn next_frame_rect(
  r: &mut Reader,
  control: &mut Control,
  loops: &mut Option<u16>,
) -> Result<Option<Rect>, GifError> {
  loop {
    match r.u8()? {
      0x21 => match r.u8()? {
        0xF9 => {
          if r.u8()? != 4 {
            return Err(GifError("bad graphic control"));
          }
          let flags = r.u8()?;
          let delay = r.u16()? as u16;
          let index = r.u8()?;
          if r.u8()? != 0 {
            return Err(GifError("bad graphic control"));
          }
          *control = Control {
            transparent: (flags & 1 != 0).then_some(index),
            disposal: match (flags >> 2) & 7 {
              2 => Disposal::RestoreBackground,
              3 | 4 => Disposal::RestorePrevious,
              _ => Disposal::Keep,
            },
            delay,
          };
        }
        0xFF => read_application(r, loops)?,
        _ => r.skip_blocks()?,
      },
      0x2C => {
        let (x0, y0, w, h) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
        return Ok(Some(Rect {
          x0,
          y0,
          x1: x0 + w,
          y1: y0 + h,
        }));
      }
      _ => return Ok(None),
    }
  }
}

impl<'a> GifDecoder<'a> {
  /// Reads the header and the frame list. An error here is an image Chrome fails to
  /// open (ImageDecoder's `tracks.ready` rejects).
  pub fn new(data: &'a [u8]) -> Result<Self, GifError> {
    let mut r = Reader { data, pos: 0 };
    let magic = r.take(6)?;
    if &magic[..4] != b"GIF8" || !matches!(magic[4], b'7' | b'9') || magic[5] != b'a' {
      return Err(GifError("bad header"));
    }
    let (mut width, mut height) = (r.u16()?, r.u16()?);
    let flags = r.u8()?;
    r.take(2)?; // background color index, pixel aspect ratio
    let mut palette = [[0, 0, 0, 255]; 256];
    if flags & 0x80 != 0 {
      fill_palette(&mut palette, r.take(3 << (1 + (flags & 7)))?);
    }

    let mut frames: Vec<GifFrameInfo> = Vec::new();
    let mut control = Control::default();
    let mut loops = None;
    loop {
      // Before the first frame rect, an error fails the image (Wuffs
      // decode_image_config). After it, the frame list ends at the error.
      let rect = match next_frame_rect(&mut r, &mut control, &mut loops) {
        Ok(Some(rect)) => rect,
        Ok(None) => break,
        Err(e) if frames.is_empty() => return Err(e),
        Err(_) => break,
      };
      if frames.is_empty() {
        width = width.max(rect.x1);
        height = height.max(rect.y1);
      }
      frames.push(GifFrameInfo {
        has_alpha: false,
        delay: control.delay,
        required: None,
        rect,
        transparent: control.transparent,
        disposal: control.disposal,
        pos: r.pos,
      });
      control = Control::default();
      // Wuffs skip_frame: the next frame is only listed when this one's data is whole
      let skipped = r
        .flags_and_palette()
        .and_then(|_| r.literal_width())
        .and_then(|_| r.skip_blocks());
      if skipped.is_err() {
        break;
      }
    }

    // SkWuffsCodec rejects an empty canvas; Blink ImageDecoder::SetSize rejects one
    // whose 4-byte pixels overflow an int32.
    if width == 0 || height == 0 {
      return Err(GifError("empty image"));
    }
    if u64::from(width) * u64::from(height) * 4 > i32::MAX as u64 {
      return Err(GifError("image too large"));
    }
    let mut gif = GifDecoder {
      data,
      width,
      height,
      palette,
      frames,
      loops,
    };
    for i in 0..gif.frames.len() {
      let (has_alpha, required) = alpha_and_required_frame(&gif.frames, i, gif.screen());
      gif.frames[i].has_alpha = has_alpha;
      gif.frames[i].required = required;
    }
    Ok(gif)
  }

  fn screen(&self) -> Rect {
    Rect {
      x0: 0,
      y0: 0,
      x1: self.width,
      y1: self.height,
    }
  }

  pub fn width(&self) -> u32 {
    self.width
  }

  pub fn height(&self) -> u32 {
    self.height
  }

  pub fn frames(&self) -> &[GifFrameInfo] {
    &self.frames
  }

  /// Chrome's repetitionCount. Wuffs num_animation_loops counts plays: the last loop count
  /// read plus one (0 is forever), else 1 for an animation and 0 for a still image. Skia
  /// subtracts one and makes 0 forever (SkWuffsCodec::onGetRepetitionCount).
  pub fn repetition_count(&self) -> f64 {
    match self.loops {
      Some(0) => f64::INFINITY,
      Some(count) => f64::from(count),
      None if self.frames.len() > 1 => 0.0,
      None => f64::INFINITY,
    }
  }

  /// Decodes the frames in order. Each item is the full canvas as unpremultiplied
  /// BGRA, or the error of a frame Chrome fails to decode (ImageDecoder's `decode()`
  /// rejects for that frame), also when there is no memory for its canvas.
  pub fn decode(&self) -> GifFrames<'_, 'a> {
    let mut last_dependent = vec![None; self.frames.len()];
    for (i, frame) in self.frames.iter().enumerate() {
      if let Some(required) = frame.required {
        last_dependent[required] = Some(i);
      }
    }
    GifFrames {
      gif: self,
      next: 0,
      last_dependent,
      kept: Vec::new(),
      latest: None,
    }
  }

  /// Draws frame `i` on `canvas` (Wuffs decode_frame with the BGRA_NONPREMUL swizzler)
  fn draw(&self, frame: &GifFrameInfo, canvas: &mut [u8]) -> Result<(), GifError> {
    let mut r = Reader {
      data: self.data,
      pos: frame.pos,
    };
    let (flags, local) = r.flags_and_palette()?;
    let mut palette = match local {
      Some(table) => {
        let mut palette = [[0, 0, 0, 255]; 256];
        fill_palette(&mut palette, table);
        palette
      }
      None => self.palette,
    };
    if let Some(index) = frame.transparent {
      palette[usize::from(index)] = [0; 4];
    }
    let mut lzw = Lzw::new(r.literal_width()?);
    let rect = frame.rect;
    let mut out = Pixels {
      canvas,
      width: self.width,
      height: self.height,
      rect,
      x: rect.x0,
      y: rect.y0,
      interlace: if flags & 0x40 != 0 { 4 } else { 0 },
      palette: &palette,
    };
    let mut lzw_done = false;
    loop {
      let block = match r.u8()? {
        0 => break,
        n => r.take(usize::from(n))?,
      };
      if lzw_done {
        continue;
      }
      match lzw.decode(block, &mut out) {
        LzwStep::More => {}
        LzwStep::End => lzw_done = true,
        // QUIRK_IGNORE_TOO_MUCH_PIXEL_DATA: a bad code after the last row is fine
        LzwStep::BadCode if out.y >= rect.y1 && out.interlace == 0 => lzw_done = true,
        LzwStep::BadCode => return Err(GifError("bad LZW code")),
      }
    }
    if out.y < rect.y1 && rect.x0 != rect.x1 && rect.y0 != rect.y1 {
      return Err(GifError("not enough pixel data"));
    }
    Ok(())
  }
}

/// RGB triples to BGRA, opaque
fn fill_palette(palette: &mut [[u8; 4]; 256], table: &[u8]) {
  for (entry, &[r, g, b]) in palette.iter_mut().zip(table.as_chunks().0) {
    *entry = [b, g, r, 255];
  }
}

/// Skia's SkFrameHolder::setAlphaAndRequiredFrame (src/codec/SkCodec.cpp) for frame
/// `i`: whether it has alpha, and the frame it is drawn on. Wuffs never asks to
/// overwrite instead of blend, so every GIF frame blends over the frame before it.
/// The branch names are Skia's.
fn alpha_and_required_frame(
  frames: &[GifFrameInfo],
  i: usize,
  screen: Rect,
) -> (bool, Option<usize>) {
  let frame = &frames[i];
  let reports_alpha = frame.transparent.is_some();
  let rect = frame.rect.on(screen);
  // IND1: the first frame
  if i == 0 {
    return (reports_alpha || rect != screen, None);
  }
  // IND2: an opaque frame that fills the screen
  if !reports_alpha && rect == screen {
    return (false, None);
  }
  let mut prev = i - 1;
  while frames[prev].disposal == Disposal::RestorePrevious {
    // IND3
    if prev == 0 {
      return (true, None);
    }
    prev -= 1;
  }
  let clear_prev = frames[prev].disposal == Disposal::RestoreBackground;
  let mut prev_rect = frames[prev].rect.on(screen);
  // IND4
  if clear_prev && (prev_rect == screen || frames[prev].required.is_none()) {
    return (true, None);
  }
  // DEP5
  if reports_alpha {
    return (frames[prev].has_alpha || clear_prev, Some(prev));
  }
  while rect.contains(prev_rect) {
    // IND6
    let Some(required) = frames[prev].required else {
      return (true, None);
    };
    prev = required;
    prev_rect = frames[prev].rect.on(screen);
  }
  // DEP7
  (
    frames[prev].disposal == Disposal::RestoreBackground || frames[prev].has_alpha,
    Some(prev),
  )
}

/// Frames in order, each drawn on the frame Blink would start from
pub struct GifFrames<'g, 'a> {
  gif: &'g GifDecoder<'a>,
  next: usize,
  /// The last frame drawn on frame k, for each k
  last_dependent: Vec<Option<usize>>,
  /// Canvases that later frames may start from
  kept: Vec<(usize, Vec<u8>)>,
  /// The latest frame that decoded and is not restore-previous
  latest: Option<usize>,
}

impl GifFrames<'_, '_> {
  /// Blink SkiaImageDecoderBase::Decode: a frame with a required frame starts from
  /// the latest decoded frame after it that is not restore-previous, or else from
  /// the required frame with restore-background applied. A failed required frame
  /// fails this frame.
  fn start(&self, i: usize) -> Result<Vec<u8>, GifError> {
    let gif = self.gif;
    let size = gif.width as usize * gif.height as usize * 4;
    let Some(required) = gif.frames[i].required else {
      let mut canvas = room_for(size)?;
      canvas.resize(size, 0);
      return Ok(canvas);
    };
    let base = self
      .latest
      .filter(|&latest| latest > required)
      .unwrap_or(required);
    let (_, kept) = self
      .kept
      .iter()
      .find(|(k, _)| *k == base)
      .ok_or(GifError("frame depends on a frame that failed"))?;
    let mut canvas = room_for(size)?;
    canvas.extend_from_slice(kept);
    let frame = &gif.frames[required];
    if base == required && frame.disposal == Disposal::RestoreBackground {
      let b = frame.rect.on(gif.screen());
      let stride = gif.width as usize * 4;
      for y in b.y0..b.y1 {
        let row = y as usize * stride;
        canvas[row + b.x0 as usize * 4..row + b.x1 as usize * 4].fill(0);
      }
    }
    Ok(canvas)
  }
}

impl Iterator for GifFrames<'_, '_> {
  type Item = Result<Vec<u8>, GifError>;

  fn next(&mut self) -> Option<Self::Item> {
    let i = self.next;
    let frame = self.gif.frames.get(i)?;
    self.next += 1;
    let result = self.start(i).and_then(|mut canvas| {
      self.gif.draw(frame, &mut canvas)?;
      Ok(canvas)
    });
    if result.is_ok() && frame.disposal != Disposal::RestorePrevious {
      self.latest = Some(i);
    }
    // keep the latest frame while frames follow, and the frames that later frames require
    let (latest, last_dependent) = (self.latest, &self.last_dependent);
    let more = self.next < self.gif.frames.len();
    let keep = |k: usize| (more && Some(k) == latest) || last_dependent[k].is_some_and(|j| j > i);
    self.kept.retain(|(k, _)| keep(*k));
    // without memory for the copy, the frames drawn on this one fail
    if let Ok(canvas) = &result
      && keep(i)
      && let Ok(mut copy) = room_for(canvas.len())
    {
      copy.extend_from_slice(canvas);
      self.kept.push((i, copy));
    }
    Some(result)
  }
}

/// An empty buffer with room for `size` bytes, or an error when the memory is not there (Blink
/// then fails the frame)
fn room_for(size: usize) -> Result<Vec<u8>, GifError> {
  let mut buffer = Vec::new();
  buffer
    .try_reserve_exact(size)
    .map_err(|_| GifError("out of memory"))?;
  Ok(buffer)
}

/// Writes LZW output into the frame rect (Wuffs copy_to_image_buffer)
struct Pixels<'c> {
  canvas: &'c mut [u8],
  width: u32,
  height: u32,
  rect: Rect,
  x: u32,
  y: u32,
  /// 0 for non-interlaced frames and after the last interlace pass
  interlace: usize,
  palette: &'c [[u8; 4]; 256],
}

// Interlace passes, last to first (index 4 is the first pass)
const INTERLACE_START: [u32; 5] = [u32::MAX, 1, 2, 4, 0];
const INTERLACE_DELTA: [u32; 5] = [1, 2, 4, 8, 8];

impl Pixels<'_> {
  fn put(&mut self, indexes: &[u8]) {
    let mut indexes = indexes.iter();
    // Pixel data past the frame rect is ignored. A zero-width frame moves down one
    // row per step until it is done.
    while self.y < self.rect.y1 {
      if self.x < self.rect.x1 {
        let Some(&index) = indexes.next() else {
          return;
        };
        if self.x < self.width && self.y < self.height {
          let color = self.palette[usize::from(index)];
          // the transparent index keeps the pixel below
          if color[3] != 0 {
            let at = (self.y as usize * self.width as usize + self.x as usize) * 4;
            self.canvas[at..at + 4].copy_from_slice(&color);
          }
        }
        self.x += 1;
        if self.x < self.rect.x1 {
          continue;
        }
      } else if indexes.len() == 0 {
        return;
      }
      self.x = self.rect.x0;
      self.y = self.y.saturating_add(INTERLACE_DELTA[self.interlace]);
      while self.interlace > 0 && self.y >= self.rect.y1 {
        self.interlace -= 1;
        self.y = self.rect.y0.saturating_add(INTERLACE_START[self.interlace]);
      }
    }
  }
}

/// GIF LZW decoder with Wuffs' rules (std/lzw/decode_lzw.wuffs): literal widths 0 to
/// 8, the code width grows when the next code reaches it, the table stops at 4096
/// codes, and a code past the next one is an error.
struct Lzw {
  literal_width: u32,
  clear: u32,
  end: u32,
  /// next table code
  save: u32,
  prev: u32,
  width: u32,
  bits: u32,
  n_bits: u32,
  prefix: Vec<u16>,
  suffix: Vec<u8>,
  first: Vec<u8>,
  len: Vec<u16>,
  stack: Vec<u8>,
}

enum LzwStep {
  More,
  End,
  BadCode,
}

impl Lzw {
  fn new(literal_width: u32) -> Self {
    let clear = 1 << literal_width;
    let mut lzw = Lzw {
      literal_width,
      clear,
      end: clear + 1,
      save: 0,
      prev: 0,
      width: 0,
      bits: 0,
      n_bits: 0,
      prefix: vec![0; 4096],
      suffix: vec![0; 4096],
      first: vec![0; 4096],
      len: vec![0; 4096],
      stack: Vec::with_capacity(4096),
    };
    for code in 0..clear as usize {
      lzw.suffix[code] = code as u8;
      lzw.first[code] = code as u8;
      lzw.len[code] = 1;
    }
    lzw.reset();
    lzw
  }

  fn reset(&mut self) {
    self.save = self.end;
    self.prev = self.end;
    self.width = self.literal_width + 1;
  }

  /// Decodes one data sub-block
  fn decode(&mut self, block: &[u8], out: &mut Pixels) -> LzwStep {
    for &byte in block {
      self.bits |= u32::from(byte) << self.n_bits;
      self.n_bits += 8;
      while self.n_bits >= self.width {
        let code = self.bits & ((1 << self.width) - 1);
        self.bits >>= self.width;
        self.n_bits -= self.width;
        if code == self.clear {
          self.reset();
          continue;
        }
        if code == self.end {
          return LzwStep::End;
        }
        if code > self.save {
          return LzwStep::BadCode;
        }
        // KwKwK: the code being defined is the previous string plus its first byte
        let c = if code == self.save { self.prev } else { code };
        let first = self.first[c as usize];
        // once the frame is full, Pixels::put drops the output; only the table changes
        if out.y < out.rect.y1 {
          self.stack.clear();
          let mut at = c as usize;
          for _ in 0..self.len[c as usize] {
            self.stack.push(self.suffix[at]);
            at = usize::from(self.prefix[at]);
          }
          self.stack.reverse();
          if code == self.save {
            self.stack.push(first);
          }
          out.put(&self.stack);
        }
        if self.save <= 4095 {
          // the first code after a clear code fills the unused end-code slot
          if self.prev != self.end {
            let (save, prev) = (self.save as usize, self.prev as usize);
            self.prefix[save] = prev as u16;
            self.suffix[save] = first;
            self.first[save] = self.first[prev];
            self.len[save] = self.len[prev] + 1;
          }
          self.save += 1;
          if self.width < 12 {
            self.width += 1 & (self.save >> self.width);
          }
          self.prev = code;
        }
      }
    }
    LzwStep::More
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::webcodecs::chrome_cases::{self, Expected, fnv1a};

  #[test]
  fn frames_match_chrome() {
    for case in chrome_cases::cases("gif/") {
      let name = &case.name;
      let gif = match (GifDecoder::new(&case.data), &case.frames[..]) {
        (Err(_), [Expected::OpenError]) => continue,
        (Ok(_), [Expected::OpenError]) => panic!("{name}: Chrome fails to open it"),
        (Err(e), _) => panic!("{name}: {e}"),
        (Ok(gif), _) => gif,
      };
      assert_eq!(Some(gif.frames().len()), case.frame_count, "{name}");
      for (i, (frame, expected)) in gif.decode().zip(&case.frames).enumerate() {
        match (frame, expected) {
          (Err(_), Expected::Error) => {}
          (
            Ok(pixels),
            &Expected::Frame {
              alpha,
              width,
              height,
              hash,
            },
          ) => {
            assert_eq!(gif.frames()[i].has_alpha, alpha, "{name} frame {i}");
            assert_eq!((gif.width(), gif.height()), (width, height), "{name}");
            assert_eq!(fnv1a(&pixels), hash, "{name} frame {i} pixels");
          }
          (frame, expected) => panic!("{name} frame {i}: {:?} vs {expected:?}", frame.err()),
        }
      }
    }
  }

  #[test]
  fn broken_files_do_not_panic() {
    for case in chrome_cases::cases("gif/") {
      for data in chrome_cases::broken_copies(&case.data) {
        let Ok(gif) = GifDecoder::new(&data) else {
          continue;
        };
        // keep the test small; the canvas limit itself is Chrome's (4 * w * h < 2^31)
        if gif.width() * gif.height() > 1 << 20 {
          continue;
        }
        let size = gif.width() as usize * gif.height() as usize * 4;
        assert!(gif.decode().flatten().all(|pixels| pixels.len() == size));
      }
    }
  }

  #[test]
  fn canvas_limit() {
    let gif = |w: u16, h: u16| {
      let mut data = b"GIF89a".to_vec();
      data.extend([w.to_le_bytes(), h.to_le_bytes()].concat());
      data.extend([0, 0, 0, 0x3B]);
      GifDecoder::new(&data).map(|gif| (gif.width(), gif.height()))
    };
    assert_eq!(gif(23170, 23170), Ok((23170, 23170)));
    assert!(gif(23171, 23171).is_err());
    assert!(gif(0, 7).is_err());
  }

  fn frame(rect: [u32; 4], transparent: bool, disposal: Disposal) -> GifFrameInfo {
    let [x, y, w, h] = rect;
    GifFrameInfo {
      has_alpha: false,
      delay: 0,
      required: None,
      rect: Rect {
        x0: x,
        y0: y,
        x1: x + w,
        y1: y + h,
      },
      transparent: transparent.then_some(0),
      disposal,
      pos: 0,
    }
  }

  const FULL: [u32; 4] = [0, 0, 11, 5];
  use Disposal::{Keep, RestoreBackground as Background, RestorePrevious as Previous};

  /// Skia's alpha for each frame on an 11x5 screen
  fn alpha(mut frames: Vec<GifFrameInfo>) -> Vec<bool> {
    let screen = Rect {
      x1: 11,
      y1: 5,
      ..Rect::default()
    };
    for i in 0..frames.len() {
      (frames[i].has_alpha, frames[i].required) = alpha_and_required_frame(&frames, i, screen);
    }
    frames.iter().map(|frame| frame.has_alpha).collect()
  }

  #[test]
  fn frame_alpha_follows_each_branch_of_skias_rule() {
    // IND1: the first frame has alpha when it has a transparent index or leaves screen pixels
    assert_eq!(alpha(vec![frame(FULL, false, Keep)]), [false]);
    assert_eq!(alpha(vec![frame(FULL, true, Keep)]), [true]);
    assert_eq!(alpha(vec![frame([1, 1, 4, 3], false, Keep)]), [true]);
    // IND2: an opaque frame that fills the screen, after any frame
    assert_eq!(
      alpha(vec![frame(FULL, true, Keep), frame(FULL, false, Keep)]),
      [true, false]
    );
    // IND3: every frame before it is restored to the previous image
    assert_eq!(
      alpha(vec![
        frame(FULL, false, Previous),
        frame([5, 0, 6, 5], false, Keep)
      ]),
      [false, true]
    );
    // IND4: the frame before is cleared and fills the screen (or is independent)
    assert_eq!(
      alpha(vec![
        frame(FULL, false, Background),
        frame([2, 1, 3, 2], false, Keep)
      ]),
      [false, true]
    );
    // DEP5: a frame with a transparent index takes the alpha of the frame below
    assert_eq!(
      alpha(vec![
        frame(FULL, false, Keep),
        frame([1, 1, 5, 3], true, Keep)
      ]),
      [false, false]
    );
    assert_eq!(
      alpha(vec![
        frame([0, 0, 5, 5], false, Keep),
        frame([1, 1, 5, 3], true, Keep)
      ]),
      [true, true]
    );
    // DEP5 after a cleared dependent frame
    assert_eq!(
      alpha(vec![
        frame(FULL, false, Keep),
        frame([1, 1, 5, 3], true, Background),
        frame([2, 2, 3, 2], true, Keep),
      ]),
      [false, false, true]
    );
    // IND6: it covers every frame back to an independent one with alpha
    assert_eq!(
      alpha(vec![
        frame([1, 1, 4, 3], false, Keep),
        frame([0, 0, 6, 5], false, Keep)
      ]),
      [true, true]
    );
    // DEP7: drawn over an opaque frame, or over a cleared one
    assert_eq!(
      alpha(vec![
        frame(FULL, false, Keep),
        frame([2, 1, 3, 2], false, Keep)
      ]),
      [false, false]
    );
    assert_eq!(
      alpha(vec![
        frame(FULL, false, Keep),
        frame([2, 1, 3, 2], true, Background),
        frame([0, 0, 4, 2], false, Keep),
      ]),
      [false, false, true]
    );
  }

  #[test]
  fn frame_alpha_matches_chrome_on_the_research_animations() {
    // gif_anim_trans.gif: X X A A A
    assert_eq!(
      alpha(vec![
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
      alpha(vec![
        frame(FULL, true, Keep),
        frame(FULL, false, Keep),
        frame(FULL, true, Keep)
      ]),
      [true, false, false]
    );
    // gif_anim_smallf0.gif: A A X
    assert_eq!(
      alpha(vec![
        frame([1, 1, 4, 3], false, Background),
        frame([5, 0, 6, 5], false, Keep),
        frame(FULL, false, Keep),
      ]),
      [true, true, false]
    );
    // gif_anim_dispose_bg_opaque.gif: X A
    assert_eq!(
      alpha(vec![
        frame(FULL, false, Background),
        frame([2, 1, 3, 2], false, Keep)
      ]),
      [false, true]
    );
  }

  /// A GIF89a: a 2-color global table, and per frame its rect and graphic control flags
  fn gif_file(screen: (u16, u16), frames: &[([u16; 4], Option<u8>)]) -> Vec<u8> {
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
  fn reads_frame_alpha_like_wuffs() {
    let alpha = |data: &[u8]| {
      GifDecoder::new(data).map(|gif| gif.frames().iter().map(|f| f.has_alpha).collect::<Vec<_>>())
    };
    // a transparent index, and the screen growing to the first frame
    assert_eq!(
      alpha(&gif_file((11, 5), &[([0, 0, 11, 5], Some(1))])),
      Ok(vec![true])
    );
    assert_eq!(
      alpha(&gif_file((11, 5), &[([0, 0, 11, 5], Some(0))])),
      Ok(vec![false])
    );
    assert_eq!(
      alpha(&gif_file((4, 2), &[([0, 0, 11, 5], None)])),
      Ok(vec![false])
    );
    // the graphic control extension applies to the next frame only
    assert_eq!(
      alpha(&gif_file(
        (11, 5),
        &[([0, 0, 11, 5], Some(1)), ([0, 0, 11, 5], None)]
      )),
      Ok(vec![true, false])
    );
    // disposal 4 is "restore previous" too: frame 1 has nothing opaque below it
    for disposal in [3, 4] {
      assert_eq!(
        alpha(&gif_file(
          (11, 5),
          &[([0, 0, 11, 5], Some(disposal << 2)), ([5, 0, 6, 5], None)]
        )),
        Ok(vec![false, true])
      );
    }
    // an application extension between frames, and a truncated last frame, still count
    let mut data = gif_file((11, 5), &[([0, 0, 11, 5], None)]);
    data.pop();
    data.extend(b"\x21\xff\x0bNETSCAPE2.0\x03\x01\0\0\0");
    data.extend([0x2c, 0, 0, 0, 0, 11, 0, 5, 0, 0, 2, 2]);
    assert_eq!(alpha(&data), Ok(vec![false, false]));
  }

  #[test]
  fn reads_the_loop_count_like_wuffs() {
    // the header and global table, the extensions, `n` frames and the trailer
    let file = gif_file((11, 5), &[([0, 0, 11, 5], None)]);
    let (head, frame) = file[..file.len() - 1].split_at(19);
    let read = |extensions: &[u8], n: usize| {
      let mut data = [head, extensions, &frame.repeat(n)].concat();
      data.push(0x3b);
      let gif = GifDecoder::new(&data).unwrap();
      (gif.frames().len(), gif.repetition_count())
    };
    let netscape = |count: u16| {
      [
        b"\x21\xff\x0bNETSCAPE2.0\x03\x01",
        &count.to_le_bytes()[..],
        b"\0",
      ]
      .concat()
    };
    assert_eq!(read(&netscape(5), 2), (2, 5.0));
    assert_eq!(read(&netscape(0), 2), (2, f64::INFINITY));
    assert_eq!(
      read(b"\x21\xff\x0bANIMEXTS1.0\x03\x01\x04\x00\x00", 1),
      (1, 4.0)
    );
    // without one, an animation plays once and a still image loops forever
    assert_eq!(read(b"", 2), (2, 0.0));
    assert_eq!(read(b"", 1), (1, f64::INFINITY));
    // the last one counts; one in a comment or with sub-block id 2 does not
    assert_eq!(read(&[netscape(3), netscape(7)].concat(), 2), (2, 7.0));
    assert_eq!(
      read(&[b"\x21\xfe\x13", &netscape(5)[..], b"\0"].concat(), 2),
      (2, 0.0)
    );
    assert_eq!(
      read(b"\x21\xff\x0bNETSCAPE2.0\x03\x02\x05\x00\x00", 2),
      (2, 0.0)
    );
    // an empty sub-block after the id does not end the extension
    assert_eq!(
      read(b"\x21\xff\x0bNETSCAPE2.0\x00\x00", 1),
      (1, f64::INFINITY)
    );
  }
}
