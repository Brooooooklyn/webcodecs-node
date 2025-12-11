//! Safe wrapper around FFmpeg AVFrame
//!
//! Provides RAII-based memory management and safe access to frame data.

use crate::ffi::{
  self, AVColorPrimaries, AVColorRange, AVColorSpace, AVColorTransferCharacteristic, AVFrame,
  AVPictureType, AVPixelFormat, AVSampleFormat,
  accessors::{
    ff_get_audio_buffer_size,
    ffframe_data,
    ffframe_extended_data_plane,
    ffframe_get_channel_layout,
    ffframe_get_channels,
    ffframe_get_color_primaries,
    ffframe_get_color_range,
    ffframe_get_color_trc,
    ffframe_get_colorspace,
    ffframe_get_duration,
    ffframe_get_format,
    ffframe_get_height,
    ffframe_get_key_frame,
    // Audio accessors
    ffframe_get_nb_samples,
    ffframe_get_pict_type,
    ffframe_get_pts,
    ffframe_get_sample_rate,
    ffframe_get_width,
    ffframe_linesize,
    ffframe_set_channel_layout,
    ffframe_set_channels,
    ffframe_set_color_primaries,
    ffframe_set_color_range,
    ffframe_set_color_trc,
    ffframe_set_colorspace,
    ffframe_set_duration,
    ffframe_set_format,
    ffframe_set_height,
    ffframe_set_nb_samples,
    ffframe_set_pict_type,
    ffframe_set_pts,
    ffframe_set_sample_rate,
    ffframe_set_width,
  },
  avutil::{av_frame_alloc, av_frame_clone, av_frame_free, av_frame_get_buffer, av_frame_unref},
};
use std::ptr::NonNull;

use super::CodecError;

/// Safe wrapper around AVFrame with RAII cleanup
pub struct Frame {
  ptr: NonNull<AVFrame>,
}

impl Frame {
  /// Allocate a new empty frame
  pub fn new() -> Result<Self, CodecError> {
    let ptr = unsafe { av_frame_alloc() };
    NonNull::new(ptr)
      .map(|ptr| Self { ptr })
      .ok_or(CodecError::AllocationFailed("AVFrame"))
  }

  /// Allocate a frame with buffer for the given video format and dimensions
  pub fn new_video(width: u32, height: u32, format: AVPixelFormat) -> Result<Self, CodecError> {
    let mut frame = Self::new()?;

    unsafe {
      ffframe_set_width(frame.as_mut_ptr(), width as i32);
      ffframe_set_height(frame.as_mut_ptr(), height as i32);
      ffframe_set_format(frame.as_mut_ptr(), format.as_raw());
    }

    // Allocate buffer with 32-byte alignment for SIMD
    let ret = unsafe { av_frame_get_buffer(frame.as_mut_ptr(), 32) };
    ffi::check_error(ret)?;

    Ok(frame)
  }

  /// Allocate a frame with buffer for audio samples
  pub fn new_audio(
    nb_samples: u32,
    channels: u32,
    sample_rate: u32,
    format: AVSampleFormat,
  ) -> Result<Self, CodecError> {
    let mut frame = Self::new()?;

    unsafe {
      ffframe_set_nb_samples(frame.as_mut_ptr(), nb_samples as i32);
      ffframe_set_channels(frame.as_mut_ptr(), channels as i32);
      ffframe_set_sample_rate(frame.as_mut_ptr(), sample_rate as i32);
      ffframe_set_format(frame.as_mut_ptr(), format.as_raw());
    }

    // Allocate buffer with 32-byte alignment for SIMD
    let ret = unsafe { av_frame_get_buffer(frame.as_mut_ptr(), 32) };
    ffi::check_error(ret)?;

    Ok(frame)
  }

  /// Create a Frame from a raw pointer (takes ownership)
  ///
  /// # Safety
  /// The pointer must be a valid AVFrame allocated by FFmpeg
  pub unsafe fn from_raw(ptr: *mut AVFrame) -> Option<Self> {
    NonNull::new(ptr).map(|ptr| Self { ptr })
  }

