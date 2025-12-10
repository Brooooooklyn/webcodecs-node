//! Safe wrapper around FFmpeg AVCodecContext
//!
//! Provides encoding and decoding functionality with RAII cleanup.

use crate::ffi::{
  self, AVCodec, AVCodecContext, AVCodecID, AVHWDeviceType, AVPixelFormat,
  accessors::{
    ffctx_get_extradata, ffctx_get_extradata_size, ffctx_get_frame_size, ffctx_get_height,
    ffctx_get_pix_fmt, ffctx_get_sample_rate, ffctx_get_width, ffctx_set_bit_rate,
    ffctx_set_channels, ffctx_set_framerate, ffctx_set_gop_size, ffctx_set_height,
    ffctx_set_hw_device_ctx, ffctx_set_level, ffctx_set_max_b_frames, ffctx_set_pix_fmt,
    ffctx_set_profile, ffctx_set_rc_buffer_size, ffctx_set_rc_max_rate, ffctx_set_sample_fmt,
    ffctx_set_sample_rate, ffctx_set_thread_count, ffctx_set_time_base, ffctx_set_width,
  },
  avcodec::{
    avcodec_alloc_context3, avcodec_find_decoder, avcodec_find_encoder,
    avcodec_find_encoder_by_name, avcodec_flush_buffers, avcodec_free_context, avcodec_open2,
    avcodec_receive_frame, avcodec_receive_packet, avcodec_send_frame, avcodec_send_packet,
  },
  avutil::{av_opt_set, av_opt_set_int, opt_flag},
  error::{AVERROR_EAGAIN, AVERROR_EOF},
};
use std::ffi::CString;
use std::ptr::NonNull;

use super::{
  AudioDecoderConfig, AudioEncoderConfig, BitrateMode, CodecError, CodecResult, DecoderConfig,
  EncoderConfig, Frame, HwDeviceContext, Packet,
};

/// Result of encoder creation with metadata about hardware acceleration
pub struct EncoderCreationResult {
  /// The created codec context
  pub context: CodecContext,
  /// Whether the encoder uses hardware acceleration
  pub is_hardware: bool,
  /// Name of the encoder (e.g., "h264_videotoolbox", "libx264")
  pub encoder_name: String,
}

/// Result of decoder creation with metadata about hardware acceleration
pub struct DecoderCreationResult {
  /// The created codec context
  pub context: CodecContext,
  /// Whether the decoder uses hardware acceleration
  pub is_hardware: bool,
}

/// Type of codec (encoder or decoder)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecType {
  Encoder,
  Decoder,
}

/// Safe wrapper around AVCodecContext
pub struct CodecContext {
  ptr: NonNull<AVCodecContext>,
  codec: *const AVCodec,
  codec_type: CodecType,
  hw_device: Option<HwDeviceContext>,
}

impl CodecContext {
  // ========================================================================
  // Encoder Creation
  // ========================================================================

  /// Create a new encoder context for the given codec ID
  pub fn new_encoder(codec_id: AVCodecID) -> CodecResult<Self> {
    let codec = unsafe { avcodec_find_encoder(codec_id.as_raw()) };
    if codec.is_null() {
      return Err(CodecError::EncoderNotFound(codec_id));
    }
    Self::from_codec(codec, CodecType::Encoder)
  }

  /// Create a new encoder context by codec name (e.g., "libx264", "h264_videotoolbox")
  pub fn new_encoder_by_name(name: &str) -> CodecResult<Self> {
    let c_name =
      CString::new(name).map_err(|_| CodecError::InvalidConfig("Invalid codec name".into()))?;
    let codec = unsafe { avcodec_find_encoder_by_name(c_name.as_ptr()) };
    if codec.is_null() {
      return Err(CodecError::CodecNotFound(name.to_string()));
    }
    Self::from_codec(codec, CodecType::Encoder)
  }

