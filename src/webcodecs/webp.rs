// libwebp's notice, for the code this file ports from it (the files named below):
// Copyright 2010 Google Inc. All Rights Reserved.
// Copyright 2012 Google Inc. All Rights Reserved.
// Use of this source code is governed by a BSD-style license that can be found in
// licenses/libwebp-COPYING.txt. An additional intellectual property rights grant can be found
// in licenses/libwebp-PATENTS.txt.

//! WebP as Chrome 155 reads and draws it.
//!
//! Chrome reads a WebP with libwebp's demuxer (WebPDemuxPartial, src/demux/demux.c) and draws it
//! with Blink's WEBPImageDecoder. `demux` is a port of the demuxer: it lists the frames of the
//! data so far, the same way, with the same errors. `features` is a port of WebPGetFeatures
//! (src/dec/webp_dec.c, with VP8GetInfo from src/dec/vp8_dec.c and VP8LGetInfo from
//! src/dec/vp8l_dec.c), libwebp b43b2caa, the revision Chromium 155 builds.
//! `start_canvas` and `draw` draw the frames like Blink:
//!
//! - a frame of an animation counts once all of its ANMF chunk is here; a still image counts at
//!   the header of its image chunk (VP8X), or once the image chunk tells its size (VP8, VP8L)
//! - each frame starts from a clear canvas or from the canvas of the frame before it
//!   (ImageDecoder::FindRequiredPreviousFrame), cleared where that frame disposes to the
//!   background, and blends with Blink's integer math (BlendSrcOverDstNonPremultiplied)
//!
//! What Blink does with the demuxer's result (WEBPImageDecoder::UpdateDemuxer) is left to the
//! caller (`image_decoder`), which keeps the state Blink keeps between two reads.

const TAG_SIZE: usize = 4;
const CHUNK_HEADER_SIZE: usize = 8;
const RIFF_HEADER_SIZE: usize = 12;
const ANMF_CHUNK_SIZE: u32 = 16;
const ANIM_CHUNK_SIZE: u32 = 6;
const VP8X_CHUNK_SIZE: u32 = 10;
const VP8_FRAME_HEADER_SIZE: usize = 10;
const VP8L_FRAME_HEADER_SIZE: usize = 5;
const MAX_CHUNK_PAYLOAD: u32 = !0u32 - CHUNK_HEADER_SIZE as u32 - 1;
const MAX_IMAGE_AREA: u64 = 1 << 32;

pub(crate) const ALPHA_FLAG: u32 = 0x10;
pub(crate) const ANIMATION_FLAG: u32 = 0x02;
const ICCP_FLAG: u32 = 0x20;
const EXIF_FLAG: u32 = 0x08;
const XMP_FLAG: u32 = 0x04;
const ALL_VALID_FLAGS: u32 = ALPHA_FLAG | ANIMATION_FLAG | ICCP_FLAG | EXIF_FLAG | XMP_FLAG;

const fn fourcc(tag: &[u8; 4]) -> u32 {
  u32::from_le_bytes(*tag)
}

const ALPH: u32 = fourcc(b"ALPH");
const VP8: u32 = fourcc(b"VP8 ");
const VP8L: u32 = fourcc(b"VP8L");
const VP8X: u32 = fourcc(b"VP8X");
const ANIM: u32 = fourcc(b"ANIM");
const ANMF: u32 = fourcc(b"ANMF");
const ICCP: u32 = fourcc(b"ICCP");
const EXIF: u32 = fourcc(b"EXIF");
const XMP: u32 = fourcc(b"XMP ");

fn le16(b: &[u8]) -> u32 {
  u32::from(b[0]) | u32::from(b[1]) << 8
}

fn le24(b: &[u8]) -> u32 {
  le16(b) | u32::from(b[2]) << 16
}

fn le32(b: &[u8]) -> u32 {
  le24(b) | u32::from(b[3]) << 24
}

/// libwebp's VP8StatusCode, for the codes that matter here
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
  NotEnoughData,
  BitstreamError,
}

/// WebPBitstreamFeatures
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Features {
  pub(crate) width: u32,
  pub(crate) height: u32,
  pub(crate) has_alpha: bool,
  /// 1: lossy, 2: lossless, 0: an animation
  pub(crate) format: u8,
}

