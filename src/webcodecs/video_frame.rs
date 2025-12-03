//! VideoFrame - WebCodecs API implementation
//!
//! Represents a frame of video data that can be displayed or encoded.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/VideoFrame

use crate::codec::Frame;
use crate::ffi::AVPixelFormat;
use crate::webcodecs::error::invalid_state_error;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use std::sync::{Arc, Mutex};

/// Video pixel format (WebCodecs spec)
#[napi(string_enum)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPixelFormat {
  /// Planar YUV 4:2:0, 12bpp, (1 Cr & Cb sample per 2x2 Y samples)
  I420,
  /// Planar YUV 4:2:0, 12bpp, with alpha plane
  I420A,
  /// Planar YUV 4:2:2, 16bpp
  I422,
  /// Planar YUV 4:4:4, 24bpp
  I444,
  /// Semi-planar YUV 4:2:0, 12bpp (Y plane + interleaved UV)
  NV12,
  /// Semi-planar YUV 4:2:0, 12bpp (Y plane + interleaved VU) - per W3C WebCodecs spec
  NV21,
  /// RGBA 32bpp
  RGBA,
  /// RGBX 32bpp (alpha ignored)
  RGBX,
  /// BGRA 32bpp
  BGRA,
  /// BGRX 32bpp (alpha ignored)
  BGRX,
}

impl VideoPixelFormat {
  /// Convert from FFmpeg pixel format
  pub fn from_av_format(format: AVPixelFormat) -> Option<Self> {
    match format {
      AVPixelFormat::Yuv420p => Some(VideoPixelFormat::I420),
      AVPixelFormat::Yuva420p => Some(VideoPixelFormat::I420A),
      AVPixelFormat::Yuv422p => Some(VideoPixelFormat::I422),
      AVPixelFormat::Yuv444p => Some(VideoPixelFormat::I444),
      AVPixelFormat::Nv12 => Some(VideoPixelFormat::NV12),
      AVPixelFormat::Nv21 => Some(VideoPixelFormat::NV21),
      AVPixelFormat::Rgba => Some(VideoPixelFormat::RGBA),
      AVPixelFormat::Bgra => Some(VideoPixelFormat::BGRA),
      _ => None,
    }
  }

  /// Convert to FFmpeg pixel format
  pub fn to_av_format(&self) -> AVPixelFormat {
    match self {
      VideoPixelFormat::I420 => AVPixelFormat::Yuv420p,
      VideoPixelFormat::I420A => AVPixelFormat::Yuva420p,
      VideoPixelFormat::I422 => AVPixelFormat::Yuv422p,
      VideoPixelFormat::I444 => AVPixelFormat::Yuv444p,
      VideoPixelFormat::NV12 => AVPixelFormat::Nv12,
      VideoPixelFormat::NV21 => AVPixelFormat::Nv21,
      VideoPixelFormat::RGBA => AVPixelFormat::Rgba,
      VideoPixelFormat::RGBX => AVPixelFormat::Rgba, // Treat as RGBA
      VideoPixelFormat::BGRA => AVPixelFormat::Bgra,
      VideoPixelFormat::BGRX => AVPixelFormat::Bgra, // Treat as BGRA
    }
  }

  /// Get bytes per pixel for packed formats, 1 for planar
  pub fn bytes_per_sample(&self) -> usize {
    match self {
      VideoPixelFormat::I420 | VideoPixelFormat::I420A => 1,
      VideoPixelFormat::I422 | VideoPixelFormat::I444 => 1,
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => 1,
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => 4,
    }
  }
}

/// VideoColorSpaceInit for constructing VideoColorSpace
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoColorSpaceInit {
  /// Color primaries (e.g., "bt709", "bt2020")
  pub primaries: Option<String>,
  /// Transfer function (e.g., "bt709", "srgb", "pq", "hlg")
  pub transfer: Option<String>,
  /// Matrix coefficients (e.g., "bt709", "bt2020-ncl")
  pub matrix: Option<String>,
  /// Full range flag
  pub full_range: Option<bool>,
}

