//! WebCodecs pixel formats for decoded AVFrames.
//!
//! A VideoFrame's `format` must describe the bytes of its AVFrame: `copyTo()`,
//! `allocationSize()` and the plane layout all follow the format. FFmpeg decoders
//! also output formats WebCodecs has no name for (rgb24, pal8, gray, monob, ya8,
//! rgb48be, argb, yuvj*, gbrp, ...). Each decoded frame gets a format here:
//!
//! - same memory as a WebCodecs format: keep the frame (yuvj* is relabelled yuv*
//!   with a full-range tag; gbrp from video or a still AVIF is I444 with G, B, R planes)
//! - anything else: convert once, to the format real Chrome reports for the input
//!   (images: BGRX, or BGRA when the image has alpha; video: the nearest YUV format)
//! - PNG, GIF and BMP: always BGRA or BGRX, by the header, with Skia's bytes (see `skia_image`)
//! - a lossy WebP that is not simple lossy: BGRA or BGRX, with libwebp's bytes (see `webp_rgb`)
//!
//! Hardware frames keep the label they had before (I420 for p010 and its siblings):
//! how to expose them is a separate product decision. `copyTo()` refuses to read
//! them as I420 because their memory does not match.

use crate::codec::{CodecError, Frame};
use crate::ffi::accessors::{
  ffframe_convert_decoded, ffframe_palette_has_alpha, ffpixfmt_describe,
};
use crate::ffi::avutil::av_frame_copy_props;
use crate::ffi::{AVCodecID, AVColorRange, AVColorSpace, AVPixelFormat, check_error};
use crate::webcodecs::VideoPixelFormat;
use crate::webcodecs::image_hints::PixelHint;

// libavutil/pixdesc.h AV_PIX_FMT_FLAG_*
const FLAG_PAL: i32 = 1 << 1;
const FLAG_HWACCEL: i32 = 1 << 3;
const FLAG_RGB: i32 = 1 << 5;
const FLAG_ALPHA: i32 = 1 << 7;
const FLAG_BAYER: i32 = 1 << 8;
const FLAG_XYZ: i32 = 1 << 10;

/// Where a decoded frame comes from. Chrome picks different formats per source.
#[derive(Clone, Copy, Debug)]
pub(crate) enum DecodedSource {
  /// ImageDecoder, with what the file header says, when it says it:
  /// - `alpha`: the alpha it declares (WebP alpha flag; PNG, GIF and BMP: see `image_hints`,
  ///   for each frame of a GIF). Chrome labels the image BGRA or BGRX by it.
  /// - `yuv`: whether Chrome decodes the image to YUV planes (JPEG: the sampling
  ///   factors; AVIF: one still image without alpha; WebP: a simple lossy file, whose
  ///   first chunk is VP8). An AVIF with YUV planes keeps them either way: WebCodecs has a
  ///   format for them, its alpha plane is not decoded, and BGRX would lose precision.
  ///   Only planar RGB (gbrp) follows Chrome.
  /// - `size`: the size of a JPEG that Chrome scales for desiredWidth/desiredHeight (with
  ///   `yuv: Some(false)`: such a JPEG is BGRX). It is resized in the conversion pass.
  /// - `pixels`: what a PNG or BMP header says about pixels FFmpeg decodes differently.
  Image {
    codec: AVCodecID,
    alpha: Option<bool>,
    yuv: Option<bool>,
    size: Option<(u32, u32)>,
    pixels: PixelHint,
  },
  /// VideoDecoder, software decoder
  Video,
  /// VideoDecoder, frame downloaded from a hardware surface
  HardwareVideo,
}

struct PixDesc {
  flags: i32,
  nb_components: i32,
  log2_chroma_w: i32,
  log2_chroma_h: i32,
  depth: i32,
}

fn describe(raw: i32) -> Result<PixDesc, CodecError> {
  let mut out = [0i32; 5];
  check_error(unsafe { ffpixfmt_describe(raw, out.as_mut_ptr()) })?;
  Ok(PixDesc {
    flags: out[0],
    nb_components: out[1],
    log2_chroma_w: out[2],
    log2_chroma_h: out[3],
    depth: out[4],
  })
}

