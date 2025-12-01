//! Hardware acceleration function declarations
//!
//! Provides hardware device context management for VideoToolbox, CUDA, VAAPI, etc.

use super::types::*;
use std::os::raw::{c_char, c_int, c_void};

extern "C" {
    // ========================================================================
    // Hardware Device Context
    // ========================================================================

    /// Create a hardware device context for the specified type
    ///
    /// # Arguments
    /// * `device_ctx` - On success, pointer to the created context
    /// * `type_` - Hardware device type
    /// * `device` - Device name (e.g., "/dev/dri/renderD128" for VAAPI, NULL for default)
    /// * `opts` - Options dictionary (can be NULL)
    /// * `flags` - Currently unused, should be 0
    ///
    /// # Returns
    /// 0 on success, negative AVERROR on failure
    pub fn av_hwdevice_ctx_create(
        device_ctx: *mut *mut AVBufferRef,
        type_: c_int,
        device: *const c_char,
        opts: *mut AVDictionary,
        flags: c_int,
    ) -> c_int;

    /// Create a new reference to a hardware device context
    pub fn av_hwdevice_ctx_alloc(type_: c_int) -> *mut AVBufferRef;

    /// Finalize the device context before use
    pub fn av_hwdevice_ctx_init(ref_: *mut AVBufferRef) -> c_int;

    /// Iterate over supported device types
    ///
    /// # Arguments
    /// * `prev` - Previous type (AV_HWDEVICE_TYPE_NONE to start)
    ///
    /// # Returns
    /// Next type, or AV_HWDEVICE_TYPE_NONE when done
    pub fn av_hwdevice_iterate_types(prev: c_int) -> c_int;

    /// Get the string name of a hardware device type
    pub fn av_hwdevice_get_type_name(type_: c_int) -> *const c_char;

    /// Get hardware device type from name
    pub fn av_hwdevice_find_type_by_name(name: *const c_char) -> c_int;

    // ========================================================================
    // Hardware Frames Context
    // ========================================================================

    /// Allocate a hardware frames context for a given device
    ///
    /// # Arguments
    /// * `device_ctx` - Reference to the device context
    ///
    /// # Returns
    /// Newly created hardware frames context, or NULL on failure
    pub fn av_hwframe_ctx_alloc(device_ctx: *mut AVBufferRef) -> *mut AVBufferRef;

    /// Finalize the hardware frames context before use
    pub fn av_hwframe_ctx_init(ref_: *mut AVBufferRef) -> c_int;

    /// Allocate a new frame from the hardware frames pool
    ///
    /// # Arguments
    /// * `hwframe_ctx` - Reference to the frames context
    /// * `frame` - Empty frame to fill with hardware reference
    /// * `flags` - Currently unused, should be 0
    pub fn av_hwframe_get_buffer(
        hwframe_ctx: *mut AVBufferRef,
        frame: *mut AVFrame,
        flags: c_int,
    ) -> c_int;

    /// Copy data between hardware and software frames
    ///
    /// # Arguments
    /// * `dst` - Destination frame
    /// * `src` - Source frame
    /// * `flags` - Currently unused, should be 0
    ///
    /// If src is HW frame and dst is SW frame: download
    /// If src is SW frame and dst is HW frame: upload
    pub fn av_hwframe_transfer_data(
        dst: *mut AVFrame,
        src: *const AVFrame,
        flags: c_int,
    ) -> c_int;

    /// Get constraints on hardware frames for a device/configuration
    pub fn av_hwdevice_get_hwframe_constraints(
        device_ctx: *mut AVBufferRef,
        hwconfig: *const c_void,
    ) -> *mut AVHWFramesConstraints;

    /// Free an AVHWFramesConstraints structure
    pub fn av_hwframe_constraints_free(constraints: *mut *mut AVHWFramesConstraints);

    // ========================================================================
    // Hardware Frame Mapping
    // ========================================================================

    /// Map a hardware frame to a software frame
    ///
    /// # Arguments
    /// * `dst` - Destination frame (empty)
    /// * `src` - Source hardware frame
    /// * `flags` - Mapping flags (AV_HWFRAME_MAP_*)
    pub fn av_hwframe_map(dst: *mut AVFrame, src: *const AVFrame, flags: c_int) -> c_int;

    // ========================================================================
    // Codec Hardware Config
    // ========================================================================

    /// Get hardware configuration for a codec
    pub fn avcodec_get_hw_config(codec: *const AVCodec, index: c_int) -> *const AVCodecHWConfig;
}