  /// Get the raw pointer (for FFmpeg API calls)
  #[inline]
  pub fn as_ptr(&self) -> *const AVFrame {
    self.ptr.as_ptr()
  }

  /// Get the mutable raw pointer (for FFmpeg API calls)
  #[inline]
  pub fn as_mut_ptr(&mut self) -> *mut AVFrame {
    self.ptr.as_ptr()
  }

  /// Consume the Frame and return the raw pointer
  /// The caller is responsible for freeing the frame
  pub fn into_raw(self) -> *mut AVFrame {
    let ptr = self.ptr.as_ptr();
    std::mem::forget(self);
    ptr
  }

  // ========================================================================
  // Dimensions and Format
  // ========================================================================

  /// Get frame width
  #[inline]
  pub fn width(&self) -> u32 {
    unsafe { ffframe_get_width(self.as_ptr()) as u32 }
  }

  /// Get frame height
  #[inline]
  pub fn height(&self) -> u32 {
    unsafe { ffframe_get_height(self.as_ptr()) as u32 }
  }

  /// Set frame dimensions
  pub fn set_dimensions(&mut self, width: u32, height: u32) {
    unsafe {
      ffframe_set_width(self.as_mut_ptr(), width as i32);
      ffframe_set_height(self.as_mut_ptr(), height as i32);
    }
  }

  /// Get pixel format
  pub fn format(&self) -> AVPixelFormat {
    let fmt = unsafe { ffframe_get_format(self.as_ptr()) };
    // Safe conversion - unknown formats become None
    match fmt {
      // 8-bit YUV planar formats
      0 => AVPixelFormat::Yuv420p,
      4 => AVPixelFormat::Yuv422p,
      5 => AVPixelFormat::Yuv444p,
      33 => AVPixelFormat::Yuva420p,
      78 => AVPixelFormat::Yuva422p,
      79 => AVPixelFormat::Yuva444p,
      // Semi-planar formats
      23 => AVPixelFormat::Nv12,
      24 => AVPixelFormat::Nv21,
      // RGB formats
      2 => AVPixelFormat::Rgb24,
      3 => AVPixelFormat::Bgr24,
      25 => AVPixelFormat::Argb,
      26 => AVPixelFormat::Rgba,
      27 => AVPixelFormat::Abgr,
      28 => AVPixelFormat::Bgra,
      // 10-bit YUV formats
      62 => AVPixelFormat::Yuv420p10le,
      64 => AVPixelFormat::Yuv422p10le,
      68 => AVPixelFormat::Yuv444p10le,
      87 => AVPixelFormat::Yuva420p10le,
      89 => AVPixelFormat::Yuva422p10le,
      91 => AVPixelFormat::Yuva444p10le,
      // 12-bit YUV formats
      123 => AVPixelFormat::Yuv420p12le,
      127 => AVPixelFormat::Yuv422p12le,
      131 => AVPixelFormat::Yuv444p12le,
      _ => AVPixelFormat::None,
    }
  }

  /// Set pixel format
  pub fn set_format(&mut self, format: AVPixelFormat) {
    unsafe { ffframe_set_format(self.as_mut_ptr(), format.as_raw()) }
  }

  // ========================================================================
  // Timestamps
  // ========================================================================

  /// Get presentation timestamp (in time_base units)
  #[inline]
  pub fn pts(&self) -> i64 {
    unsafe { ffframe_get_pts(self.as_ptr()) }
  }

  /// Set presentation timestamp
  #[inline]
  pub fn set_pts(&mut self, pts: i64) {
    unsafe { ffframe_set_pts(self.as_mut_ptr(), pts) }
  }

  /// Get duration (in time_base units)
  #[inline]
  pub fn duration(&self) -> i64 {
    unsafe { ffframe_get_duration(self.as_ptr()) }
  }

  /// Set duration
  #[inline]
  pub fn set_duration(&mut self, duration: i64) {
    unsafe { ffframe_set_duration(self.as_mut_ptr(), duration) }
  }

  // ========================================================================
  // Frame Type
  // ========================================================================

  /// Check if this is a key frame
  #[inline]
  pub fn is_key_frame(&self) -> bool {
    unsafe { ffframe_get_key_frame(self.as_ptr()) != 0 }
  }