/// WebPGetFeatures (src/dec/webp_dec.c GetFeatures, ParseHeadersInternal with no headers)
pub(crate) fn features(data: &[u8]) -> Result<Features, Status> {
  if data.len() < RIFF_HEADER_SIZE {
    return Err(Status::NotEnoughData);
  }
  let mut data = data;
  let mut features = Features::default();
  // ParseRIFF (have_all_data is 0)
  let mut riff_size = 0usize;
  if &data[..TAG_SIZE] == b"RIFF" {
    if &data[8..12] != b"WEBP" {
      return Err(Status::BitstreamError);
    }
    let size = le32(&data[4..]);
    if size < (TAG_SIZE + CHUNK_HEADER_SIZE) as u32 || size > MAX_CHUNK_PAYLOAD {
      return Err(Status::BitstreamError);
    }
    riff_size = size as usize;
    data = &data[RIFF_HEADER_SIZE..];
  }
  let found_riff = riff_size > 0;
  // ParseVP8X
  let mut found_vp8x = false;
  let (mut canvas_width, mut canvas_height) = (0, 0);
  let mut animation = false;
  if data.len() < CHUNK_HEADER_SIZE {
    return Err(Status::NotEnoughData);
  }
  if &data[..TAG_SIZE] == b"VP8X" {
    if le32(&data[4..]) != VP8X_CHUNK_SIZE {
      return Err(Status::BitstreamError);
    }
    if data.len() < CHUNK_HEADER_SIZE + VP8X_CHUNK_SIZE as usize {
      return Err(Status::NotEnoughData);
    }
    let flags = le32(&data[8..]);
    let width = 1 + le24(&data[12..]);
    let height = 1 + le24(&data[15..]);
    if u64::from(width) * u64::from(height) >= MAX_IMAGE_AREA {
      return Err(Status::BitstreamError);
    }
    canvas_width = width;
    canvas_height = height;
    animation = flags & ANIMATION_FLAG != 0;
    features.has_alpha = flags & ALPHA_FLAG != 0;
    data = &data[CHUNK_HEADER_SIZE + VP8X_CHUNK_SIZE as usize..];
    found_vp8x = true;
  }
  if !found_riff && found_vp8x {
    return Err(Status::BitstreamError);
  }
  features.width = canvas_width;
  features.height = canvas_height;
  // with VP8X: a NotEnoughData from here on still returns the VP8X features
  let partial = |features: Features, status: Status| {
    if status == Status::NotEnoughData && found_vp8x {
      Ok(features)
    } else {
      Err(status)
    }
  };
  if found_vp8x && animation {
    return Ok(features);
  }
  if data.len() < TAG_SIZE {
    return partial(features, Status::NotEnoughData);
  }
  // ParseOptionalChunks
  let mut alpha_data = false;
  if (found_riff && found_vp8x) || (!found_riff && !found_vp8x && &data[..TAG_SIZE] == b"ALPH") {
    let mut total_size = (TAG_SIZE + CHUNK_HEADER_SIZE) as u64 + u64::from(VP8X_CHUNK_SIZE);
    loop {
      if data.len() < CHUNK_HEADER_SIZE {
        return partial(features, Status::NotEnoughData);
      }
      let chunk_size = le32(&data[4..]);
      if chunk_size > MAX_CHUNK_PAYLOAD {
        return partial(features, Status::BitstreamError);
      }
      let disk_chunk_size = (CHUNK_HEADER_SIZE as u64 + u64::from(chunk_size) + 1) & !1;
      total_size += disk_chunk_size;
      if riff_size > 0 && total_size > riff_size as u64 {
        return partial(features, Status::BitstreamError);
      }
      if &data[..TAG_SIZE] == b"VP8 " || &data[..TAG_SIZE] == b"VP8L" {
        break;
      }
      if (data.len() as u64) < disk_chunk_size {
        return partial(features, Status::NotEnoughData);
      }
      if &data[..TAG_SIZE] == b"ALPH" {
        alpha_data = true;
        features.has_alpha = true;
      }
      data = &data[disk_chunk_size as usize..];
    }
  }
  // ParseVP8Header (have_all_data is 0)
  if data.len() < CHUNK_HEADER_SIZE {
    return partial(features, Status::NotEnoughData);
  }
  let is_vp8 = &data[..TAG_SIZE] == b"VP8 ";
  let is_vp8l = &data[..TAG_SIZE] == b"VP8L";
  let (compressed_size, lossless) = if is_vp8 || is_vp8l {
    let size = le32(&data[4..]);
    let minimal_size = TAG_SIZE + CHUNK_HEADER_SIZE;
    if riff_size >= minimal_size && size as usize > riff_size - minimal_size {
      return partial(features, Status::BitstreamError);
    }
    data = &data[CHUNK_HEADER_SIZE..];
    (size as usize, is_vp8l)
  } else {
    // a raw VP8 or VP8L bitstream
    (data.len(), vp8l_signature(data))
  };
  if compressed_size as u64 > u64::from(MAX_CHUNK_PAYLOAD) {
    return Err(Status::BitstreamError);
  }
  features.format = if lossless { 2 } else { 1 };
  let (width, height) = if !lossless {
    if data.len() < VP8_FRAME_HEADER_SIZE {
      return partial(features, Status::NotEnoughData);
    }
    vp8_info(data, compressed_size).ok_or(Status::BitstreamError)?
  } else {
    if data.len() < VP8L_FRAME_HEADER_SIZE {
      return partial(features, Status::NotEnoughData);
    }
    let (width, height, alpha) = vp8l_info(data).ok_or(Status::BitstreamError)?;
    features.has_alpha = alpha;
    (width, height)
  };
  if found_vp8x && (canvas_width != width || canvas_height != height) {
    return Err(Status::BitstreamError);
  }
  features.width = width;
  features.height = height;
  features.has_alpha |= alpha_data;
  Ok(features)
}

/// VP8GetInfo (src/dec/vp8_dec.c)
fn vp8_info(data: &[u8], chunk_size: usize) -> Option<(u32, u32)> {
  if data.len() < VP8_FRAME_HEADER_SIZE || data[3..6] != [0x9d, 0x01, 0x2a] {
    return None;
  }
  let bits = u32::from(data[0]) | u32::from(data[1]) << 8 | u32::from(data[2]) << 16;
  let width = (u32::from(data[7]) << 8 | u32::from(data[6])) & 0x3fff;
  let height = (u32::from(data[9]) << 8 | u32::from(data[8])) & 0x3fff;
  // a key frame, a known profile, shown, a first partition that fits, a size
  if bits & 1 != 0
    || (bits >> 1) & 7 > 3
    || (bits >> 4) & 1 == 0
    || (bits >> 5) as usize >= chunk_size
    || width == 0
    || height == 0
  {
    return None;
  }
  Some((width, height))
}

/// VP8LCheckSignature (src/dec/vp8l_dec.c)
fn vp8l_signature(data: &[u8]) -> bool {
  data.len() >= VP8L_FRAME_HEADER_SIZE && data[0] == 0x2f && data[4] >> 5 == 0
}

/// VP8LGetInfo (src/dec/vp8l_dec.c): after the signature byte, 14 bits of width - 1, 14 bits of
/// height - 1, the alpha bit and 3 bits of version 0
fn vp8l_info(data: &[u8]) -> Option<(u32, u32, bool)> {
  if !vp8l_signature(data) {
    return None;
  }
  let bits = le32(&data[1..]);
  let width = (bits & 0x3fff) + 1;
  let height = ((bits >> 14) & 0x3fff) + 1;
  let alpha = (bits >> 28) & 1 != 0;
  Some((width, height, alpha))
}

/// libwebp's WebPDemuxState
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DemuxState {
  /// WEBP_DEMUX_PARSE_ERROR
  ParseError,
  /// WEBP_DEMUX_PARSING_HEADER
  ParsingHeader,
  /// WEBP_DEMUX_PARSED_HEADER
  ParsedHeader,
  /// WEBP_DEMUX_DONE
  Done,
}

/// A part of the data: offset and size
pub(crate) type Span = (usize, usize);

/// libwebp's Frame
#[derive(Clone, Debug, Default)]
pub(crate) struct DemuxFrame {
  pub(crate) x_offset: u32,
  pub(crate) y_offset: u32,
  pub(crate) width: u32,
  pub(crate) height: u32,
  pub(crate) has_alpha: bool,
  pub(crate) duration: u32,
  pub(crate) dispose_background: bool,
  pub(crate) no_blend: bool,
  pub(crate) frame_num: u32,
  pub(crate) complete: bool,
  /// The image chunk (VP8 or VP8L): its header and the part of its padded payload that is here
  pub(crate) image: Span,
  /// The ALPH chunk, the same way
  pub(crate) alpha: Span,
}

/// libwebp's WebPDemuxer
#[derive(Clone, Debug)]
pub(crate) struct Demux {
  pub(crate) state: DemuxState,
  is_ext_format: bool,
  pub(crate) flags: u32,
  pub(crate) canvas_width: i64,
  pub(crate) canvas_height: i64,
  pub(crate) loop_count: u32,
  pub(crate) frames: Vec<DemuxFrame>,
  /// No RIFF header: a raw VP8 or VP8L bitstream (CreateRawImageDemuxer)
  pub(crate) raw: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parse {
  Ok,
  NeedMore,
  Error,
}

struct Mem<'a> {
  buf: &'a [u8],
  start: usize,
  end: usize,
  riff_end: usize,
}

