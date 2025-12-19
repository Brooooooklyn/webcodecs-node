//! libavformat function declarations
//!
//! Provides muxing and demuxing functionality for container formats.

use super::types::*;
use std::os::raw::{c_char, c_int, c_uint, c_void};

// ============================================================================
// Opaque Types (format-specific)
// ============================================================================

/// Opaque AVFormatContext structure (muxer/demuxer context)
#[repr(C)]
pub struct AVFormatContext {
  _opaque: [u8; 0],
  _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVOutputFormat structure (output format descriptor)
#[repr(C)]
pub struct AVOutputFormat {
  _opaque: [u8; 0],
  _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVInputFormat structure (input format descriptor)
#[repr(C)]
pub struct AVInputFormat {
  _opaque: [u8; 0],
  _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVIOContext structure (I/O abstraction)
#[repr(C)]
pub struct AVIOContext {
  _opaque: [u8; 0],
  _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVStream structure (stream within container)
#[repr(C)]
pub struct AVStream {
  _opaque: [u8; 0],
  _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

/// Opaque AVCodecParameters structure (stream codec parameters)
#[repr(C)]
pub struct AVCodecParameters {
  _opaque: [u8; 0],
  _marker: std::marker::PhantomData<(*mut u8, std::marker::PhantomPinned)>,
}

// ============================================================================
// Callback Types for Custom I/O
// ============================================================================

/// Read callback for custom I/O
///
/// # Arguments
/// * `opaque` - User-provided opaque pointer
/// * `buf` - Buffer to read into
/// * `buf_size` - Size of buffer
///
/// # Returns
/// Number of bytes read, or negative AVERROR on error
pub type ReadPacketFn =
  unsafe extern "C" fn(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int;

/// Write callback for custom I/O
///
/// # Arguments
/// * `opaque` - User-provided opaque pointer
/// * `buf` - Buffer containing data to write
/// * `buf_size` - Number of bytes to write
///
/// # Returns
/// Number of bytes written, or negative AVERROR on error
pub type WritePacketFn =
  unsafe extern "C" fn(opaque: *mut c_void, buf: *const u8, buf_size: c_int) -> c_int;

/// Seek callback for custom I/O
///
/// # Arguments
/// * `opaque` - User-provided opaque pointer
/// * `offset` - Seek offset
/// * `whence` - Seek mode (SEEK_SET, SEEK_CUR, SEEK_END, or AVSEEK_SIZE)
///
/// # Returns
/// New position, or negative AVERROR on error
pub type SeekFn = unsafe extern "C" fn(opaque: *mut c_void, offset: i64, whence: c_int) -> i64;

unsafe extern "C" {
  // ========================================================================
  // Output Context (Muxing)
  // ========================================================================

  /// Allocate an AVFormatContext for output
  ///
  /// # Arguments
  /// * `ctx` - Pointer to receive the allocated context (set to NULL on failure)
  /// * `oformat` - Output format to use (can be NULL to auto-detect)
  /// * `format_name` - Short name of the format (e.g., "mp4", "webm")
  /// * `filename` - Filename for format detection (can be NULL)
  ///
  /// # Returns
  /// * >= 0 on success
  /// * AVERROR(ENOMEM) if allocation failed
  /// * AVERROR(EINVAL) if no format found
  pub fn avformat_alloc_output_context2(
    ctx: *mut *mut AVFormatContext,
    oformat: *const AVOutputFormat,
    format_name: *const c_char,
    filename: *const c_char,
  ) -> c_int;

  /// Allocate an AVFormatContext
  ///
  /// Must be freed with avformat_free_context() or avformat_close_input()
  pub fn avformat_alloc_context() -> *mut AVFormatContext;

  /// Free an AVFormatContext and all its streams
  ///
  /// # Safety
  /// The context pointer becomes invalid after this call
  pub fn avformat_free_context(ctx: *mut AVFormatContext);

  // ========================================================================
  // Stream Management
  // ========================================================================

  /// Add a new stream to the format context
  ///
  /// # Arguments
  /// * `ctx` - Format context
  /// * `codec` - Codec used by the stream (can be NULL)
  ///
  /// # Returns
  /// Pointer to newly created stream, or NULL on error
  pub fn avformat_new_stream(ctx: *mut AVFormatContext, codec: *const AVCodec) -> *mut AVStream;

  // ========================================================================
  // Muxing Operations
  // ========================================================================

  /// Write the stream header to the output file
  ///
  /// # Arguments
  /// * `ctx` - Format context
  /// * `options` - Muxer options (can be NULL)
  ///
  /// # Returns
  /// * AVSTREAM_INIT_IN_WRITE_HEADER on success if codec not fully initialized
  /// * AVSTREAM_INIT_IN_INIT_OUTPUT on success if codec was fully initialized
  /// * Negative AVERROR on error
  pub fn avformat_write_header(ctx: *mut AVFormatContext, options: *mut *mut AVDictionary)
  -> c_int;

  /// Write a packet to the output file (interleaved)
  ///
  /// This function will buffer packets internally to ensure proper interleaving.
  /// Pass NULL to flush the interleaving queue.
  ///
  /// # Arguments
  /// * `ctx` - Format context
  /// * `pkt` - Packet to write, or NULL to flush
  ///
  /// # Returns
  /// * 0 on success
  /// * Negative AVERROR on error
  pub fn av_interleaved_write_frame(ctx: *mut AVFormatContext, pkt: *mut AVPacket) -> c_int;

  /// Write a packet to the output file (non-interleaved)
  ///
  /// The caller is responsible for correct interleaving.
  ///
  /// # Arguments
  /// * `ctx` - Format context
  /// * `pkt` - Packet to write
  ///
  /// # Returns
  /// * 0 on success
  /// * Negative AVERROR on error
  pub fn av_write_frame(ctx: *mut AVFormatContext, pkt: *mut AVPacket) -> c_int;

  /// Write the stream trailer to the output file
  ///
  /// Must be called after all packets have been written.
  ///
  /// # Arguments
  /// * `ctx` - Format context
  ///
  /// # Returns
  /// * 0 on success
  /// * Negative AVERROR on error
  pub fn av_write_trailer(ctx: *mut AVFormatContext) -> c_int;

  // ========================================================================
  // Input Context (Demuxing)
  // ========================================================================

  /// Open an input stream and read the header
  ///
  /// # Arguments
  /// * `ps` - Pointer to context (will be allocated if NULL)
  /// * `url` - URL/filename to open
  /// * `fmt` - Input format (NULL for auto-detect)
  /// * `options` - Demuxer options
  ///
  /// # Returns
  /// * 0 on success
  /// * Negative AVERROR on error
  pub fn avformat_open_input(
    ps: *mut *mut AVFormatContext,
    url: *const c_char,
    fmt: *const AVInputFormat,
    options: *mut *mut AVDictionary,
  ) -> c_int;

  /// Close an opened input AVFormatContext
  ///
  /// Frees the context and all its contents and sets *s to NULL.
  pub fn avformat_close_input(s: *mut *mut AVFormatContext);

  /// Read packets of a media file to get stream information
  ///
  /// This is useful for file formats with no headers such as MPEG.
  ///
  /// # Arguments
  /// * `ic` - Format context
  /// * `options` - Per-stream options (can be NULL)
  ///
  /// # Returns
  /// * >= 0 on success
  /// * AVERROR_xxx on failure
  pub fn avformat_find_stream_info(
    ic: *mut AVFormatContext,
    options: *mut *mut AVDictionary,
  ) -> c_int;

  /// Find the "best" stream in the file
  ///
  /// # Arguments
  /// * `ic` - Format context
  /// * `type_` - Stream type (AVMEDIA_TYPE_VIDEO, AVMEDIA_TYPE_AUDIO, etc.)
  /// * `wanted_stream_nb` - Desired stream number, or -1 for automatic
  /// * `related_stream` - Related stream for disposition check
  /// * `decoder_ret` - Pointer to receive the decoder (can be NULL)
  /// * `flags` - Reserved (should be 0)
  ///
  /// # Returns
  /// * >= 0 on success (stream index)
  /// * AVERROR_STREAM_NOT_FOUND if not found
  /// * AVERROR_DECODER_NOT_FOUND if decoder not found
  pub fn av_find_best_stream(
    ic: *mut AVFormatContext,
    type_: c_int,
    wanted_stream_nb: c_int,
    related_stream: c_int,
    decoder_ret: *mut *const AVCodec,
    flags: c_int,
  ) -> c_int;

  /// Return the next frame of a stream
  ///
  /// # Arguments
  /// * `s` - Format context
  /// * `pkt` - Packet to fill (must be allocated, will be unreferenced first)
  ///
  /// # Returns
  /// * 0 on success
  /// * AVERROR_EOF at end of file
  /// * Negative AVERROR on error
  pub fn av_read_frame(s: *mut AVFormatContext, pkt: *mut AVPacket) -> c_int;

  /// Seek to a keyframe at the given timestamp
  ///
  /// # Arguments
  /// * `s` - Format context
  /// * `stream_index` - Stream index, or -1 for default
  /// * `timestamp` - Timestamp in stream time base units (or AV_TIME_BASE if stream_index is -1)
  /// * `flags` - Seek flags (AVSEEK_FLAG_*)
  ///
  /// # Returns
  /// * >= 0 on success
  /// * Negative AVERROR on error
  pub fn av_seek_frame(
    s: *mut AVFormatContext,
    stream_index: c_int,
    timestamp: i64,
    flags: c_int,
  ) -> c_int;

  /// Seek to timestamp with min/max constraints
  ///
  /// # Arguments
  /// * `s` - Format context
  /// * `stream_index` - Stream index
  /// * `min_ts` - Minimum acceptable timestamp
  /// * `ts` - Target timestamp
  /// * `max_ts` - Maximum acceptable timestamp
  /// * `flags` - Seek flags
  ///
  /// # Returns
  /// * >= 0 on success
  /// * Negative AVERROR on error
  pub fn avformat_seek_file(
    s: *mut AVFormatContext,
    stream_index: c_int,
    min_ts: i64,
    ts: i64,
    max_ts: i64,
    flags: c_int,
  ) -> c_int;

  // ========================================================================
  // Custom I/O
  // ========================================================================

  /// Allocate and initialize an AVIOContext for custom I/O
  ///
  /// # Arguments
  /// * `buffer` - Memory block for buffering (must be allocated with av_malloc)
  /// * `buffer_size` - Size of the buffer
  /// * `write_flag` - 1 if writing, 0 if reading
  /// * `opaque` - User-provided opaque pointer passed to callbacks
  /// * `read_packet` - Read callback (NULL for write-only)
  /// * `write_packet` - Write callback (NULL for read-only)
  /// * `seek` - Seek callback (can be NULL for non-seekable)
  ///
  /// # Returns
  /// Pointer to allocated context, or NULL on failure
  ///
  /// # Safety
  /// The buffer must be allocated with av_malloc and remains owned by the AVIOContext
  pub fn avio_alloc_context(
    buffer: *mut u8,
    buffer_size: c_int,
    write_flag: c_int,
    opaque: *mut c_void,
    read_packet: Option<ReadPacketFn>,
    write_packet: Option<WritePacketFn>,
    seek: Option<SeekFn>,
  ) -> *mut AVIOContext;

  /// Free the AVIOContext
  ///
  /// # Safety
  /// The internal buffer is NOT freed. Caller must free it separately.
  pub fn avio_context_free(s: *mut *mut AVIOContext);

  /// Force flushing of buffered data to the output
  pub fn avio_flush(s: *mut AVIOContext);

  /// Open a file for I/O
  ///
  /// # Arguments
  /// * `s` - Pointer to receive the I/O context
  /// * `url` - URL/filename to open
  /// * `flags` - AVIO_FLAG_* flags
  ///
  /// # Returns
  /// * >= 0 on success
  /// * Negative AVERROR on error
  pub fn avio_open(s: *mut *mut AVIOContext, url: *const c_char, flags: c_int) -> c_int;

  /// Close an I/O context opened by avio_open
  ///
  /// # Returns
  /// * 0 on success
  /// * Negative AVERROR on error
  pub fn avio_close(s: *mut AVIOContext) -> c_int;

  /// Open a file for I/O with additional options
  pub fn avio_open2(
    s: *mut *mut AVIOContext,
    url: *const c_char,
    flags: c_int,
    int_cb: *const c_void,
    options: *mut *mut AVDictionary,
  ) -> c_int;

  // ========================================================================
  // Format Detection
  // ========================================================================

  /// Guess the output format by short name, filename, or MIME type
  ///
  /// # Arguments
  /// * `short_name` - Format short name (e.g., "mp4", "webm")
  /// * `filename` - Filename with extension
  /// * `mime_type` - MIME type
  ///
  /// # Returns
  /// Pointer to output format, or NULL if not found
  pub fn av_guess_format(
    short_name: *const c_char,
    filename: *const c_char,
    mime_type: *const c_char,
  ) -> *const AVOutputFormat;

  /// Probe the input buffer to determine the input format
  pub fn av_probe_input_buffer2(
    pb: *mut AVIOContext,
    fmt: *mut *const AVInputFormat,
    url: *const c_char,
    logctx: *mut c_void,
    offset: c_uint,
    max_probe_size: c_uint,
  ) -> c_int;

  // ========================================================================
  // Codec Parameters
  // ========================================================================

  /// Allocate a new AVCodecParameters and set its fields to default values
  pub fn avcodec_parameters_alloc() -> *mut AVCodecParameters;

  /// Free an AVCodecParameters instance
  pub fn avcodec_parameters_free(par: *mut *mut AVCodecParameters);

  /// Copy codec parameters from a codec context to AVCodecParameters
  pub fn avcodec_parameters_from_context(
    par: *mut AVCodecParameters,
    codec: *const AVCodecContext,
  ) -> c_int;

  /// Fill codec context with codec parameters
  pub fn avcodec_parameters_to_context(
    codec: *mut AVCodecContext,
    par: *const AVCodecParameters,
  ) -> c_int;

  /// Copy the contents of src to dst
  pub fn avcodec_parameters_copy(
    dst: *mut AVCodecParameters,
    src: *const AVCodecParameters,
  ) -> c_int;
}

// ============================================================================
// Constants
// ============================================================================

/// Seek flags
pub mod seek_flag {
  use std::os::raw::c_int;

  /// Seek backward
  pub const BACKWARD: c_int = 1;
  /// Seeking based on position in bytes
  pub const BYTE: c_int = 2;
  /// Seek to any frame (not just keyframes)
  pub const ANY: c_int = 4;
  /// Seeking based on frame number
  pub const FRAME: c_int = 8;
}

/// Seek whence values
pub mod seek_whence {
  use std::os::raw::c_int;

  /// Seek from beginning
  pub const SEEK_SET: c_int = 0;
  /// Seek from current position
  pub const SEEK_CUR: c_int = 1;
  /// Seek from end
  pub const SEEK_END: c_int = 2;
  /// Return file size (special whence value for seek callback)
  pub const AVSEEK_SIZE: c_int = 0x10000;
  /// Force seek even if not efficient
  pub const AVSEEK_FORCE: c_int = 0x20000;
}

/// AVIO flags
pub mod avio_flag {
  use std::os::raw::c_int;

  /// Read-only
  pub const READ: c_int = 1;
  /// Write-only
  pub const WRITE: c_int = 2;
  /// Read-write
  pub const READ_WRITE: c_int = READ | WRITE;
}

/// Media types (for av_find_best_stream)
pub mod media_type {
  use std::os::raw::c_int;

  pub const UNKNOWN: c_int = -1;
  pub const VIDEO: c_int = 0;
  pub const AUDIO: c_int = 1;
  pub const DATA: c_int = 2;
  pub const SUBTITLE: c_int = 3;
  pub const ATTACHMENT: c_int = 4;
}

/// Format context flags
pub mod avfmt_flag {
  use std::os::raw::c_int;

  /// Caller must not alter stream info
  pub const NOFILE: c_int = 0x0001;
  /// Needs global header
  pub const GLOBALHEADER: c_int = 0x0040;
  /// Format wants AVPicture structure for raw picture data
  pub const RAWPICTURE: c_int = 0x0020;
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Find output format by short name (safe wrapper)
pub fn find_output_format(short_name: &str) -> *const AVOutputFormat {
  let c_name = std::ffi::CString::new(short_name).unwrap();
  unsafe { av_guess_format(c_name.as_ptr(), std::ptr::null(), std::ptr::null()) }
}