  /// Get picture type (I, P, B, etc.)
  pub fn pict_type(&self) -> AVPictureType {
    let t = unsafe { ffframe_get_pict_type(self.as_ptr()) };
    match t {
      1 => AVPictureType::I,
      2 => AVPictureType::P,
      3 => AVPictureType::B,
      _ => AVPictureType::None,
    }
  }

  /// Set picture type (I, P, B, etc.) - used to force keyframes
  pub fn set_pict_type(&mut self, pict_type: AVPictureType) {
    let t = match pict_type {
      AVPictureType::None => 0,
      AVPictureType::I => 1,
      AVPictureType::P => 2,
      AVPictureType::B => 3,
      AVPictureType::S => 4,
      AVPictureType::Si => 5,
      AVPictureType::Sp => 6,
      AVPictureType::Bi => 7,
    };
    unsafe { ffframe_set_pict_type(self.as_mut_ptr(), t) }
  }

  // ========================================================================
  // Color Space
  // ========================================================================

  /// Get color primaries
  pub fn color_primaries(&self) -> AVColorPrimaries {
    let p = unsafe { ffframe_get_color_primaries(self.as_ptr()) };
    match p {
      1 => AVColorPrimaries::Bt709,
      5 => AVColorPrimaries::Bt470bg,
      6 => AVColorPrimaries::Smpte170m,
      9 => AVColorPrimaries::Bt2020,
      _ => AVColorPrimaries::Unspecified,
    }
  }

  /// Set color primaries
  pub fn set_color_primaries(&mut self, primaries: AVColorPrimaries) {
    unsafe { ffframe_set_color_primaries(self.as_mut_ptr(), primaries as i32) }
  }

  /// Get color transfer characteristic
  pub fn color_trc(&self) -> AVColorTransferCharacteristic {
    let t = unsafe { ffframe_get_color_trc(self.as_ptr()) };
    match t {
      1 => AVColorTransferCharacteristic::Bt709,
      6 => AVColorTransferCharacteristic::Smpte170m,
      13 => AVColorTransferCharacteristic::Iec61966_2_1,
      16 => AVColorTransferCharacteristic::Smpte2084,
      18 => AVColorTransferCharacteristic::AribStdB67,
      _ => AVColorTransferCharacteristic::Unspecified,
    }
  }

  /// Set color transfer characteristic
  pub fn set_color_trc(&mut self, trc: AVColorTransferCharacteristic) {
    unsafe { ffframe_set_color_trc(self.as_mut_ptr(), trc as i32) }
  }

  /// Get color space (matrix coefficients)
  pub fn colorspace(&self) -> AVColorSpace {
    let s = unsafe { ffframe_get_colorspace(self.as_ptr()) };
    match s {
      0 => AVColorSpace::Rgb,
      1 => AVColorSpace::Bt709,
      5 => AVColorSpace::Bt470bg,
      6 => AVColorSpace::Smpte170m,
      9 => AVColorSpace::Bt2020Ncl,
      _ => AVColorSpace::Unspecified,
    }
  }

  /// Set color space
  pub fn set_colorspace(&mut self, colorspace: AVColorSpace) {
    unsafe { ffframe_set_colorspace(self.as_mut_ptr(), colorspace as i32) }
  }

  /// Get color range
  pub fn color_range(&self) -> AVColorRange {
    let r = unsafe { ffframe_get_color_range(self.as_ptr()) };
    match r {
      1 => AVColorRange::Mpeg,
      2 => AVColorRange::Jpeg,
      _ => AVColorRange::Unspecified,
    }
  }

  /// Set color range
  pub fn set_color_range(&mut self, range: AVColorRange) {
    unsafe { ffframe_set_color_range(self.as_mut_ptr(), range as i32) }
  }

  // ========================================================================
  // Audio Properties
  // ========================================================================

  /// Get number of audio samples per channel
  #[inline]
  pub fn nb_samples(&self) -> u32 {
    unsafe { ffframe_get_nb_samples(self.as_ptr()) as u32 }
  }