impl Mem<'_> {
  fn data_size(&self) -> usize {
    self.end - self.start
  }

  fn size_is_invalid(&self, size: u64) -> bool {
    size > (self.riff_end - self.start) as u64
  }

  fn read_le32(&mut self) -> u32 {
    let value = le32(&self.buf[self.start..]);
    self.start += 4;
    value
  }

  fn read_le24(&mut self) -> u32 {
    let value = le24(&self.buf[self.start..]);
    self.start += 3;
    value
  }
}

/// WebPDemuxPartial: None for libwebp's NULL, with the state it reports
pub(crate) fn demux(data: &[u8]) -> (Option<Demux>, DemuxState) {
  if data.is_empty() {
    return (None, DemuxState::ParseError);
  }
  let mut mem = Mem {
    buf: data,
    start: 0,
    end: data.len(),
    riff_end: 0,
  };
  // ReadHeader
  let header = if data.len() < RIFF_HEADER_SIZE + CHUNK_HEADER_SIZE {
    Parse::NeedMore
  } else if &data[..4] != b"RIFF" || &data[8..12] != b"WEBP" {
    Parse::Error
  } else {
    let riff_size = le32(&data[4..]);
    if riff_size < CHUNK_HEADER_SIZE as u32 || riff_size > MAX_CHUNK_PAYLOAD {
      Parse::Error
    } else {
      mem.riff_end = riff_size as usize + CHUNK_HEADER_SIZE;
      if mem.end > mem.riff_end {
        mem.end = mem.riff_end;
      }
      mem.start = RIFF_HEADER_SIZE;
      Parse::Ok
    }
  };
  match header {
    Parse::Ok => {}
    // a raw VP8 or VP8L bitstream (CreateRawImageDemuxer)
    Parse::Error => {
      return match features(data) {
        Ok(f) => {
          let frame = DemuxFrame {
            width: f.width,
            height: f.height,
            has_alpha: f.has_alpha,
            frame_num: 1,
            complete: true,
            image: (0, data.len()),
            ..DemuxFrame::default()
          };
          let demux = Demux {
            state: DemuxState::Done,
            is_ext_format: false,
            flags: if f.has_alpha { ALPHA_FLAG } else { 0 },
            canvas_width: i64::from(f.width),
            canvas_height: i64::from(f.height),
            loop_count: 1,
            frames: vec![frame],
            raw: true,
          };
          (Some(demux), DemuxState::Done)
        }
        Err(Status::NotEnoughData) => (None, DemuxState::ParsingHeader),
        Err(Status::BitstreamError) => (None, DemuxState::ParseError),
      };
    }
    Parse::NeedMore => return (None, DemuxState::ParsingHeader),
  }
  let partial = mem.end < mem.riff_end;
  let mut demux = Demux {
    state: DemuxState::ParsingHeader,
    is_ext_format: false,
    flags: 0,
    canvas_width: -1,
    canvas_height: -1,
    loop_count: 1,
    frames: Vec::new(),
    raw: false,
  };
  let tag = le32(&data[mem.start..]);
  let (mut status, simple) = match tag {
    VP8 | VP8L => (parse_single_image(&mut demux, &mut mem), true),
    VP8X => (parse_vp8x(&mut demux, &mut mem), false),
    _ => (Parse::Error, true),
  };
  if status == Parse::Ok {
    demux.state = DemuxState::Done;
  }
  if status == Parse::NeedMore && !partial {
    status = Parse::Error;
  }
  if status != Parse::Error {
    let valid = if simple {
      valid_simple_format(&demux)
    } else {
      valid_extended_format(&demux)
    };
    if !valid {
      status = Parse::Error;
    }
  }
  if status == Parse::Error {
    return (None, DemuxState::ParseError);
  }
  let state = demux.state;
  (Some(demux), state)
}

/// AddFrame: the frame before must be complete
fn add_frame(demux: &mut Demux, frame: DemuxFrame) -> bool {
  if demux.frames.last().is_some_and(|last| !last.complete) {
    return false;
  }
  demux.frames.push(frame);
  true
}

/// StoreFrame
fn store_frame(frame_num: u32, min_size: u32, mem: &mut Mem, frame: &mut DemuxFrame) -> Parse {
  let mut alpha_chunks = 0;
  let mut image_chunks = 0;
  let mut done = mem.data_size() < CHUNK_HEADER_SIZE || mem.data_size() < min_size as usize;
  let mut status = Parse::Ok;
  if done {
    return Parse::NeedMore;
  }
  loop {
    let chunk_start = mem.start;
    let tag = mem.read_le32();
    let payload_size = mem.read_le32();
    if payload_size > MAX_CHUNK_PAYLOAD {
      return Parse::Error;
    }
    let padded = u64::from(payload_size + (payload_size & 1));
    let available = padded.min(mem.data_size() as u64) as usize;
    let chunk_size = CHUNK_HEADER_SIZE + available;
    if mem.size_is_invalid(padded) {
      return Parse::Error;
    }
    if padded > mem.data_size() as u64 {
      status = Parse::NeedMore;
    }
    let mut leave = false;
    match tag {
      ALPH if alpha_chunks == 0 => {
        alpha_chunks += 1;
        frame.alpha = (chunk_start, chunk_size);
        frame.has_alpha = true;
        frame.frame_num = frame_num;
        mem.start += available;
      }
      VP8L if alpha_chunks > 0 => return Parse::Error,
      VP8 | VP8L if image_chunks == 0 => {
        match features(&mem.buf[chunk_start..chunk_start + chunk_size]) {
          Err(Status::NotEnoughData) if status == Parse::NeedMore => return Parse::NeedMore,
          Err(_) => return Parse::Error,
          Ok(f) => {
            image_chunks += 1;
            frame.image = (chunk_start, chunk_size);
            frame.width = f.width;
            frame.height = f.height;
            frame.has_alpha |= f.has_alpha;
            frame.frame_num = frame_num;
            frame.complete = status == Parse::Ok;
            mem.start += available;
          }
        }
      }
      _ => leave = true,
    }
    if leave {
      // the chunk belongs to the level above
      mem.start -= CHUNK_HEADER_SIZE;
      done = true;
    }
    if mem.start == mem.riff_end {
      done = true;
    } else if mem.data_size() < CHUNK_HEADER_SIZE {
      status = Parse::NeedMore;
    }
    if done || status != Parse::Ok {
      return status;
    }
  }
}

