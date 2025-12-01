//! VideoFrame - WebCodecs API implementation
//!
//! Represents a frame of video data that can be displayed or encoded.
//! See: https://developer.mozilla.org/en-US/docs/Web/API/VideoFrame

use crate::codec::Frame;
use crate::ffi::AVPixelFormat;
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
            VideoPixelFormat::NV12 => 1,
            VideoPixelFormat::RGBA
            | VideoPixelFormat::RGBX
            | VideoPixelFormat::BGRA
            | VideoPixelFormat::BGRX => 4,
        }
    }
}

/// Video color space parameters (WebCodecs spec)
#[napi(object)]
#[derive(Debug, Clone, Default)]
pub struct VideoColorSpace {
    /// Color primaries (e.g., "bt709", "bt2020")
    pub primaries: Option<String>,
    /// Transfer function (e.g., "bt709", "srgb", "pq", "hlg")
    pub transfer: Option<String>,
    /// Matrix coefficients (e.g., "bt709", "bt2020-ncl")
    pub matrix: Option<String>,
    /// Full range flag
    pub full_range: Option<bool>,
}

/// Options for creating a VideoFrame
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameInit {
    /// Pixel format
    pub format: VideoPixelFormat,
    /// Coded width in pixels
    pub coded_width: u32,
    /// Coded height in pixels
    pub coded_height: u32,
    /// Timestamp in microseconds
    pub timestamp: i64,
    /// Duration in microseconds (optional)
    pub duration: Option<i64>,
    /// Display width (defaults to coded_width)
    pub display_width: Option<u32>,
    /// Display height (defaults to coded_height)
    pub display_height: Option<u32>,
    /// Color space parameters
    pub color_space: Option<VideoColorSpace>,
}