  /// Set number of audio samples per channel
  #[inline]
  pub fn set_nb_samples(&mut self, nb_samples: u32) {
    unsafe { ffframe_set_nb_samples(self.as_mut_ptr(), nb_samples as i32) }
  }

  /// Get audio sample rate in Hz
  #[inline]
  pub fn sample_rate(&self) -> u32 {
    unsafe { ffframe_get_sample_rate(self.as_ptr()) as u32 }
  }

  /// Set audio sample rate
  #[inline]
  pub fn set_sample_rate(&mut self, sample_rate: u32) {
    unsafe { ffframe_set_sample_rate(self.as_mut_ptr(), sample_rate as i32) }
  }

  /// Get number of audio channels
  #[inline]
  pub fn channels(&self) -> u32 {
    unsafe { ffframe_get_channels(self.as_ptr()) as u32 }
  }

  /// Set number of audio channels
  #[inline]
  pub fn set_channels(&mut self, channels: u32) {
    unsafe { ffframe_set_channels(self.as_mut_ptr(), channels as i32) }
  }

  /// Get audio channel layout mask
  #[inline]
  pub fn channel_layout(&self) -> u64 {
    unsafe { ffframe_get_channel_layout(self.as_ptr()) }
  }

  /// Set audio channel layout mask
  #[inline]
  pub fn set_channel_layout(&mut self, layout: u64) {
    unsafe { ffframe_set_channel_layout(self.as_mut_ptr(), layout) }
  }

  /// Get audio sample format
  pub fn sample_format(&self) -> AVSampleFormat {
    let fmt = unsafe { ffframe_get_format(self.as_ptr()) };
    match fmt {
      0 => AVSampleFormat::U8,
      1 => AVSampleFormat::S16,
      2 => AVSampleFormat::S32,
      3 => AVSampleFormat::Flt,
      4 => AVSampleFormat::Dbl,
      5 => AVSampleFormat::U8p,
      6 => AVSampleFormat::S16p,
      7 => AVSampleFormat::S32p,
      8 => AVSampleFormat::Fltp,
      9 => AVSampleFormat::Dblp,
      10 => AVSampleFormat::S64,
      11 => AVSampleFormat::S64p,
      _ => AVSampleFormat::None,
    }
  }

  /// Set audio sample format
  pub fn set_sample_format(&mut self, format: AVSampleFormat) {
    unsafe { ffframe_set_format(self.as_mut_ptr(), format.as_raw()) }
  }

  /// Check if this is an audio frame (has samples but no dimensions)
  #[inline]
  pub fn is_audio(&self) -> bool {
    self.nb_samples() > 0 && self.width() == 0 && self.height() == 0
  }

  /// Check if this is a video frame (has dimensions)
  #[inline]
  pub fn is_video(&self) -> bool {
    self.width() > 0 && self.height() > 0
  }

  /// Get the buffer size needed for audio data
  pub fn audio_buffer_size(&self) -> usize {
    if !self.is_audio() {
      return 0;
    }
    let size = unsafe {
      ff_get_audio_buffer_size(
        self.channels() as i32,
        self.nb_samples() as i32,
        self.sample_format().as_raw(),
        1, // alignment
      )
    };
    size.max(0) as usize
  }

  // ========================================================================
  // Data Access
  // ========================================================================

  /// Get pointer to plane data
  ///
  /// # Safety
  /// The returned pointer is valid only while the frame is alive and unmodified
  pub fn data(&self, plane: usize) -> *const u8 {
    unsafe { ffframe_data(self.ptr.as_ptr(), plane as i32) as *const u8 }
  }

  /// Get mutable pointer to plane data
  ///
  /// # Safety
  /// The returned pointer is valid only while the frame is alive
  pub fn data_mut(&mut self, plane: usize) -> *mut u8 {
    unsafe { ffframe_data(self.as_mut_ptr(), plane as i32) }
  }

  /// Get line size (stride) for a plane
  #[inline]
  pub fn linesize(&self, plane: usize) -> i32 {
    unsafe { ffframe_linesize(self.as_ptr(), plane as i32) }
  }

