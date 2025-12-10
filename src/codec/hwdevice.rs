//! Safe wrapper around FFmpeg hardware device context
//!
//! Provides hardware acceleration device management for VideoToolbox, CUDA, VAAPI, etc.

use crate::ffi::{
  self, AVBufferRef, AVHWDeviceType,
  avutil::av_buffer_unref,
  hwaccel::{av_hwdevice_ctx_create, av_hwdevice_get_type_name, av_hwdevice_iterate_types},
};
use std::ffi::CStr;
use std::ptr::NonNull;

use super::{CodecError, CodecResult};

/// Safe wrapper around FFmpeg hardware device context
pub struct HwDeviceContext {
  ptr: NonNull<AVBufferRef>,
  device_type: AVHWDeviceType,
}

impl HwDeviceContext {
  /// Create a new hardware device context
  pub fn new(device_type: AVHWDeviceType) -> CodecResult<Self> {
    let mut device_ctx: *mut AVBufferRef = std::ptr::null_mut();

    let ret = unsafe {
      av_hwdevice_ctx_create(
        &mut device_ctx,
        device_type.as_raw(),
        std::ptr::null(),     // Use default device
        std::ptr::null_mut(), // No options
        0,                    // Flags
      )
    };

    ffi::check_error(ret)?;

    NonNull::new(device_ctx)
      .map(|ptr| Self { ptr, device_type })
      .ok_or(CodecError::HardwareError(
        "Failed to create hardware device context".into(),
      ))
  }

  /// Try to create the best available hardware device for the current platform
  pub fn new_best_available() -> Option<Self> {
    // Platform-specific priority
    #[cfg(target_os = "macos")]
    {
      if let Ok(ctx) = Self::new(AVHWDeviceType::Videotoolbox) {
        return Some(ctx);
      }
    }

    // NVIDIA CUDA
    if let Ok(ctx) = Self::new(AVHWDeviceType::Cuda) {
      return Some(ctx);
    }

    // Linux VAAPI
    #[cfg(target_os = "linux")]
    {
      if let Ok(ctx) = Self::new(AVHWDeviceType::Vaapi) {
        return Some(ctx);
      }
    }

    // Windows D3D11VA
    #[cfg(target_os = "windows")]
    {
      if let Ok(ctx) = Self::new(AVHWDeviceType::D3d11va) {
        return Some(ctx);
      }
    }

    // Intel Quick Sync
    if let Ok(ctx) = Self::new(AVHWDeviceType::Qsv) {
      return Some(ctx);
    }

    None
  }

  /// Get the raw pointer
  #[inline]
  pub fn as_ptr(&self) -> *mut AVBufferRef {
    self.ptr.as_ptr()
  }

  /// Get the device type
  #[inline]
  pub fn device_type(&self) -> AVHWDeviceType {
    self.device_type
  }

  /// Get device type name
  pub fn device_name(&self) -> &'static str {
    let name_ptr = unsafe { av_hwdevice_get_type_name(self.device_type.as_raw()) };
    if name_ptr.is_null() {
      "unknown"
    } else {
      unsafe { CStr::from_ptr(name_ptr) }
        .to_str()
        .unwrap_or("unknown")
    }
  }

  /// Check if a hardware device type is available
  pub fn is_available(device_type: AVHWDeviceType) -> bool {
    // Try to create and immediately drop
    Self::new(device_type).is_ok()
  }

  /// Get all available hardware device types
  pub fn available_types() -> Vec<AVHWDeviceType> {
    let mut types = Vec::new();
    let mut current = unsafe { av_hwdevice_iterate_types(0) };

    while current != 0 {
      let device_type = match current {
        1 => Some(AVHWDeviceType::Vdpau),
        2 => Some(AVHWDeviceType::Cuda),
        3 => Some(AVHWDeviceType::Vaapi),
        4 => Some(AVHWDeviceType::Dxva2),
        5 => Some(AVHWDeviceType::Qsv),
        6 => Some(AVHWDeviceType::Videotoolbox),
        7 => Some(AVHWDeviceType::D3d11va),
        8 => Some(AVHWDeviceType::Drm),
        9 => Some(AVHWDeviceType::Opencl),
        10 => Some(AVHWDeviceType::Mediacodec),
        11 => Some(AVHWDeviceType::Vulkan),
        _ => None,
      };

      if let Some(dt) = device_type {
        // Only include if actually available (can create context)
        if Self::is_available(dt) {
          types.push(dt);
        }
      }

      current = unsafe { av_hwdevice_iterate_types(current) };
    }

    types
  }
}

impl Drop for HwDeviceContext {
  fn drop(&mut self) {
    unsafe {
      let mut ptr = self.ptr.as_ptr();
      av_buffer_unref(&mut ptr);
    }
  }
}

// Hardware device contexts can be shared across threads
unsafe impl Send for HwDeviceContext {}
unsafe impl Sync for HwDeviceContext {}

impl std::fmt::Debug for HwDeviceContext {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("HwDeviceContext")
      .field("type", &self.device_type)
      .field("name", &self.device_name())
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_available_types() {
    let types = HwDeviceContext::available_types();
    println!("Available hardware device types: {:?}", types);
    // Don't assert on specific types as they're platform-dependent
  }

  #[test]
  #[cfg(target_os = "macos")]
  fn test_videotoolbox() {
    let result = HwDeviceContext::new(AVHWDeviceType::Videotoolbox);
    // VideoToolbox should be available on macOS
    assert!(result.is_ok(), "VideoToolbox should be available on macOS");
    if let Ok(ctx) = result {
      assert_eq!(ctx.device_name(), "videotoolbox");
    }
  }
}