/// What to do with a decoded frame.
enum Choice {
  /// Its memory already is this format.
  Keep(VideoPixelFormat),
  /// Convert it to this format (`image`: an image, see `ffframe_convert_decoded`).
  Convert {
    format: VideoPixelFormat,
    desc: PixDesc,
    image: bool,
  },
  /// Convert a lossy WebP's planes to BGRA or BGRX the way libwebp does (see `webp_rgb`).
  WebpRgb(VideoPixelFormat),
}

/// Give a decoded frame the WebCodecs format that describes its bytes, converting
/// it when FFmpeg's format has no WebCodecs equivalent.
pub(crate) fn decoded_frame_format(
  mut frame: Frame,
  source: DecodedSource,
) -> Result<(Frame, VideoPixelFormat), CodecError> {
  if let DecodedSource::Image {
    codec: codec @ (AVCodecID::Png | AVCodecID::Gif | AVCodecID::Bmp),
    alpha,
    pixels,
    ..
  } = source
  {
    return skia_image(frame, codec, alpha, pixels);
  }
  let size = match source {
    DecodedSource::Image { size, .. } => size,
    _ => None,
  };
  match choose(&mut frame, source)? {
    Choice::Keep(format) => Ok((frame, format)),
    // One swscale pass to the final size: a full-size converted copy of a large
    // image is never made.
    Choice::Convert {
      format,
      desc,
      image,
    } => Ok((convert(&frame, format, &desc, image, size)?, format)),
    Choice::WebpRgb(format) => Ok((webp_rgb(&frame, format == VideoPixelFormat::BGRA)?, format)),
  }
}

/// Pick the frame's format. A yuvj* frame is relabelled yuv* here.
fn choose(frame: &mut Frame, source: DecodedSource) -> Result<Choice, CodecError> {
  let av = frame.format();
  // Chrome keeps the YUV planes of a simple lossy WebP only. It decodes any other lossy WebP
  // with libwebp, to BGRA when the VP8X header has the alpha flag and to BGRX otherwise (an
  // ALPH chunk without the flag is not used; the flag without one gives alpha 255).
  if let DecodedSource::Image {
    codec: AVCodecID::Webp,
    alpha,
    yuv: Some(false),
    ..
  } = source
    && matches!(av, AVPixelFormat::Yuv420p | AVPixelFormat::Yuva420p)
  {
    return Ok(Choice::WebpRgb(
      if alpha.unwrap_or(av == AVPixelFormat::Yuva420p) {
        VideoPixelFormat::BGRA
      } else {
        VideoPixelFormat::BGRX
      },
    ));
  }
  // Chrome decodes a JPEG to BGRX unless its sampling factors are ones it reads as YUV,
  // even when FFmpeg's planes have a WebCodecs layout (FFmpeg reads 2x2/1x2/1x2 as yuvj422p).
  let rgb_only = matches!(
    source,
    DecodedSource::Image {
      codec: AVCodecID::Mjpeg,
      yuv: Some(false),
      ..
    }
  );
  if !rgb_only {
    if let Some(format) = VideoPixelFormat::from_av_format(av) {
      return Ok(Choice::Keep(format));
    }

    // yuvj* is yuv* with full range: same planes, so only the format field changes.
    let relabel = match av {
      AVPixelFormat::Yuvj420p => Some(AVPixelFormat::Yuv420p),
      AVPixelFormat::Yuvj422p => Some(AVPixelFormat::Yuv422p),
      AVPixelFormat::Yuvj444p => Some(AVPixelFormat::Yuv444p),
      _ => None,
    };
    if let Some(yuv) = relabel {
      frame.set_format(yuv);
      frame.set_color_range(AVColorRange::Jpeg);
      let format = VideoPixelFormat::from_av_format(yuv).unwrap_or(VideoPixelFormat::I420);
      return Ok(Choice::Keep(format));
    }
  }

  match source {
    // Unchanged from before this mapping existed (see the module comment).
    DecodedSource::HardwareVideo => Ok(Choice::Keep(VideoPixelFormat::I420)),
    DecodedSource::Video => {
      // Chrome reports RGB video as I444 with the G, B, R planes.
      if let Some(format) = planar_rgb_format(av) {
        return Ok(Choice::Keep(format));
      }
      let desc = describe(frame.format_raw())?;
      Ok(Choice::Convert {
        format: video_target(&desc),
        desc,
        image: false,
      })
    }
    DecodedSource::Image {
      codec, alpha, yuv, ..
    } => {
      // Chrome reports a still AVIF with the identity matrix as I444 with the G, B, R
      // planes (an animated one, or one with alpha, as BGRX/BGRA).
      if codec == AVCodecID::Av1
        && yuv == Some(true)
        && let Some(format) = planar_rgb_format(av)
      {
        return Ok(Choice::Keep(format));
      }
      let desc = describe(frame.format_raw())?;
      let format = if image_has_alpha(frame, &desc, codec, alpha) {
        VideoPixelFormat::BGRA
      } else {
        VideoPixelFormat::BGRX
      };
      Ok(Choice::Convert {
        format,
        desc,
        image: true,
      })
    }
  }
}

