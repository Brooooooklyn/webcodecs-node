//! Muxer context wrapper for FFmpeg libavformat
//!
//! Provides RAII wrapper around AVFormatContext for muxing operations.

use super::CodecError;
use super::avio_context::CustomIOContext;
use super::io_buffer::StreamingBufferHandle;
use crate::ffi::accessors::{
  ffcodecpar_set_bit_rate, ffcodecpar_set_channels, ffcodecpar_set_codec_id,
  ffcodecpar_set_codec_type, ffcodecpar_set_extradata, ffcodecpar_set_format,
  ffcodecpar_set_frame_size, ffcodecpar_set_height, ffcodecpar_set_sample_rate,
  ffcodecpar_set_width, fffmt_get_oformat_flags, fffmt_set_pb, ffstream_get_codecpar,
  ffstream_get_index, ffstream_set_time_base,
};
use crate::ffi::avformat::{
  AVFormatContext, av_interleaved_write_frame, av_write_trailer, avfmt_flag,
  avformat_alloc_output_context2, avformat_free_context, avformat_new_stream,
  avformat_write_header, media_type,
};
use crate::ffi::{AVCodecID, AVPixelFormat, AVRational, AVSampleFormat};
use std::ffi::CString;
use std::os::raw::c_int;
use std::ptr::{self, NonNull};

/// Container format for muxing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
  /// MP4 container (MPEG-4 Part 14)
  Mp4,
  /// WebM container (Matroska subset for web)
  WebM,
  /// Matroska container
  Mkv,
}

impl ContainerFormat {
  /// Get FFmpeg format short name
  pub fn short_name(&self) -> &'static str {
    match self {
      ContainerFormat::Mp4 => "mp4",
      ContainerFormat::WebM => "webm",
      ContainerFormat::Mkv => "matroska",
    }
  }

  /// Get file extension
  pub fn extension(&self) -> &'static str {
    match self {
      ContainerFormat::Mp4 => "mp4",
      ContainerFormat::WebM => "webm",
      ContainerFormat::Mkv => "mkv",
    }
  }
}

/// Output destination for muxer
pub enum MuxerOutput {
  /// Write to memory buffer
  Buffer,
  /// Write to streaming buffer with specified capacity
  Streaming(usize),
}

/// Video stream configuration
#[derive(Debug, Clone)]
pub struct VideoStreamConfig {
  /// Codec ID (H.264, H.265, VP8, VP9, AV1)
  pub codec_id: AVCodecID,
  /// Video width in pixels
  pub width: u32,
  /// Video height in pixels
  pub height: u32,
  /// Pixel format
  pub pixel_format: AVPixelFormat,
  /// Time base for timestamps (typically 1/1000000 for microseconds)
  pub time_base: AVRational,
  /// Bitrate in bits per second (optional)
  pub bitrate: Option<u64>,
  /// Codec extradata (avcC, hvcC, av1C, etc.)
  pub extradata: Option<Vec<u8>>,
}

/// Audio stream configuration
#[derive(Debug, Clone)]
pub struct AudioStreamConfig {
  /// Codec ID (AAC, Opus, etc.)
  pub codec_id: AVCodecID,
  /// Sample rate in Hz
  pub sample_rate: u32,
  /// Number of channels
  pub channels: u32,
  /// Sample format
  pub sample_format: AVSampleFormat,
  /// Time base for timestamps (typically 1/sample_rate)
  pub time_base: AVRational,
  /// Bitrate in bits per second (optional)
  pub bitrate: Option<u64>,
  /// Frame size (samples per frame, for codecs like AAC)
  pub frame_size: Option<u32>,
  /// Codec extradata
  pub extradata: Option<Vec<u8>>,
}

/// Muxer options
#[derive(Debug, Clone, Default)]
pub struct MuxerOptions {
  /// Move moov atom to beginning for MP4 (faststart)
  pub fast_start: bool,
  /// Use fragmented MP4 for streaming
  pub fragmented: bool,
  /// Enable live streaming mode for WebM/MKV
  /// When enabled, clusters are output as soon as complete (cluster-at-a-time)
  pub live: bool,
}

