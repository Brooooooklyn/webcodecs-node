//! Safe wrapper around FFmpeg hardware frames context
//!
//! Provides GPU frame pool management for zero-copy hardware encoding.
//! Works with VideoToolbox (macOS), CUDA/NVENC (NVIDIA), VAAPI (Linux), etc.

use crate::ffi::{
  AVBufferRef, AVHWDeviceType, AVPixelFormat, FFmpegError,
  accessors::{
    ffhwframes_get_format, ffhwframes_get_height, ffhwframes_get_sw_format, ffhwframes_get_width,
    ffhwframes_set_format, ffhwframes_set_height, ffhwframes_set_initial_pool_size,
    ffhwframes_set_sw_format, ffhwframes_set_width,
  },
  avutil::av_buffer_unref,
  hwaccel::{
    av_hwframe_ctx_alloc, av_hwframe_ctx_init, av_hwframe_get_buffer, av_hwframe_transfer_data,
  },
};
use std::ptr::NonNull;

use super::{CodecError, CodecResult, frame::Frame, hwdevice::HwDeviceContext};

/// Configuration for creating a hardware frames context
#[derive(Debug, Clone)]
pub struct HwFrameConfig {
  /// Width of frames in the pool
  pub width: u32,
  /// Height of frames in the pool
  pub height: u32,
  /// Software pixel format (format of CPU frames before upload)
  pub sw_format: AVPixelFormat,
  /// Hardware pixel format (format on GPU) - if None, auto-detected from device type
  pub hw_format: Option<AVPixelFormat>,
  /// Initial pool size (number of pre-allocated frames)
  pub pool_size: u32,
}

impl Default for HwFrameConfig {
  fn default() -> Self {
    Self {
      width: 1920,
      height: 1080,
      sw_format: AVPixelFormat::Nv12, // Most hardware encoders prefer NV12
      hw_format: None,                // Auto-detect
      pool_size: 20,                  // Pre-allocate 20 frames for smooth encoding
    }
  }
}

/// Safe wrapper around FFmpeg hardware frames context
///
/// Manages a pool of GPU-resident frames for zero-copy hardware encoding.
/// Frames allocated from this context can be used directly with hardware encoders.
pub struct HwFrameContext {
  ptr: NonNull<AVBufferRef>,
  device_type: AVHWDeviceType,
  sw_format: AVPixelFormat,
  hw_format: AVPixelFormat,
  width: u32,
  height: u32,
}

impl HwFrameContext {
  /// Create a new hardware frames context
  ///
  /// # Arguments
  /// * `device` - The hardware device context to create frames for
  /// * `config` - Configuration for the frame pool
  ///
  /// # Returns
  /// A new hardware frames context on success
  pub fn new(device: &HwDeviceContext, config: HwFrameConfig) -> CodecResult<Self> {
    // Allocate the hardware frames context
    let frames_ref = unsafe { av_hwframe_ctx_alloc(device.as_ptr()) };
    if frames_ref.is_null() {
      return Err(CodecError::HardwareError(
        "Failed to allocate hardware frames context".into(),
      ));
    }

    let device_type = device.device_type();

    // Determine hardware pixel format based on device type if not specified
    let hw_format = config
      .hw_format
      .unwrap_or_else(|| get_hw_pixel_format(device_type));

    // Configure the frames context
    unsafe {
      ffhwframes_set_format(frames_ref, hw_format.as_raw());
      ffhwframes_set_sw_format(frames_ref, config.sw_format.as_raw());
      ffhwframes_set_width(frames_ref, config.width as i32);
      ffhwframes_set_height(frames_ref, config.height as i32);
      ffhwframes_set_initial_pool_size(frames_ref, config.pool_size as i32);
    }

    // Initialize the frames context
    // Note: FFmpeg may log warnings about unsupported pixel formats during probing.
    // These warnings are redirected to tracing and filtered appropriately.
    let ret = unsafe { av_hwframe_ctx_init(frames_ref) };

    if ret < 0 {
      // Free the context on failure
      unsafe {
        let mut ptr = frames_ref;
        av_buffer_unref(&mut ptr);
      }
      return Err(CodecError::HardwareError(format!(
        "Failed to initialize hardware frames context: {}",
        FFmpegError::from_code(ret)
      )));
    }

    Ok(Self {
      ptr: NonNull::new(frames_ref).unwrap(), // Safe: we checked for null above
      device_type,
      sw_format: config.sw_format,
      hw_format,
      width: config.width,
      height: config.height,
    })
  }

  /// Allocate a new frame from the hardware frame pool
  ///
  /// The returned frame is GPU-resident and can be used directly with hardware encoders.
  pub fn allocate_frame(&self) -> CodecResult<Frame> {
    let mut frame = Frame::new()?;

    let ret = unsafe { av_hwframe_get_buffer(self.ptr.as_ptr(), frame.as_mut_ptr(), 0) };
    if ret < 0 {
      return Err(CodecError::HardwareError(format!(
        "Failed to allocate hardware frame: {}",
        FFmpegError::from_code(ret)
      )));
    }

    Ok(frame)
  }