/// A PNG, GIF or BMP frame as Chrome decodes it with Skia: BGRA, or BGRX (alpha 255) when the
/// header declares no alpha, never premultiplied. FFmpeg's bytes already are Skia's for BGRA
/// (also BMP's bgr0, which is BGRA whose alpha is all 0), for RGBA once R and B are swapped, and
/// for the formats swscale converts exactly (palette, gray, 8-bit RGB). Skia keeps the high byte
/// of a 16-bit PNG sample and scales a 16-bit BMP's bitfields with the image crate's tables,
/// where swscale rounds or replicates bits. The frame is tagged RGB matrix, full range, like a
/// converted one.
fn skia_image(
  frame: Frame,
  codec: AVCodecID,
  alpha: Option<bool>,
  pixels: PixelHint,
) -> Result<(Frame, VideoPixelFormat), CodecError> {
  let desc = describe(frame.format_raw())?;
  let alpha = alpha.unwrap_or_else(|| image_has_alpha(&frame, &desc, codec, None));
  let format = if alpha {
    VideoPixelFormat::BGRA
  } else {
    VideoPixelFormat::BGRX
  };
  let swap = frame.format() == AVPixelFormat::Rgba;
  let mut frame = match (frame.format(), pixels) {
    (AVPixelFormat::Bgra | AVPixelFormat::Bgr0 | AVPixelFormat::Rgba, _) => frame,
    // big-endian samples: the high byte comes first
    (AVPixelFormat::Gray16be, _) => unpack(&frame, 2, alpha, |p| [p[0], p[0], p[0], 255])?,
    (AVPixelFormat::Ya16be, _) => unpack(&frame, 4, alpha, |p| [p[0], p[0], p[0], p[2]])?,
    (AVPixelFormat::Rgb48be, _) => unpack(&frame, 6, alpha, |p| [p[4], p[2], p[0], 255])?,
    (AVPixelFormat::Rgba64be, _) => unpack(&frame, 8, alpha, |p| [p[4], p[2], p[0], p[6]])?,
    // FFmpeg keeps the 16-bit words of the file, alpha bits included
    (
      AVPixelFormat::Rgb555le | AVPixelFormat::Rgb565le | AVPixelFormat::Rgb444le,
      PixelHint::Bitfields([r, g, b, a]),
    ) => unpack(&frame, 2, alpha, |p| {
      let word = u32::from(u16::from_le_bytes([p[0], p[1]]));
      [b.read(word), g.read(word), r.read(word), a.read(word)]
    })?,
    _ => convert(&frame, format, &desc, true, None)?,
  };
  // FFmpeg drops the tRNS chunk of a 1, 2 or 4-bit gray PNG (it compares the scaled samples).
  let key = match pixels {
    PixelHint::GrayKey(key) => Some(key),
    _ => None,
  };
  if swap || !alpha || key.is_some() {
    frame.make_writable()?;
    edit_pixels(&mut frame, |px| {
      if swap {
        px.swap(0, 2);
      }
      if let Some(key) = key {
        px[3] = if Some(px[0]) == key { 0 } else { 255 };
      }
      if !alpha {
        px[3] = 255;
      }
    })?;
  }
  frame.set_format(AVPixelFormat::Bgra);
  frame.set_colorspace(AVColorSpace::Rgb);
  frame.set_color_range(AVColorRange::Jpeg);
  Ok((frame, format))
}

