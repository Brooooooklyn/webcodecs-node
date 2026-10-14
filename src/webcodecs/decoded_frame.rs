//! WebCodecs pixel formats for decoded AVFrames.
//!
//! A VideoFrame's `format` must describe the bytes of its AVFrame: `copyTo()`,
//! `allocationSize()` and the plane layout all follow the format. FFmpeg decoders
//! also output formats WebCodecs has no name for (rgb24, pal8, gray, monob, ya8,
//! rgb48be, argb, yuvj*, gbrp, ...). Each decoded frame gets a format here:
//!
//! - same memory as a WebCodecs format: keep the frame (yuvj* is relabelled yuv*
//!   with a full-range tag; gbrp from video is I444 with G, B, R planes)
//! - anything else: convert once, to the format real Chrome reports for the input
//!   (images: BGRX, or BGRA when the image has alpha; video: the nearest YUV format)
//!
//! Hardware frames keep the label they had before (I420 for p010 and its siblings):
//! how to expose them is a separate product decision. `copyTo()` refuses to read
//! them as I420 because their memory does not match.

use crate::codec::{CodecError, Frame};
use crate::ffi::accessors::{
  ffframe_convert_decoded, ffframe_palette_has_alpha, ffpixfmt_describe,
};
use crate::ffi::{AVCodecID, AVColorRange, AVPixelFormat, check_error};
use crate::webcodecs::VideoPixelFormat;

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
  /// ImageDecoder. `webp_alpha` is the alpha hint in a WebP header, when there is one.
  Image {
    codec: AVCodecID,
    webp_alpha: Option<bool>,
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

/// Give a decoded frame the WebCodecs format that describes its bytes, converting
/// it when FFmpeg's format has no WebCodecs equivalent.
pub(crate) fn decoded_frame_format(
  mut frame: Frame,
  source: DecodedSource,
) -> Result<(Frame, VideoPixelFormat), CodecError> {
  let av = frame.format();
  if let Some(format) = VideoPixelFormat::from_av_format(av) {
    return Ok((frame, format));
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
    return Ok((frame, format));
  }

  match source {
    // Unchanged from before this mapping existed (see the module comment).
    DecodedSource::HardwareVideo => Ok((frame, VideoPixelFormat::I420)),
    DecodedSource::Video => {
      // Chrome reports RGB video as I444 with the G, B, R planes.
      let planar_rgb = match av {
        AVPixelFormat::Gbrp => Some(VideoPixelFormat::I444),
        AVPixelFormat::Gbrp10le => Some(VideoPixelFormat::I444P10),
        AVPixelFormat::Gbrp12le => Some(VideoPixelFormat::I444P12),
        _ => None,
      };
      if let Some(format) = planar_rgb {
        return Ok((frame, format));
      }
      let desc = describe(frame.format_raw())?;
      let format = video_target(&desc);
      Ok((convert(&frame, format, &desc, false)?, format))
    }
    DecodedSource::Image { codec, webp_alpha } => {
      let desc = describe(frame.format_raw())?;
      let format = if image_has_alpha(&frame, &desc, codec, webp_alpha) {
        VideoPixelFormat::BGRA
      } else {
        VideoPixelFormat::BGRX
      };
      Ok((convert(&frame, format, &desc, true)?, format))
    }
  }
}

/// Whether Chrome labels the decoded image BGRA (true) or BGRX.
fn image_has_alpha(
  frame: &Frame,
  desc: &PixDesc,
  codec: AVCodecID,
  webp_alpha: Option<bool>,
) -> bool {
  let palette = desc.flags & FLAG_PAL != 0;
  // Chrome follows the WebP header hint, not the pixels.
  if codec == AVCodecID::Webp
    && let Some(hint) = webp_alpha
  {
    return hint;
  }
  match codec {
    // JPEG has no alpha (CMYK decodes to gbrap with an opaque alpha plane).
    AVCodecID::Mjpeg => false,
    // BMP palettes have no alpha; FFmpeg zero-fills the entries the file does not list.
    AVCodecID::Bmp if palette => false,
    // PNG tRNS lowers palette alpha below 255.
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

fn convert(
  src: &Frame,
  format: VideoPixelFormat,
  desc: &PixDesc,
  image: bool,
) -> Result<Frame, CodecError> {
  if desc.flags & FLAG_HWACCEL != 0 {
    return Err(CodecError::InvalidState(
      "hardware frame was not downloaded".into(),
    ));
  }
  let mut dst = Frame::new_video(src.width(), src.height(), format.to_av_format())?;
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
