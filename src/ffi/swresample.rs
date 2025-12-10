//! libswresample function declarations
//!
//! Provides audio resampling and format conversion functionality.
//! This is the audio equivalent of libswscale for video.

use super::types::*;
use std::os::raw::c_int;

unsafe extern "C" {
  // ========================================================================
  // Context Management
  // ========================================================================

  /// Allocate an empty SwrContext
  ///
  /// This must be followed by swr_alloc_set_opts* and swr_init.
  /// Alternatively, use swr_alloc_set_opts for older FFmpeg versions.
  pub fn swr_alloc() -> *mut SwrContext;

  /// Free the given SwrContext and set the pointer to NULL
  pub fn swr_free(s: *mut *mut SwrContext);

  /// Close the context and free internal allocations, but keep the
  /// context available for reuse.
  ///
  /// Call swr_init to reinitialize after this.
  pub fn swr_close(s: *mut SwrContext);

  /// Initialize context after setting user parameters
  ///
  /// # Returns
  /// 0 on success, negative AVERROR code on failure
  pub fn swr_init(s: *mut SwrContext) -> c_int;

  /// Check if context needs to be re-initialized
  ///
  /// Returns 1 if swresample reinitialization is needed, 0 otherwise
  pub fn swr_is_initialized(s: *const SwrContext) -> c_int;

  /// Allocate and set options in one call (FFmpeg 5.1+)
  ///
  /// # Arguments
  /// * `ps` - Pointer to SwrContext pointer (will be allocated if NULL)
  /// * `out_ch_layout` - Output channel layout
  /// * `out_sample_fmt` - Output sample format
  /// * `out_sample_rate` - Output sample rate
  /// * `in_ch_layout` - Input channel layout
  /// * `in_sample_fmt` - Input sample format
  /// * `in_sample_rate` - Input sample rate
  /// * `log_offset` - Logging offset (usually 0)
  /// * `log_ctx` - Logging context (usually NULL)
  ///
  /// # Returns
  /// 0 on success, negative AVERROR code on failure
  #[allow(clippy::too_many_arguments)]
  pub fn swr_alloc_set_opts2(
    ps: *mut *mut SwrContext,
    out_ch_layout: *const AVChannelLayout,
    out_sample_fmt: c_int,
    out_sample_rate: c_int,
    in_ch_layout: *const AVChannelLayout,
    in_sample_fmt: c_int,
    in_sample_rate: c_int,
    log_offset: c_int,
    log_ctx: *mut std::ffi::c_void,
  ) -> c_int;

  /// Initialize a channel layout with a default layout for a given number of channels
  pub fn av_channel_layout_default(ch_layout: *mut AVChannelLayout, nb_channels: c_int);

  /// Free any allocated data in the channel layout and reset it
  pub fn av_channel_layout_uninit(channel_layout: *mut AVChannelLayout);

  // ========================================================================
  // Conversion Operations
  // ========================================================================

  /// Convert audio data
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `out` - Output buffers (one per channel for planar, one for interleaved)
  /// * `out_count` - Amount of space available for output in samples per channel
  /// * `in_` - Input buffers (one per channel for planar, one for interleaved)
  /// * `in_count` - Number of input samples available in one channel
  ///
  /// # Returns
  /// Number of samples output per channel, or negative AVERROR code on error
  ///
  /// # Notes
  /// - If out or in_count is 0, swr_convert will do nothing
  /// - in_count of 0 can be used for flushing remaining samples
  /// - out can be NULL to read buffered samples
  pub fn swr_convert(
    s: *mut SwrContext,
    out: *mut *mut u8,
    out_count: c_int,
    r#in: *const *const u8,
    in_count: c_int,
  ) -> c_int;

  /// Convert audio using AVFrame structures
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `output` - Destination frame (allocates buffers if NULL data pointers)
  /// * `input` - Source frame
  ///
  /// # Returns
  /// 0 on success, negative AVERROR code on failure
  pub fn swr_convert_frame(
    s: *mut SwrContext,
    output: *mut AVFrame,
    input: *const AVFrame,
  ) -> c_int;

  /// Configure or reconfigure the SwrContext using AVFrame parameters
  ///
  /// Drops existing conversion configuration and uses the provided frames
  /// as templates for parameters.
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `output` - Output frame template
  /// * `input` - Input frame template
  ///
  /// # Returns
  /// 0 on success, negative AVERROR code on failure
  pub fn swr_config_frame(
    s: *mut SwrContext,
    output: *const AVFrame,
    input: *const AVFrame,
  ) -> c_int;

  // ========================================================================
  // Delay and Buffer Management
  // ========================================================================

  /// Get the delay (buffered samples) in the resampler
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `base` - Time base (e.g., 1 for samples, sample_rate for seconds)
  ///
  /// # Returns
  /// Delay in the given time base
  pub fn swr_get_delay(s: *const SwrContext, base: i64) -> i64;

  /// Get the number of samples needed for the next swr_convert call
  ///
  /// For accurate calculation, this needs the actual samples to be converted.
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `in_samples` - Number of input samples
  ///
  /// # Returns
  /// Upper bound on the number of output samples
  pub fn swr_get_out_samples(s: *const SwrContext, in_samples: c_int) -> c_int;

  /// Drops the specified number of output samples
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `count` - Number of samples to drop
  ///
  /// # Returns
  /// Number of samples dropped, or negative AVERROR on error
  pub fn swr_drop_output(s: *mut SwrContext, count: c_int) -> c_int;

  /// Injects the specified number of silence samples
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `count` - Number of silence samples to inject
  ///
  /// # Returns
  /// Number of samples injected, or negative AVERROR on error
  pub fn swr_inject_silence(s: *mut SwrContext, count: c_int) -> c_int;

  // ========================================================================
  // Option Access
  // ========================================================================

  /// Set a context option using AVOptions
  ///
  /// # Arguments
  /// * `s` - SwrContext
  /// * `name` - Option name
  /// * `value` - Option value
  /// * `search_flags` - Search flags (usually 0)
  ///
  /// # Returns
  /// 0 on success, negative AVERROR on error
  pub fn av_opt_set(
    obj: *mut std::ffi::c_void,
    name: *const std::os::raw::c_char,
    val: *const std::os::raw::c_char,
    search_flags: c_int,
  ) -> c_int;

  /// Set an integer option
  pub fn av_opt_set_int(
    obj: *mut std::ffi::c_void,
    name: *const std::os::raw::c_char,
    val: i64,
    search_flags: c_int,
  ) -> c_int;

  /// Set a sample format option
  pub fn av_opt_set_sample_fmt(
    obj: *mut std::ffi::c_void,
    name: *const std::os::raw::c_char,
    fmt: c_int,
    search_flags: c_int,
  ) -> c_int;

  /// Set a channel layout option (legacy)
  #[cfg(not(feature = "ffmpeg_5_1"))]
  pub fn av_opt_set_channel_layout(
    obj: *mut std::ffi::c_void,
    name: *const std::os::raw::c_char,
    ch_layout: i64,
    search_flags: c_int,
  ) -> c_int;
}