/// Muxer context wrapper
///
/// Provides RAII wrapper around AVFormatContext for muxing operations.
pub struct MuxerContext {
  /// Pointer to AVFormatContext
  ptr: NonNull<AVFormatContext>,
  /// Custom I/O context (for buffer/streaming output)
  io_ctx: Option<CustomIOContext>,
  /// Video stream index
  video_stream_index: Option<i32>,
  /// Audio stream index
  audio_stream_index: Option<i32>,
  /// Whether header has been written
  header_written: bool,
  /// Whether trailer has been written (finalized)
  finalized: bool,
  /// Container format
  format: ContainerFormat,
}

impl MuxerContext {
  /// Create a new muxer context
  ///
  /// # Arguments
  /// * `format` - Container format (MP4, WebM, MKV)
  /// * `output` - Output destination (Buffer or Streaming)
  pub fn new(format: ContainerFormat, output: MuxerOutput) -> Result<Self, CodecError> {
    let format_name = CString::new(format.short_name()).unwrap();

    // Allocate output format context
    let mut ctx_ptr: *mut AVFormatContext = ptr::null_mut();
    let ret = unsafe {
      avformat_alloc_output_context2(&mut ctx_ptr, ptr::null(), format_name.as_ptr(), ptr::null())
    };

    if ret < 0 || ctx_ptr.is_null() {
      return Err(CodecError::AllocationFailed("AVFormatContext"));
    }

    // Create custom I/O context based on output mode
    // Must be done before wrapping ctx_ptr in NonNull to ensure proper cleanup on failure
    let io_ctx = match output {
      MuxerOutput::Buffer => match CustomIOContext::new_buffer_write() {
        Ok(ctx) => Some(ctx),
        Err(e) => {
          // Free the format context before returning error
          unsafe { avformat_free_context(ctx_ptr) };
          return Err(CodecError::InvalidConfig(e));
        }
      },
      MuxerOutput::Streaming(capacity) => match CustomIOContext::new_streaming_write(capacity) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
          // Free the format context before returning error
          unsafe { avformat_free_context(ctx_ptr) };
          return Err(CodecError::InvalidConfig(e));
        }
      },
    };

    let ptr = unsafe { NonNull::new_unchecked(ctx_ptr) };

    // Set the custom I/O context
    if let Some(ref io) = io_ctx {
      unsafe {
        fffmt_set_pb(ctx_ptr, io.as_ptr());
      }
    }

    Ok(Self {
      ptr,
      io_ctx,
      video_stream_index: None,
      audio_stream_index: None,
      header_written: false,
      finalized: false,
      format,
    })
  }

  /// Add a video stream to the muxer
  ///
  /// Must be called before `write_header`.
  pub fn add_video_stream(&mut self, config: &VideoStreamConfig) -> Result<i32, CodecError> {
    if self.header_written {
      return Err(CodecError::InvalidState(
        "Cannot add stream after header is written".to_string(),
      ));
    }

    // Validate codec for format
    self.validate_video_codec(config.codec_id)?;

    // Create new stream
    let stream = unsafe { avformat_new_stream(self.ptr.as_ptr(), ptr::null()) };
    if stream.is_null() {
      return Err(CodecError::AllocationFailed("AVStream"));
    }

    // Configure codec parameters
    let codecpar = unsafe { ffstream_get_codecpar(stream) };
    if codecpar.is_null() {
      return Err(CodecError::AllocationFailed("AVCodecParameters"));
    }

    unsafe {
      // Set codec type and ID
      ffcodecpar_set_codec_type(codecpar, media_type::VIDEO);
      ffcodecpar_set_codec_id(codecpar, config.codec_id as c_int);

      // Set dimensions
      ffcodecpar_set_width(codecpar, config.width as c_int);
      ffcodecpar_set_height(codecpar, config.height as c_int);

      // Set pixel format
      ffcodecpar_set_format(codecpar, config.pixel_format as c_int);

      // Set bitrate if provided
      if let Some(bitrate) = config.bitrate {
        ffcodecpar_set_bit_rate(codecpar, bitrate as i64);
      }

      // Set extradata if provided
      if let Some(ref extradata) = config.extradata {
        let ret = ffcodecpar_set_extradata(codecpar, extradata.as_ptr(), extradata.len() as c_int);
        if ret < 0 {
          return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
        }
      }

      // Set time base on stream
      ffstream_set_time_base(stream, config.time_base.num, config.time_base.den);
    }

    // Get stream index
    let index = unsafe { ffstream_get_index(stream) };
    self.video_stream_index = Some(index);

    Ok(index)
  }

  /// Add an audio stream to the muxer
  ///
  /// Must be called before `write_header`.
  pub fn add_audio_stream(&mut self, config: &AudioStreamConfig) -> Result<i32, CodecError> {
    if self.header_written {
      return Err(CodecError::InvalidState(
        "Cannot add stream after header is written".to_string(),
      ));
    }

    // Validate codec for format
    self.validate_audio_codec(config.codec_id)?;

    // Create new stream
    let stream = unsafe { avformat_new_stream(self.ptr.as_ptr(), ptr::null()) };
    if stream.is_null() {
      return Err(CodecError::AllocationFailed("AVStream"));
    }

    // Configure codec parameters
    let codecpar = unsafe { ffstream_get_codecpar(stream) };
    if codecpar.is_null() {
      return Err(CodecError::AllocationFailed("AVCodecParameters"));
    }

    unsafe {
      // Set codec type and ID
      ffcodecpar_set_codec_type(codecpar, media_type::AUDIO);
      ffcodecpar_set_codec_id(codecpar, config.codec_id as c_int);

      // Set audio parameters
      ffcodecpar_set_sample_rate(codecpar, config.sample_rate as c_int);
      ffcodecpar_set_channels(codecpar, config.channels as c_int);
      ffcodecpar_set_format(codecpar, config.sample_format as c_int);

      // Set bitrate if provided
      if let Some(bitrate) = config.bitrate {
        ffcodecpar_set_bit_rate(codecpar, bitrate as i64);
      }

      // Set frame size if provided
      if let Some(frame_size) = config.frame_size {
        ffcodecpar_set_frame_size(codecpar, frame_size as c_int);
      }

      // Set extradata if provided
      if let Some(ref extradata) = config.extradata {
        let ret = ffcodecpar_set_extradata(codecpar, extradata.as_ptr(), extradata.len() as c_int);
        if ret < 0 {
          return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
        }
      }

      // Set time base on stream
      ffstream_set_time_base(stream, config.time_base.num, config.time_base.den);
    }

    // Get stream index
    let index = unsafe { ffstream_get_index(stream) };
    self.audio_stream_index = Some(index);

    Ok(index)
  }

  /// Write the container header
  ///
  /// Must be called after adding streams and before writing packets.
  pub fn write_header(&mut self, options: Option<&MuxerOptions>) -> Result<(), CodecError> {
    if self.header_written {
      return Err(CodecError::InvalidState(
        "Header already written".to_string(),
      ));
    }

    if self.video_stream_index.is_none() && self.audio_stream_index.is_none() {
      return Err(CodecError::InvalidConfig("No streams added".to_string()));
    }

    // Apply options if provided
    let mut dict_ptr: *mut crate::ffi::types::AVDictionary = ptr::null_mut();

    if let Some(opts) = options {
      if self.format == ContainerFormat::Mp4 {
        let movflags = if opts.fragmented {
          "frag_keyframe+empty_moov+default_base_moof"
        } else if opts.fast_start {
          "faststart"
        } else {
          ""
        };

        if !movflags.is_empty() {
          let key = CString::new("movflags").unwrap();
          let value = CString::new(movflags).unwrap();
          unsafe {
            crate::ffi::avutil::av_dict_set(&mut dict_ptr, key.as_ptr(), value.as_ptr(), 0);
          }
        }
      } else if (self.format == ContainerFormat::WebM || self.format == ContainerFormat::Mkv)
        && opts.live
      {
        // For WebM/Matroska, enable live mode for cluster-at-a-time output
        let key = CString::new("live").unwrap();
        let value = CString::new("1").unwrap();
        unsafe {
          crate::ffi::avutil::av_dict_set(&mut dict_ptr, key.as_ptr(), value.as_ptr(), 0);
        }
      }
    }

    // Write header
    let ret = unsafe { avformat_write_header(self.ptr.as_ptr(), &mut dict_ptr) };

    // Free dictionary if allocated
    if !dict_ptr.is_null() {
      unsafe {
        crate::ffi::avutil::av_dict_free(&mut dict_ptr);
      }
    }

    if ret < 0 {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    self.header_written = true;
    Ok(())
  }

  /// Write a packet to the muxer
  ///
  /// The packet's stream_index must match a stream added to this muxer.
  pub fn write_packet(&mut self, packet: &mut super::Packet) -> Result<(), CodecError> {
    if !self.header_written {
      return Err(CodecError::InvalidState("Header not written".to_string()));
    }

    if self.finalized {
      return Err(CodecError::InvalidState(
        "Muxer already finalized".to_string(),
      ));
    }

    let ret = unsafe { av_interleaved_write_frame(self.ptr.as_ptr(), packet.as_mut_ptr()) };

    if ret < 0 {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    Ok(())
  }

  /// Flush any buffered packets
  pub fn flush(&mut self) -> Result<(), CodecError> {
    if !self.header_written || self.finalized {
      return Ok(());
    }

    // Flush interleaver by passing NULL packet
    let ret = unsafe { av_interleaved_write_frame(self.ptr.as_ptr(), ptr::null_mut()) };

    if ret < 0 {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    Ok(())
  }

  /// Finalize the muxer (write trailer)
  ///
  /// Must be called after all packets have been written.
  pub fn finalize(&mut self) -> Result<(), CodecError> {
    if !self.header_written {
      return Err(CodecError::InvalidState("Header not written".to_string()));
    }

    if self.finalized {
      return Ok(());
    }

    let ret = unsafe { av_write_trailer(self.ptr.as_ptr()) };

    if ret < 0 {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    self.finalized = true;

    // Flush the I/O context
    if let Some(ref io) = self.io_ctx {
      io.flush();
    }

    Ok(())
  }

  /// Take the output buffer data (for buffer mode)
  ///
  /// Returns the muxed data and clears the buffer.
  /// Returns None if not in buffer mode or not finalized.
  pub fn take_buffer(&mut self) -> Option<Vec<u8>> {
    if !self.finalized {
      return None;
    }

    self.io_ctx.as_mut().and_then(|io| io.take_buffer_data())
  }

  /// Get a handle to the streaming buffer (for streaming mode)
  ///
  /// Returns None if not in streaming mode.
  pub fn get_streaming_handle(&self) -> Option<StreamingBufferHandle> {
    self
      .io_ctx
      .as_ref()
      .and_then(|io| io.get_streaming_handle())
  }

  /// Finish streaming (for streaming mode)
  ///
  /// Signals that no more data will be written.
  pub fn finish_streaming(&self) {
    if let Some(ref io) = self.io_ctx {
      io.finish_streaming();
    }
  }

  /// Get video stream index
  pub fn video_stream_index(&self) -> Option<i32> {
    self.video_stream_index
  }

  /// Get audio stream index
  pub fn audio_stream_index(&self) -> Option<i32> {
    self.audio_stream_index
  }

  /// Check if header has been written
  pub fn is_header_written(&self) -> bool {
    self.header_written
  }

  /// Check if muxer is finalized
  pub fn is_finalized(&self) -> bool {
    self.finalized
  }

  /// Check if format needs global header
  pub fn needs_global_header(&self) -> bool {
    let flags = unsafe { fffmt_get_oformat_flags(self.ptr.as_ptr()) };
    (flags & avfmt_flag::GLOBALHEADER) != 0
  }

  /// Update video stream extradata dynamically
  ///
  /// This can be used to update codec-specific parameters (avcC, hvcC, av1C)
  /// when they become available from encoder metadata.
  pub fn update_video_extradata(&mut self, extradata: &[u8]) -> Result<(), CodecError> {
    let video_idx = self
      .video_stream_index
      .ok_or_else(|| CodecError::InvalidState("No video stream to update".to_string()))?;

    unsafe {
      let stream = crate::ffi::accessors::fffmt_get_stream(self.ptr.as_ptr(), video_idx as u32);
      if stream.is_null() {
        return Err(CodecError::InvalidState(
          "Video stream not found".to_string(),
        ));
      }

      let codecpar = ffstream_get_codecpar(stream);
      if codecpar.is_null() {
        return Err(CodecError::AllocationFailed("AVCodecParameters"));
      }

      let ret = ffcodecpar_set_extradata(codecpar, extradata.as_ptr(), extradata.len() as c_int);
      if ret < 0 {
        return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
      }
    }

    Ok(())
  }

  /// Update audio stream extradata dynamically
  ///
  /// This can be used to update codec-specific parameters (esds, OpusHead)
  /// when they become available from encoder metadata.
  pub fn update_audio_extradata(&mut self, extradata: &[u8]) -> Result<(), CodecError> {
    let audio_idx = self
      .audio_stream_index
      .ok_or_else(|| CodecError::InvalidState("No audio stream to update".to_string()))?;

    unsafe {
      let stream = crate::ffi::accessors::fffmt_get_stream(self.ptr.as_ptr(), audio_idx as u32);
      if stream.is_null() {
        return Err(CodecError::InvalidState(
          "Audio stream not found".to_string(),
        ));
      }

      let codecpar = ffstream_get_codecpar(stream);
      if codecpar.is_null() {
        return Err(CodecError::AllocationFailed("AVCodecParameters"));
      }

      let ret = ffcodecpar_set_extradata(codecpar, extradata.as_ptr(), extradata.len() as c_int);
      if ret < 0 {
        return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
      }
    }

    Ok(())
  }

  /// Validate video codec for the container format
  fn validate_video_codec(&self, codec_id: AVCodecID) -> Result<(), CodecError> {
    let valid = match self.format {
      ContainerFormat::Mp4 => {
        matches!(codec_id, AVCodecID::H264 | AVCodecID::Hevc | AVCodecID::Av1)
      }
      ContainerFormat::WebM => matches!(codec_id, AVCodecID::Vp8 | AVCodecID::Vp9 | AVCodecID::Av1),
      ContainerFormat::Mkv => true, // MKV accepts most codecs
    };

    if valid {
      Ok(())
    } else {
      Err(CodecError::InvalidConfig(format!(
        "Video codec {:?} is not supported in {:?} container",
        codec_id, self.format
      )))
    }
  }

  /// Validate audio codec for the container format
  fn validate_audio_codec(&self, codec_id: AVCodecID) -> Result<(), CodecError> {
    let valid = match self.format {
      ContainerFormat::Mp4 => matches!(
        codec_id,
        AVCodecID::Aac | AVCodecID::Mp3 | AVCodecID::Flac | AVCodecID::Opus
      ),
      ContainerFormat::WebM => matches!(codec_id, AVCodecID::Opus | AVCodecID::Vorbis),
      ContainerFormat::Mkv => true, // MKV accepts most codecs
    };

    if valid {
      Ok(())
    } else {
      Err(CodecError::InvalidConfig(format!(
        "Audio codec {:?} is not supported in {:?} container",
        codec_id, self.format
      )))
    }
  }
}

impl Drop for MuxerContext {
  fn drop(&mut self) {
    // Write trailer if not already done
    if self.header_written && !self.finalized {
      let _ = self.finalize();
    }

    // Clear pb before freeing context to avoid double-free
    // (CustomIOContext owns the AVIO buffer)
    unsafe {
      fffmt_set_pb(self.ptr.as_ptr(), ptr::null_mut());
    }

    // Free format context
    unsafe {
      avformat_free_context(self.ptr.as_ptr());
    }

    // io_ctx is dropped automatically
  }
}

// SAFETY: MuxerContext owns all its resources and can be safely sent between threads
unsafe impl Send for MuxerContext {}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_container_format_names() {
    assert_eq!(ContainerFormat::Mp4.short_name(), "mp4");
    assert_eq!(ContainerFormat::WebM.short_name(), "webm");
    assert_eq!(ContainerFormat::Mkv.short_name(), "matroska");
  }

  #[test]
  fn test_muxer_creation() {
    let muxer = MuxerContext::new(ContainerFormat::Mp4, MuxerOutput::Buffer);
    assert!(muxer.is_ok());
  }
}