/// Video color space parameters (WebCodecs spec) - as a class per spec
#[napi]
#[derive(Debug, Clone, Default)]
pub struct VideoColorSpace {
  primaries: Option<String>,
  transfer: Option<String>,
  matrix: Option<String>,
  full_range: Option<bool>,
}

#[napi]
impl VideoColorSpace {
  /// Create a new VideoColorSpace
  #[napi(constructor)]
  pub fn new(init: Option<VideoColorSpaceInit>) -> Self {
    match init {
      Some(init) => VideoColorSpace {
        primaries: init.primaries,
        transfer: init.transfer,
        matrix: init.matrix,
        full_range: init.full_range,
      },
      None => VideoColorSpace::default(),
    }
  }

  /// Get color primaries
  #[napi(getter)]
  pub fn primaries(&self) -> Option<String> {
    self.primaries.clone()
  }

  /// Get transfer characteristics
  #[napi(getter)]
  pub fn transfer(&self) -> Option<String> {
    self.transfer.clone()
  }

  /// Get matrix coefficients
  #[napi(getter)]
  pub fn matrix(&self) -> Option<String> {
    self.matrix.clone()
  }

  /// Get full range flag
  #[napi(getter)]
  pub fn full_range(&self) -> Option<bool> {
    self.full_range
  }

  /// Convert to JSON-compatible object (W3C spec uses toJSON)
  #[napi(js_name = "toJSON")]
  pub fn to_json(&self) -> VideoColorSpaceInit {
    VideoColorSpaceInit {
      primaries: self.primaries.clone(),
      transfer: self.transfer.clone(),
      matrix: self.matrix.clone(),
      full_range: self.full_range,
    }
  }
}

/// DOMRectReadOnly - W3C WebCodecs spec compliant rect class
/// Used for codedRect and visibleRect properties
#[napi(js_name = "DOMRectReadOnly")]
#[derive(Debug, Clone)]
pub struct DOMRectReadOnly {
  x: f64,
  y: f64,
  width: f64,
  height: f64,
}

#[napi]
impl DOMRectReadOnly {
  /// Create a new DOMRectReadOnly
  #[napi(constructor)]
  pub fn new(x: Option<f64>, y: Option<f64>, width: Option<f64>, height: Option<f64>) -> Self {
    DOMRectReadOnly {
      x: x.unwrap_or(0.0),
      y: y.unwrap_or(0.0),
      width: width.unwrap_or(0.0),
      height: height.unwrap_or(0.0),
    }
  }

  /// X coordinate
  #[napi(getter)]
  pub fn x(&self) -> f64 {
    self.x
  }

  /// Y coordinate
  #[napi(getter)]
  pub fn y(&self) -> f64 {
    self.y
  }

  /// Width
  #[napi(getter)]
  pub fn width(&self) -> f64 {
    self.width
  }

  /// Height
  #[napi(getter)]
  pub fn height(&self) -> f64 {
    self.height
  }

  /// Top edge (same as y)
  #[napi(getter)]
  pub fn top(&self) -> f64 {
    self.y
  }

  /// Right edge (x + width)
  #[napi(getter)]
  pub fn right(&self) -> f64 {
    self.x + self.width
  }

  /// Bottom edge (y + height)
  #[napi(getter)]
  pub fn bottom(&self) -> f64 {
    self.y + self.height
  }

  /// Left edge (same as x)
  #[napi(getter)]
  pub fn left(&self) -> f64 {
    self.x
  }

  /// Convert to JSON (W3C spec uses toJSON)
  #[napi(js_name = "toJSON")]
  pub fn to_json(&self) -> DOMRectInit {
    DOMRectInit {
      x: Some(self.x),
      y: Some(self.y),
      width: Some(self.width),
      height: Some(self.height),
    }
  }
}

/// Options for creating a VideoFrame from buffer data (VideoFrameBufferInit per spec)
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameBufferInit {
  /// Pixel format
  pub format: VideoPixelFormat,
  /// Coded width in pixels
  pub coded_width: u32,
  /// Coded height in pixels
  pub coded_height: u32,
  /// Timestamp in microseconds
  pub timestamp: i64,
  /// Duration in microseconds (optional)
  /// Note: W3C spec uses unsigned long long, but JS number can represent up to 2^53 safely
  pub duration: Option<i64>,
  /// Display width (defaults to coded_width)
  pub display_width: Option<u32>,
  /// Display height (defaults to coded_height)
  pub display_height: Option<u32>,
  /// Color space parameters (uses init object)
  pub color_space: Option<VideoColorSpaceInit>,
}