  /// Create encoder with hardware acceleration preference
  pub fn new_encoder_with_hw(
    codec_id: AVCodecID,
    hw_type: Option<AVHWDeviceType>,
  ) -> CodecResult<Self> {
    // Try hardware encoder first if requested
    if let Some(hw) = hw_type
      && let Some(name) = get_hw_encoder_name(codec_id, hw)
      && let Ok(mut ctx) = Self::new_encoder_by_name(name)
    {
      // Try to create and attach hardware device context
      // Some encoders (like VideoToolbox) don't need it, but VAAPI does
      if hw_encoder_needs_device_context(hw)
        && let Ok(hw_device) = HwDeviceContext::new(hw)
      {
        ctx.set_hw_device(hw_device);
      }
      // If hw device creation fails, the encoder might still work
      // (e.g., VideoToolbox manages its own device)
      return Ok(ctx);
    }

    // Fall back to software encoder
    if let Some(name) = get_sw_encoder_name(codec_id)
      && let Ok(ctx) = Self::new_encoder_by_name(name)
    {
      return Ok(ctx);
    }

    // Last resort: generic codec ID lookup
    Self::new_encoder(codec_id)
  }

  /// Create encoder with hardware acceleration preference and return metadata
  ///
  /// Unlike `new_encoder_with_hw`, this function returns information about
  /// whether hardware acceleration was actually used, enabling runtime fallback.
  pub fn new_encoder_with_hw_info(
    codec_id: AVCodecID,
    hw_type: Option<AVHWDeviceType>,
  ) -> CodecResult<EncoderCreationResult> {
    // Try hardware encoder first if requested
    if let Some(hw) = hw_type
      && let Some(name) = get_hw_encoder_name(codec_id, hw)
      && let Ok(mut ctx) = Self::new_encoder_by_name(name)
    {
      // Try to create and attach hardware device context
      if hw_encoder_needs_device_context(hw)
        && let Ok(hw_device) = HwDeviceContext::new(hw)
      {
        ctx.set_hw_device(hw_device);
      }
      return Ok(EncoderCreationResult {
        context: ctx,
        is_hardware: true,
        encoder_name: name.to_string(),
      });
    }

    // Fall back to software encoder
    if let Some(name) = get_sw_encoder_name(codec_id)
      && let Ok(ctx) = Self::new_encoder_by_name(name)
    {
      return Ok(EncoderCreationResult {
        context: ctx,
        is_hardware: false,
        encoder_name: name.to_string(),
      });
    }

    // Last resort: generic codec ID lookup
    let ctx = Self::new_encoder(codec_id)?;
    Ok(EncoderCreationResult {
      context: ctx,
      is_hardware: false,
      encoder_name: format!("codec_{:?}", codec_id),
    })
  }

  /// Create decoder with hardware acceleration preference
  pub fn new_decoder_with_hw(
    codec_id: AVCodecID,
    hw_type: Option<AVHWDeviceType>,
  ) -> CodecResult<Self> {
    // Create the software decoder (hardware decoding uses the same decoder
    // with hw_device_ctx attached)
    let mut ctx = Self::new_decoder(codec_id)?;

    // Attach hardware device context if requested
    if let Some(hw) = hw_type
      && let Ok(hw_device) = HwDeviceContext::new(hw)
    {
      ctx.set_hw_device(hw_device);
    }
    // Hardware decode will fall back to software if device creation fails

    Ok(ctx)
  }

  /// Create decoder with hardware acceleration preference and return metadata
  ///
  /// Unlike `new_decoder_with_hw`, this function returns information about
  /// whether hardware acceleration was actually used, enabling runtime fallback.
  pub fn new_decoder_with_hw_info(
    codec_id: AVCodecID,
    hw_type: Option<AVHWDeviceType>,
  ) -> CodecResult<DecoderCreationResult> {
    let mut ctx = Self::new_decoder(codec_id)?;
    let mut is_hardware = false;

    // Attach hardware device context if requested
    if let Some(hw) = hw_type
      && let Ok(hw_device) = HwDeviceContext::new(hw)
    {
      ctx.set_hw_device(hw_device);
      is_hardware = true;
    }
    // Hardware decode will fall back to software if device creation fails

    Ok(DecoderCreationResult {
      context: ctx,
      is_hardware,
    })
  }

  // ========================================================================
  // Decoder Creation
  // ========================================================================