/// A new BGRA frame from the packed `size`-byte pixels of `src`: `read` gives the B, G, R and A
/// of each, and A is 255 without `alpha`.
fn unpack(
  src: &Frame,
  size: usize,
  alpha: bool,
  read: impl Fn(&[u8]) -> [u8; 4],
) -> Result<Frame, CodecError> {
  let mut dst = Frame::new_video(src.width(), src.height(), AVPixelFormat::Bgra)?;
  check_error(unsafe { av_frame_copy_props(dst.as_mut_ptr(), src.as_ptr()) })?;
  let (width, stride, dst_stride) = (
    src.width() as usize * size,
    src.linesize(0) as usize,
    dst.linesize(0) as usize,
  );
  let from = src.plane_data(0).ok_or_else(no_pixels)?;
  let to = dst.plane_data_mut(0).ok_or_else(no_pixels)?;
  for (from, to) in from.chunks(stride).zip(to.chunks_mut(dst_stride)) {
    for (p, px) in from[..width]
      .chunks_exact(size)
      .zip(to.as_chunks_mut::<4>().0)
    {
      let [b, g, r, a] = read(p);
      *px = [b, g, r, if alpha { a } else { 255 }];
    }
  }
  Ok(dst)
}

/// Run `edit` on each pixel of a packed 32-bit frame.
fn edit_pixels(frame: &mut Frame, edit: impl FnMut(&mut [u8; 4])) -> Result<(), CodecError> {
  let (width, stride) = (frame.width() as usize * 4, frame.linesize(0) as usize);
  let data = frame.plane_data_mut(0).ok_or_else(no_pixels)?;
  data
    .chunks_mut(stride)
    .flat_map(|row| row[..width].as_chunks_mut::<4>().0)
    .for_each(edit);
  Ok(())
}

/// A lossy WebP frame as Chrome decodes it with libwebp: BGRA, never premultiplied, with the
/// alpha plane when `alpha` (else alpha 255). FFmpeg's yuv(a)420p planes are libwebp's. Each
/// pixel takes libwebp's fancy upsampling of the chroma (upsampling.c UPSAMPLE_FUNC over the row
/// pairs of io_dec.c EmitFancyRGB) and its YUV to RGB math (yuv.h VP8YuvToBgr).
fn webp_rgb(src: &Frame, alpha: bool) -> Result<Frame, CodecError> {
  let (width, height) = (src.width() as usize, src.height() as usize);
  // each plane as a function from a row number to the row's samples
  let plane = |i: usize| {
    let stride = src.linesize(i) as usize;
    src
      .plane_data(i)
      .map(|data| move |row: usize| &data[row * stride..])
  };
  let [Some(y), Some(u), Some(v)] = [0, 1, 2].map(plane) else {
    return Err(no_pixels());
  };
  let a = if alpha { plane(3) } else { None };
  let mut dst = Frame::new_video(src.width(), src.height(), AVPixelFormat::Bgra)?;
  check_error(unsafe { av_frame_copy_props(dst.as_mut_ptr(), src.as_ptr()) })?;
  let stride = dst.linesize(0) as usize;
  let out = dst.plane_data_mut(0).ok_or_else(no_pixels)?;
  let cols: Vec<_> = (0..width)
    .map(|col| chroma_neighbours(col, width.div_ceil(2)))
    .collect();
  for (row, line) in out.chunks_mut(stride).enumerate() {
    let (near, far) = chroma_neighbours(row, height.div_ceil(2));
    let [u, v] = [&u, &v].map(|plane| [plane(near), plane(far)]);
    let (luma, alpha) = (y(row), a.as_ref().map(|a| a(row)));
    let pixels = line[..width * 4].as_chunks_mut::<4>().0.iter_mut();
    for (col, (px, &cols)) in pixels.zip(&cols).enumerate() {
      let [u, v] = [u, v].map(|rows| upsample(rows, cols));
      let y = mult_hi(i32::from(luma[col]), 19077);
      *px = [
        clip8(y + mult_hi(u, 33050) - 17685),
        clip8(y - mult_hi(u, 6419) - mult_hi(v, 13320) + 8708),
        clip8(y + mult_hi(v, 26149) - 14234),
        alpha.map_or(255, |alpha| alpha[col]),
      ];
    }
  }
  dst.set_colorspace(AVColorSpace::Rgb);
  dst.set_color_range(AVColorRange::Jpeg);
  Ok(dst)
}