/// Options for creating a VideoFrame from an image source (VideoFrameInit per spec)
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameInit {
  /// Timestamp in microseconds (required per spec when creating from VideoFrame)
  pub timestamp: Option<i64>,
  /// Duration in microseconds (optional)
  pub duration: Option<i64>,
  /// Alpha handling: "keep" (default) or "discard"
  pub alpha: Option<String>,
  /// Visible rect (optional)
  pub visible_rect: Option<DOMRectInit>,
  /// Display width (optional)
  pub display_width: Option<u32>,
  /// Display height (optional)
  pub display_height: Option<u32>,
}

/// Options for copyTo operation
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameCopyToOptions {
  /// Target pixel format (for format conversion)
  pub format: Option<VideoPixelFormat>,
  /// Region to copy (not yet implemented)
  pub rect: Option<DOMRectInit>,
  /// Layout for output planes
  pub layout: Option<Vec<PlaneLayout>>,
}

/// DOMRectInit for specifying regions
#[napi(object)]
#[derive(Debug, Clone)]
pub struct DOMRectInit {
  pub x: Option<f64>,
  pub y: Option<f64>,
  pub width: Option<f64>,
  pub height: Option<f64>,
}

/// Layout information for a single plane per WebCodecs spec
#[napi(object)]
#[derive(Debug, Clone)]
pub struct PlaneLayout {
  /// Byte offset from the start of the buffer to the start of the plane
  pub offset: u32,
  /// Number of bytes per row (stride)
  pub stride: u32,
}

/// Rectangle for specifying a region
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameRect {
  pub x: u32,
  pub y: u32,
  pub width: u32,
  pub height: u32,
}

/// Internal state for VideoFrame
struct VideoFrameInner {
  frame: Frame,
  timestamp_us: i64,
  duration_us: Option<i64>,
  display_width: u32,
  display_height: u32,
  color_space: VideoColorSpace,
  closed: bool,
}

/// VideoFrame - represents a frame of video
///
/// This is a WebCodecs-compliant VideoFrame implementation backed by FFmpeg.
#[napi]
pub struct VideoFrame {
  inner: Arc<Mutex<Option<VideoFrameInner>>>,
}

#[napi]
impl VideoFrame {
  /// Create a new VideoFrame from raw buffer data (BufferSource per spec)
  ///
  /// This is the VideoFrameBufferInit constructor form.
  /// Use `fromVideoFrame()` to create from another VideoFrame.
  #[napi(constructor)]
  pub fn new(data: Uint8Array, init: VideoFrameBufferInit) -> Result<Self> {
    let format = init.format.to_av_format();
    let width = init.coded_width;
    let height = init.coded_height;

    // Create internal frame
    let mut frame = Frame::new_video(width, height, format).map_err(|e| {
      Error::new(
        Status::GenericFailure,
        format!("Failed to create frame: {}", e),
      )
    })?;

    // Copy data into the frame
    Self::copy_data_to_frame(&mut frame, &data, init.format, width, height)?;

    // Set timestamps (convert from microseconds to time_base units)
    // We use microseconds as time_base internally
    frame.set_pts(init.timestamp);
    if let Some(duration) = init.duration {
      frame.set_duration(duration);
    }

    let display_width = init.display_width.unwrap_or(width);
    let display_height = init.display_height.unwrap_or(height);

    let color_space = VideoColorSpace::new(init.color_space);

    let inner = VideoFrameInner {
      frame,
      timestamp_us: init.timestamp,
      duration_us: init.duration,
      display_width,
      display_height,
      color_space,
      closed: false,
    };

    Ok(Self {
      inner: Arc::new(Mutex::new(Some(inner))),
    })
  }

