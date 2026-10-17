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
      size: self.width as usize * self.height as usize * 4,
      next: 0,
      last_dependent,
      kept: Vec::new(),
      latest: None,
    }
  }

  /// `decode` without pixels: each item is the error `decode` gives for that frame, or an empty
  /// canvas. It allocates no canvas.
  pub fn check(&self) -> GifFrames<'_, 'a> {
    GifFrames {
      size: 0,
      ..self.decode()
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
    // a dry run (`check`) has no canvas: it reads the same data, and writes no pixel
    let (width, height) = if canvas.is_empty() {
      (0, 0)
    } else {
      (self.width, self.height)
    };
    let mut out = Pixels {
      canvas,
      width,
      height,
      rect,
      x: rect.x0,
      y: rect.y0,
      interlace: if flags & 0x40 != 0 { 4 } else { 0 },
      palette: &palette,
      direct: is_inside(rect, width, height),
      stack: Vec::new(),
      runs: Vec::new(),
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

/// Whether `rect` is at least one pixel wide and inside a `width` x `height` canvas (none in a
/// dry run). Then each index lands on the canvas until the frame is full, so `Pixels` writes
/// the frame straight from the LZW table (`Pixels::direct`).
fn is_inside(rect: Rect, width: u32, height: u32) -> bool {
  rect.x0 < rect.x1 && rect.x1 <= width && rect.y1 <= height
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
  /// Bytes of a canvas, 0 in a dry run (`check`)
  size: usize,
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
    let size = self.size;
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
    // (a dry run has no pixels to clear)
    if size > 0 && base == required && frame.disposal == Disposal::RestoreBackground {
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

/// Counts the steps of `Pixels` and `Lzw`, for the work bounds in the tests
#[cfg(test)]
fn work(steps: usize) {
  tests::WORK.with(|work| work.set(work.get() + steps as u64));
}

#[cfg(not(test))]
fn work(_: usize) {}

/// Counts the indexes that `Output::read_back` reads out of the LZW table, for the tests
#[cfg(test)]
fn indexes_read(n: usize) {
  tests::INDEXES.with(|read| read.set(read.get() + n as u64));
}

#[cfg(not(test))]
fn indexes_read(_: usize) {}

/// Writes LZW output into the frame rect (Wuffs copy_to_image_buffer). The pixels go in
/// row by row, and in an interlaced frame pass by pass. There are two ways, chosen per frame:
///
/// - `direct`: the frame is inside the canvas, so each index lands on it until the frame is
///   full. Each code's string is written straight from the table (`put_direct`). A frame
///   costs steps for its codes and its pixels, at most the canvas.
/// - otherwise (part of the frame is off the canvas, or a dry run): output that lands off the
///   canvas is skipped without being read (`put`), so a frame costs steps for its codes, the
///   pixels it writes and its rows on the canvas, whatever the size of its rect.
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
  /// The frame is inside the canvas and at least one pixel wide: `put_direct` writes it
  direct: bool,
  /// `put_direct`: a string that runs past the end of its row, last index first
  stack: Vec<u8>,
  /// `put`: the parts of one LZW output that land on the canvas (offset, length, first pixel)
  runs: Vec<(usize, usize, usize)>,
}

// Interlace passes, last to first (index 4 is the first pass)
const INTERLACE_START: [u32; 5] = [u32::MAX, 1, 2, 4, 0];
const INTERLACE_DELTA: [u32; 5] = [1, 2, 4, 8, 8];

impl Pixels<'_> {
  /// Puts the output of one LZW code in a frame inside the canvas (`direct`). Pixel data past
  /// the frame rect is ignored.
  fn put_direct(&mut self, output: &Output) {
    let rect = self.rect;
    let len = output.len();
    let palette = self.palette;
    let width = self.width as usize;
    if len <= (rect.x1 - self.x) as usize {
      // the rest of this row holds it: written from its last index back along its prefixes
      let at = self.y as usize * width + self.x as usize;
      let pixels = &mut self.canvas.as_chunks_mut::<4>().0[at..at + len];
      output.read_back(output.code, 0, len, |i, index| {
        let color = palette[usize::from(index)];
        // the transparent index keeps the pixel below
        if color[3] != 0 {
          pixels[i] = color;
        }
      });
      self.x += len as u32;
      if self.x == rect.x1 {
        self.next_row();
      }
      return;
    }
    // it runs past the end of the row: read into the stack, then written row by row
    let stack = &mut self.stack;
    stack.clear();
    output.read_back(output.code, 0, len, |_, index| stack.push(index));
    // the indexes still to write are stack[..left], last index first
    let mut left = len;
    while left > 0 && self.y < rect.y1 {
      work(1);
      let n = ((rect.x1 - self.x) as usize).min(left);
      let at = self.y as usize * width + self.x as usize;
      let pixels = &mut self.canvas.as_chunks_mut::<4>().0[at..at + n];
      for (pixel, &index) in pixels
        .iter_mut()
        .zip(self.stack[left - n..left].iter().rev())
      {
        let color = palette[usize::from(index)];
        if color[3] != 0 {
          *pixel = color;
        }
      }
      left -= n;
      self.x += n as u32;
      if self.x == rect.x1 {
        self.next_row();
      }
    }
  }

  /// Puts the output of one LZW code in any frame, and reads only the indexes that land on the
  /// canvas. Pixel data past the frame rect is ignored.
  fn put(&mut self, output: &Output) {
    let rect = self.rect;
    let len = output.len();
    self.runs.clear();
    let mut done = 0;
    while done < len && self.y < rect.y1 {
      work(1);
      if rect.x0 == rect.x1 {
        // a zero-width frame moves down one row per step until it is done
        self.y = if self.interlace > 0 {
          u32::MAX
        } else {
          rect.y1
        };
        self.interlace = 0;
        break;
      }
      let left = len - done;
      if self.x < self.width && self.y < self.height {
        // on the canvas, up to its right edge or the end of the row
        let n = ((rect.x1.min(self.width) - self.x) as usize).min(left);
        let pixel = self.y as usize * self.width as usize + self.x as usize;
        self.runs.push((done, n, pixel));
        done += n;
        self.x += n as u32;
        if self.x == rect.x1 {
          self.next_row();
        }
      } else {
        let n = self.off_canvas().min(left as u64);
        done += n as usize;
        self.skip(n);
      }
    }
    let (canvas, palette) = (&mut *self.canvas, self.palette);
    output.read_rev(&self.runs, |pixel, index| {
      let color = palette[usize::from(index)];
      // the transparent index keeps the pixel below
      if color[3] != 0 {
        canvas[pixel * 4..pixel * 4 + 4].copy_from_slice(&color);
      }
    });
  }

  /// Pixels from here (off the canvas) to the next one on the canvas, or to the end of
  /// this pass
  fn off_canvas(&self) -> u64 {
    let rect = self.rect;
    let delta = INTERLACE_DELTA[self.interlace];
    let rest = u64::from(rect.x1 - self.x);
    let rows_below = u64::from((rect.y1 - 1 - self.y) / delta);
    // the rows below in this pass are off the canvas when it ends above them or left of the rect
    if rect.x0 >= self.width || self.y + delta >= self.height {
      rest + rows_below * u64::from(rect.x1 - rect.x0)
    } else {
      rest
    }
  }

  /// Moves over `n` pixels without writing them, as far as one step per pixel would. `n` is at
  /// most the pixels left in this pass, and the frame is at least one pixel wide.
  fn skip(&mut self, n: u64) {
    let width = u64::from(self.rect.x1 - self.rect.x0);
    let x = u64::from(self.x - self.rect.x0) + n;
    let rows = x / width;
    if rows > 0 {
      // to the last row passed, then on to the next row
      self.y += (rows - 1) as u32 * INTERLACE_DELTA[self.interlace];
      self.next_row();
    }
    self.x = self.rect.x0 + (x % width) as u32;
  }

  /// To the start of the next row, which in an interlaced frame is in the next pass that has
  /// rows once this pass is done
  fn next_row(&mut self) {
    self.x = self.rect.x0;
    self.y = self.y.saturating_add(INTERLACE_DELTA[self.interlace]);
    while self.interlace > 0 && self.y >= self.rect.y1 {
      self.interlace -= 1;
      self.y = self.rect.y0.saturating_add(INTERLACE_START[self.interlace]);
    }
  }
}

/// The output of one LZW code: the string of table code `code`, then `extra` for a code
/// that is being defined (KwKwK)
struct Output<'l> {
  lzw: &'l Lzw,
  code: usize,
  extra: Option<u8>,
}

impl Output<'_> {
  fn len(&self) -> usize {
    usize::from(self.lzw.len[self.code]) + usize::from(self.extra.is_some())
  }

  /// Calls `f` with each offset in `start..end` and its index, last to first. `code` is the code
  /// of the first `end` bytes of the output (of its whole string when `end` takes the extra
  /// byte). Returns the code of the first `start` bytes. Every output path reads its indexes
  /// through here.
  #[inline(always)]
  fn read_back(
    &self,
    mut code: usize,
    start: usize,
    mut end: usize,
    mut f: impl FnMut(usize, u8),
  ) -> usize {
    work(end - start);
    indexes_read(end - start);
    let lzw = self.lzw;
    if let Some(extra) = self.extra
      && end > usize::from(lzw.len[self.code])
    {
      end -= 1;
      f(end, extra);
    }
    for i in (start..end).rev() {
      f(i, lzw.suffix[code]);
      code = usize::from(lzw.prefix[code]);
    }
    code
  }

  /// Calls `f` with the pixel and the byte for each byte of `runs` (offset, length, first
  /// pixel; in order), last to first. The string is read once from its end, with a jump over
  /// each gap between runs, so it takes O(bytes + runs x log(len)) steps.
  fn read_rev(&self, runs: &[(usize, usize, usize)], mut f: impl FnMut(usize, u8)) {
    let len = usize::from(self.lzw.len[self.code]);
    // a code whose string is at least as long as the end of the next run
    let mut code = self.code;
    for &(start, n, pixel) in runs.iter().rev() {
      let end = start + n;
      code = self.lzw.prefix_code(code, end.min(len));
      code = self.read_back(code, start, end, |i, index| f(pixel + i - start, index));
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
  /// A shorter prefix of each code's string, so that any prefix is found in O(log len) steps
  /// (Myers' jump pointers). Only `Pixels::put` needs it: it is made on the first block decoded
  /// for a frame that is not `direct`, and filled as codes are defined.
  jump: Vec<u16>,
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
      jump: Vec::new(),
    };
    for code in 0..clear as usize {
      lzw.suffix[code] = code as u8;
      lzw.first[code] = code as u8;
      lzw.len[code] = 1;
    }
    lzw.reset();
    lzw
  }

  /// The code of the first `len` bytes of `code`'s string (1 <= `len` <= its length)
  fn prefix_code(&self, mut code: usize, len: usize) -> usize {
    while usize::from(self.len[code]) > len {
      work(1);
      let jump = usize::from(self.jump[code]);
      code = if usize::from(self.len[jump]) >= len {
        jump
      } else {
        usize::from(self.prefix[code])
      };
    }
    code
  }

  fn reset(&mut self) {
    self.save = self.end;
    self.prev = self.end;
    self.width = self.literal_width + 1;
  }

  /// Decodes one data sub-block
  fn decode(&mut self, block: &[u8], out: &mut Pixels) -> LzwStep {
    if out.direct {
      return self.decode_into::<true>(block, out);
    }
    if self.jump.is_empty() {
      // each literal is its own jump
      self.jump.reserve_exact(4096);
      self.jump.extend(0..self.clear as u16);
    }
    self.decode_into::<false>(block, out)
  }

  /// `decode`, with `DIRECT` equal to `out.direct`. Kept out of line: inlined into the frame
  /// loop of the image decoder, it ran about 1.2 times slower on GIFs with short strings.
  #[inline(never)]
  fn decode_into<const DIRECT: bool>(&mut self, block: &[u8], out: &mut Pixels) -> LzwStep {
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
        work(1);
        // KwKwK: the code being defined is the previous string plus its first byte
        let c = if code == self.save { self.prev } else { code };
        let first = self.first[c as usize];
        // once the frame is full, the output is dropped; only the table changes
        if out.y < out.rect.y1 {
          let output = Output {
            lzw: self,
            code: c as usize,
            extra: (code == self.save).then_some(first),
          };
          if DIRECT {
            out.put_direct(&output);
          } else {
            out.put(&output);
          }
        }
        if self.save <= 4095 {
          // the first code after a clear code fills the unused end-code slot
          if self.prev != self.end {
            let (save, prev) = (self.save as usize, self.prev as usize);
            self.prefix[save] = prev as u16;
            self.suffix[save] = first;
            self.first[save] = self.first[prev];
            self.len[save] = self.len[prev] + 1;
            if !DIRECT {
              // when the jump of `prev` is as long as the jump after it, jump over both; else
              // to `prev`
              let jump = usize::from(self.jump[prev]);
              let jump2 = self.jump[jump];
              let same =
                self.len[prev] - self.len[jump] == self.len[jump] - self.len[usize::from(jump2)];
              if save >= self.jump.len() {
                self.jump.resize(save + 1, 0);
              }
              self.jump[save] = if same { jump2 } else { prev as u16 };
            }
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
  use std::cell::Cell;

  thread_local! {
    /// Steps counted by `work`
    pub(super) static WORK: Cell<u64> = const { Cell::new(0) };
    /// Indexes counted by `indexes_read`
    pub(super) static INDEXES: Cell<u64> = const { Cell::new(0) };
  }

  /// The output path of 33da7b1, before output off the canvas was skipped: `decode` reads each
  /// code's whole string into a stack, and `put` steps over each index of it. The tests check
  /// that `GifDecoder::draw` gives the same results.
  mod reference {
    use super::super::*;
    use std::cell::Cell;

    thread_local! {
      /// Bytes read into the stack
      pub static READ: Cell<u64> = const { Cell::new(0) };
      /// Bytes that land on the canvas
      pub static ON: Cell<u64> = const { Cell::new(0) };
    }

    pub fn put(out: &mut Pixels, indexes: &[u8]) {
      let mut indexes = indexes.iter();
      // Pixel data past the frame rect is ignored. A zero-width frame moves down one
      // row per step until it is done.
      while out.y < out.rect.y1 {
        if out.x < out.rect.x1 {
          let Some(&index) = indexes.next() else {
            return;
          };
          if out.x < out.width && out.y < out.height {
            ON.set(ON.get() + 1);
            let color = out.palette[usize::from(index)];
            // the transparent index keeps the pixel below
            if color[3] != 0 {
              let at = (out.y as usize * out.width as usize + out.x as usize) * 4;
              out.canvas[at..at + 4].copy_from_slice(&color);
            }
          }
          out.x += 1;
          if out.x < out.rect.x1 {
            continue;
          }
        } else if indexes.len() == 0 {
          return;
        }
        out.x = out.rect.x0;
        out.y = out.y.saturating_add(INTERLACE_DELTA[out.interlace]);
        while out.interlace > 0 && out.y >= out.rect.y1 {
          out.interlace -= 1;
          out.y = out.rect.y0.saturating_add(INTERLACE_START[out.interlace]);
        }
      }
    }

    /// Decodes one data sub-block
    pub fn decode(lzw: &mut Lzw, stack: &mut Vec<u8>, block: &[u8], out: &mut Pixels) -> LzwStep {
      for &byte in block {
        lzw.bits |= u32::from(byte) << lzw.n_bits;
        lzw.n_bits += 8;
        while lzw.n_bits >= lzw.width {
          let code = lzw.bits & ((1 << lzw.width) - 1);
          lzw.bits >>= lzw.width;
          lzw.n_bits -= lzw.width;
          if code == lzw.clear {
            lzw.reset();
            continue;
          }
          if code == lzw.end {
            return LzwStep::End;
          }
          if code > lzw.save {
            return LzwStep::BadCode;
          }
          // KwKwK: the code being defined is the previous string plus its first byte
          let c = if code == lzw.save { lzw.prev } else { code };
          let first = lzw.first[c as usize];
          // once the frame is full, Pixels::put drops the output; only the table changes
          if out.y < out.rect.y1 {
            stack.clear();
            let mut at = c as usize;
            for _ in 0..lzw.len[c as usize] {
              stack.push(lzw.suffix[at]);
              at = usize::from(lzw.prefix[at]);
            }
            stack.reverse();
            if code == lzw.save {
              stack.push(first);
            }
            READ.set(READ.get() + stack.len() as u64);
            put(out, stack);
          }
          if lzw.save <= 4095 {
            // the first code after a clear code fills the unused end-code slot
            if lzw.prev != lzw.end {
              let (save, prev) = (lzw.save as usize, lzw.prev as usize);
              lzw.prefix[save] = prev as u16;
              lzw.suffix[save] = first;
              lzw.first[save] = lzw.first[prev];
              lzw.len[save] = lzw.len[prev] + 1;
            }
            lzw.save += 1;
            if lzw.width < 12 {
              lzw.width += 1 & (lzw.save >> lzw.width);
            }
            lzw.prev = code;
          }
        }
      }
      LzwStep::More
    }

    /// Draws frame `i` on `canvas` (Wuffs decode_frame with the BGRA_NONPREMUL swizzler)
    pub fn draw(gif: &GifDecoder, frame: &GifFrameInfo, canvas: &mut [u8]) -> Result<(), GifError> {
      let mut r = Reader {
        data: gif.data,
        pos: frame.pos,
      };
      let (flags, local) = r.flags_and_palette()?;
      let mut palette = match local {
        Some(table) => {
          let mut palette = [[0, 0, 0, 255]; 256];
          fill_palette(&mut palette, table);
          palette
        }
        None => gif.palette,
      };
      if let Some(index) = frame.transparent {
        palette[usize::from(index)] = [0; 4];
      }
      let mut lzw = Lzw::new(r.literal_width()?);
      let mut stack = Vec::with_capacity(4096);
      let rect = frame.rect;
      // a dry run (`check`) has no canvas: it reads the same data, and writes no pixel
      let (width, height) = if canvas.is_empty() {
        (0, 0)
      } else {
        (gif.width, gif.height)
      };
      let mut out = Pixels {
        canvas,
        width,
        height,
        rect,
        x: rect.x0,
        y: rect.y0,
        interlace: if flags & 0x40 != 0 { 4 } else { 0 },
        palette: &palette,
        direct: false,
        stack: Vec::new(),
        runs: Vec::new(),
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
        match decode(&mut lzw, &mut stack, block, &mut out) {
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

  /// The errors of `check`, or of `decode` with each canvas as 0 bytes
  fn check_and_decode(gif: &GifDecoder) -> [Vec<Result<usize, GifError>>; 2] {
    [
      gif.check().map(|r| r.map(|c| c.len())).collect(),
      gif.decode().map(|r| r.map(|_| 0)).collect(),
    ]
  }

  #[test]
  fn check_finds_the_errors_of_decode() {
    for case in chrome_cases::cases("gif/") {
      let copies = chrome_cases::broken_copies(&case.data);
      for data in std::iter::once(case.data.clone()).chain(copies) {
        let Ok(gif) = GifDecoder::new(&data) else {
          continue;
        };
        if gif.width() * gif.height() > 1 << 20 {
          continue;
        }
        let [checked, decoded] = check_and_decode(&gif);
        assert_eq!(checked, decoded, "{}", case.name);
      }
    }
  }

  /// LZW data with literal width 2: a clear code, `pixels` codes of index 0, then `tail`. The
  /// code width grows as in `Lzw::decode`.
  fn lzw_data(pixels: usize, tail: &[u32]) -> Vec<u8> {
    let (clear, end) = (4, 5);
    let (mut bits, mut n, mut width, mut save) = (0u64, 0, 3, end);
    let mut data = Vec::new();
    let codes = std::iter::once(clear).chain(std::iter::repeat_n(0, pixels));
    for code in codes.chain(tail.iter().copied()) {
      bits |= u64::from(code) << n;
      n += width;
      while n >= 8 {
        data.push(bits as u8);
        bits >>= 8;
        n -= 8;
      }
      if code == clear {
        (width, save) = (3, end);
      } else if code != end && save <= 4095 {
        save += 1;
        if width < 12 {
          width += 1 & (save >> width);
        }
      }
    }
    if n > 0 {
      data.push(bits as u8);
    }
    data
  }

  /// A GIF with one frame (`rect`: x, y, w, h) on a 4x10 screen, with `lzw` in one sub-block
  fn one_frame_gif(rect: [u16; 4], interlaced: bool, lzw: &[u8]) -> Vec<u8> {
    let mut file = b"GIF89a".to_vec();
    // a 2-color global table
    file.extend([4, 0, 10, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255, 0x2c]);
    rect.iter().for_each(|v| file.extend(v.to_le_bytes()));
    file.extend([if interlaced { 0x40 } else { 0 }, 2, lzw.len() as u8]);
    file.extend(lzw);
    file.extend([0, 0x3b]);
    file
  }

  #[test]
  fn check_follows_the_rows_of_decode() {
    // Frames of 0x0 to 3x9 pixels, interlaced or not, with pixel data that stops before, at and
    // after the last pixel of each row and pass, then ends, has a bad code (a clear code, then
    // the code past the next table code), or just stops. Each file is also cut inside its LZW
    // data and before its block terminator.
    let tails: [&[u32]; 3] = [&[5], &[4, 7], &[]];
    let mut seen = std::collections::BTreeMap::new();
    for w in 0..=3 {
      for h in 0..=9 {
        for [x, y] in [[0, 0], [1, 1]] {
          for interlaced in [false, true] {
            for pixels in 0..=usize::from(w * h) + 2 {
              for tail in tails {
                let lzw = lzw_data(pixels, tail);
                let file = one_frame_gif([x, y, w, h], interlaced, &lzw);
                let lzw_at = file.len() - 2 - lzw.len();
                for len in [file.len(), file.len() - 2, lzw_at + lzw.len() / 2] {
                  let gif = GifDecoder::new(&file[..len]).unwrap();
                  assert_eq!(gif.frames().len(), 1);
                  let [checked, decoded] = check_and_decode(&gif);
                  let frame = format!("{w}x{h} at {x},{y}, interlaced {interlaced}");
                  let data = format!("{pixels} pixels, then {tail:?}, cut at {len}");
                  assert_eq!(checked, decoded, "{frame}, {data}");
                  let error = decoded[0].as_ref().err().map(|e| e.0);
                  *seen.entry((interlaced, error)).or_insert(0) += 1;
                }
              }
            }
          }
        }
      }
    }
    // each result comes up, with and without interlacing
    for interlaced in [false, true] {
      for error in [
        None,
        Some("bad LZW code"),
        Some("not enough pixel data"),
        Some("truncated data"),
      ] {
        assert!(seen.contains_key(&(interlaced, error)), "{seen:?}");
      }
    }
  }

  /// SplitMix64
  struct Rng(u64);

  impl Rng {
    /// A number in 0..n
    fn below(&mut self, n: u64) -> u64 {
      self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
      let mut z = self.0;
      z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
      z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
      (z ^ (z >> 31)) % n
    }
  }

  /// LZW data of literal width `lw` with these codes. The code width grows as in `Lzw::decode`.
  fn pack(lw: u32, codes: &[u32]) -> Vec<u8> {
    let (clear, end) = (1 << lw, (1 << lw) + 1);
    let (mut bits, mut n, mut width, mut save) = (0u64, 0, lw + 1, end);
    let mut data = Vec::new();
    for &code in codes {
      bits |= u64::from(code) << n;
      n += width;
      while n >= 8 {
        data.push(bits as u8);
        bits >>= 8;
        n -= 8;
      }
      if code == clear {
        (width, save) = (lw + 1, end);
      } else if code != end && save <= 4095 {
        save += 1;
        if width < 12 {
          width += 1 & (save >> width);
        }
      }
    }
    if n > 0 {
      data.push(bits as u8);
    }
    data
  }

  /// `count` LZW codes of literal width 2 whose strings grow by one: a clear code, index 1,
  /// each new code (KwKwK) up to `longest`, then `longest` again. Code `c` gives `c - 4`
  /// indexes, so the output is far larger than the data.
  fn growing(longest: u32, count: usize) -> Vec<u32> {
    let codes = [4, 1].into_iter().chain(6..=longest);
    codes
      .chain(std::iter::repeat(longest))
      .take(count)
      .collect()
  }

  /// A clear code, then `count` valid LZW codes of literal width `lw`: mostly the newest table
  /// codes, whose strings grow long, then other table codes, literals and a few clear codes
  fn random_codes(rng: &mut Rng, lw: u32, count: usize) -> Vec<u32> {
    let (clear, end) = (1 << lw, (1 << lw) + 1);
    let (mut codes, mut save) = (vec![clear], end);
    for _ in 0..count {
      // the codes that may come: literals, and table codes up to the next one (KwKwK)
      let last = save.min(4095);
      let code = match rng.below(1000) {
        _ if save == end || last <= end => rng.below(u64::from(clear)) as u32,
        0 => clear,
        1..800 => last - rng.below(u64::from(last - end).min(3)) as u32,
        800..900 => end + 1 + rng.below(u64::from(last - end)) as u32,
        _ => rng.below(u64::from(clear)) as u32,
      };
      codes.push(code);
      if code == clear {
        save = end;
      } else if save <= 4095 {
        save += 1;
      }
    }
    codes
  }

  /// A frame for `frames_gif`: rect (x, y, w, h), interlaced, transparent index, LZW data of
  /// literal width 2
  type TestFrame<'a> = ([u16; 4], bool, Option<u8>, &'a [u8]);

  /// A GIF89a with a 2-color global table on a `screen`, and these frames
  fn frames_gif(screen: (u16, u16), frames: &[TestFrame]) -> Vec<u8> {
    let mut b = b"GIF89a".to_vec();
    b.extend(screen.0.to_le_bytes());
    b.extend(screen.1.to_le_bytes());
    b.extend([0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    for &(rect, interlaced, transparent, lzw) in frames {
      if let Some(index) = transparent {
        b.extend([0x21, 0xf9, 4, 1, 0, 0, index, 0]);
      }
      b.push(0x2c);
      rect.iter().for_each(|v| b.extend(v.to_le_bytes()));
      b.extend([if interlaced { 0x40 } else { 0 }, 2]);
      for block in lzw.chunks(255) {
        b.push(block.len() as u8);
        b.extend(block);
      }
      b.push(0);
    }
    b.push(0x3b);
    b
  }

  /// `frames_gif` with a 1x1 first frame, so that the canvas is the screen, then `frame`
  fn second_frame_gif(screen: (u16, u16), frame: TestFrame) -> Vec<u8> {
    let dot = pack(2, &[4, 1, 5]);
    frames_gif(screen, &[([0, 0, 1, 1], false, None, &dot), frame])
  }

  /// What `same_as_reference` counted for one canvas, summed over the frames
  #[derive(Clone, Copy, Debug, Default)]
  struct Counts {
    /// steps of `draw` (`work`)
    steps: u64,
    /// indexes `draw` read out of the LZW table (`Output::read_back`)
    indexes: u64,
    /// indexes the reference read: each code's whole string
    read: u64,
    /// indexes the reference put on the canvas
    on: u64,
  }

  /// Draws each frame of `file` with `GifDecoder::draw` and with the reference, from the same
  /// canvas and with no canvas (`check`), and checks that the results and the pixels are the
  /// same. Also checks the indexes `draw` read: on a frame inside the canvas, each code's whole
  /// string (the direct path, like the reference); on any other frame, only the indexes that
  /// land on the canvas. Returns the counts with the canvas, then with none.
  fn same_as_reference(file: &[u8]) -> [Counts; 2] {
    let gif = GifDecoder::new(file).unwrap();
    let size = gif.width() as usize * gif.height() as usize * 4;
    // each pixel different, so that every write shows
    let start: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
    let mut counts = [Counts::default(); 2];
    for (i, frame) in gif.frames().iter().enumerate() {
      for (canvas, counts) in [&start[..], &[]].into_iter().zip(&mut counts) {
        let (mut new, mut old) = (canvas.to_vec(), canvas.to_vec());
        let before = (WORK.get(), INDEXES.get());
        let result = gif.draw(frame, &mut new);
        let (steps, indexes) = (WORK.get() - before.0, INDEXES.get() - before.1);
        let before = (reference::READ.get(), reference::ON.get());
        assert_eq!(result, reference::draw(&gif, frame, &mut old), "frame {i}");
        let (read, on) = (
          reference::READ.get() - before.0,
          reference::ON.get() - before.1,
        );
        assert!(new == old, "frame {i}: other pixels");
        let r = frame.rect;
        let inside =
          !canvas.is_empty() && r.x0 < r.x1 && r.x1 <= gif.width() && r.y1 <= gif.height();
        let expected = if inside { read } else { on };
        assert_eq!(
          indexes, expected,
          "frame {i} {r:?}, inside {inside}: indexes read"
        );
        counts.steps += steps;
        counts.indexes += indexes;
        counts.read += read;
        counts.on += on;
      }
    }
    counts
  }

  #[test]
  fn output_off_the_canvas_is_skipped() {
    // Codex G1: a 1x1 canvas, then a frame of up to 65535x65535 pixels with long LZW strings.
    // Stepping over each index off the canvas took 6.77 s for a 1.58 MiB file.
    let codes = growing(1000, 2000);
    let data = |tail: &[u32]| pack(2, &[&codes[..], tail].concat());
    let cases = [
      // the data ends early, ends with an end code, or with a bad code
      ([0, 0, 65535, 65535], false, data(&[])),
      ([0, 0, 65535, 65535], true, data(&[])),
      ([0, 0, 65535, 65535], false, data(&[5])),
      ([0, 0, 65535, 65535], true, data(&[4, 7])),
      // the frame fills, then a bad code comes
      ([0, 0, 65535, 8], false, data(&[4, 7])),
      ([0, 0, 65535, 8], true, data(&[4, 7])),
      // strings across rows
      ([0, 0, 999, 65535], true, data(&[])),
      ([0, 0, 1, 65535], false, data(&[5])),
      // right of the canvas, below it
      ([1, 0, 65535, 65535], false, data(&[])),
      ([0, 1, 65535, 65535], true, data(&[4, 7])),
    ];
    for (rect, interlaced, lzw) in cases {
      let [canvas, dry] =
        same_as_reference(&second_frame_gif((1, 1), (rect, interlaced, None, &lzw)));
      // Only the indexes that land on the canvas are read out of the LZW table: the dot of frame
      // 0, and the first index of frame 1 when it starts on the canvas. This holds for whatever
      // path writes the output, as each one reads the table through `Output::read_back`.
      let on_canvas = 1 + u64::from(rect[..2] == [0, 0]);
      assert_eq!(
        (canvas.indexes, dry.indexes),
        (on_canvas, 0),
        "{rect:?}: indexes read"
      );
      // the two frames, with and without a canvas: a few steps per code
      let (steps, codes) = (canvas.steps + dry.steps, 2 * (3 + codes.len() as u64 + 2));
      assert!(
        steps <= 4 * codes,
        "{rect:?}: {steps} steps for {codes} codes"
      );
      // the reference read every index while the frame was not full
      let read = canvas.read + dry.read;
      assert!(
        read > 20 * steps,
        "{rect:?}: the reference read {read} indexes"
      );
    }
  }

  #[test]
  fn long_strings_on_a_narrow_canvas_are_read_where_they_land() {
    // a 1x1024 canvas under a frame 2043 pixels wide: one index of each row lands on the canvas,
    // while the codes are up to 2043 indexes long (code 2047)
    let codes = growing(2047, 2100);
    let lzw = pack(2, &codes);
    for interlaced in [false, true] {
      let file = second_frame_gif((1, 1024), ([0, 0, 2043, 1024], interlaced, None, &lzw));
      let [canvas, dry] = same_as_reference(&file);
      let (steps, read) = (canvas.steps + dry.steps, canvas.read + dry.read);
      // codes and rows, with and without a canvas: for each row, one search for a prefix
      // (about 3 log2 of the string length) and one index
      let (codes, rows) = (2 * (3 + codes.len() as u64), 1024);
      assert!(
        steps <= 4 * codes + 30 * rows,
        "{steps} steps for {codes} codes; the reference read {read} indexes"
      );
    }
  }

  #[test]
  fn narrow_frames_on_the_canvas_read_each_string_once() {
    // a frame 1 pixel wide on a 1x65535 canvas: each index of a code is a row of its own, so each
    // string lands on the canvas in as many parts as it has indexes
    let codes = growing(1000, 1100);
    let lzw = pack(2, &codes);
    for interlaced in [false, true] {
      let file = frames_gif((1, 65535), &[([0, 0, 1, 65535], interlaced, None, &lzw)]);
      let [canvas, dry] = same_as_reference(&file);
      let steps = canvas.steps + dry.steps;
      // with and without a canvas: a step per code, and per pixel a step and a byte
      let (codes, pixels) = (codes.len() as u64, 65535);
      assert!(steps <= 2 * codes + 3 * pixels, "{steps} steps");
    }
  }

  #[test]
  fn frames_inside_the_canvas_take_the_direct_path() {
    // On a 6x8 canvas, codes whose strings grow by one (1, 2, 3, ... indexes): they run across
    // rows, and the last one past the end of the frame
    let lzw = pack(2, &growing(44, 60));
    let inside = [
      [0, 0, 6, 8],
      [1, 2, 5, 6],
      [0, 0, 1, 8],
      [0, 3, 5, 1],
      [5, 6, 1, 2],
    ];
    let partly_off = [
      [0, 0, 7, 8],
      [0, 0, 6, 9],
      [1, 2, 6, 6],
      [5, 7, 2, 2],
      [3, 0, 65535, 3],
    ];
    for interlaced in [false, true] {
      for (rects, direct) in [(inside, true), (partly_off, false)] {
        for [x, y, w, h] in rects {
          let rect = Rect {
            x0: x,
            y0: y,
            x1: x + w,
            y1: y + h,
          };
          assert_eq!(is_inside(rect, 6, 8), direct, "{rect:?}");
          // no canvas: a dry run reads no index
          assert!(!is_inside(rect, 0, 0), "{rect:?}");
          let frame = (
            [x as u16, y as u16, w as u16, h as u16],
            interlaced,
            None,
            &lzw[..],
          );
          let [canvas, dry] = same_as_reference(&second_frame_gif((6, 8), frame));
          // the paths read different indexes here: the reference read more than it put on the
          // canvas
          assert!(canvas.read > canvas.on, "{rect:?}: {canvas:?}");
          let expected = if direct { canvas.read } else { canvas.on };
          assert_eq!(canvas.indexes, expected, "{rect:?}: {canvas:?}");
          assert_eq!(dry.indexes, 0, "{rect:?}");
        }
      }
    }
    // the direct path keeps no jump pointers; the other one keeps one per code
    for direct in [true, false] {
      let mut canvas = vec![0; 6 * 8 * 4];
      let mut out = Pixels {
        canvas: &mut canvas,
        width: 6,
        height: 8,
        rect: Rect {
          x0: 0,
          y0: 0,
          x1: 6,
          y1: 8,
        },
        x: 0,
        y: 0,
        interlace: 0,
        palette: &[[0; 4]; 256],
        direct,
        stack: Vec::new(),
        runs: Vec::new(),
      };
      let mut lzw = Lzw::new(2);
      lzw.decode(&lzw_data(30, &[]), &mut out);
      let expected = if direct { 0 } else { lzw.save as usize };
      assert_eq!(lzw.jump.len(), expected, "direct {direct}");
    }
  }

  #[test]
  fn zero_width_frames_take_one_step() {
    // the reference moved each frame 0 pixels wide and 65535 tall down its rows one step at a time
    let lzw = pack(2, &[4, 1, 1, 5]);
    let dot = pack(2, &[4, 1, 5]);
    let mut frames = vec![([0, 0, 1, 1], false, None, &dot[..])];
    for i in 0..100 {
      frames.push(([0, 0, 0, 65535], i % 2 == 0, None, &lzw[..]));
    }
    let [canvas, dry] = same_as_reference(&frames_gif((1, 1), &frames));
    let steps = canvas.steps + dry.steps;
    // per frame and canvas: 2 codes, one put
    assert!(steps <= 2 * 101 * 4, "{steps} steps");
  }

  #[test]
  fn frames_off_the_right_and_bottom_match_the_reference() {
    // Frame rects start at 0 or more, so they leave the canvas only on the right and the bottom.
    // On a 5x7 canvas: frames inside it, across its right edge, its bottom edge or both, and
    // wholly past them, interlaced or not. One pixel per code, stopping at each pixel of each row
    // and pass, then long strings across the edges.
    let tails: [&[u32]; 3] = [&[5], &[4, 7], &[]];
    let mut rng = Rng(0x6f6666);
    for x in [0, 2, 4, 5, 6] {
      for y in [0, 3, 6, 7, 9] {
        for w in [0, 1, 3, 6] {
          for h in [0, 1, 4, 9, 13] {
            for interlaced in [false, true] {
              let rect = [x, y, w, h];
              for pixels in 0..=usize::from(w * h) + 2 {
                for tail in tails {
                  let lzw = lzw_data(pixels, tail);
                  same_as_reference(&second_frame_gif((5, 7), (rect, interlaced, None, &lzw)));
                }
              }
              for _ in 0..8 {
                let count = rng.below(60) as usize;
                let mut codes = random_codes(&mut rng, 2, count);
                codes.extend(tails[rng.below(3) as usize]);
                let lzw = pack(2, &codes);
                let transparent = [None, Some(0), Some(1)][rng.below(3) as usize];
                same_as_reference(&second_frame_gif(
                  (5, 7),
                  (rect, interlaced, transparent, &lzw),
                ));
              }
            }
          }
        }
      }
    }
  }

  fn step(step: LzwStep) -> u8 {
    match step {
      LzwStep::More => 0,
      LzwStep::End => 1,
      LzwStep::BadCode => 2,
    }
  }

  #[test]
  fn pixels_follow_the_reference_after_each_byte() {
    // Random literal widths, canvases (none in a dry run), rects (some up to 65535 pixels wide or
    // tall, or past the canvas), palettes with a transparent index and LZW codes, one byte at a
    // time: the position, the pixels and the LZW result are those of the reference.
    let mut rng = Rng(0x7069);
    for round in 0..20000 {
      let lw = rng.below(9) as u32;
      let (mut width, mut height) = (rng.below(7) as u32, rng.below(7) as u32);
      if width == 0 || height == 0 {
        (width, height) = (0, 0);
      }
      let mut corner = || match rng.below(10) {
        0 => 65535,
        _ => rng.below(9) as u32,
      };
      let (x0, y0) = (corner(), corner());
      let mut size = || match rng.below(10) {
        0 => 65535 - rng.below(2) as u32,
        _ => rng.below(11) as u32,
      };
      let (w, h) = (size(), size());
      let rect = Rect {
        x0,
        y0,
        x1: x0 + w,
        y1: y0 + h,
      };
      let interlace = if rng.below(2) == 0 { 4 } else { 0 };
      // a frame inside the canvas may take either path
      let direct = is_inside(rect, width, height) && rng.below(2) == 0;
      let mut palette = [[0u8; 4]; 256];
      for color in &mut palette {
        *color = [rng.below(256) as u8, rng.below(256) as u8, 0, 255];
      }
      palette[rng.below(4) as usize] = [0; 4];
      let count = rng.below(200) as usize;
      let mut codes = random_codes(&mut rng, lw, count);
      let clear = 1 << lw;
      match rng.below(3) {
        0 => codes.push(clear + 1),
        1 => codes.extend([clear, clear + 3]),
        _ => {}
      }
      let data = pack(lw, &codes);
      let start: Vec<u8> = (0..width * height * 4).map(|i| (i % 251) as u8).collect();
      let (mut new_canvas, mut old_canvas) = (start.clone(), start);
      let pixels = |canvas| Pixels {
        canvas,
        width,
        height,
        rect,
        x: x0,
        y: y0,
        interlace,
        palette: &palette,
        direct,
        stack: Vec::new(),
        runs: Vec::new(),
      };
      let (mut new, mut old) = (pixels(&mut new_canvas[..]), pixels(&mut old_canvas[..]));
      let (mut lzw, mut old_lzw, mut stack) = (Lzw::new(lw), Lzw::new(lw), Vec::new());
      let before = (INDEXES.get(), reference::READ.get(), reference::ON.get());
      for (i, &byte) in data.iter().enumerate() {
        let at = format!("round {round}, byte {i}, {rect:?} on {width}x{height}");
        let result = step(lzw.decode(&[byte], &mut new));
        let old_result = step(reference::decode(
          &mut old_lzw,
          &mut stack,
          &[byte],
          &mut old,
        ));
        assert_eq!(result, old_result, "{at}");
        let position = |p: &Pixels| (p.x, p.y, p.interlace);
        assert_eq!(position(&new), position(&old), "{at}");
        assert!(new.canvas == old.canvas, "{at}");
        if result != 0 {
          break;
        }
      }
      // the direct path reads each code's whole string, the other one the indexes on the canvas
      let indexes = INDEXES.get() - before.0;
      let (read, on) = (
        reference::READ.get() - before.1,
        reference::ON.get() - before.2,
      );
      let expected = if direct { read } else { on };
      assert_eq!(indexes, expected, "round {round}, direct {direct}");
    }
  }

  #[test]
  fn prefix_codes_take_few_steps() {
    // the longest strings (code 4095 of `growing`), and random tables of each literal width
    let mut rng = Rng(0x6a756d70);
    for round in 0..18 {
      let lw = if round == 0 { 2 } else { round % 9 };
      let codes = if round == 0 {
        growing(4095, 4092)
      } else {
        random_codes(&mut rng, lw, 4000)
      };
      let mut lzw = Lzw::new(lw);
      // an empty frame: the codes only fill the table
      let mut out = Pixels {
        canvas: &mut [],
        width: 0,
        height: 0,
        rect: Rect::default(),
        x: 0,
        y: 0,
        interlace: 0,
        palette: &[[0; 4]; 256],
        direct: false,
        stack: Vec::new(),
        runs: Vec::new(),
      };
      lzw.decode(&pack(lw, &codes), &mut out);
      let mut longest = 0;
      for code in (0..lzw.clear).chain(lzw.end + 1..lzw.save.min(4096)) {
        let code = code as usize;
        let len = usize::from(lzw.len[code]);
        longest = longest.max(len);
        let mut lens = vec![1, len];
        lens.extend((0..16).map(|_| 1 + rng.below(len as u64) as usize));
        for n in lens {
          let mut walked = code;
          for _ in n..len {
            walked = usize::from(lzw.prefix[walked]);
          }
          let before = WORK.get();
          assert_eq!(
            lzw.prefix_code(code, n),
            walked,
            "round {round}, code {code}, {n} of {len}"
          );
          // about 3 log2(len) steps
          let steps = WORK.get() - before;
          assert!(
            steps <= 36,
            "round {round}, code {code}, {n} of {len}: {steps} steps"
          );
        }
      }
      if round == 0 {
        assert_eq!(longest, 4091);
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