/// Options for copyTo operation
#[napi(object)]
#[derive(Debug, Clone)]
pub struct VideoFrameCopyToOptions {
    /// Target pixel format (for format conversion)
    pub format: Option<VideoPixelFormat>,
    /// Region to copy (not yet implemented)
    pub rect: Option<VideoFrameRect>,
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
    /// Create a new VideoFrame from raw data
    #[napi(constructor)]
    pub fn new(data: Buffer, init: VideoFrameInit) -> Result<Self> {
        let format = init.format.to_av_format();
        let width = init.coded_width;
        let height = init.coded_height;

        // Create internal frame
        let mut frame = Frame::new_video(width, height, format).map_err(|e| {
            Error::new(Status::GenericFailure, format!("Failed to create frame: {}", e))
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

        let inner = VideoFrameInner {
            frame,
            timestamp_us: init.timestamp,
            duration_us: init.duration,
            display_width,
            display_height,
            color_space: init.color_space.unwrap_or_default(),
            closed: false,
        };

        Ok(Self {
            inner: Arc::new(Mutex::new(Some(inner))),
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
        let guard = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        match guard.as_ref() {
            Some(inner) if !inner.closed => {
                Ok(VideoPixelFormat::from_av_format(inner.frame.format()))
            }
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

    /// Calculate the allocation size needed for copyTo
    #[napi]
    pub fn allocation_size(&self, options: Option<VideoFrameCopyToOptions>) -> Result<u32> {
        self.with_inner(|inner| {
            let format = options
                .as_ref()
                .and_then(|o| o.format)
                .unwrap_or_else(|| {
                    VideoPixelFormat::from_av_format(inner.frame.format())
                        .unwrap_or(VideoPixelFormat::I420)
                });

            let width = inner.frame.width();
            let height = inner.frame.height();

            Ok(Self::calculate_buffer_size(format, width, height))
        })
    }

    /// Copy frame data to a Uint8Array
    #[napi]
    pub fn copy_to(&self, destination: Uint8Array) -> Result<()> {
        self.with_inner_mut(|inner| {
            let size = Self::calculate_buffer_size(
                VideoPixelFormat::from_av_format(inner.frame.format())
                    .unwrap_or(VideoPixelFormat::I420),
                inner.frame.width(),
                inner.frame.height(),
            ) as usize;

            if destination.len() < size {
                return Err(Error::new(
                    Status::GenericFailure,
                    format!("Buffer too small: need {} bytes, got {}", size, destination.len()),
                ));
            }

            // Get mutable access to the destination buffer
            let dest_slice = destination.as_ref();
            let dest_ptr = dest_slice.as_ptr() as *mut u8;
            let dest_buffer = unsafe { std::slice::from_raw_parts_mut(dest_ptr, destination.len()) };

            inner.frame.copy_to_buffer(dest_buffer).map_err(|e| {
                Error::new(Status::GenericFailure, format!("Copy failed: {}", e))
            })?;

            Ok(())
        })
    }

    /// Clone this VideoFrame
    #[napi(js_name = "clone")]
    pub fn clone_frame(&self) -> Result<VideoFrame> {
        self.with_inner(|inner| {
            let cloned_frame = inner.frame.try_clone().map_err(|e| {
                Error::new(Status::GenericFailure, format!("Clone failed: {}", e))
            })?;

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
        let mut guard = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

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
        let guard = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        match guard.as_ref() {
            Some(inner) if !inner.closed => f(inner),
            _ => Err(Error::new(
                Status::GenericFailure,
                "VideoFrame is closed or invalid",
            )),
        }
    }

    fn with_inner_mut<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut VideoFrameInner) -> Result<R>,
    {
        let mut guard = self.inner.lock().map_err(|_| {
            Error::new(Status::GenericFailure, "Lock poisoned")
        })?;

        match guard.as_mut() {
            Some(inner) if !inner.closed => f(inner),
            _ => Err(Error::new(
                Status::GenericFailure,
                "VideoFrame is closed or invalid",
            )),
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
            VideoPixelFormat::NV12 => {
                // Y plane + interleaved UV plane
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
                    let y_plane = frame.plane_data_mut(0).ok_or_else(|| {
                        Error::new(Status::GenericFailure, "Failed to get Y plane")
                    })?;
                    for row in 0..height as usize {
                        let src_start = row * width as usize;
                        let dst_start = row * linesize0;
                        y_plane[dst_start..dst_start + width as usize]
                            .copy_from_slice(&data[src_start..src_start + width as usize]);
                    }
                }

                // Copy U plane
                {
                    let u_plane = frame.plane_data_mut(1).ok_or_else(|| {
                        Error::new(Status::GenericFailure, "Failed to get U plane")
                    })?;
                    for row in 0..u_height {
                        let src_start = y_size + row * u_width;
                        let dst_start = row * linesize1;
                        u_plane[dst_start..dst_start + u_width]
                            .copy_from_slice(&data[src_start..src_start + u_width]);
                    }
                }

                // Copy V plane
                {
                    let v_plane = frame.plane_data_mut(2).ok_or_else(|| {
                        Error::new(Status::GenericFailure, "Failed to get V plane")
                    })?;
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
                    let a_plane = frame.plane_data_mut(3).ok_or_else(|| {
                        Error::new(Status::GenericFailure, "Failed to get A plane")
                    })?;
                    for row in 0..height as usize {
                        let src_start = a_offset + row * width as usize;
                        let dst_start = row * linesize3;
                        a_plane[dst_start..dst_start + width as usize]
                            .copy_from_slice(&data[src_start..src_start + width as usize]);
                    }
                }
            }
            VideoPixelFormat::NV12 => {
                let y_size = (width * height) as usize;
                let uv_height = (height / 2) as usize;

                // Copy Y plane
                {
                    let y_plane = frame.plane_data_mut(0).ok_or_else(|| {
                        Error::new(Status::GenericFailure, "Failed to get Y plane")
                    })?;
                    for row in 0..height as usize {
                        let src_start = row * width as usize;
                        let dst_start = row * linesize0;
                        y_plane[dst_start..dst_start + width as usize]
                            .copy_from_slice(&data[src_start..src_start + width as usize]);
                    }
                }

                // Copy UV plane (interleaved)
                {
                    let uv_plane = frame.plane_data_mut(1).ok_or_else(|| {
                        Error::new(Status::GenericFailure, "Failed to get UV plane")
                    })?;
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
                let plane = frame.plane_data_mut(0).ok_or_else(|| {
                    Error::new(Status::GenericFailure, "Failed to get plane")
                })?;
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