  /// Create a new VideoFrame from another VideoFrame (image source constructor per spec)
  ///
  /// This clones the source VideoFrame and applies any overrides from init.
  /// Per W3C spec, this is equivalent to `new VideoFrame(videoFrame, init)`.
  #[napi(factory)]
  pub fn from_video_frame(source: &VideoFrame, init: Option<VideoFrameInit>) -> Result<Self> {
    source.with_inner(|source_inner| {
      // Clone the underlying frame data
      let cloned_frame = source_inner
        .frame
        .try_clone()
        .map_err(|e| Error::new(Status::GenericFailure, format!("Clone failed: {}", e)))?;

      let init = init.unwrap_or(VideoFrameInit {
        timestamp: None,
        duration: None,
        alpha: None,
        visible_rect: None,
        display_width: None,
        display_height: None,
      });

      // Apply overrides from init
      let timestamp_us = init.timestamp.unwrap_or(source_inner.timestamp_us);
      let duration_us = init.duration.or(source_inner.duration_us);
      let display_width = init.display_width.unwrap_or(source_inner.display_width);
      let display_height = init.display_height.unwrap_or(source_inner.display_height);

      // Note: alpha handling and visible_rect cropping are not yet implemented
      // visible_rect would require sub-region copying which is complex
      if init.visible_rect.is_some() {
        return Err(Error::new(
          Status::GenericFailure,
          "VideoFrame visibleRect parameter is not yet implemented",
        ));
      }

      let new_inner = VideoFrameInner {
        frame: cloned_frame,
        timestamp_us,
        duration_us,
        display_width,
        display_height,
        color_space: source_inner.color_space.clone(),
        closed: false,
      };

      Ok(VideoFrame {
        inner: Arc::new(Mutex::new(Some(new_inner))),
      })
    })
  }

  /// Create a VideoFrame from an internal Frame (for encoder output)
  pub fn from_internal(frame: Frame, timestamp_us: i64, duration_us: Option<i64>) -> Self {
    let width = frame.width();
    let height = frame.height();

    let inner = VideoFrameInner {
      frame,
      timestamp_us,
      duration_us,
      display_width: width,
      display_height: height,
      color_space: VideoColorSpace::default(),
      closed: false,
    };

    Self {
      inner: Arc::new(Mutex::new(Some(inner))),
    }
  }