  /// Upload a CPU frame to GPU memory
  ///
  /// # Arguments
  /// * `sw_frame` - A software (CPU) frame with pixel data
  ///
  /// # Returns
  /// A hardware (GPU) frame containing the uploaded data
  ///
  /// The input frame must have the same dimensions and a compatible pixel format.
  /// Most hardware encoders expect NV12 format for optimal performance.
  pub fn upload_frame(&self, sw_frame: &Frame) -> CodecResult<Frame> {
    // Allocate a GPU frame from the pool
    let mut hw_frame = self.allocate_frame()?;

    // Transfer data from CPU to GPU
    let ret = unsafe { av_hwframe_transfer_data(hw_frame.as_mut_ptr(), sw_frame.as_ptr(), 0) };
    if ret < 0 {
      return Err(CodecError::HardwareError(format!(
        "Failed to upload frame to GPU: {}",
        FFmpegError::from_code(ret)
      )));
    }

    // Copy metadata from source frame
    hw_frame.set_pts(sw_frame.pts());
    hw_frame.set_duration(sw_frame.duration());

    Ok(hw_frame)
  }

  /// Get the raw pointer for attaching to encoder context
  #[inline]
  pub fn as_ptr(&self) -> *mut AVBufferRef {
    self.ptr.as_ptr()
  }

  /// Get the device type
  #[inline]
  pub fn device_type(&self) -> AVHWDeviceType {
    self.device_type
  }

  /// Get the software pixel format (CPU frame format)
  #[inline]
  pub fn sw_format(&self) -> AVPixelFormat {
    self.sw_format
  }

  /// Get the hardware pixel format (GPU frame format)
  #[inline]
  pub fn hw_format(&self) -> AVPixelFormat {
    self.hw_format
  }

  /// Get frame width
  #[inline]
  pub fn width(&self) -> u32 {
    self.width
  }

  /// Get frame height
  #[inline]
  pub fn height(&self) -> u32 {
    self.height
  }

  /// Get the actual configured width from the context
  #[inline]
  pub fn actual_width(&self) -> u32 {
    unsafe { ffhwframes_get_width(self.ptr.as_ptr()) as u32 }
  }

  /// Get the actual configured height from the context
  #[inline]
  pub fn actual_height(&self) -> u32 {
    unsafe { ffhwframes_get_height(self.ptr.as_ptr()) as u32 }
  }

  /// Get the actual software format from the context
  #[inline]
  pub fn actual_sw_format(&self) -> AVPixelFormat {
    let raw = unsafe { ffhwframes_get_sw_format(self.ptr.as_ptr()) };
    AVPixelFormat::from_raw(raw)
  }

  /// Get the actual hardware format from the context
  #[inline]
  pub fn actual_hw_format(&self) -> AVPixelFormat {
    let raw = unsafe { ffhwframes_get_format(self.ptr.as_ptr()) };
    AVPixelFormat::from_raw(raw)
  }
}

impl Drop for HwFrameContext {
  fn drop(&mut self) {
    unsafe {
      let mut ptr = self.ptr.as_ptr();
      av_buffer_unref(&mut ptr);
    }
  }
}

// Hardware frames contexts can be shared across threads
unsafe impl Send for HwFrameContext {}
unsafe impl Sync for HwFrameContext {}

impl std::fmt::Debug for HwFrameContext {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("HwFrameContext")
      .field("device_type", &self.device_type)
      .field("sw_format", &self.sw_format)
      .field("hw_format", &self.hw_format)
      .field("width", &self.width)
      .field("height", &self.height)
      .finish()
  }
}

/// Get the hardware pixel format for a given device type
fn get_hw_pixel_format(device_type: AVHWDeviceType) -> AVPixelFormat {
  match device_type {
    AVHWDeviceType::Videotoolbox => AVPixelFormat::Videotoolbox,
    AVHWDeviceType::Cuda => AVPixelFormat::Cuda,
    AVHWDeviceType::Vaapi => AVPixelFormat::Vaapi,
    AVHWDeviceType::Qsv => AVPixelFormat::Qsv,
    AVHWDeviceType::D3d11va => AVPixelFormat::D3d11,
    AVHWDeviceType::Dxva2 => AVPixelFormat::Dxva2Vld,
    AVHWDeviceType::Vulkan => AVPixelFormat::Vulkan,
    _ => AVPixelFormat::None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_hwframe_config_default() {
    let config = HwFrameConfig::default();
    assert_eq!(config.width, 1920);
    assert_eq!(config.height, 1080);
    assert_eq!(config.sw_format, AVPixelFormat::Nv12);
    assert_eq!(config.pool_size, 20);
  }

  #[test]
  #[cfg(target_os = "macos")]
  fn test_hwframe_context_creation_macos() {
    // Try to create a VideoToolbox hardware frames context
    let device = match HwDeviceContext::new(AVHWDeviceType::Videotoolbox) {
      Ok(d) => d,
      Err(_) => {
        println!("VideoToolbox not available, skipping test");
        return;
      }
    };

    let config = HwFrameConfig {
      width: 1280,
      height: 720,
      sw_format: AVPixelFormat::Nv12,
      hw_format: None,
      pool_size: 8,
    };

    let frames_ctx = HwFrameContext::new(&device, config);
    if let Ok(ctx) = frames_ctx {
      assert_eq!(ctx.width(), 1280);
      assert_eq!(ctx.height(), 720);
      assert_eq!(ctx.device_type(), AVHWDeviceType::Videotoolbox);
    } else {
      println!("Hardware frames context creation failed (may not be supported)");
    }
  }

  #[test]
  fn test_hw_pixel_format_mapping() {
    assert_eq!(
      get_hw_pixel_format(AVHWDeviceType::Videotoolbox),
      AVPixelFormat::Videotoolbox
    );
    assert_eq!(
      get_hw_pixel_format(AVHWDeviceType::Cuda),
      AVPixelFormat::Cuda
    );
    assert_eq!(
      get_hw_pixel_format(AVHWDeviceType::Vaapi),
      AVPixelFormat::Vaapi
    );
  }
}