  /// Create a new decoder context for the given codec ID.
  /// For AV1, we prefer libdav1d over libaom because:
  /// 1. libdav1d is more stable on darwin/aarch64 (Apple Silicon)
  /// 2. libaom has known SIGSEGV issues during cleanup on some platforms
  /// 3. libdav1d generally has better performance
  pub fn new_decoder(codec_id: AVCodecID) -> CodecResult<Self> {
    // For AV1, try libdav1d first to avoid libaom stability issues
    if codec_id == AVCodecID::Av1 {
      let decoder_name = c"libdav1d";
      let codec = unsafe { ffi::avcodec::avcodec_find_decoder_by_name(decoder_name.as_ptr()) };
      if !codec.is_null() {
        return Self::from_codec(codec, CodecType::Decoder);
      }
      // Fall back to default if libdav1d not available
    }

    let codec = unsafe { avcodec_find_decoder(codec_id.as_raw()) };
    if codec.is_null() {
      return Err(CodecError::DecoderNotFound(codec_id));
    }
    Self::from_codec(codec, CodecType::Decoder)
  }

  fn from_codec(codec: *const AVCodec, codec_type: CodecType) -> CodecResult<Self> {
    let ptr = unsafe { avcodec_alloc_context3(codec) };
    NonNull::new(ptr)
      .map(|ptr| Self {
        ptr,
        codec,
        codec_type,
        hw_device: None,
      })
      .ok_or(CodecError::AllocationFailed("AVCodecContext"))
  }

  // ========================================================================
  // Configuration
  // ========================================================================

  /// Configure the encoder with the given settings
  pub fn configure_encoder(&mut self, config: &EncoderConfig) -> CodecResult<()> {
    if self.codec_type != CodecType::Encoder {
      return Err(CodecError::InvalidState("Not an encoder context".into()));
    }

    unsafe {
      let ctx = self.ptr.as_ptr();

      // Video dimensions
      ffctx_set_width(ctx, config.width as i32);
      ffctx_set_height(ctx, config.height as i32);

      // Pixel format
      ffctx_set_pix_fmt(ctx, config.pixel_format.as_raw());

      // Rate control based on bitrate mode
      match config.bitrate_mode {
        BitrateMode::Constant => {
          // CBR: Set bitrate and optionally rc_max_rate equal to bitrate
          ffctx_set_bit_rate(ctx, config.bitrate as i64);
          let rc_max = config.rc_max_rate.unwrap_or(config.bitrate);
          ffctx_set_rc_max_rate(ctx, rc_max as i64);
          if let Some(buf_size) = config.rc_buffer_size {
            ffctx_set_rc_buffer_size(ctx, buf_size as i32);
          }
        }
        BitrateMode::Variable => {
          // VBR: Set bitrate with higher rc_max_rate
          ffctx_set_bit_rate(ctx, config.bitrate as i64);
          let rc_max = config.rc_max_rate.unwrap_or(config.bitrate * 2);
          ffctx_set_rc_max_rate(ctx, rc_max as i64);
          if let Some(buf_size) = config.rc_buffer_size {
            ffctx_set_rc_buffer_size(ctx, buf_size as i32);
          }
        }
        BitrateMode::Quantizer => {
          // CRF/CQ mode: Set bitrate to 0 and use CRF option
          ffctx_set_bit_rate(ctx, 0);
          let crf_value = config.crf.unwrap_or(23) as i64; // Default CRF 23

          // Set CRF via av_opt_set_int (works for x264, x265)
          let crf_key = CString::new("crf").expect("CString::new failed");
          av_opt_set_int(
            ctx as *mut std::ffi::c_void,
            crf_key.as_ptr(),
            crf_value,
            opt_flag::SEARCH_CHILDREN,
          );

          // Also try "cq" for VP9/AV1 (constrained quality)
          let cq_key = CString::new("cq").expect("CString::new failed");
          av_opt_set_int(
            ctx as *mut std::ffi::c_void,
            cq_key.as_ptr(),
            crf_value,
            opt_flag::SEARCH_CHILDREN,
          );

          // And "qp" as fallback for some codecs
          let qp_key = CString::new("qp").expect("CString::new failed");
          av_opt_set_int(
            ctx as *mut std::ffi::c_void,
            qp_key.as_ptr(),
            crf_value,
            opt_flag::SEARCH_CHILDREN,
          );
        }
      }

      // Time base (inverse of framerate for encoding)
      ffctx_set_time_base(
        ctx,
        config.framerate_den as i32,
        config.framerate_num as i32,
      );

      // Framerate
      ffctx_set_framerate(
        ctx,
        config.framerate_num as i32,
        config.framerate_den as i32,
      );

      // GOP settings
      ffctx_set_gop_size(ctx, config.gop_size as i32);
      ffctx_set_max_b_frames(ctx, config.max_b_frames as i32);

      // Threading
      if config.thread_count > 0 {
        ffctx_set_thread_count(ctx, config.thread_count as i32);
      }

      // Profile and level
      if let Some(profile) = config.profile {
        ffctx_set_profile(ctx, profile);
      }
      if let Some(level) = config.level {
        ffctx_set_level(ctx, level);
      }

      // Configure x265 via x265-params:
      // - preset=medium: Ensures consistent quality across platforms
      // - log-level=error: Suppress verbose x265 logging
      // - qpmax=40: Limit maximum QP to prevent extreme quality degradation
      //   (default is 69, which allows very low quality in edge cases like single-frame encoding)
      av_opt_set(
        ctx as *mut std::ffi::c_void,
        c"x265-params".as_ptr(),
        c"preset=medium:log-level=error:qpmax=40".as_ptr(),
        opt_flag::SEARCH_CHILDREN,
      );
    }

    Ok(())
  }