// ============================================================================
// Dithering Modes
// ============================================================================

/// No dithering
pub const SWR_DITHER_NONE: c_int = 0;

/// Rectangular dithering
pub const SWR_DITHER_RECTANGULAR: c_int = 1;

/// Triangular dithering
pub const SWR_DITHER_TRIANGULAR: c_int = 2;

/// Triangular high-pass dithering
pub const SWR_DITHER_TRIANGULAR_HIGHPASS: c_int = 3;

// ============================================================================
// Engine Selection
// ============================================================================

/// Software engine (default)
pub const SWR_ENGINE_SWR: c_int = 0;

/// SOX resampler engine
pub const SWR_ENGINE_SOXR: c_int = 1;

// ============================================================================
// Filter Type
// ============================================================================

/// Cubic filter
pub const SWR_FILTER_TYPE_CUBIC: c_int = 0;

/// Blackman-Nuttall windowed sinc
pub const SWR_FILTER_TYPE_BLACKMAN_NUTTALL: c_int = 1;

/// Kaiser windowed sinc
pub const SWR_FILTER_TYPE_KAISER: c_int = 2;

// ============================================================================
// Helper Functions
// ============================================================================

/// Create and configure a SwrContext using simple parameters
///
/// This uses the newer `swr_alloc_set_opts2` API (FFmpeg 5.1+).
///
/// # Arguments
/// * `out_channels` - Number of output channels
/// * `out_sample_rate` - Output sample rate
/// * `out_sample_fmt` - Output sample format
/// * `in_channels` - Number of input channels
/// * `in_sample_rate` - Input sample rate
/// * `in_sample_fmt` - Input sample format
///
/// # Returns
/// Configured and initialized SwrContext, or NULL on error
///
/// # Safety
/// This function is unsafe because it calls FFmpeg C functions that allocate
/// memory and require proper initialization. The returned pointer must be
/// freed with `swr_free` when no longer needed.
pub unsafe fn create_resampler(
  out_channels: u32,
  out_sample_rate: u32,
  out_sample_fmt: AVSampleFormat,
  in_channels: u32,
  in_sample_rate: u32,
  in_sample_fmt: AVSampleFormat,
) -> *mut SwrContext {
  unsafe {
    // Create channel layouts
    let mut out_ch_layout: AVChannelLayout = std::mem::zeroed();
    let mut in_ch_layout: AVChannelLayout = std::mem::zeroed();

    av_channel_layout_default(&mut out_ch_layout, out_channels as c_int);
    av_channel_layout_default(&mut in_ch_layout, in_channels as c_int);

    let mut ctx: *mut SwrContext = std::ptr::null_mut();

    let ret = swr_alloc_set_opts2(
      &mut ctx,
      &out_ch_layout,
      out_sample_fmt.as_raw(),
      out_sample_rate as c_int,
      &in_ch_layout,
      in_sample_fmt.as_raw(),
      in_sample_rate as c_int,
      0,
      std::ptr::null_mut(),
    );

    // Clean up channel layouts
    av_channel_layout_uninit(&mut out_ch_layout);
    av_channel_layout_uninit(&mut in_ch_layout);

    if ret < 0 || ctx.is_null() {
      return std::ptr::null_mut();
    }

    if swr_init(ctx) < 0 {
      let mut ctx_ptr = ctx;
      swr_free(&mut ctx_ptr);
      return std::ptr::null_mut();
    }

    ctx
  }
}