  /// Get the pixel format
  #[napi(getter)]
  pub fn format(&self) -> Result<Option<VideoPixelFormat>> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(VideoPixelFormat::from_av_format(inner.frame.format())),
      _ => Ok(None),
    }
  }

  /// Get the coded width in pixels
  #[napi(getter)]
  pub fn coded_width(&self) -> Result<u32> {
    self.with_inner(|inner| Ok(inner.frame.width()))
  }

  /// Get the coded height in pixels
  #[napi(getter)]
  pub fn coded_height(&self) -> Result<u32> {
    self.with_inner(|inner| Ok(inner.frame.height()))
  }

  /// Get the display width in pixels
  #[napi(getter)]
  pub fn display_width(&self) -> Result<u32> {
    self.with_inner(|inner| Ok(inner.display_width))
  }

  /// Get the display height in pixels
  #[napi(getter)]
  pub fn display_height(&self) -> Result<u32> {
    self.with_inner(|inner| Ok(inner.display_height))
  }

  /// Get the coded rect (the region containing valid pixel data)
  /// Returns DOMRectReadOnly per W3C WebCodecs spec
  /// Throws InvalidStateError if the VideoFrame is closed
  #[napi(getter)]
  pub fn coded_rect(&self) -> Result<DOMRectReadOnly> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(DOMRectReadOnly {
        x: 0.0,
        y: 0.0,
        width: inner.frame.width() as f64,
        height: inner.frame.height() as f64,
      }),
      _ => Err(invalid_state_error("VideoFrame is closed")),
    }
  }

  /// Get the visible rect (the region of coded data that should be displayed)
  /// Returns DOMRectReadOnly per W3C WebCodecs spec
  /// Throws InvalidStateError if the VideoFrame is closed
  #[napi(getter)]
  pub fn visible_rect(&self) -> Result<DOMRectReadOnly> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => Ok(DOMRectReadOnly {
        x: 0.0,
        y: 0.0,
        width: inner.display_width as f64,
        height: inner.display_height as f64,
      }),
      _ => Err(invalid_state_error("VideoFrame is closed")),
    }
  }

  /// Get the presentation timestamp in microseconds
  #[napi(getter)]
  pub fn timestamp(&self) -> Result<i64> {
    self.with_inner(|inner| Ok(inner.timestamp_us))
  }

  /// Get the duration in microseconds
  #[napi(getter)]
  pub fn duration(&self) -> Result<Option<i64>> {
    self.with_inner(|inner| Ok(inner.duration_us))
  }

  /// Get the color space parameters
  #[napi(getter)]
  pub fn color_space(&self) -> Result<VideoColorSpace> {
    self.with_inner(|inner| Ok(inner.color_space.clone()))
  }

  /// Get whether this VideoFrame has been closed (W3C WebCodecs spec)
  #[napi(getter)]
  pub fn closed(&self) -> Result<bool> {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    Ok(guard.is_none() || guard.as_ref().is_none_or(|i| i.closed))
  }

  /// Calculate the allocation size needed for copyTo
  #[napi]
  pub fn allocation_size(&self, options: Option<VideoFrameCopyToOptions>) -> Result<u32> {
    self.with_inner(|inner| {
      let format = options.as_ref().and_then(|o| o.format).unwrap_or_else(|| {
        VideoPixelFormat::from_av_format(inner.frame.format()).unwrap_or(VideoPixelFormat::I420)
      });

      let width = inner.frame.width();
      let height = inner.frame.height();

      Ok(Self::calculate_buffer_size(format, width, height))
    })
  }

  /// Copy frame data to a Uint8Array
  ///
  /// Returns a Promise that resolves with an array of PlaneLayout objects.
  /// Options can specify target format. The rect parameter is not yet implemented.
  #[napi]
  pub async fn copy_to(
    &self,
    mut destination: Uint8Array,
    options: Option<VideoFrameCopyToOptions>,
  ) -> Result<Vec<PlaneLayout>> {
    // Throw error if rect is specified since it's not implemented
    if options.as_ref().and_then(|o| o.rect.as_ref()).is_some() {
      return Err(Error::new(
        Status::GenericFailure,
        "VideoFrame.copyTo rect parameter is not yet implemented",
      ));
    }

    // Get format, size info and validate destination buffer (brief lock)
    let (format, width, height, size) = {
      let guard = self
        .inner
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      let inner = match guard.as_ref() {
        Some(inner) if !inner.closed => inner,
        _ => {
          return Err(invalid_state_error("VideoFrame is closed"))
        }
      };

      let format =
        VideoPixelFormat::from_av_format(inner.frame.format()).unwrap_or(VideoPixelFormat::I420);
      let width = inner.frame.width();
      let height = inner.frame.height();
      let size = Self::calculate_buffer_size(format, width, height) as usize;

      (format, width, height, size)
    };

    if destination.len() < size {
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "Buffer too small: need {} bytes, got {}",
          size,
          destination.len()
        ),
      ));
    }

    // Clone inner Arc for the blocking thread
    let inner_clone = self.inner.clone();

    // Perform the copy in a blocking thread to not block the event loop
    let copied_data = spawn_blocking(move || -> Result<Vec<u8>> {
      let guard = inner_clone
        .lock()
        .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

      let inner = match guard.as_ref() {
        Some(inner) if !inner.closed => inner,
        _ => {
          return Err(invalid_state_error("VideoFrame is closed"))
        }
      };

      // Allocate temporary buffer and copy frame data
      let mut temp_buffer = vec![0u8; size];
      inner
        .frame
        .copy_to_buffer(&mut temp_buffer)
        .map_err(|e| Error::new(Status::GenericFailure, format!("Copy failed: {}", e)))?;

      Ok(temp_buffer)
    })
    .await
    .map_err(|e| Error::new(Status::GenericFailure, format!("Copy task failed: {}", e)))??;

    // Copy from temp buffer to destination (this is fast since destination is already allocated)
    let dest_buffer = unsafe { destination.as_mut() };
    dest_buffer[..size].copy_from_slice(&copied_data);

    // Calculate and return plane layouts
    let layouts = Self::get_plane_layouts(format, width, height);
    Ok(layouts)
  }

  /// Calculate plane layouts for a given format
  fn get_plane_layouts(format: VideoPixelFormat, width: u32, height: u32) -> Vec<PlaneLayout> {
    match format {
      VideoPixelFormat::I420 => {
        let y_size = width * height;
        let uv_stride = width / 2;
        let uv_size = uv_stride * (height / 2);
        vec![
          PlaneLayout {
            offset: 0,
            stride: width,
          },
          PlaneLayout {
            offset: y_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size,
            stride: uv_stride,
          },
        ]
      }
      VideoPixelFormat::I420A => {
        let y_size = width * height;
        let uv_stride = width / 2;
        let uv_size = uv_stride * (height / 2);
        vec![
          PlaneLayout {
            offset: 0,
            stride: width,
          },
          PlaneLayout {
            offset: y_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size * 2,
            stride: width,
          },
        ]
      }
      VideoPixelFormat::I422 => {
        let y_size = width * height;
        let uv_stride = width / 2;
        let uv_size = uv_stride * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: width,
          },
          PlaneLayout {
            offset: y_size,
            stride: uv_stride,
          },
          PlaneLayout {
            offset: y_size + uv_size,
            stride: uv_stride,
          },
        ]
      }
      VideoPixelFormat::I444 => {
        let plane_size = width * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: width,
          },
          PlaneLayout {
            offset: plane_size,
            stride: width,
          },
          PlaneLayout {
            offset: plane_size * 2,
            stride: width,
          },
        ]
      }
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => {
        let y_size = width * height;
        vec![
          PlaneLayout {
            offset: 0,
            stride: width,
          },
          PlaneLayout {
            offset: y_size,
            stride: width,
          },
        ]
      }
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => {
        vec![PlaneLayout {
          offset: 0,
          stride: width * 4,
        }]
      }
    }
  }

  /// Clone this VideoFrame
  #[napi(js_name = "clone")]
  pub fn clone_frame(&self) -> Result<VideoFrame> {
    self.with_inner(|inner| {
      let cloned_frame = inner
        .frame
        .try_clone()
        .map_err(|e| Error::new(Status::GenericFailure, format!("Clone failed: {}", e)))?;

      let new_inner = VideoFrameInner {
        frame: cloned_frame,
        timestamp_us: inner.timestamp_us,
        duration_us: inner.duration_us,
        display_width: inner.display_width,
        display_height: inner.display_height,
        color_space: inner.color_space.clone(),
        closed: false,
      };

      Ok(VideoFrame {
        inner: Arc::new(Mutex::new(Some(new_inner))),
      })
    })
  }

  /// Close and release resources
  #[napi]
  pub fn close(&self) -> Result<()> {
    let mut guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    if let Some(inner) = guard.as_mut() {
      inner.closed = true;
    }
    *guard = None;

    Ok(())
  }

  // ========================================================================
  // Internal helpers (crate-visible only)
  // ========================================================================

  /// Borrow internal frame for encoding (crate internal use)
  #[allow(dead_code)]
  pub(crate) fn with_frame<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&Frame) -> R,
  {
    self.with_inner(|inner| Ok(f(&inner.frame)))
  }

  fn with_inner<F, R>(&self, f: F) -> Result<R>
  where
    F: FnOnce(&VideoFrameInner) -> Result<R>,
  {
    let guard = self
      .inner
      .lock()
      .map_err(|_| Error::new(Status::GenericFailure, "Lock poisoned"))?;

    match guard.as_ref() {
      Some(inner) if !inner.closed => f(inner),
      _ => Err(invalid_state_error("VideoFrame is closed")),
    }
  }

  fn calculate_buffer_size(format: VideoPixelFormat, width: u32, height: u32) -> u32 {
    let w = width;
    let h = height;

    match format {
      VideoPixelFormat::I420 => {
        // Y plane + U plane + V plane (4:2:0)
        w * h + (w / 2) * (h / 2) * 2
      }
      VideoPixelFormat::I420A => {
        // Y + U + V + A (4:2:0 with alpha)
        w * h * 2 + (w / 2) * (h / 2) * 2
      }
      VideoPixelFormat::I422 => {
        // Y plane + U plane + V plane (4:2:2)
        w * h + (w / 2) * h * 2
      }
      VideoPixelFormat::I444 => {
        // Y + U + V (4:4:4)
        w * h * 3
      }
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => {
        // Y plane + interleaved UV/VU plane
        w * h + w * (h / 2)
      }
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => {
        // 4 bytes per pixel
        w * h * 4
      }
    }
  }

  fn copy_data_to_frame(
    frame: &mut Frame,
    data: &[u8],
    format: VideoPixelFormat,
    width: u32,
    height: u32,
  ) -> Result<()> {
    let expected_size = Self::calculate_buffer_size(format, width, height) as usize;

    if data.len() < expected_size {
      return Err(Error::new(
        Status::GenericFailure,
        format!(
          "Input data too small: need {} bytes, got {}",
          expected_size,
          data.len()
        ),
      ));
    }

    // Get all linesizes first to avoid borrow conflicts
    let linesize0 = frame.linesize(0) as usize;
    let linesize1 = frame.linesize(1) as usize;
    let linesize2 = frame.linesize(2) as usize;
    let linesize3 = frame.linesize(3) as usize;

    match format {
      VideoPixelFormat::I420 | VideoPixelFormat::I420A => {
        let y_size = (width * height) as usize;
        let u_width = (width / 2) as usize;
        let u_height = (height / 2) as usize;
        let v_offset = y_size + u_width * u_height;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * width as usize;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy U plane
        {
          let u_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get U plane"))?;
          for row in 0..u_height {
            let src_start = y_size + row * u_width;
            let dst_start = row * linesize1;
            u_plane[dst_start..dst_start + u_width]
              .copy_from_slice(&data[src_start..src_start + u_width]);
          }
        }

        // Copy V plane
        {
          let v_plane = frame
            .plane_data_mut(2)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get V plane"))?;
          for row in 0..u_height {
            let src_start = v_offset + row * u_width;
            let dst_start = row * linesize2;
            v_plane[dst_start..dst_start + u_width]
              .copy_from_slice(&data[src_start..src_start + u_width]);
          }
        }

        // Copy A plane if present
        if format == VideoPixelFormat::I420A {
          let a_offset = v_offset + u_width * u_height;
          let a_plane = frame
            .plane_data_mut(3)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get A plane"))?;
          for row in 0..height as usize {
            let src_start = a_offset + row * width as usize;
            let dst_start = row * linesize3;
            a_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }
      }
      VideoPixelFormat::NV12 | VideoPixelFormat::NV21 => {
        // NV12: Y plane + interleaved UV
        // NV21: Y plane + interleaved VU (same layout, just U/V swapped)
        let y_size = (width * height) as usize;
        let uv_height = (height / 2) as usize;

        // Copy Y plane
        {
          let y_plane = frame
            .plane_data_mut(0)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get Y plane"))?;
          for row in 0..height as usize {
            let src_start = row * width as usize;
            let dst_start = row * linesize0;
            y_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }

        // Copy UV/VU plane (interleaved)
        {
          let uv_plane = frame
            .plane_data_mut(1)
            .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get UV/VU plane"))?;
          for row in 0..uv_height {
            let src_start = y_size + row * width as usize;
            let dst_start = row * linesize1;
            uv_plane[dst_start..dst_start + width as usize]
              .copy_from_slice(&data[src_start..src_start + width as usize]);
          }
        }
      }
      VideoPixelFormat::RGBA
      | VideoPixelFormat::RGBX
      | VideoPixelFormat::BGRA
      | VideoPixelFormat::BGRX => {
        let row_bytes = (width * 4) as usize;

        // Copy packed RGBA data
        let plane = frame
          .plane_data_mut(0)
          .ok_or_else(|| Error::new(Status::GenericFailure, "Failed to get plane"))?;
        for row in 0..height as usize {
          let src_start = row * row_bytes;
          let dst_start = row * linesize0;
          plane[dst_start..dst_start + row_bytes]
            .copy_from_slice(&data[src_start..src_start + row_bytes]);
        }
      }
      _ => {
        return Err(Error::new(
          Status::GenericFailure,
          format!("Unsupported format: {:?}", format),
        ));
      }
    }

    Ok(())
  }
}