/// The chroma sample nearest to sample `i` of a 4:2:0 row or column, and the next nearest (on the
/// other side of `i`), out of `count`. At an edge there is none: the nearest is used again.
fn chroma_neighbours(i: usize, count: usize) -> (usize, usize) {
  let near = i / 2;
  let far = if i % 2 == 1 {
    near + 1
  } else {
    near.wrapping_sub(1)
  };
  (near, if far < count { far } else { near })
}

/// libwebp's fancy upsampling: a pixel's chroma from the 4 samples around it, in the nearest chroma
/// row and the next nearest, at the nearest column and the next nearest (`cols`). They are weighted
/// 9:3:3:1 and rounded in libwebp's two steps. At an edge column libwebp weights the nearest column
/// of the two rows 3:1 instead, which gives the same value.
fn upsample([near_row, far_row]: [&[u8]; 2], (near, far): (usize, usize)) -> i32 {
  let nearest = i32::from(near_row[near]);
  let diagonal = (nearest
    + 3 * i32::from(near_row[far])
    + 3 * i32::from(far_row[near])
    + i32::from(far_row[far])
    + 8)
    >> 3;
  (diagonal + nearest) >> 1
}

/// libwebp yuv.h MultHi: the fixed-point product, with 6 fraction bits left for `clip8`.
fn mult_hi(v: i32, coeff: i32) -> i32 {
  (v * coeff) >> 8
}

/// libwebp yuv.h VP8Clip8
fn clip8(v: i32) -> u8 {
  (v >> 6).clamp(0, 255) as u8
}

fn no_pixels() -> CodecError {
  CodecError::InvalidState("decoded frame has no pixels".into())
}

/// I444 formats with the planes of FFmpeg's planar RGB (G, B, R).
fn planar_rgb_format(av: AVPixelFormat) -> Option<VideoPixelFormat> {
  match av {
    AVPixelFormat::Gbrp => Some(VideoPixelFormat::I444),
    AVPixelFormat::Gbrp10le => Some(VideoPixelFormat::I444P10),
    AVPixelFormat::Gbrp12le => Some(VideoPixelFormat::I444P12),
    _ => None,
  }
}

/// Whether Chrome labels the decoded image BGRA (true) or BGRX.
fn image_has_alpha(
  frame: &Frame,
  desc: &PixDesc,
  codec: AVCodecID,
  header_alpha: Option<bool>,
) -> bool {
  // Chrome follows the header (WebP alpha flag, PNG color type or tRNS chunk), not the
  // pixels: a PNG whose tRNS entries are all 255 is still BGRA.
  if let Some(alpha) = header_alpha {
    return alpha;
  }
  let palette = desc.flags & FLAG_PAL != 0;
  match codec {
    // JPEG has no alpha (CMYK decodes to gbrap with an opaque alpha plane).
    AVCodecID::Mjpeg => false,
    // BMP palettes have no alpha; FFmpeg zero-fills the entries the file does not list.
    AVCodecID::Bmp if palette => false,
    // Other palettes (a PNG whose header was not read): an entry that is not opaque.
    _ if palette => unsafe { ffframe_palette_has_alpha(frame.as_ptr()) != 0 },
    _ => desc.flags & FLAG_ALPHA != 0,
  }
}