// ============================================================================
// Opaque Types
// ============================================================================

/// Hardware frames constraints
#[repr(C)]
pub struct AVHWFramesConstraints {
    _opaque: [u8; 0],
}

/// Codec hardware config
#[repr(C)]
pub struct AVCodecHWConfig {
    _opaque: [u8; 0],
}

// ============================================================================
// Hardware Frame Map Flags
// ============================================================================

/// Map for reading
pub const AV_HWFRAME_MAP_READ: c_int = 1 << 0;

/// Map for writing
pub const AV_HWFRAME_MAP_WRITE: c_int = 1 << 1;

/// Map for overwriting (discard previous contents)
pub const AV_HWFRAME_MAP_OVERWRITE: c_int = 1 << 2;

/// Mapping will be done directly without copying
pub const AV_HWFRAME_MAP_DIRECT: c_int = 1 << 3;

// ============================================================================
// Helper Functions
// ============================================================================

/// Check if a hardware device type is available
pub fn is_hwdevice_available(device_type: AVHWDeviceType) -> bool {
    let mut current = unsafe { av_hwdevice_iterate_types(0) };
    while current != 0 {
        if current == device_type.as_raw() {
            return true;
        }
        current = unsafe { av_hwdevice_iterate_types(current) };
    }
    false
}

/// Get available hardware device types
pub fn get_available_hwdevice_types() -> Vec<AVHWDeviceType> {
    let mut types = Vec::new();
    let mut current = unsafe { av_hwdevice_iterate_types(0) };

    while current != 0 {
        match current {
            1 => types.push(AVHWDeviceType::Vdpau),
            2 => types.push(AVHWDeviceType::Cuda),
            3 => types.push(AVHWDeviceType::Vaapi),
            4 => types.push(AVHWDeviceType::Dxva2),
            5 => types.push(AVHWDeviceType::Qsv),
            6 => types.push(AVHWDeviceType::Videotoolbox),
            7 => types.push(AVHWDeviceType::D3d11va),
            8 => types.push(AVHWDeviceType::Drm),
            9 => types.push(AVHWDeviceType::Opencl),
            10 => types.push(AVHWDeviceType::Mediacodec),
            11 => types.push(AVHWDeviceType::Vulkan),
            _ => {}
        }
        current = unsafe { av_hwdevice_iterate_types(current) };
    }

    types
}

/// Get hardware device type name
pub fn get_hwdevice_type_name(device_type: AVHWDeviceType) -> Option<&'static str> {
    let name_ptr = unsafe { av_hwdevice_get_type_name(device_type.as_raw()) };
    if name_ptr.is_null() {
        return None;
    }
    let cstr = unsafe { std::ffi::CStr::from_ptr(name_ptr) };
    cstr.to_str().ok()
}

/// Platform-specific preferred hardware device type
#[cfg(target_os = "macos")]
pub const PREFERRED_HW_DEVICE: AVHWDeviceType = AVHWDeviceType::Videotoolbox;

#[cfg(target_os = "linux")]
pub const PREFERRED_HW_DEVICE: AVHWDeviceType = AVHWDeviceType::Vaapi;

#[cfg(target_os = "windows")]
pub const PREFERRED_HW_DEVICE: AVHWDeviceType = AVHWDeviceType::D3d11va;

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub const PREFERRED_HW_DEVICE: AVHWDeviceType = AVHWDeviceType::None;
