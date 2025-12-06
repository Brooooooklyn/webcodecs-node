//! Hardware acceleration utilities
//!
//! Provides JavaScript-accessible functions for querying hardware acceleration support.

use crate::codec::HwDeviceContext;
use crate::ffi::AVHWDeviceType;
use napi_derive::napi;

/// Hardware accelerator information
#[napi(object)]
pub struct HardwareAccelerator {
  /// Internal name (e.g., "videotoolbox", "cuda", "vaapi")
  pub name: String,
  /// Human-readable description
  pub description: String,
  /// Whether this accelerator is available on this system
  pub available: bool,
}

/// Get list of all known hardware accelerators and their availability
#[napi]
pub fn get_hardware_accelerators() -> Vec<HardwareAccelerator> {
  let accelerators = [
    (
      AVHWDeviceType::Videotoolbox,
      "videotoolbox",
      "Apple VideoToolbox (macOS)",
    ),
    (AVHWDeviceType::Cuda, "cuda", "NVIDIA CUDA/NVENC"),
    (
      AVHWDeviceType::Vaapi,
      "vaapi",
      "Video Acceleration API (Linux)",
    ),
    (
      AVHWDeviceType::D3d11va,
      "d3d11va",
      "Direct3D 11 Video Acceleration (Windows)",
    ),
    (AVHWDeviceType::Qsv, "qsv", "Intel Quick Sync Video"),
    (
      AVHWDeviceType::Dxva2,
      "dxva2",
      "DirectX Video Acceleration 2 (Windows)",
    ),
    (AVHWDeviceType::Vdpau, "vdpau", "NVIDIA VDPAU (Linux)"),
    (AVHWDeviceType::Vulkan, "vulkan", "Vulkan Video"),
  ];

  accelerators
    .iter()
    .map(|(hw_type, name, desc)| HardwareAccelerator {
      name: name.to_string(),
      description: desc.to_string(),
      available: HwDeviceContext::is_available(*hw_type),
    })
    .collect()
}

/// Get available hardware accelerators (only those that can be used)
#[napi]
pub fn get_available_hardware_accelerators() -> Vec<String> {
  get_hardware_accelerators()
    .into_iter()
    .filter(|a| a.available)
    .map(|a| a.name)
    .collect()
}

/// Check if a specific hardware accelerator is available
#[napi]
pub fn is_hardware_accelerator_available(name: String) -> bool {
  let hw_type = match name.as_str() {
    "videotoolbox" => Some(AVHWDeviceType::Videotoolbox),
    "cuda" | "nvenc" => Some(AVHWDeviceType::Cuda),
    "vaapi" => Some(AVHWDeviceType::Vaapi),
    "d3d11va" => Some(AVHWDeviceType::D3d11va),
    "qsv" => Some(AVHWDeviceType::Qsv),
    "dxva2" => Some(AVHWDeviceType::Dxva2),
    "vdpau" => Some(AVHWDeviceType::Vdpau),
    "vulkan" => Some(AVHWDeviceType::Vulkan),
    _ => None,
  };

  hw_type.map(HwDeviceContext::is_available).unwrap_or(false)
}

/// Get the preferred hardware accelerator for the current platform
#[napi]
pub fn get_preferred_hardware_accelerator() -> Option<String> {
  #[cfg(target_os = "macos")]
  {
    if HwDeviceContext::is_available(AVHWDeviceType::Videotoolbox) {
      return Some("videotoolbox".to_string());
    }
  }

  #[cfg(target_os = "linux")]
  {
    if HwDeviceContext::is_available(AVHWDeviceType::Vaapi) {
      return Some("vaapi".to_string());
    }
  }

  #[cfg(target_os = "windows")]
  {
    if HwDeviceContext::is_available(AVHWDeviceType::D3d11va) {
      return Some("d3d11va".to_string());
    }
    if HwDeviceContext::is_available(AVHWDeviceType::Dxva2) {
      return Some("dxva2".to_string());
    }
  }

  // Try CUDA as fallback (cross-platform with NVIDIA)
  if HwDeviceContext::is_available(AVHWDeviceType::Cuda) {
    return Some("cuda".to_string());
  }

  // Try QSV as fallback (Intel integrated graphics)
  if HwDeviceContext::is_available(AVHWDeviceType::Qsv) {
    return Some("qsv".to_string());
  }

  None
}