  /// Get plane data as a slice (read-only)
  ///
  /// Returns None if the plane doesn't exist or has no data
  pub fn plane_data(&self, plane: usize) -> Option<&[u8]> {
    let ptr = self.data(plane);
    if ptr.is_null() {
      return None;
    }

    let linesize = self.linesize(plane);
    if linesize <= 0 {
      return None;
    }

    let height = match plane {
      0 => self.height() as usize,
      1 | 2 => {
        // For YUV420, chroma planes are half height
        match self.format() {
          AVPixelFormat::Yuv420p | AVPixelFormat::Nv12 | AVPixelFormat::Yuva420p => {
            (self.height() as usize).div_ceil(2)
          }
          _ => self.height() as usize,
        }
      }
      3 => self.height() as usize, // Alpha plane
      _ => return None,
    };

    let size = linesize as usize * height;
    Some(unsafe { std::slice::from_raw_parts(ptr, size) })
  }

  /// Get mutable plane data as a slice
  pub fn plane_data_mut(&mut self, plane: usize) -> Option<&mut [u8]> {
    let ptr = self.data_mut(plane);
    if ptr.is_null() {
      return None;
    }

    let linesize = self.linesize(plane);
    if linesize <= 0 {
      return None;
    }

    let height = match plane {
      0 => self.height() as usize,
      1 | 2 => match self.format() {
        // 8-bit and 10/12-bit 4:2:0 formats: U/V planes are half height
        AVPixelFormat::Yuv420p
        | AVPixelFormat::Nv12
        | AVPixelFormat::Yuva420p
        | AVPixelFormat::Yuv420p10le
        | AVPixelFormat::Yuv420p12le
        | AVPixelFormat::Yuva420p10le => (self.height() as usize).div_ceil(2),
        _ => self.height() as usize,
      },
      3 => self.height() as usize,
      _ => return None,
    };

    let size = linesize as usize * height;
    Some(unsafe { std::slice::from_raw_parts_mut(ptr, size) })
  }