/// The YUV format closest to a video decoder's format, or BGRX/BGRA for RGB-like formats.
fn video_target(desc: &PixDesc) -> VideoPixelFormat {
  use VideoPixelFormat as F;
  let alpha = desc.flags & FLAG_ALPHA != 0;
  if desc.flags & (FLAG_RGB | FLAG_PAL | FLAG_BAYER | FLAG_XYZ) != 0 {
    return if alpha { F::BGRA } else { F::BGRX };
  }
  // Gray has no chroma: 4:2:0 adds the least. Layouts with no WebCodecs format
  // (4:4:0, 4:1:1, 4:1:0) go to 4:4:4 so that no chroma sample is dropped.
  let chroma = if desc.nb_components < 3 {
    420
  } else {
    match (desc.log2_chroma_w, desc.log2_chroma_h) {
      (1, 1) => 420,
      (1, 0) => 422,
      _ => 444,
    }
  };
  let bits = match desc.depth {
    ..=8 => 8,
    9..=10 => 10,
    _ => 12,
  };
  match (chroma, bits, alpha) {
    (420, 8, false) => F::I420,
    (420, 8, true) => F::I420A,
    (420, 10, false) => F::I420P10,
    (420, _, true) => F::I420AP10,
    (420, _, false) => F::I420P12,
    (422, 8, false) => F::I422,
    (422, 8, true) => F::I422A,
    (422, 10, false) => F::I422P10,
    (422, _, true) => F::I422AP10,
    (422, _, false) => F::I422P12,
    (_, 8, false) => F::I444,
    (_, 8, true) => F::I444A,
    (_, 10, false) => F::I444P10,
    (_, _, true) => F::I444AP10,
    (_, _, false) => F::I444P12,
  }
}