/// ParseAnimationFrame
fn parse_animation_frame(demux: &mut Demux, mem: &mut Mem, frame_chunk_size: u32) -> Parse {
  let is_animation = demux.flags & ANIMATION_FLAG != 0;
  // NewFrame
  if mem.size_is_invalid(u64::from(ANMF_CHUNK_SIZE)) || frame_chunk_size < ANMF_CHUNK_SIZE {
    return Parse::Error;
  }
  if mem.data_size() < ANMF_CHUNK_SIZE as usize {
    return Parse::NeedMore;
  }
  let mut frame = DemuxFrame {
    x_offset: 2 * mem.read_le24(),
    y_offset: 2 * mem.read_le24(),
    width: 1 + mem.read_le24(),
    height: 1 + mem.read_le24(),
    duration: mem.read_le24(),
    ..DemuxFrame::default()
  };
  let bits = mem.buf[mem.start];
  mem.start += 1;
  frame.dispose_background = bits & 1 != 0;
  frame.no_blend = bits & 2 != 0;
  if u64::from(frame.width) * u64::from(frame.height) >= MAX_IMAGE_AREA {
    return Parse::Error;
  }
  let start = mem.start;
  let payload_size = frame_chunk_size - ANMF_CHUNK_SIZE;
  let mut status = store_frame(demux.frames.len() as u32 + 1, payload_size, mem, &mut frame);
  if status != Parse::Error && mem.start - start > payload_size as usize {
    status = Parse::Error;
  }
  if status != Parse::Error && is_animation && frame.frame_num > 0 && !add_frame(demux, frame) {
    status = Parse::Error;
  }
  status
}

/// ParseSingleImage
fn parse_single_image(demux: &mut Demux, mem: &mut Mem) -> Parse {
  if !demux.frames.is_empty() || mem.size_is_invalid(CHUNK_HEADER_SIZE as u64) {
    return Parse::Error;
  }
  if mem.data_size() < CHUNK_HEADER_SIZE {
    return Parse::NeedMore;
  }
  let mut frame = DemuxFrame::default();
  let mut status = store_frame(1, 0, mem, &mut frame);
  if status != Parse::Error {
    // an ALPH chunk without the alpha flag is not used
    if demux.flags & ALPHA_FLAG == 0 && frame.alpha.1 > 0 {
      frame.alpha = (0, 0);
      frame.has_alpha = false;
    }
    if !demux.is_ext_format && frame.width > 0 && frame.height > 0 {
      demux.state = DemuxState::ParsedHeader;
      demux.canvas_width = i64::from(frame.width);
      demux.canvas_height = i64::from(frame.height);
      if frame.has_alpha {
        demux.flags |= ALPHA_FLAG;
      }
    }
    if !add_frame(demux, frame) {
      status = Parse::Error;
    }
  }
  status
}

/// ParseVP8XChunks
fn parse_vp8x_chunks(demux: &mut Demux, mem: &mut Mem) -> Parse {
  let is_animation = demux.flags & ANIMATION_FLAG != 0;
  let mut anim_chunks = 0;
  let mut status;
  loop {
    let tag = mem.read_le32();
    let chunk_size = mem.read_le32();
    if chunk_size > MAX_CHUNK_PAYLOAD {
      return Parse::Error;
    }
    let padded = chunk_size + (chunk_size & 1);
    if mem.size_is_invalid(u64::from(padded)) {
      return Parse::Error;
    }
    status = Parse::Ok;
    let mut skip = false;
    match tag {
      VP8X => return Parse::Error,
      ALPH | VP8 | VP8L => {
        // an image outside ANMF chunks: not in an animation
        if anim_chunks > 0 || is_animation {
          return Parse::Error;
        }
        mem.start -= CHUNK_HEADER_SIZE;
        status = parse_single_image(demux, mem);
      }
      ANIM => {
        if padded < ANIM_CHUNK_SIZE {
          return Parse::Error;
        }
        if (mem.data_size() as u64) < u64::from(padded) {
          status = Parse::NeedMore;
        } else if anim_chunks == 0 {
          anim_chunks += 1;
          mem.start += 4; // the background color
          demux.loop_count = le16(&mem.buf[mem.start..]);
          mem.start += 2;
          mem.start += (padded - ANIM_CHUNK_SIZE) as usize;
        } else {
          skip = true;
        }
      }
      ANMF => {
        if anim_chunks == 0 {
          return Parse::Error;
        }
        status = parse_animation_frame(demux, mem, padded);
      }
      ICCP | EXIF | XMP => skip = true,
      _ => skip = true,
    }
    if skip {
      if padded as usize <= mem.data_size() {
        mem.start += padded as usize;
      } else {
        status = Parse::NeedMore;
      }
    }
    if mem.start == mem.riff_end {
      return status;
    } else if mem.data_size() < CHUNK_HEADER_SIZE {
      status = Parse::NeedMore;
    }
    if status != Parse::Ok {
      return status;
    }
  }
}

/// ParseVP8X
fn parse_vp8x(demux: &mut Demux, mem: &mut Mem) -> Parse {
  if mem.data_size() < CHUNK_HEADER_SIZE {
    return Parse::NeedMore;
  }
  demux.is_ext_format = true;
  mem.start += TAG_SIZE;
  let size = mem.read_le32();
  if !(VP8X_CHUNK_SIZE..=MAX_CHUNK_PAYLOAD).contains(&size) {
    return Parse::Error;
  }
  let size = size + (size & 1);
  if mem.size_is_invalid(u64::from(size)) {
    return Parse::Error;
  }
  if (mem.data_size() as u64) < u64::from(size) {
    return Parse::NeedMore;
  }
  demux.flags = u32::from(mem.buf[mem.start]);
  mem.start += 4;
  demux.canvas_width = i64::from(1 + mem.read_le24());
  demux.canvas_height = i64::from(1 + mem.read_le24());
  if demux.canvas_width as u64 * demux.canvas_height as u64 >= MAX_IMAGE_AREA {
    return Parse::Error;
  }
  mem.start += (size - VP8X_CHUNK_SIZE) as usize;
  demux.state = DemuxState::ParsedHeader;
  if mem.size_is_invalid(CHUNK_HEADER_SIZE as u64) {
    return Parse::Error;
  }
  if mem.data_size() < CHUNK_HEADER_SIZE {
    return Parse::NeedMore;
  }
  parse_vp8x_chunks(demux, mem)
}

/// IsValidSimpleFormat
fn valid_simple_format(demux: &Demux) -> bool {
  if demux.state == DemuxState::ParsingHeader {
    return true;
  }
  if demux.canvas_width <= 0 || demux.canvas_height <= 0 {
    return false;
  }
  let Some(frame) = demux.frames.first() else {
    return demux.state != DemuxState::Done;
  };
  frame.width > 0 && frame.height > 0
}