  /// Copy frame data to a contiguous buffer
  pub fn copy_to_buffer(&self, buffer: &mut [u8]) -> Result<usize, CodecError> {
    let format = self.format();
    let num_planes = format.num_planes();
    let mut offset = 0;

    for plane in 0..num_planes {
      if let Some(data) = self.plane_data(plane) {
        let linesize = self.linesize(plane) as usize;
        let width_bytes = match plane {
          0 => match format {
            // RGBA/BGRA formats: 4 bytes per pixel
            AVPixelFormat::Rgba
            | AVPixelFormat::Bgra
            | AVPixelFormat::Argb
            | AVPixelFormat::Abgr => self.width() as usize * 4,
            // RGB24/BGR24: 3 bytes per pixel
            AVPixelFormat::Rgb24 | AVPixelFormat::Bgr24 => self.width() as usize * 3,
            // 8-bit 4:2:2 formats: Y plane is full width (1 byte per sample)
            AVPixelFormat::Yuv422p | AVPixelFormat::Yuva422p => self.width() as usize,
            // 8-bit 4:4:4 formats: Y plane is full width (1 byte per sample)
            AVPixelFormat::Yuv444p | AVPixelFormat::Yuva444p => self.width() as usize,
            // 10-bit and 12-bit formats: 2 bytes per sample
            AVPixelFormat::Yuv420p10le
            | AVPixelFormat::Yuv420p12le
            | AVPixelFormat::Yuva420p10le
            | AVPixelFormat::Yuv422p10le
            | AVPixelFormat::Yuv422p12le
            | AVPixelFormat::Yuva422p10le
            | AVPixelFormat::Yuv444p10le
            | AVPixelFormat::Yuv444p12le
            | AVPixelFormat::Yuva444p10le => self.width() as usize * 2,
            // Default: 1 byte per pixel (8-bit YUV Y plane)
            _ => self.width() as usize,
          },
          _ => match format {
            // 8-bit 4:2:0 formats: U/V planes are half width
            AVPixelFormat::Yuv420p | AVPixelFormat::Yuva420p => (self.width() as usize).div_ceil(2),
            // NV12: UV interleaved, full width
            AVPixelFormat::Nv12 | AVPixelFormat::Nv21 => self.width() as usize,
            // 8-bit 4:2:2 formats: U/V planes are half width
            AVPixelFormat::Yuv422p | AVPixelFormat::Yuva422p => (self.width() as usize).div_ceil(2),
            // 8-bit 4:4:4 formats: U/V planes are full width
            AVPixelFormat::Yuv444p | AVPixelFormat::Yuva444p => self.width() as usize,
            // 10-bit and 12-bit 4:2:0: U/V planes are half width, 2 bytes per sample
            AVPixelFormat::Yuv420p10le
            | AVPixelFormat::Yuv420p12le
            | AVPixelFormat::Yuva420p10le => (self.width() as usize).div_ceil(2) * 2,
            // 10-bit and 12-bit 4:2:2: U/V planes are half width, 2 bytes per sample
            AVPixelFormat::Yuv422p10le
            | AVPixelFormat::Yuv422p12le
            | AVPixelFormat::Yuva422p10le => (self.width() as usize).div_ceil(2) * 2,
            // 10-bit and 12-bit 4:4:4: U/V planes are full width, 2 bytes per sample
            AVPixelFormat::Yuv444p10le
            | AVPixelFormat::Yuv444p12le
            | AVPixelFormat::Yuva444p10le => self.width() as usize * 2,
            _ => self.width() as usize,
          },
        };

        let height = match plane {
          0 | 3 => self.height() as usize,
          _ => match format {
            // 8-bit and 10/12-bit 4:2:0 formats: U/V planes are half height
            AVPixelFormat::Yuv420p
            | AVPixelFormat::Nv12
            | AVPixelFormat::Nv21
            | AVPixelFormat::Yuva420p
            | AVPixelFormat::Yuv420p10le
            | AVPixelFormat::Yuv420p12le
            | AVPixelFormat::Yuva420p10le => (self.height() as usize).div_ceil(2),
            _ => self.height() as usize,
          },
        };

        // Copy row by row (handle stride)
        for row in 0..height {
          let src_start = row * linesize;
          let dst_start = offset + row * width_bytes;

          if dst_start + width_bytes > buffer.len() {
            return Err(CodecError::InvalidConfig("Buffer too small".into()));
          }

          buffer[dst_start..dst_start + width_bytes]
            .copy_from_slice(&data[src_start..src_start + width_bytes]);
        }

        offset += width_bytes * height;
      }
    }

    Ok(offset)
  }

  // ========================================================================
  // Audio Data Access
  // ========================================================================

  /// Get pointer to audio channel data (for planar formats)
  ///
  /// For planar formats, each channel has its own buffer.
  /// For interleaved formats, all data is in channel 0.
  pub fn audio_data(&self, channel: usize) -> *const u8 {
    unsafe { ffframe_extended_data_plane(self.ptr.as_ptr() as *mut _, channel as i32) as *const u8 }
  }

  /// Get mutable pointer to audio channel data
  pub fn audio_data_mut(&mut self, channel: usize) -> *mut u8 {
    unsafe { ffframe_extended_data_plane(self.as_mut_ptr(), channel as i32) }
  }

  /// Get audio channel data as a slice (for planar formats)
  ///
  /// Returns None if the channel doesn't exist or has no data
  pub fn audio_channel_data(&self, channel: usize) -> Option<&[u8]> {
    if !self.is_audio() {
      return None;
    }

    let format = self.sample_format();
    let channels = self.channels() as usize;
    let nb_samples = self.nb_samples() as usize;

    // For planar formats, each channel has separate data
    // For interleaved formats, all data is in channel 0
    if format.is_planar() {
      if channel >= channels {
        return None;
      }
    } else if channel != 0 {
      return None;
    }

    let ptr = self.audio_data(channel);
    if ptr.is_null() {
      return None;
    }

    let bytes_per_sample = format.bytes_per_sample();
    let size = if format.is_planar() {
      nb_samples * bytes_per_sample
    } else {
      nb_samples * bytes_per_sample * channels
    };

    Some(unsafe { std::slice::from_raw_parts(ptr, size) })
  }