  /// Apply hardware encoder-specific options based on the encoder name and latency mode
  ///
  /// This method sets sensible FFmpeg options for hardware encoders to optimize
  /// for either low-latency (realtime) or high-quality encoding.
  ///
  /// # Arguments
  /// * `encoder_name` - The FFmpeg encoder name (e.g., "h264_videotoolbox", "h264_nvenc")
  /// * `realtime` - If true, optimize for low latency; otherwise optimize for quality
  ///
  /// # Hardware encoder options applied:
  ///
  /// ## VideoToolbox (macOS)
  /// - realtime=1: Enable realtime encoding mode
  /// - allow_sw=0: Disable software fallback for consistent behavior
  ///
  /// ## NVENC (NVIDIA)
  /// - preset=p4 (quality) / p1 (realtime): Encoding speed/quality tradeoff
  /// - tune=hq (quality) / ll (realtime): Tune for high quality or low latency
  /// - rc-lookahead=20 (quality) / 0 (realtime): Look-ahead frames for better quality
  /// - spatial-aq=1: Enable spatial adaptive quantization for quality
  ///
  /// ## VAAPI (Linux)
  /// - quality=4 (balanced): 0-8 scale, lower = better quality
  ///
  /// ## QSV (Intel)
  /// - preset=medium (quality) / veryfast (realtime): Speed preset
  /// - look_ahead=1 (quality) / 0 (realtime): Enable look-ahead for better quality
  pub fn apply_hw_encoder_options(&mut self, encoder_name: &str, realtime: bool) {
    unsafe {
      let ctx = self.ptr.as_ptr() as *mut std::ffi::c_void;

      // VideoToolbox (macOS)
      if encoder_name.contains("videotoolbox") {
        if realtime {
          // Enable realtime encoding for low latency
          av_opt_set_int(ctx, c"realtime".as_ptr(), 1, opt_flag::SEARCH_CHILDREN);
        }
        // Disable software fallback for consistent hardware behavior
        av_opt_set_int(ctx, c"allow_sw".as_ptr(), 0, opt_flag::SEARCH_CHILDREN);
      }
      // NVENC (NVIDIA)
      else if encoder_name.contains("nvenc") {
        if realtime {
          // Low latency: fastest preset, tune for low latency, no look-ahead
          av_opt_set(
            ctx,
            c"preset".as_ptr(),
            c"p1".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
          av_opt_set(
            ctx,
            c"tune".as_ptr(),
            c"ll".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
          av_opt_set_int(ctx, c"rc-lookahead".as_ptr(), 0, opt_flag::SEARCH_CHILDREN);
        } else {
          // Quality mode: balanced preset, HQ tune, enable look-ahead and AQ
          av_opt_set(
            ctx,
            c"preset".as_ptr(),
            c"p4".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
          av_opt_set(
            ctx,
            c"tune".as_ptr(),
            c"hq".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
          av_opt_set_int(ctx, c"rc-lookahead".as_ptr(), 20, opt_flag::SEARCH_CHILDREN);
          av_opt_set_int(ctx, c"spatial-aq".as_ptr(), 1, opt_flag::SEARCH_CHILDREN);
        }
      }
      // VAAPI (Linux)
      else if encoder_name.contains("vaapi") {
        // quality: 0-8, lower = better quality
        // Use 2 for quality mode, 6 for realtime
        let quality = if realtime { 6 } else { 2 };
        av_opt_set_int(ctx, c"quality".as_ptr(), quality, opt_flag::SEARCH_CHILDREN);
      }
      // QSV (Intel)
      else if encoder_name.contains("qsv") {
        if realtime {
          // Low latency: fastest preset, no look-ahead
          av_opt_set(
            ctx,
            c"preset".as_ptr(),
            c"veryfast".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
          av_opt_set_int(ctx, c"look_ahead".as_ptr(), 0, opt_flag::SEARCH_CHILDREN);
        } else {
          // Quality mode: balanced preset with look-ahead
          av_opt_set(
            ctx,
            c"preset".as_ptr(),
            c"medium".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
          av_opt_set_int(ctx, c"look_ahead".as_ptr(), 1, opt_flag::SEARCH_CHILDREN);
        }
      }
      // D3D11VA / AMF (AMD on Windows) - limited options but try common ones
      else if encoder_name.contains("amf") {
        if realtime {
          av_opt_set(
            ctx,
            c"quality".as_ptr(),
            c"speed".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
        } else {
          av_opt_set(
            ctx,
            c"quality".as_ptr(),
            c"balanced".as_ptr(),
            opt_flag::SEARCH_CHILDREN,
          );
        }
      }
      // Note: Unknown hardware encoders get no special options (safe default)
    }
  }

  /// Configure the audio encoder with the given settings
  pub fn configure_audio_encoder(&mut self, config: &AudioEncoderConfig) -> CodecResult<()> {
    if self.codec_type != CodecType::Encoder {
      return Err(CodecError::InvalidState("Not an encoder context".into()));
    }

    unsafe {
      let ctx = self.ptr.as_ptr();

      // Audio parameters
      ffctx_set_sample_rate(ctx, config.sample_rate as i32);
      ffctx_set_sample_fmt(ctx, config.sample_format.as_raw());
      ffctx_set_channels(ctx, config.channels as i32);

      // Bitrate
      if config.bitrate > 0 {
        ffctx_set_bit_rate(ctx, config.bitrate as i64);
      }

      // Threading
      if config.thread_count > 0 {
        ffctx_set_thread_count(ctx, config.thread_count as i32);
      }

      // Time base: 1/sample_rate for audio
      ffctx_set_time_base(ctx, 1, config.sample_rate as i32);
    }

    Ok(())
  }

  /// Configure the audio decoder with the given settings
  pub fn configure_audio_decoder(&mut self, config: &AudioDecoderConfig) -> CodecResult<()> {
    if self.codec_type != CodecType::Decoder {
      return Err(CodecError::InvalidState("Not a decoder context".into()));
    }

    unsafe {
      let ctx = self.ptr.as_ptr();

      // Set sample rate and channels if known
      if config.sample_rate > 0 {
        ffctx_set_sample_rate(ctx, config.sample_rate as i32);
      }
      if config.channels > 0 {
        ffctx_set_channels(ctx, config.channels as i32);
      }

      // Threading
      if config.thread_count > 0 {
        ffctx_set_thread_count(ctx, config.thread_count as i32);
      } else {
        ffctx_set_thread_count(ctx, 0);
      }

      // TODO: Set extradata if provided
    }

    Ok(())
  }

  /// Configure the decoder with the given settings
  pub fn configure_decoder(&mut self, config: &DecoderConfig) -> CodecResult<()> {
    if self.codec_type != CodecType::Decoder {
      return Err(CodecError::InvalidState("Not a decoder context".into()));
    }

    unsafe {
      let ctx = self.ptr.as_ptr();

      // Threading (use frame threading for decoders)
      if config.thread_count > 0 {
        ffctx_set_thread_count(ctx, config.thread_count as i32);
      } else {
        // Auto-detect thread count
        ffctx_set_thread_count(ctx, 0);
      }

      // Low-latency mode (for optimizeForLatency in WebCodecs)
      // AV_CODEC_FLAG_LOW_DELAY = (1 << 19) = 524288
      // Forces low delay output to reduce buffering
      if config.low_latency {
        ffi::accessors::ffctx_set_flags(ctx, 1 << 19);
      }

      // TODO: Set extradata if provided
      // This requires allocating memory with av_malloc
    }

    Ok(())
  }

  /// Set hardware device context for hardware-accelerated encoding/decoding
  pub fn set_hw_device(&mut self, hw_device: HwDeviceContext) {
    unsafe {
      ffctx_set_hw_device_ctx(self.ptr.as_ptr(), hw_device.as_ptr());
    }
    self.hw_device = Some(hw_device);
  }

  /// Open the codec (must be called after configuration)
  pub fn open(&mut self) -> CodecResult<()> {
    let ret = unsafe { avcodec_open2(self.ptr.as_ptr(), self.codec, std::ptr::null_mut()) };
    ffi::check_error(ret)?;
    Ok(())
  }

  // ========================================================================
  // Encoding
  // ========================================================================

  /// Send a frame to the encoder
  ///
  /// Returns Ok(true) if frame was accepted, Ok(false) if encoder needs output drained first
  pub fn send_frame(&mut self, frame: Option<&Frame>) -> CodecResult<bool> {
    let frame_ptr = frame.map(|f| f.as_ptr()).unwrap_or(std::ptr::null());
    let ret = unsafe { avcodec_send_frame(self.ptr.as_ptr(), frame_ptr) };

    if ret == AVERROR_EAGAIN {
      return Ok(false);
    }
    ffi::check_error(ret)?;
    Ok(true)
  }

  /// Receive an encoded packet from the encoder
  ///
  /// Returns Ok(Some(packet)) if a packet is available, Ok(None) if more input needed
  pub fn receive_packet(&mut self) -> CodecResult<Option<Packet>> {
    let mut pkt = Packet::new()?;
    let ret = unsafe { avcodec_receive_packet(self.ptr.as_ptr(), pkt.as_mut_ptr()) };

    if ret == AVERROR_EAGAIN || ret == AVERROR_EOF {
      return Ok(None);
    }
    ffi::check_error(ret)?;
    Ok(Some(pkt))
  }

  /// Encode a frame and return all available packets
  pub fn encode(&mut self, frame: Option<&Frame>) -> CodecResult<Vec<Packet>> {
    let mut packets = Vec::new();

    // Send frame
    if !self.send_frame(frame)? {
      // Encoder is full, drain first
      while let Some(pkt) = self.receive_packet()? {
        packets.push(pkt);
      }
      // Retry sending frame
      self.send_frame(frame)?;
    }

    // Receive all available packets
    while let Some(pkt) = self.receive_packet()? {
      packets.push(pkt);
    }

    Ok(packets)
  }

  /// Flush the encoder (call with None frame, then drain all packets)
  pub fn flush_encoder(&mut self) -> CodecResult<Vec<Packet>> {
    self.encode(None)
  }

  // ========================================================================
  // Decoding
  // ========================================================================

  /// Send a packet to the decoder
  ///
  /// Returns Ok(true) if packet was accepted, Ok(false) if decoder needs output drained first
  pub fn send_packet(&mut self, packet: Option<&Packet>) -> CodecResult<bool> {
    let pkt_ptr = packet.map(|p| p.as_ptr()).unwrap_or(std::ptr::null());
    let ret = unsafe { avcodec_send_packet(self.ptr.as_ptr(), pkt_ptr) };

    if ret == AVERROR_EAGAIN {
      return Ok(false);
    }
    ffi::check_error(ret)?;
    Ok(true)
  }

  /// Receive a decoded frame from the decoder
  ///
  /// Returns Ok(Some(frame)) if a frame is available, Ok(None) if more input needed
  pub fn receive_frame(&mut self) -> CodecResult<Option<Frame>> {
    let mut frame = Frame::new()?;
    let ret = unsafe { avcodec_receive_frame(self.ptr.as_ptr(), frame.as_mut_ptr()) };

    if ret == AVERROR_EAGAIN || ret == AVERROR_EOF {
      return Ok(None);
    }
    ffi::check_error(ret)?;
    Ok(Some(frame))
  }

  /// Decode a packet and return all available frames
  pub fn decode(&mut self, packet: Option<&Packet>) -> CodecResult<Vec<Frame>> {
    let mut frames = Vec::new();

    // Send packet
    if !self.send_packet(packet)? {
      // Decoder is full, drain first
      while let Some(frame) = self.receive_frame()? {
        frames.push(frame);
      }
      // Retry sending packet
      self.send_packet(packet)?;
    }

    // Receive all available frames
    while let Some(frame) = self.receive_frame()? {
      frames.push(frame);
    }

    Ok(frames)
  }

  /// Flush the decoder
  pub fn flush_decoder(&mut self) -> CodecResult<Vec<Frame>> {
    self.decode(None)
  }

  // ========================================================================
  // Utility
  // ========================================================================

  /// Flush internal codec buffers
  pub fn flush(&mut self) {
    unsafe { avcodec_flush_buffers(self.ptr.as_ptr()) }
  }

  /// Get raw pointer (for FFmpeg API calls)
  #[inline]
  pub fn as_ptr(&self) -> *const AVCodecContext {
    self.ptr.as_ptr()
  }

  /// Get mutable raw pointer
  #[inline]
  pub fn as_mut_ptr(&mut self) -> *mut AVCodecContext {
    self.ptr.as_ptr()
  }

  /// Get codec type
  #[inline]
  pub fn codec_type(&self) -> CodecType {
    self.codec_type
  }

  /// Get configured width
  pub fn width(&self) -> u32 {
    unsafe { ffctx_get_width(self.as_ptr()) as u32 }
  }

  /// Get configured height
  pub fn height(&self) -> u32 {
    unsafe { ffctx_get_height(self.as_ptr()) as u32 }
  }

  /// Get configured pixel format
  pub fn pixel_format(&self) -> AVPixelFormat {
    let fmt = unsafe { ffctx_get_pix_fmt(self.as_ptr()) };
    match fmt {
      0 => AVPixelFormat::Yuv420p,
      4 => AVPixelFormat::Yuv422p,
      5 => AVPixelFormat::Yuv444p,
      23 => AVPixelFormat::Nv12,
      26 => AVPixelFormat::Rgba,
      28 => AVPixelFormat::Bgra,
      _ => AVPixelFormat::None,
    }
  }

  /// Get configured sample rate (for audio)
  pub fn sample_rate(&self) -> u32 {
    unsafe { ffctx_get_sample_rate(self.as_ptr()) as u32 }
  }

  /// Get codec frame size (samples per frame for audio)
  pub fn frame_size(&self) -> u32 {
    unsafe { ffctx_get_frame_size(self.as_ptr()) as u32 }
  }

  /// Get codec extradata (e.g., SPS/PPS for H.264)
  pub fn extradata(&self) -> Option<&[u8]> {
    unsafe {
      let ptr = ffctx_get_extradata(self.as_ptr());
      let size = ffctx_get_extradata_size(self.as_ptr());
      if ptr.is_null() || size <= 0 {
        None
      } else {
        Some(std::slice::from_raw_parts(ptr, size as usize))
      }
    }
  }
}

impl Drop for CodecContext {
  fn drop(&mut self) {
    unsafe {
      // avcodec_free_context handles both closing the codec and freeing the context.
      // Do NOT call avcodec_close separately - it's deprecated and calling both
      // can cause issues with some codecs (e.g., libaom-av1).
      let mut ptr = self.ptr.as_ptr();
      avcodec_free_context(&mut ptr);
    }
  }
}

// CodecContext is NOT Sync - FFmpeg contexts are not thread-safe
unsafe impl Send for CodecContext {}

impl std::fmt::Debug for CodecContext {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("CodecContext")
      .field("type", &self.codec_type)
      .field("width", &self.width())
      .field("height", &self.height())
      .field("pixel_format", &self.pixel_format())
      .finish()
  }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get hardware encoder name for a codec
fn get_hw_encoder_name(codec_id: AVCodecID, hw_type: AVHWDeviceType) -> Option<&'static str> {
  match (codec_id, hw_type) {
    // H.264
    (AVCodecID::H264, AVHWDeviceType::Videotoolbox) => Some("h264_videotoolbox"),
    (AVCodecID::H264, AVHWDeviceType::Cuda) => Some("h264_nvenc"),
    (AVCodecID::H264, AVHWDeviceType::Vaapi) => Some("h264_vaapi"),
    (AVCodecID::H264, AVHWDeviceType::Qsv) => Some("h264_qsv"),

    // H.265/HEVC
    (AVCodecID::Hevc, AVHWDeviceType::Videotoolbox) => Some("hevc_videotoolbox"),
    (AVCodecID::Hevc, AVHWDeviceType::Cuda) => Some("hevc_nvenc"),
    (AVCodecID::Hevc, AVHWDeviceType::Vaapi) => Some("hevc_vaapi"),
    (AVCodecID::Hevc, AVHWDeviceType::Qsv) => Some("hevc_qsv"),

    // VP9
    (AVCodecID::Vp9, AVHWDeviceType::Vaapi) => Some("vp9_vaapi"),
    (AVCodecID::Vp9, AVHWDeviceType::Qsv) => Some("vp9_qsv"),

    // AV1
    (AVCodecID::Av1, AVHWDeviceType::Cuda) => Some("av1_nvenc"),
    (AVCodecID::Av1, AVHWDeviceType::Vaapi) => Some("av1_vaapi"),
    (AVCodecID::Av1, AVHWDeviceType::Qsv) => Some("av1_qsv"),

    _ => None,
  }
}

/// Get software encoder name for a codec.
/// For AV1, we prefer librav1e (rav1e) because:
/// 1. It's more stable on darwin/aarch64 (Apple Silicon)
/// 2. libaom-av1 has known SIGSEGV issues during cleanup on some platforms
/// 3. SVT-AV1 also has reported segfault issues on macOS
fn get_sw_encoder_name(codec_id: AVCodecID) -> Option<&'static str> {
  match codec_id {
    AVCodecID::H264 => Some("libx264"),
    AVCodecID::Hevc => Some("libx265"),
    AVCodecID::Vp8 => Some("libvpx"),
    AVCodecID::Vp9 => Some("libvpx-vp9"),
    // Prefer librav1e over libaom-av1/libsvtav1 for stability on macOS
    AVCodecID::Av1 => Some("librav1e"),
    _ => None,
  }
}

/// Check if a hardware encoder type requires explicit device context setup
fn hw_encoder_needs_device_context(hw_type: AVHWDeviceType) -> bool {
  match hw_type {
    // VAAPI always needs device context
    AVHWDeviceType::Vaapi => true,
    // CUDA/NVENC needs device context
    AVHWDeviceType::Cuda => true,
    // QSV needs device context
    AVHWDeviceType::Qsv => true,
    // VideoToolbox manages its own device internally
    AVHWDeviceType::Videotoolbox => false,
    // D3D11VA needs device context
    AVHWDeviceType::D3d11va => true,
    // Other types - be conservative and try to set device
    _ => true,
  }
}

/// Get software encoder name for an audio codec
pub fn get_audio_encoder_name(codec_id: AVCodecID) -> Option<&'static str> {
  match codec_id {
    AVCodecID::Aac => Some("aac"), // Native FFmpeg AAC or libfdk_aac if available
    AVCodecID::Opus => Some("libopus"),
    AVCodecID::Mp3 => Some("libmp3lame"),
    AVCodecID::Flac => Some("flac"),
    AVCodecID::Vorbis => Some("libvorbis"),
    AVCodecID::PcmS16le => None, // PCM doesn't need encoder
    AVCodecID::PcmS16be => None,
    AVCodecID::PcmF32le => None,
    AVCodecID::PcmF32be => None,
    AVCodecID::Ac3 => Some("ac3"),
    AVCodecID::Alac => Some("alac"),
    _ => None,
  }
}