/// IsValidExtendedFormat
fn valid_extended_format(demux: &Demux) -> bool {
  let is_animation = demux.flags & ANIMATION_FLAG != 0;
  if demux.state == DemuxState::ParsingHeader {
    return true;
  }
  if demux.canvas_width <= 0 || demux.canvas_height <= 0 {
    return false;
  }
  if demux.state == DemuxState::Done && demux.frames.is_empty() {
    return false;
  }
  if demux.flags & !ALL_VALID_FLAGS != 0 {
    return false;
  }
  let count = demux.frames.len();
  for (i, f) in demux.frames.iter().enumerate() {
    if !is_animation && f.frame_num > 1 {
      return false;
    }
    if f.complete {
      if f.alpha.1 == 0 && f.image.1 == 0 {
        return false;
      }
      if f.alpha.1 > 0 && f.alpha.0 > f.image.0 {
        return false;
      }
      if f.width == 0 || f.height == 0 {
        return false;
      }
    } else {
      if demux.state == DemuxState::Done {
        return false;
      }
      if f.alpha.1 > 0 && f.image.1 > 0 && f.alpha.0 > f.image.0 {
        return false;
      }
      if i + 1 < count {
        return false;
      }
    }
    if f.width > 0 && f.height > 0 {
      let (w, h) = (demux.canvas_width, demux.canvas_height);
      let fits = if !is_animation {
        f.x_offset == 0 && f.y_offset == 0 && i64::from(f.width) == w && i64::from(f.height) == h
      } else {
        i64::from(f.width) + i64::from(f.x_offset) <= w
          && i64::from(f.height) + i64::from(f.y_offset) <= h
      };
      if !fits {
        return false;
      }
    }
  }
  true
}

/// Rect with exclusive right and bottom edges
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
  pub(crate) x0: u32,
  pub(crate) y0: u32,
  pub(crate) x1: u32,
  pub(crate) y1: u32,
}

impl Rect {
  /// gfx::Rect::Contains
  fn contains(self, r: Rect) -> bool {
    self.x0 <= r.x0 && self.y0 <= r.y0 && self.x1 >= r.x1 && self.y1 >= r.y1
  }
}

/// A frame of an animation, as Blink keeps it (WEBPImageDecoder::InitializeNewFrame)
#[derive(Clone, Debug)]
pub(crate) struct AnimFrame {
  pub(crate) rect: Rect,
  pub(crate) duration_ms: u32,
  pub(crate) dispose_background: bool,
  pub(crate) blend: bool,
  /// The frame this one is drawn on (FindRequiredPreviousFrame): the one before it, or None
  pub(crate) required: Option<usize>,
}

/// Blink's view of the animation frames: rect, timing, dispose and blend, and required frame
pub(crate) fn anim_frames(demux: &Demux) -> Vec<AnimFrame> {
  let canvas = Rect {
    x0: 0,
    y0: 0,
    x1: demux.canvas_width.max(0) as u32,
    y1: demux.canvas_height.max(0) as u32,
  };
  let mut frames: Vec<AnimFrame> = Vec::with_capacity(demux.frames.len());
  for (i, f) in demux.frames.iter().enumerate() {
    let rect = Rect {
      x0: f.x_offset,
      y0: f.y_offset,
      x1: f.x_offset + f.width,
      y1: f.y_offset + f.height,
    };
    let blend = !f.no_blend;
    // ImageDecoder::FindRequiredPreviousFrame (WebP frames dispose to keep or background)
    let required = if i == 0 || ((!f.has_alpha || !blend) && rect.contains(canvas)) {
      None
    } else {
      let prev = &frames[i - 1];
      if !prev.dispose_background {
        Some(i - 1)
      } else if prev.rect.contains(canvas) || prev.required.is_none() {
        None
      } else {
        Some(i - 1)
      }
    };
    frames.push(AnimFrame {
      rect,
      duration_ms: f.duration,
      dispose_background: f.dispose_background,
      blend,
      required,
    });
  }
  frames
}

/// Whether Blink labels each frame BGRA: the alpha flag, or a background with alpha
/// (WEBPImageDecoder::OnInitFrameBuffer, DecodeSingleFrame)
pub(crate) fn frame_alpha(frames: &[AnimFrame], flags: u32, width: u32, height: u32) -> Vec<bool> {
  let canvas = Rect {
    x0: 0,
    y0: 0,
    x1: width,
    y1: height,
  };
  let mut alpha: Vec<bool> = Vec::with_capacity(frames.len());
  for frame in frames {
    let background = match frame.required {
      None => !frame.rect.contains(canvas),
      Some(r) => alpha[r] || frames[r].dispose_background,
    };
    alpha.push(flags & ALPHA_FLAG != 0 || background);
  }
  alpha
}

/// The canvas frame `i` starts from (ImageDecoder::InitFrameBuffer): clear, or the canvas of
/// the frame it requires, cleared in that frame's rect when it disposes to the background
pub(crate) fn start_canvas(
  frames: &[AnimFrame],
  i: usize,
  width: u32,
  height: u32,
  required_canvas: Option<&[u8]>,
) -> Vec<u8> {
  let (Some(r), Some(base)) = (frames[i].required, required_canvas) else {
    return vec![0; width as usize * height as usize * 4];
  };
  let mut canvas = base.to_vec();
  let prev = &frames[r];
  if prev.dispose_background {
    let stride = width as usize * 4;
    for y in prev.rect.y0..prev.rect.y1 {
      let row = y as usize * stride;
      canvas[row + prev.rect.x0 as usize * 4..row + prev.rect.x1 as usize * 4].fill(0);
    }
  }
  canvas
}