  /// Get mutable audio channel data as a slice
  pub fn audio_channel_data_mut(&mut self, channel: usize) -> Option<&mut [u8]> {
    if !self.is_audio() {
      return None;
    }

    let format = self.sample_format();
    let channels = self.channels() as usize;
    let nb_samples = self.nb_samples() as usize;

    if format.is_planar() {
      if channel >= channels {
        return None;
      }
    } else if channel != 0 {
      return None;
    }

    let ptr = self.audio_data_mut(channel);
    if ptr.is_null() {
      return None;
    }

    let bytes_per_sample = format.bytes_per_sample();
    let size = if format.is_planar() {
      nb_samples * bytes_per_sample
    } else {
      nb_samples * bytes_per_sample * channels
    };

    Some(unsafe { std::slice::from_raw_parts_mut(ptr, size) })
  }

  /// Copy audio data to a contiguous interleaved buffer
  ///
  /// Converts planar data to interleaved format if necessary
  pub fn copy_audio_to_buffer(&self, buffer: &mut [u8]) -> Result<usize, CodecError> {
    if !self.is_audio() {
      return Err(CodecError::InvalidConfig("Not an audio frame".into()));
    }

    let format = self.sample_format();
    let channels = self.channels() as usize;
    let nb_samples = self.nb_samples() as usize;
    let bytes_per_sample = format.bytes_per_sample();
    let total_size = nb_samples * channels * bytes_per_sample;

    if buffer.len() < total_size {
      return Err(CodecError::InvalidConfig("Buffer too small".into()));
    }

    if format.is_planar() {
      // Convert planar to interleaved
      for sample in 0..nb_samples {
        for ch in 0..channels {
          let src_offset = sample * bytes_per_sample;
          let dst_offset = (sample * channels + ch) * bytes_per_sample;

          if let Some(ch_data) = self.audio_channel_data(ch) {
            buffer[dst_offset..dst_offset + bytes_per_sample]
              .copy_from_slice(&ch_data[src_offset..src_offset + bytes_per_sample]);
          }
        }
      }
    } else {
      // Already interleaved, just copy
      if let Some(data) = self.audio_channel_data(0) {
        buffer[..total_size].copy_from_slice(&data[..total_size]);
      }
    }

    Ok(total_size)
  }

  // ========================================================================
  // Lifecycle
  // ========================================================================

  /// Unreference the frame data (but keep the frame structure)
  pub fn unref(&mut self) {
    unsafe { av_frame_unref(self.as_mut_ptr()) }
  }

  /// Clone the frame (creates a new reference to the same data)
  pub fn try_clone(&self) -> Result<Self, CodecError> {
    let ptr = unsafe { av_frame_clone(self.as_ptr()) };
    NonNull::new(ptr)
      .map(|ptr| Self { ptr })
      .ok_or(CodecError::AllocationFailed("frame clone"))
  }
}

impl Drop for Frame {
  fn drop(&mut self) {
    unsafe {
      let mut ptr = self.ptr.as_ptr();
      av_frame_free(&mut ptr);
    }
  }
}

// Frame data can be sent between threads
unsafe impl Send for Frame {}

// Multiple threads can read frame data concurrently (but not write)
// Note: FFmpeg contexts are NOT Sync, but frame data is
unsafe impl Sync for Frame {}

impl std::fmt::Debug for Frame {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Frame")
      .field("width", &self.width())
      .field("height", &self.height())
      .field("format", &self.format())
      .field("pts", &self.pts())
      .field("key_frame", &self.is_key_frame())
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_frame_allocation() {
    let frame = Frame::new().unwrap();
    assert_eq!(frame.width(), 0);
    assert_eq!(frame.height(), 0);
  }

  #[test]
  fn test_video_frame_allocation() {
    let frame = Frame::new_video(1920, 1080, AVPixelFormat::Yuv420p).unwrap();
    assert_eq!(frame.width(), 1920);
    assert_eq!(frame.height(), 1080);
    assert_eq!(frame.format(), AVPixelFormat::Yuv420p);

    // Check that plane data is allocated
    assert!(!frame.data(0).is_null());
    assert!(!frame.data(1).is_null());
    assert!(!frame.data(2).is_null());
  }
}
