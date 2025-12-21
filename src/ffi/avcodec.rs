//! libavcodec function declarations
//!
//! Provides encoding and decoding functionality.

use super::types::*;
use std::os::raw::{c_char, c_int};

unsafe extern "C" {
  // ========================================================================
  // Codec Discovery
  // ========================================================================

  /// Find an encoder by codec ID
  pub fn avcodec_find_encoder(id: c_int) -> *const AVCodec;

  /// Find an encoder by name (e.g., "libx264", "h264_videotoolbox")
  pub fn avcodec_find_encoder_by_name(name: *const c_char) -> *const AVCodec;

  /// Find a decoder by codec ID
  pub fn avcodec_find_decoder(id: c_int) -> *const AVCodec;

  /// Find a decoder by name
  pub fn avcodec_find_decoder_by_name(name: *const c_char) -> *const AVCodec;

  // ========================================================================
  // Codec Context Lifecycle
  // ========================================================================

  /// Allocate an AVCodecContext and set its fields to default values
  pub fn avcodec_alloc_context3(codec: *const AVCodec) -> *mut AVCodecContext;

  /// Free the codec context and everything associated with it
  pub fn avcodec_free_context(avctx: *mut *mut AVCodecContext);

  /// Initialize the AVCodecContext to use the given AVCodec
  pub fn avcodec_open2(
    avctx: *mut AVCodecContext,
    codec: *const AVCodec,
    options: *mut *mut AVDictionary,
  ) -> c_int;

  /// Close a given AVCodecContext and free all data associated with it
  /// (but not the AVCodecContext itself)
  pub fn avcodec_close(avctx: *mut AVCodecContext) -> c_int;

  // ========================================================================
  // Encoding (send frame, receive packet)
  // ========================================================================

  /// Supply a raw video frame to the encoder
  ///
  /// # Arguments
  /// * `avctx` - Codec context
  /// * `frame` - AVFrame containing the raw video data, or NULL to flush
  ///
  /// # Returns
  /// * 0 on success
  /// * AVERROR(EAGAIN) - output not available, must read with receive_packet first
  /// * AVERROR_EOF - encoder has been flushed, no more output
  /// * AVERROR(EINVAL) - codec not opened, or requires flush
  /// * AVERROR(ENOMEM) - failed to add packet to queue
  pub fn avcodec_send_frame(avctx: *mut AVCodecContext, frame: *const AVFrame) -> c_int;

  /// Read encoded data from the encoder
  ///
  /// # Arguments
  /// * `avctx` - Codec context
  /// * `avpkt` - Packet to store encoded data
  ///
  /// # Returns
  /// * 0 on success
  /// * AVERROR(EAGAIN) - output not available, must send more input
  /// * AVERROR_EOF - encoder has been fully flushed
  /// * AVERROR(EINVAL) - codec not opened
  pub fn avcodec_receive_packet(avctx: *mut AVCodecContext, avpkt: *mut AVPacket) -> c_int;

  // ========================================================================
  // Decoding (send packet, receive frame)
  // ========================================================================

  /// Supply raw packet data to the decoder
  ///
  /// # Arguments
  /// * `avctx` - Codec context
  /// * `avpkt` - AVPacket containing compressed data, or NULL to flush
  ///
  /// # Returns
  /// * 0 on success
  /// * AVERROR(EAGAIN) - output not available, must read with receive_frame first
  /// * AVERROR_EOF - decoder has been flushed
  /// * AVERROR(EINVAL) - codec not opened
  /// * AVERROR(ENOMEM) - failed to add packet to queue
  pub fn avcodec_send_packet(avctx: *mut AVCodecContext, avpkt: *const AVPacket) -> c_int;

  /// Return decoded output data from the decoder
  ///
  /// # Arguments
  /// * `avctx` - Codec context
  /// * `frame` - Frame to store decoded data
  ///
  /// # Returns
  /// * 0 on success
  /// * AVERROR(EAGAIN) - output not available, must send more input
  /// * AVERROR_EOF - decoder has been fully flushed
  /// * AVERROR(EINVAL) - codec not opened
  pub fn avcodec_receive_frame(avctx: *mut AVCodecContext, frame: *mut AVFrame) -> c_int;

  // ========================================================================
  // Codec Control
  // ========================================================================

  /// Reset the internal codec state / flush internal buffers
  /// Should be called when seeking or switching to a different stream
  pub fn avcodec_flush_buffers(avctx: *mut AVCodecContext);

  // ========================================================================
  // Packet Management
  // ========================================================================

  /// Allocate an AVPacket and set its fields to default values
  pub fn av_packet_alloc() -> *mut AVPacket;

  /// Free the packet, if the packet is reference counted, it will be unreferenced first
  pub fn av_packet_free(pkt: *mut *mut AVPacket);

  /// Wipe the packet. Unreference the buffer and reset fields to defaults
  pub fn av_packet_unref(pkt: *mut AVPacket);

  /// Create a new packet that references the same data as src
  pub fn av_packet_ref(dst: *mut AVPacket, src: *const AVPacket) -> c_int;

  /// Allocate new buffer for the packet with size bytes
  pub fn av_new_packet(pkt: *mut AVPacket, size: c_int) -> c_int;

  /// Reduce packet size, correctly zeroing padding
  pub fn av_shrink_packet(pkt: *mut AVPacket, size: c_int);

  /// Increase packet size, correctly zeroing padding
  pub fn av_grow_packet(pkt: *mut AVPacket, grow_by: c_int) -> c_int;

  /// Get side data from a packet
  ///
  /// # Arguments
  /// * `pkt` - packet to get side data from
  /// * `type_` - type of side data to get
  /// * `size` - pointer to store size of side data
  ///
  /// # Returns
  /// Pointer to side data, or NULL if not found
  pub fn av_packet_get_side_data(pkt: *const AVPacket, type_: c_int, size: *mut usize)
  -> *const u8;

  /// Allocate new side data for a packet
  ///
  /// # Arguments
  /// * `pkt` - packet to add side data to
  /// * `type_` - type of side data
  /// * `size` - size of side data in bytes
  ///
  /// # Returns
  /// Pointer to newly allocated side data, or NULL on failure
  pub fn av_packet_new_side_data(pkt: *mut AVPacket, type_: c_int, size: usize) -> *mut u8;

  // ========================================================================
  // Codec Parameters
  // ========================================================================

  /// Get the name of a codec
  pub fn avcodec_get_name(id: c_int) -> *const c_char;

  // ========================================================================
  // Threading
  // ========================================================================

  /// Get the type of threading used by the codec
  pub fn avcodec_get_type(codec_id: c_int) -> c_int;
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Find encoder by AVCodecID enum
pub fn find_encoder(codec_id: AVCodecID) -> *const AVCodec {
  unsafe { avcodec_find_encoder(codec_id.as_raw()) }
}

/// Find decoder by AVCodecID enum
pub fn find_decoder(codec_id: AVCodecID) -> *const AVCodec {
  unsafe { avcodec_find_decoder(codec_id.as_raw()) }
}

/// Find encoder by name (safe wrapper)
pub fn find_encoder_by_name(name: &str) -> *const AVCodec {
  let c_name = std::ffi::CString::new(name).unwrap();
  unsafe { avcodec_find_encoder_by_name(c_name.as_ptr()) }
}

/// Find decoder by name (safe wrapper)
pub fn find_decoder_by_name(name: &str) -> *const AVCodec {
  let c_name = std::ffi::CString::new(name).unwrap();
  unsafe { avcodec_find_decoder_by_name(c_name.as_ptr()) }
}
