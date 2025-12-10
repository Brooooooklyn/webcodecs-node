//! Rust declarations for C accessor functions
//!
//! These functions provide access to FFmpeg struct fields via the thin C accessor library.

use super::types::*;
use std::os::raw::c_int;

unsafe extern "C" {
  // ========================================================================
  // AVCodecContext Setters
  // ========================================================================

  pub fn ffctx_set_width(ctx: *mut AVCodecContext, width: c_int);
  pub fn ffctx_set_height(ctx: *mut AVCodecContext, height: c_int);
  pub fn ffctx_set_coded_width(ctx: *mut AVCodecContext, width: c_int);
  pub fn ffctx_set_coded_height(ctx: *mut AVCodecContext, height: c_int);
  pub fn ffctx_set_pix_fmt(ctx: *mut AVCodecContext, pix_fmt: c_int);
  pub fn ffctx_set_bit_rate(ctx: *mut AVCodecContext, bit_rate: i64);
  pub fn ffctx_set_rc_max_rate(ctx: *mut AVCodecContext, rc_max_rate: i64);
  pub fn ffctx_set_rc_buffer_size(ctx: *mut AVCodecContext, rc_buffer_size: c_int);
  pub fn ffctx_set_gop_size(ctx: *mut AVCodecContext, gop_size: c_int);
  pub fn ffctx_set_max_b_frames(ctx: *mut AVCodecContext, max_b_frames: c_int);
  pub fn ffctx_set_time_base(ctx: *mut AVCodecContext, num: c_int, den: c_int);
  pub fn ffctx_set_framerate(ctx: *mut AVCodecContext, num: c_int, den: c_int);
  pub fn ffctx_set_sample_aspect_ratio(ctx: *mut AVCodecContext, num: c_int, den: c_int);
  pub fn ffctx_set_thread_count(ctx: *mut AVCodecContext, thread_count: c_int);
  pub fn ffctx_set_thread_type(ctx: *mut AVCodecContext, thread_type: c_int);
  pub fn ffctx_set_color_primaries(ctx: *mut AVCodecContext, color_primaries: c_int);
  pub fn ffctx_set_color_trc(ctx: *mut AVCodecContext, color_trc: c_int);
  pub fn ffctx_set_colorspace(ctx: *mut AVCodecContext, colorspace: c_int);
  pub fn ffctx_set_color_range(ctx: *mut AVCodecContext, color_range: c_int);
  pub fn ffctx_set_flags(ctx: *mut AVCodecContext, flags: c_int);
  pub fn ffctx_set_flags2(ctx: *mut AVCodecContext, flags2: c_int);
  pub fn ffctx_set_profile(ctx: *mut AVCodecContext, profile: c_int);
  pub fn ffctx_set_level(ctx: *mut AVCodecContext, level: c_int);
  pub fn ffctx_set_hw_device_ctx(ctx: *mut AVCodecContext, hw_device_ctx: *mut AVBufferRef);
  pub fn ffctx_set_hw_frames_ctx(ctx: *mut AVCodecContext, hw_frames_ctx: *mut AVBufferRef);

  // ========================================================================
  // AVCodecContext Getters
  // ========================================================================

  pub fn ffctx_get_width(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_height(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_coded_width(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_coded_height(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_pix_fmt(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_bit_rate(ctx: *const AVCodecContext) -> i64;
  pub fn ffctx_get_gop_size(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_max_b_frames(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_time_base(ctx: *const AVCodecContext, num: *mut c_int, den: *mut c_int);
  pub fn ffctx_get_framerate(ctx: *const AVCodecContext, num: *mut c_int, den: *mut c_int);
  pub fn ffctx_get_profile(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_level(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_extradata(ctx: *const AVCodecContext) -> *const u8;
  pub fn ffctx_get_extradata_size(ctx: *const AVCodecContext) -> c_int;

  // ========================================================================
  // AVCodecContext Audio Setters
  // ========================================================================

  pub fn ffctx_set_sample_rate(ctx: *mut AVCodecContext, sample_rate: c_int);
  pub fn ffctx_set_sample_fmt(ctx: *mut AVCodecContext, sample_fmt: c_int);
  pub fn ffctx_set_channels(ctx: *mut AVCodecContext, channels: c_int);
  pub fn ffctx_set_channel_layout(ctx: *mut AVCodecContext, channel_layout: u64);
  pub fn ffctx_set_frame_size(ctx: *mut AVCodecContext, frame_size: c_int);

  // ========================================================================
  // AVCodecContext Audio Getters
  // ========================================================================

  pub fn ffctx_get_sample_rate(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_sample_fmt(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_channels(ctx: *const AVCodecContext) -> c_int;
  pub fn ffctx_get_channel_layout(ctx: *const AVCodecContext) -> u64;
  pub fn ffctx_get_frame_size(ctx: *const AVCodecContext) -> c_int;

  // ========================================================================
  // AVFrame Setters
  // ========================================================================

  pub fn ffframe_set_width(frame: *mut AVFrame, width: c_int);
  pub fn ffframe_set_height(frame: *mut AVFrame, height: c_int);
  pub fn ffframe_set_format(frame: *mut AVFrame, format: c_int);
  pub fn ffframe_set_pts(frame: *mut AVFrame, pts: i64);
  pub fn ffframe_set_duration(frame: *mut AVFrame, duration: i64);
  pub fn ffframe_set_pkt_dts(frame: *mut AVFrame, pkt_dts: i64);
  pub fn ffframe_set_time_base(frame: *mut AVFrame, num: c_int, den: c_int);
  pub fn ffframe_set_key_frame(frame: *mut AVFrame, key_frame: c_int);
  pub fn ffframe_set_pict_type(frame: *mut AVFrame, pict_type: c_int);
  pub fn ffframe_set_color_primaries(frame: *mut AVFrame, color_primaries: c_int);
  pub fn ffframe_set_color_trc(frame: *mut AVFrame, color_trc: c_int);
  pub fn ffframe_set_colorspace(frame: *mut AVFrame, colorspace: c_int);
  pub fn ffframe_set_color_range(frame: *mut AVFrame, color_range: c_int);
  pub fn ffframe_set_sample_aspect_ratio(frame: *mut AVFrame, num: c_int, den: c_int);
  pub fn ffframe_set_data(frame: *mut AVFrame, plane: c_int, data: *mut u8);
  pub fn ffframe_set_linesize(frame: *mut AVFrame, plane: c_int, linesize: c_int);

  // ========================================================================
  // AVFrame Getters
  // ========================================================================

  pub fn ffframe_get_width(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_height(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_format(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_pts(frame: *const AVFrame) -> i64;
  pub fn ffframe_get_duration(frame: *const AVFrame) -> i64;
  pub fn ffframe_get_pkt_dts(frame: *const AVFrame) -> i64;
  pub fn ffframe_get_time_base(frame: *const AVFrame, num: *mut c_int, den: *mut c_int);
  pub fn ffframe_get_key_frame(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_pict_type(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_color_primaries(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_color_trc(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_colorspace(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_color_range(frame: *const AVFrame) -> c_int;

  // ========================================================================
  // AVFrame Audio Setters
  // ========================================================================

  pub fn ffframe_set_nb_samples(frame: *mut AVFrame, nb_samples: c_int);
  pub fn ffframe_set_sample_rate(frame: *mut AVFrame, sample_rate: c_int);
  pub fn ffframe_set_channels(frame: *mut AVFrame, channels: c_int);
  pub fn ffframe_set_channel_layout(frame: *mut AVFrame, channel_layout: u64);

  // ========================================================================
  // AVFrame Audio Getters
  // ========================================================================

  pub fn ffframe_get_nb_samples(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_sample_rate(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_channels(frame: *const AVFrame) -> c_int;
  pub fn ffframe_get_channel_layout(frame: *const AVFrame) -> u64;

  // ========================================================================
  // AVFrame Data Access
  // ========================================================================

  pub fn ffframe_data(frame: *mut AVFrame, plane: c_int) -> *mut u8;
  pub fn ffframe_data_const(frame: *const AVFrame, plane: c_int) -> *const u8;
  pub fn ffframe_linesize(frame: *const AVFrame, plane: c_int) -> c_int;

  // ========================================================================
  // AVFrame Audio Data Access (extended_data for planar audio)
  // ========================================================================

  pub fn ffframe_get_extended_data(frame: *mut AVFrame) -> *mut *mut u8;
  pub fn ffframe_get_extended_data_const(frame: *const AVFrame) -> *const *const u8;
  pub fn ffframe_extended_data_plane(frame: *mut AVFrame, plane: c_int) -> *mut u8;
  pub fn ffframe_set_extended_data(frame: *mut AVFrame, extended_data: *mut *mut u8);

  // ========================================================================
  // AVPacket Getters
  // ========================================================================

  pub fn ffpkt_data(pkt: *const AVPacket) -> *const u8;
  pub fn ffpkt_data_mut(pkt: *mut AVPacket) -> *mut u8;
  pub fn ffpkt_size(pkt: *const AVPacket) -> c_int;
  pub fn ffpkt_pts(pkt: *const AVPacket) -> i64;
  pub fn ffpkt_dts(pkt: *const AVPacket) -> i64;
  pub fn ffpkt_duration(pkt: *const AVPacket) -> i64;
  pub fn ffpkt_flags(pkt: *const AVPacket) -> c_int;
  pub fn ffpkt_stream_index(pkt: *const AVPacket) -> c_int;
  pub fn ffpkt_pos(pkt: *const AVPacket) -> i64;

  // ========================================================================
  // AVPacket Setters
  // ========================================================================

  pub fn ffpkt_set_pts(pkt: *mut AVPacket, pts: i64);
  pub fn ffpkt_set_dts(pkt: *mut AVPacket, dts: i64);
  pub fn ffpkt_set_duration(pkt: *mut AVPacket, duration: i64);
  pub fn ffpkt_set_flags(pkt: *mut AVPacket, flags: c_int);
  pub fn ffpkt_set_stream_index(pkt: *mut AVPacket, stream_index: c_int);

  // ========================================================================
  // Hardware Frames Context
  // ========================================================================

  pub fn ffhwframes_set_format(ref_: *mut AVBufferRef, format: c_int);
  pub fn ffhwframes_set_sw_format(ref_: *mut AVBufferRef, sw_format: c_int);
  pub fn ffhwframes_set_width(ref_: *mut AVBufferRef, width: c_int);
  pub fn ffhwframes_set_height(ref_: *mut AVBufferRef, height: c_int);
  pub fn ffhwframes_set_initial_pool_size(ref_: *mut AVBufferRef, initial_pool_size: c_int);

  pub fn ffhwframes_get_format(ref_: *mut AVBufferRef) -> c_int;
  pub fn ffhwframes_get_sw_format(ref_: *mut AVBufferRef) -> c_int;
  pub fn ffhwframes_get_width(ref_: *mut AVBufferRef) -> c_int;
  pub fn ffhwframes_get_height(ref_: *mut AVBufferRef) -> c_int;

  // ========================================================================
  // Utility Functions
  // ========================================================================

  pub fn ff_get_buffer_size(pix_fmt: c_int, width: c_int, height: c_int, align: c_int) -> c_int;

  pub fn ff_image_fill_arrays(
    dst_data: *mut *mut u8,
    dst_linesize: *mut c_int,
    src: *const u8,
    pix_fmt: c_int,
    width: c_int,
    height: c_int,
    align: c_int,
  ) -> c_int;

  // ========================================================================
  // Audio Utility Functions
  // ========================================================================

  /// Get bytes per sample for a given sample format
  pub fn ff_get_bytes_per_sample(sample_fmt: c_int) -> c_int;

  /// Check if sample format is planar
  pub fn ff_sample_fmt_is_planar(sample_fmt: c_int) -> c_int;

  /// Get buffer size required for audio samples
  pub fn ff_get_audio_buffer_size(
    channels: c_int,
    nb_samples: c_int,
    sample_fmt: c_int,
    align: c_int,
  ) -> c_int;

  /// Fill audio data pointers and linesize for packed data
  pub fn ff_samples_fill_arrays(
    audio_data: *mut *mut u8,
    linesize: *mut c_int,
    buf: *const u8,
    channels: c_int,
    nb_samples: c_int,
    sample_fmt: c_int,
    align: c_int,
  ) -> c_int;
}

// ============================================================================
// Thread Type Flags
// ============================================================================

/// Decode more than one frame at once
pub const FF_THREAD_FRAME: c_int = 1;

/// Decode more than one part of a single frame at once
pub const FF_THREAD_SLICE: c_int = 2;

// ============================================================================
// Codec Flags
// ============================================================================

pub mod codec_flag {
  use std::os::raw::c_int;

  /// Use internal 2-pass rate control in first pass mode
  pub const PASS1: c_int = 1 << 9;

  /// Use internal 2-pass rate control in second pass mode
  pub const PASS2: c_int = 1 << 10;

  /// Place global headers in extradata instead of every keyframe
  pub const GLOBAL_HEADER: c_int = 1 << 22;

  /// Don't output frames whose parameters differ from first decoded frame
  pub const DROPCHANGED: c_int = 1 << 5;

  /// Use only bitexact stuff (except (I)DCT)
  pub const BITEXACT: c_int = 1 << 23;

  /// Allow non-spec-compliant speedup tricks
  pub const FAST: c_int = 1 << 0;

  /// Codec can export data for HW decoding
  pub const EXPORT_DATA: c_int = 1 << 5;
}

pub mod codec_flag2 {
  use std::os::raw::c_int;

  /// Allow non-compliant speedup tricks
  pub const FAST: c_int = 1 << 0;

  /// Skip bitstream encoding
  pub const NO_OUTPUT: c_int = 1 << 2;

  /// Place global headers at every keyframe instead of in extradata
  pub const LOCAL_HEADER: c_int = 1 << 3;

  /// Show all frames before the first keyframe
  pub const SHOW_ALL: c_int = 1 << 22;

  /// Export motion vectors into frame side data
  pub const EXPORT_MVS: c_int = 1 << 28;
}