/// Convert `src` to `format`, at `size` when given.
fn convert(
  src: &Frame,
  format: VideoPixelFormat,
  desc: &PixDesc,
  image: bool,
  size: Option<(u32, u32)>,
) -> Result<Frame, CodecError> {
  if desc.flags & FLAG_HWACCEL != 0 {
    return Err(CodecError::InvalidState(
      "hardware frame was not downloaded".into(),
    ));
  }
  let (width, height) = size.unwrap_or((src.width(), src.height()));
  let mut dst = Frame::new_video(width, height, format.to_av_format())?;
  // Images without a range tag are full range; video without one is limited range.
  check_error(unsafe {
    ffframe_convert_decoded(src.as_ptr(), dst.as_mut_ptr(), image as std::os::raw::c_int)
  })?;
  Ok(dst)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn desc(name: &str) -> PixDesc {
    let c = std::ffi::CString::new(name).unwrap();
    let raw = unsafe { crate::ffi::avutil::av_get_pix_fmt(c.as_ptr()) };
    describe(raw).unwrap()
  }

  #[test]
  fn image_formats_have_ffmpegs_values() {
    use AVPixelFormat as P;
    for (name, format) in [
      ("bgr0", P::Bgr0),
      ("gray16be", P::Gray16be),
      ("ya16be", P::Ya16be),
      ("rgb48be", P::Rgb48be),
      ("rgba64be", P::Rgba64be),
      ("rgb565le", P::Rgb565le),
      ("rgb555le", P::Rgb555le),
      ("rgb444le", P::Rgb444le),
    ] {
      let c = std::ffi::CString::new(name).unwrap();
      let raw = unsafe { crate::ffi::avutil::av_get_pix_fmt(c.as_ptr()) };
      assert_eq!(raw, format.as_raw(), "{name}");
      assert_eq!(AVPixelFormat::from_raw(raw), format, "{name}");
    }
  }

  /// FFmpeg's Y, U, V, A planes and Chrome 155's bytes for lossy WebPs with an ALPH chunk.
  #[test]
  fn webp_rgb_has_chromes_bytes() {
    type Planes = [&'static [u8]; 4];
    let cases: [(u32, u32, Planes, &[u8]); 5] = [
      (1, 1, [&[16], &[128], &[128], &[1]], &[0, 0, 0, 1]),
      (
        1,
        2,
        [&[16, 16], &[128], &[128], &[255, 0]],
        &[0, 0, 0, 255, 0, 0, 0, 0],
      ),
      (
        2,
        2,
        [&[12, 95, 105, 216], &[102], &[168], &[255, 255, 0, 255]],
        &[
          0, 0, 59, 255, 40, 70, 156, 255, 51, 81, 167, 0, 180, 211, 255, 255,
        ],
      ),
      (
        3,
        1,
        [&[105, 64, 39], &[100, 183], &[143, 124], &[7, 41, 11]],
        &[47, 102, 128, 7, 42, 51, 72, 41, 95, 13, 28, 11],
      ),
      (
        1,
        3,
        [&[108, 177, 104], &[164, 91], &[90, 72], &[33, 41, 201]],
        &[180, 124, 46, 33, 224, 215, 120, 41, 64, 151, 21, 201],
      ),
    ];
    for (width, height, planes, chrome) in cases {
      let mut frame = Frame::new_video(width, height, AVPixelFormat::Yuva420p).unwrap();
      for (i, plane) in planes.into_iter().enumerate() {
        let row_width = if matches!(i, 1 | 2) {
          width.div_ceil(2)
        } else {
          width
        } as usize;
        let stride = frame.linesize(i) as usize;
        let data = frame.plane_data_mut(i).unwrap();
        for (row, from) in plane.chunks(row_width).enumerate() {
          data[row * stride..][..row_width].copy_from_slice(from);
        }
      }
      let bgra = |alpha| {
        let out = webp_rgb(&frame, alpha).unwrap();
        let stride = out.linesize(0) as usize;
        let data = out.plane_data(0).unwrap();
        (0..height as usize)
          .flat_map(|row| data[row * stride..][..width as usize * 4].to_vec())
          .collect::<Vec<u8>>()
      };
      assert_eq!(bgra(true), chrome, "{width}x{height}");
      // without alpha (BGRX): the same colours, with 255 in the fourth byte
      let opaque: Vec<u8> = chrome
        .chunks(4)
        .flat_map(|px| [px[0], px[1], px[2], 255])
        .collect();
      assert_eq!(bgra(false), opaque, "{width}x{height}");
    }
  }

  #[test]
  fn video_targets_keep_chroma_and_depth() {
    use VideoPixelFormat as F;
    assert_eq!(video_target(&desc("gray")), F::I420);
    assert_eq!(video_target(&desc("gray10le")), F::I420P10);
    assert_eq!(video_target(&desc("gray12le")), F::I420P12);
    assert_eq!(video_target(&desc("yuv440p")), F::I444);
    assert_eq!(video_target(&desc("yuv440p10le")), F::I444P10);
    assert_eq!(video_target(&desc("yuv411p")), F::I444);
    assert_eq!(video_target(&desc("yuv420p9le")), F::I420P10);
    assert_eq!(video_target(&desc("yuv422p14le")), F::I422P12);
    assert_eq!(video_target(&desc("yuva444p12le")), F::I444AP10);
    assert_eq!(video_target(&desc("p010le")), F::I420P10);
    assert_eq!(video_target(&desc("nv16")), F::I422);
    assert_eq!(video_target(&desc("rgb24")), F::BGRX);
    assert_eq!(video_target(&desc("gbrap")), F::BGRA);
  }
}
