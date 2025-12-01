//! libavutil function declarations
//!
//! Provides utility functions for memory, frames, and general helpers.

use super::types::*;
use std::os::raw::{c_char, c_int, c_void};

extern "C" {
    // ========================================================================
    // Frame Management
    // ========================================================================

    /// Allocate an AVFrame and set its fields to default values
    pub fn av_frame_alloc() -> *mut AVFrame;

    /// Free the frame and any dynamically allocated objects in it
    pub fn av_frame_free(frame: *mut *mut AVFrame);

    /// Set up a new reference to the data described by the frame
    pub fn av_frame_ref(dst: *mut AVFrame, src: *const AVFrame) -> c_int;

    /// Unreference all buffers referenced by frame and reset to defaults
    pub fn av_frame_unref(frame: *mut AVFrame);

    /// Create a new frame that references the same data as src
    pub fn av_frame_clone(src: *const AVFrame) -> *mut AVFrame;

    /// Allocate new buffers for video data based on frame format/dimensions
    ///
    /// # Arguments
    /// * `frame` - Frame with format, width, height set
    /// * `align` - Buffer size alignment (0 for default, 32 recommended for SIMD)
    pub fn av_frame_get_buffer(frame: *mut AVFrame, align: c_int) -> c_int;

    /// Ensure the frame is writable, copying data if needed
    pub fn av_frame_make_writable(frame: *mut AVFrame) -> c_int;

    /// Copy frame data from src to dst
    pub fn av_frame_copy(dst: *mut AVFrame, src: *const AVFrame) -> c_int;

    /// Copy only "metadata" fields from src to dst (pts, duration, etc)
    pub fn av_frame_copy_props(dst: *mut AVFrame, src: *const AVFrame) -> c_int;

    /// Check if the frame is writable
    pub fn av_frame_is_writable(frame: *mut AVFrame) -> c_int;

    // ========================================================================
    // Memory Allocation
    // ========================================================================

    /// Allocate a memory block with alignment suitable for all memory accesses
    pub fn av_malloc(size: usize) -> *mut c_void;

    /// Allocate a memory block with alignment suitable for all memory accesses
    /// and zero all the bytes
    pub fn av_mallocz(size: usize) -> *mut c_void;

    /// Allocate, reallocate, or free a block of memory
    pub fn av_realloc(ptr: *mut c_void, size: usize) -> *mut c_void;

    /// Free a memory block which has been allocated with av_malloc
    pub fn av_free(ptr: *mut c_void);

    /// Free a memory block which has been allocated with av_malloc and set ptr to NULL
    pub fn av_freep(ptr: *mut c_void);

    // ========================================================================
    // Buffer Reference Management
    // ========================================================================

    /// Create a new reference to an AVBuffer
    pub fn av_buffer_ref(buf: *mut AVBufferRef) -> *mut AVBufferRef;

    /// Free a given reference and automatically free the buffer if no more refs
    pub fn av_buffer_unref(buf: *mut *mut AVBufferRef);

    /// Check if the buffer is writable (only one reference)
    pub fn av_buffer_is_writable(buf: *const AVBufferRef) -> c_int;

    /// Get the data pointer from the buffer ref
    pub fn av_buffer_get_opaque(buf: *const AVBufferRef) -> *mut c_void;

    // ========================================================================
    // Image Utilities
    // ========================================================================

    /// Get the required buffer size for an image with given dimensions and format
    ///
    /// # Arguments
    /// * `pix_fmt` - Pixel format
    /// * `width` - Image width
    /// * `height` - Image height
    /// * `align` - Line size alignment (typically 1)
    pub fn av_image_get_buffer_size(
        pix_fmt: c_int,
        width: c_int,
        height: c_int,
        align: c_int,
    ) -> c_int;

    /// Fill plane data pointers and linesizes for an image with given parameters
    ///
    /// # Arguments
    /// * `dst_data` - Pointers to plane data (will be filled)
    /// * `dst_linesize` - Linesizes for each plane (will be filled)
    /// * `src` - Source buffer pointer
    /// * `pix_fmt` - Pixel format
    /// * `width` - Image width
    /// * `height` - Image height
    /// * `align` - Line size alignment
    pub fn av_image_fill_arrays(
        dst_data: *mut *mut u8,
        dst_linesize: *mut c_int,
        src: *const u8,
        pix_fmt: c_int,
        width: c_int,
        height: c_int,
        align: c_int,
    ) -> c_int;

    /// Copy image data from src to dst
    pub fn av_image_copy(
        dst_data: *mut *mut u8,
        dst_linesizes: *const c_int,
        src_data: *const *const u8,
        src_linesizes: *const c_int,
        pix_fmt: c_int,
        width: c_int,
        height: c_int,
    );

    /// Copy image data to a buffer
    pub fn av_image_copy_to_buffer(
        dst: *mut u8,
        dst_size: c_int,
        src_data: *const *const u8,
        src_linesizes: *const c_int,
        pix_fmt: c_int,
        width: c_int,
        height: c_int,
        align: c_int,
    ) -> c_int;

    // ========================================================================
    // Error Handling
    // ========================================================================

    /// Put a description of the AVERROR code errnum in errbuf
    ///
    /// # Arguments
    /// * `errnum` - Error code to describe
    /// * `errbuf` - Buffer to put description in
    /// * `errbuf_size` - Size of errbuf
    ///
    /// # Returns
    /// 0 on success, negative if truncated
    pub fn av_strerror(errnum: c_int, errbuf: *mut c_char, errbuf_size: usize) -> c_int;

    // ========================================================================
    // Time/Timestamp Utilities
    // ========================================================================

    /// Rescale a 64-bit integer by 2 rational numbers
    pub fn av_rescale_q(a: i64, bq: AVRational, cq: AVRational) -> i64;

    /// Rescale a 64-bit integer by 2 rational numbers with rounding
    pub fn av_rescale_q_rnd(a: i64, bq: AVRational, cq: AVRational, rnd: c_int) -> i64;

    /// Compare two timestamps
    pub fn av_compare_ts(ts_a: i64, tb_a: AVRational, ts_b: i64, tb_b: AVRational) -> c_int;

    // ========================================================================
    // Dictionary (Options)
    // ========================================================================

    /// Set an entry in the dictionary
    pub fn av_dict_set(
        pm: *mut *mut AVDictionary,
        key: *const c_char,
        value: *const c_char,
        flags: c_int,
    ) -> c_int;

    /// Free all memory allocated for an AVDictionary
    pub fn av_dict_free(m: *mut *mut AVDictionary);

    /// Get a dictionary entry with matching key
    pub fn av_dict_get(
        m: *const AVDictionary,
        key: *const c_char,
        prev: *const c_void,
        flags: c_int,
    ) -> *const c_void;

    // ========================================================================
    // Logging
    // ========================================================================

    /// Set the logging level
    pub fn av_log_set_level(level: c_int);

    /// Get the current logging level
    pub fn av_log_get_level() -> c_int;
}