/// Draw frame `i` (its BGRA rows `pixels`, `stride` bytes apart) on `canvas`, which starts as
/// `start_canvas` gave it, like libwebp writes it and Blink blends it
/// (WEBPImageDecoder::ApplyPostProcessing). `prev_canvas`: the canvas of frame `i - 1`.
pub(crate) fn draw(
  frames: &[AnimFrame],
  i: usize,
  width: u32,
  canvas: &mut [u8],
  pixels: &[u8],
  stride: usize,
  prev_canvas: Option<&[u8]>,
) {
  let frame = &frames[i];
  let r = frame.rect;
  let canvas_stride = width as usize * 4;
  let row_bytes = (r.x1 - r.x0) as usize * 4;
  for y in 0..(r.y1 - r.y0) as usize {
    let at = (r.y0 as usize + y) * canvas_stride + r.x0 as usize * 4;
    canvas[at..at + row_bytes].copy_from_slice(&pixels[y * stride..y * stride + row_bytes]);
  }
  let (true, Some(_), Some(prev_canvas)) = (i > 0 && frame.blend, frame.required, prev_canvas)
  else {
    return;
  };
  let prev = &frames[i - 1];
  let blend_range = |canvas: &mut [u8], y: u32, left: u32, count: u32| {
    let row = y as usize * canvas_stride;
    for x in left..left + count {
      let at = row + x as usize * 4;
      if canvas[at + 3] != 0xff {
        let src = [canvas[at], canvas[at + 1], canvas[at + 2], canvas[at + 3]];
        let dst = [
          prev_canvas[at],
          prev_canvas[at + 1],
          prev_canvas[at + 2],
          prev_canvas[at + 3],
        ];
        canvas[at..at + 4].copy_from_slice(&blend_non_premultiplied(src, dst));
      }
    }
  };
  for y in r.y0..r.y1 {
    if !prev.dispose_background {
      blend_range(canvas, y, r.x0, r.x1 - r.x0);
      continue;
    }
    // findBlendRangeAtRow: the parts of the row outside the frame before
    let p = prev.rect;
    if y < p.y0 || y >= p.y1 || r.x0 >= p.x1 || r.x1 <= p.x0 {
      blend_range(canvas, y, r.x0, r.x1 - r.x0);
      continue;
    }
    if r.x0 < p.x0 {
      blend_range(canvas, y, r.x0, p.x0 - r.x0);
    }
    if r.x1 > p.x1 {
      blend_range(canvas, y, p.x1, r.x1 - p.x1);
    }
  }
}

/// Blink's BlendSrcOverDstNonPremultiplied for one BGRA pixel (image_frame.cc), in its unsigned
/// 32-bit math
pub(crate) fn blend_non_premultiplied(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
  let src_a = u32::from(src[3]);
  if src_a == 0 {
    return dst;
  }
  let dst_a = u32::from(dst[3]);
  let dst_factor_a = (dst_a * (256 - src_a)) >> 8;
  let blend_a = src_a + dst_factor_a;
  let scale = (1u32 << 24) / blend_a;
  let channel = |s: u8, d: u8| {
    let unscaled = u32::from(s) * src_a + u32::from(d) * dst_factor_a;
    (unscaled.wrapping_mul(scale) >> 24) as u8
  };
  [
    channel(src[0], dst[0]),
    channel(src[1], dst[1]),
    channel(src[2], dst[2]),
    blend_a as u8,
  ]
}

/// The payload size a chunk header at `at` declares
fn payload_size(data: &[u8], at: usize) -> usize {
  le32(&data[at + 4..]) as usize
}

/// Append the chunk at `at`, with its pad byte (FFmpeg skips a chunk of odd size whose pad byte
/// is missing)
fn push_chunk(out: &mut Vec<u8>, data: &[u8], at: usize) {
  let size = payload_size(data, at);
  out.extend_from_slice(&data[at..at + CHUNK_HEADER_SIZE + size]);
  if size & 1 == 1 {
    out.push(0);
  }
}

fn riff(mut body: Vec<u8>) -> Vec<u8> {
  let size = (body.len() - 8) as u32;
  body[4..8].copy_from_slice(&size.to_le_bytes());
  body
}

/// A frame of an animation as a still WebP of its own, for FFmpeg: its ALPH and VP8 chunks
/// behind a VP8X header with the alpha flag, or its VP8L or VP8 chunk alone
pub(crate) fn frame_still(data: &[u8], frame: &DemuxFrame) -> Vec<u8> {
  let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
  if frame.alpha.1 > 0 {
    out.extend_from_slice(b"VP8X");
    out.extend_from_slice(&VP8X_CHUNK_SIZE.to_le_bytes());
    out.extend_from_slice(&[ALPHA_FLAG as u8, 0, 0, 0]);
    out.extend_from_slice(&(frame.width - 1).to_le_bytes()[..3]);
    out.extend_from_slice(&(frame.height - 1).to_le_bytes()[..3]);
    push_chunk(&mut out, data, frame.alpha.0);
  }
  push_chunk(&mut out, data, frame.image.0);
  riff(out)
}

/// A still image for FFmpeg: the data up to the end of its image chunk, with the RIFF size of
/// that (FFmpeg fails a RIFF size past the data). A raw bitstream gets a RIFF header.
pub(crate) fn image_still(data: &[u8], demux: &Demux) -> Vec<u8> {
  let frame = &demux.frames[0];
  if demux.raw {
    return raw_still(data, frame);
  }
  let mut out = data[..frame.image.0].to_vec();
  push_chunk(&mut out, data, frame.image.0);
  riff(out)
}

/// Where the chunks of data with no RIFF header are, as libwebp reads them (CreateRawImageDemuxer,
/// then ParseHeadersInternal): like `features`, the ALPH chunks when the data starts with one
/// (ParseOptionalChunks keeps the last), then the header of a VP8 or VP8L chunk (ParseVP8Header).
/// No image chunk header: a bare bitstream. Offsets of chunk headers that are here.
struct RawChunks {
  alpha: Option<usize>,
  image: Option<usize>,
}

fn raw_chunks(data: &[u8]) -> RawChunks {
  // The bytes from `at` on; a chunk size is not checked here, so `at` can pass the end
  let left = |at: usize| data.len().saturating_sub(at);
  let tag = |at: usize| (left(at) >= TAG_SIZE).then(|| le32(&data[at..]));
  let mut alpha = None;
  let mut at = 0;
  if tag(0) == Some(ALPH) {
    while left(at) >= CHUNK_HEADER_SIZE && !matches!(tag(at), Some(VP8 | VP8L)) {
      if tag(at) == Some(ALPH) {
        alpha = Some(at);
      }
      let size = payload_size(data, at);
      at = at
        .saturating_add(CHUNK_HEADER_SIZE)
        .saturating_add(size)
        .saturating_add(size & 1);
    }
  }
  let image = (matches!(tag(at), Some(VP8 | VP8L)) && left(at) >= CHUNK_HEADER_SIZE).then_some(at);
  RawChunks { alpha, image }
}

/// The data that libwebp decodes without a RIFF header as a still WebP for FFmpeg (see
/// `raw_chunks`): the last ALPH chunk and the VP8 or VP8L chunk, or a bare bitstream. A chunk
/// keeps its own size, and its bytes that are here: a chunk cut short fails in FFmpeg, as in
/// libwebp. A stream waits for the whole chunk (`image_complete`).
fn raw_still(data: &[u8], frame: &DemuxFrame) -> Vec<u8> {
  let RawChunks { alpha, image } = raw_chunks(data);
  let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
  let Some(at) = image else {
    let lossless = vp8l_signature(data);
    out.extend_from_slice(if lossless { b"VP8L" } else { b"VP8 " });
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() & 1 == 1 {
      out.push(0);
    }
    return riff(out);
  };
  if let Some(alpha) = alpha {
    out.extend_from_slice(b"VP8X");
    out.extend_from_slice(&VP8X_CHUNK_SIZE.to_le_bytes());
    out.extend_from_slice(&[ALPHA_FLAG as u8, 0, 0, 0]);
    out.extend_from_slice(&frame.width.saturating_sub(1).to_le_bytes()[..3]);
    out.extend_from_slice(&frame.height.saturating_sub(1).to_le_bytes()[..3]);
    push_chunk_here(&mut out, data, alpha);
  }
  push_chunk_here(&mut out, data, at);
  riff(out)
}