/// Calculate the number of output samples for a given input sample count
///
/// This takes into account the sample rate conversion ratio and any
/// buffered samples in the resampler.
///
/// # Safety
/// The `ctx` pointer must be a valid SwrContext created by `create_resampler`
/// or `swr_alloc_set_opts2`, or NULL (which returns 0).
pub unsafe fn calculate_output_samples(
  ctx: *const SwrContext,
  in_samples: u32,
  in_sample_rate: u32,
  out_sample_rate: u32,
) -> u32 {
  if ctx.is_null() {
    return 0;
  }

  // Get delay in input sample rate units
  let delay = unsafe { swr_get_delay(ctx, in_sample_rate as i64) };

  // Calculate output samples: (delay + in_samples) * out_rate / in_rate
  // Add 1 for rounding
  let out_samples = ((delay + in_samples as i64) * out_sample_rate as i64 + in_sample_rate as i64
    - 1)
    / in_sample_rate as i64;

  out_samples.max(0) as u32
}

/// Flush remaining samples from the resampler
///
/// Returns the number of samples flushed
///
/// # Safety
/// The `ctx` pointer must be a valid SwrContext, and `out` must point to
/// a valid output buffer array with sufficient space for `out_count` samples.
pub unsafe fn flush_resampler(ctx: *mut SwrContext, out: *mut *mut u8, out_count: u32) -> i32 {
  if ctx.is_null() {
    return 0;
  }

  unsafe { swr_convert(ctx, out, out_count as c_int, std::ptr::null(), 0) }
}