// ============================================================================
// Logging Levels
// ============================================================================

pub mod log_level {
    use std::os::raw::c_int;

    pub const QUIET: c_int = -8;
    pub const PANIC: c_int = 0;
    pub const FATAL: c_int = 8;
    pub const ERROR: c_int = 16;
    pub const WARNING: c_int = 24;
    pub const INFO: c_int = 32;
    pub const VERBOSE: c_int = 40;
    pub const DEBUG: c_int = 48;
    pub const TRACE: c_int = 56;
}

// ============================================================================
// Dictionary Flags
// ============================================================================

pub mod dict_flag {
    use std::os::raw::c_int;

    pub const MATCH_CASE: c_int = 1;
    pub const IGNORE_SUFFIX: c_int = 2;
    pub const DONT_STRDUP_KEY: c_int = 4;
    pub const DONT_STRDUP_VAL: c_int = 8;
    pub const DONT_OVERWRITE: c_int = 16;
    pub const APPEND: c_int = 32;
    pub const MULTIKEY: c_int = 64;
}

// ============================================================================
// Rounding Modes
// ============================================================================

pub mod rounding {
    use std::os::raw::c_int;

    pub const ZERO: c_int = 0;
    pub const INF: c_int = 1;
    pub const DOWN: c_int = 2;
    pub const UP: c_int = 3;
    pub const NEAR_INF: c_int = 5;
    pub const PASS_MINMAX: c_int = 8192;
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get the size in bytes needed for a video frame
pub fn image_buffer_size(format: AVPixelFormat, width: i32, height: i32) -> i32 {
    unsafe { av_image_get_buffer_size(format.as_raw(), width, height, 1) }
}