/// `push_chunk` for a chunk whose header is here but whose payload may end past the data: only
/// its bytes that are here
fn push_chunk_here(out: &mut Vec<u8>, data: &[u8], at: usize) {
  let size = payload_size(data, at);
  let whole = CHUNK_HEADER_SIZE.saturating_add(size);
  let end = data.len().min(at.saturating_add(whole));
  out.extend_from_slice(&data[at..end]);
  if size & 1 == 1 && end - at == whole {
    out.push(0);
  }
}

/// Whether a still image's frame can be decoded with the data so far: its image chunk's payload
/// is here (libwebp's incremental decoder may finish a VP8 frame a little earlier). Data with no
/// RIFF header too, from the size in its image chunk header. A bare bitstream has no size, and
/// counts as whole as soon as its header is here: on an open stream its frame is decoded from the
/// bytes read so far, and that frame (or failure) is kept when more bytes come. Chrome waits for
/// the rest (the skill text lists this).
pub(crate) fn image_complete(data: &[u8], demux: &Demux) -> bool {
  let Some(frame) = demux.frames.first() else {
    return false;
  };
  if demux.raw {
    return raw_chunks(data).image.is_none_or(|at| {
      (data.len() - at - CHUNK_HEADER_SIZE) as u64 >= u64::from(le32(&data[at + 4..]))
    });
  }
  frame.image.1 >= CHUNK_HEADER_SIZE
    && frame.image.1 - CHUNK_HEADER_SIZE >= payload_size(data, frame.image.0)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn chunk(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() & 1 == 1 {
      out.push(0);
    }
    out
  }

  fn file(chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"RIFF\0\0\0\0WEBP".to_vec();
    chunks.iter().for_each(|c| out.extend_from_slice(c));
    riff(out)
  }

  /// A VP8L payload of this size: the signature, the size and alpha bit, then filler bytes
  fn vp8l(width: u32, height: u32, alpha: bool, extra: usize) -> Vec<u8> {
    let bits = (width - 1) | (height - 1) << 14 | u32::from(alpha) << 28;
    let mut out = vec![0x2f];
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend(std::iter::repeat_n(0u8, extra));
    out
  }

  /// A VP8 key frame header of this size with a first partition of 1 byte, then filler bytes
  fn vp8(width: u16, height: u16, extra: usize) -> Vec<u8> {
    let bits: u32 = 1 << 4 | 1 << 5;
    let mut out = bits.to_le_bytes()[..3].to_vec();
    out.extend_from_slice(&[0x9d, 0x01, 0x2a]);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend(std::iter::repeat_n(0u8, extra));
    out
  }

  fn vp8x(flags: u8, width: u32, height: u32) -> Vec<u8> {
    let mut p = vec![flags, 0, 0, 0];
    p.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    p.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
    chunk(b"VP8X", &p)
  }

  fn anmf(x: u32, y: u32, w: u32, h: u32, duration: u32, bits: u8, body: &[Vec<u8>]) -> Vec<u8> {
    let mut p = Vec::new();
    for v in [x / 2, y / 2, w - 1, h - 1, duration] {
      p.extend_from_slice(&v.to_le_bytes()[..3]);
    }
    p.push(bits);
    body.iter().for_each(|c| p.extend_from_slice(c));
    chunk(b"ANMF", &p)
  }

  fn anim(loop_count: u16) -> Vec<u8> {
    let mut p = vec![0, 0, 0, 0];
    p.extend_from_slice(&loop_count.to_le_bytes());
    chunk(b"ANIM", &p)
  }

  #[test]
  fn reads_features_like_webp_get_features() {
    let lossless = chunk(b"VP8L", &vp8l(7, 5, true, 3));
    assert_eq!(
      features(&lossless),
      Ok(Features {
        width: 7,
        height: 5,
        has_alpha: true,
        format: 2
      })
    );
    // 12 bytes at least, then the 5 bytes of the VP8L header
    assert_eq!(features(&lossless[..11]), Err(Status::NotEnoughData));
    assert_eq!(features(&lossless[..12]), Err(Status::NotEnoughData));
    assert!(features(&lossless[..13]).is_ok());
    let lossy = chunk(b"VP8 ", &vp8(9, 4, 2));
    assert_eq!(
      features(&lossy).map(|f| (f.width, f.height, f.format)),
      Ok((9, 4, 1))
    );
    assert_eq!(features(&lossy[..17]), Err(Status::NotEnoughData));
    // not a key frame
    let mut bad = lossy.clone();
    bad[8] |= 1;
    assert_eq!(features(&bad), Err(Status::BitstreamError));
    // a VP8X whose canvas is not the image's size
    let file = file(&[vp8x(0, 9, 5), lossy.clone()]);
    assert_eq!(features(&file), Err(Status::BitstreamError));
    // VP8X with data that ends early: the VP8X features
    let file = super::tests::file(&[vp8x(0x10, 9, 4), lossy]);
    assert_eq!(
      features(&file[..32]).map(|f| (f.width, f.height, f.has_alpha)),
      Ok((9, 4, true))
    );
  }

  #[test]
  fn a_simple_image_has_its_header_once_its_size_is_known() {
    let data = file(&[chunk(b"VP8L", &vp8l(7, 5, false, 30))]);
    for cut in 0..24 {
      let (demux, _) = demux(&data[..cut]);
      assert!(
        demux.is_none_or(|d| d.state < DemuxState::ParsedHeader),
        "cut {cut}"
      );
    }
    let (demux, state) = super::demux(&data[..25]);
    let demux = demux.unwrap();
    assert_eq!(state, DemuxState::ParsedHeader);
    assert_eq!((demux.canvas_width, demux.canvas_height), (7, 5));
    assert_eq!(demux.frames.len(), 1);
    assert!(!demux.frames[0].complete);
    assert!(!image_complete(&data[..25], &demux));
    let (demux, state) = super::demux(&data);
    assert_eq!(state, DemuxState::Done);
    assert!(image_complete(&data, &demux.unwrap()));
    // a lossy one needs the 10 bytes of the frame header
    let data = file(&[chunk(b"VP8 ", &vp8(9, 4, 30))]);
    assert!(
      super::demux(&data[..29])
        .0
        .is_none_or(|d| d.state < DemuxState::ParsedHeader)
    );
    assert_eq!(super::demux(&data[..30]).1, DemuxState::ParsedHeader);
  }

  #[test]
  fn a_vp8x_image_has_its_header_at_the_header_of_its_image_chunk() {
    let data = file(&[vp8x(0, 9, 4), chunk(b"VP8 ", &vp8(9, 4, 30))]);
    // VP8X complete, no frame yet
    let (demux, state) = super::demux(&data[..37]);
    assert_eq!(state, DemuxState::ParsedHeader);
    assert!(demux.unwrap().frames.is_empty());
    // the image chunk header: a frame with no number yet
    let (demux, _) = super::demux(&data[..38]);
    let demux = demux.unwrap();
    assert_eq!(demux.frames.len(), 1);
    assert_eq!(demux.frames[0].frame_num, 0);
    let (demux, _) = super::demux(&data[..48]);
    assert_eq!(demux.unwrap().frames[0].frame_num, 1);
  }

  #[test]
  fn an_animation_frame_counts_once_its_anmf_chunk_is_here() {
    let f0 = anmf(0, 0, 4, 4, 100, 0, &[chunk(b"VP8L", &vp8l(4, 4, true, 20))]);
    let f1 = anmf(2, 2, 2, 2, 50, 3, &[chunk(b"VP8L", &vp8l(2, 2, false, 9))]);
    let data = file(&[vp8x(0x12, 4, 4), anim(3), f0.clone(), f1.clone()]);
    let end0 = 12 + 18 + 14 + f0.len();
    let count = |cut: usize| super::demux(&data[..cut]).0.map(|d| d.frames.len());
    assert_eq!(count(end0 - 1), Some(0));
    assert_eq!(count(end0), Some(1));
    assert_eq!(count(data.len() - 1), Some(1));
    let (demux, state) = super::demux(&data);
    let demux = demux.unwrap();
    assert_eq!(state, DemuxState::Done);
    assert_eq!(demux.loop_count, 3);
    let frames = anim_frames(&demux);
    assert_eq!(frames.len(), 2);
    assert_eq!(
      frames[1].rect,
      Rect {
        x0: 2,
        y0: 2,
        x1: 4,
        y1: 4
      }
    );
    assert_eq!(frames[1].duration_ms, 50);
    assert!(frames[1].dispose_background && !frames[1].blend);
    // not opaque and not covering the canvas: drawn on the frame before
    assert_eq!(frames[1].required, Some(0));
    assert_eq!(frame_alpha(&frames, 0x12, 4, 4), [true, true]);
    // an image chunk outside ANMF in an animation is an error
    let bad = file(&[
      vp8x(0x12, 4, 4),
      anim(0),
      chunk(b"VP8L", &vp8l(4, 4, false, 4)),
    ]);
    assert_eq!(super::demux(&bad).1, DemuxState::ParseError);
    // an ANMF before ANIM too
    let bad = file(&[vp8x(0x12, 4, 4), f0]);
    assert_eq!(super::demux(&bad).1, DemuxState::ParseError);
  }

  #[test]
  fn a_truncated_file_is_not_done() {
    let data = file(&[chunk(b"VP8L", &vp8l(7, 5, false, 30))]);
    let (demux, state) = super::demux(&data[..data.len() - 2]);
    assert_eq!(state, DemuxState::ParsedHeader);
    assert!(demux.is_some());
  }

  #[test]
  fn frames_compose_with_blinks_rules() {
    let frame =
      |rect: [u32; 4], dispose_background: bool, blend: bool, has_alpha: bool| DemuxFrame {
        x_offset: rect[0],
        y_offset: rect[1],
        width: rect[2],
        height: rect[3],
        dispose_background,
        no_blend: !blend,
        has_alpha,
        frame_num: 1,
        complete: true,
        ..DemuxFrame::default()
      };
    let demux = |frames| Demux {
      state: DemuxState::Done,
      is_ext_format: true,
      flags: ANIMATION_FLAG,
      canvas_width: 10,
      canvas_height: 10,
      loop_count: 0,
      frames,
      raw: false,
    };
    // opaque and covering: independent; the frame before disposes to the background over the
    // whole canvas: independent too
    let frames = anim_frames(&demux(vec![
      frame([0, 0, 10, 10], true, true, true),
      frame([0, 0, 10, 10], false, true, false),
      frame([2, 2, 3, 3], false, true, true),
      frame([2, 2, 3, 3], true, true, true),
      frame([0, 0, 4, 4], false, true, true),
    ]));
    let required: Vec<_> = frames.iter().map(|f| f.required).collect();
    assert_eq!(required, [None, None, Some(1), Some(2), Some(3)]);
    // with no alpha flag: only the frame on a canvas that the frame before cleared has alpha
    assert_eq!(
      frame_alpha(&frames, ANIMATION_FLAG, 10, 10),
      [false, false, false, false, true]
    );
    let frames2 = anim_frames(&demux(vec![
      frame([1, 1, 2, 2], true, true, true),
      frame([0, 0, 4, 4], false, true, true),
    ]));
    // the frame before disposes to the background and needed no frame: independent
    assert_eq!(frames2[1].required, None);
    assert_eq!(frame_alpha(&frames2, ANIMATION_FLAG, 10, 10), [true, true]);
  }

  #[test]
  fn blends_like_blink() {
    assert_eq!(
      blend_non_premultiplied([1, 2, 3, 0], [9, 8, 7, 6]),
      [9, 8, 7, 6]
    );
    // over a clear pixel: dst_factor_a 0, scale 2^24 / 128
    assert_eq!(
      blend_non_premultiplied([200, 100, 50, 128], [0, 0, 0, 0]),
      [200, 100, 50, 128]
    );
    // half over opaque: dst_factor_a = 255 * 128 >> 8 = 127, alpha 255
    assert_eq!(
      blend_non_premultiplied([255, 0, 0, 128], [0, 0, 255, 255]),
      [127, 0, 126, 255]
    );
  }

  #[test]
  fn a_frame_still_has_its_chunks_behind_a_vp8x_header() {
    let alph = chunk(b"ALPH", &[0, 1, 2]);
    let image = chunk(b"VP8 ", &vp8(3, 2, 1));
    let f0 = anmf(0, 0, 3, 2, 10, 0, &[alph.clone(), image.clone()]);
    let data = file(&[vp8x(0x12, 3, 2), anim(0), f0]);
    let (demux, _) = super::demux(&data);
    let demux = demux.unwrap();
    let still = frame_still(&data, &demux.frames[0]);
    assert_eq!(still, file(&[vp8x(0x10, 3, 2), alph, image]));
  }
}
