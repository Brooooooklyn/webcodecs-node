//! Demuxer context wrapper for FFmpeg libavformat
//!
//! Provides RAII wrapper around AVFormatContext for demuxing operations.

use super::CodecError;
use super::avio_context::CustomIOContext;
use super::io_buffer::BufferSource;
use crate::ffi::accessors::{
  ffcodecpar_get_channels, ffcodecpar_get_codec_id, ffcodecpar_get_codec_type,
  ffcodecpar_get_extradata, ffcodecpar_get_extradata_size, ffcodecpar_get_format,
  ffcodecpar_get_height, ffcodecpar_get_sample_rate, ffcodecpar_get_width, fffmt_get_duration,
  fffmt_get_nb_streams, fffmt_get_stream, fffmt_set_pb, ffstream_get_codecpar_const,
  ffstream_get_duration, ffstream_get_index, ffstream_get_time_base,
};
use crate::ffi::avformat::{
  AVFormatContext, av_find_best_stream, av_read_frame, av_seek_frame, avformat_close_input,
  avformat_find_stream_info, avformat_free_context, avformat_open_input, media_type, seek_flag,
};
use crate::ffi::{AVCodecID, AVPixelFormat, AVSampleFormat};
use std::ffi::CString;
use std::os::raw::c_int;
use std::ptr::{self, NonNull};

/// Media type for stream identification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
  /// Video stream
  Video,
  /// Audio stream
  Audio,
  /// Subtitle stream
  Subtitle,
  /// Data stream
  Data,
}

impl MediaType {
  /// Convert to FFmpeg media type constant
  fn ffmpeg_type(&self) -> c_int {
    match self {
      MediaType::Video => media_type::VIDEO,
      MediaType::Audio => media_type::AUDIO,
      MediaType::Subtitle => media_type::SUBTITLE,
      MediaType::Data => media_type::DATA,
    }
  }

  /// Convert from FFmpeg media type constant
  fn from_ffmpeg(value: c_int) -> Option<Self> {
    match value {
      x if x == media_type::VIDEO => Some(MediaType::Video),
      x if x == media_type::AUDIO => Some(MediaType::Audio),
      x if x == media_type::SUBTITLE => Some(MediaType::Subtitle),
      x if x == media_type::DATA => Some(MediaType::Data),
      _ => None,
    }
  }
}

/// Information about a stream in the container
#[derive(Debug, Clone)]
pub struct StreamInfo {
  /// Stream index
  pub index: i32,
  /// Media type (Video, Audio, etc.)
  pub media_type: MediaType,
  /// Codec ID
  pub codec_id: AVCodecID,
  /// Video width (if video)
  pub width: Option<u32>,
  /// Video height (if video)
  pub height: Option<u32>,
  /// Video pixel format (if video)
  pub pixel_format: Option<AVPixelFormat>,
  /// Audio sample rate (if audio)
  pub sample_rate: Option<u32>,
  /// Audio channels (if audio)
  pub channels: Option<u32>,
  /// Audio sample format (if audio)
  pub sample_format: Option<AVSampleFormat>,
  /// Stream time base (num, den)
  pub time_base: (i32, i32),
  /// Stream duration in time_base units
  pub duration: Option<i64>,
  /// Codec extradata (avcC, hvcC, etc.)
  pub extradata: Option<Vec<u8>>,
}

/// Demuxer context wrapper
///
/// Provides RAII wrapper around AVFormatContext for demuxing operations.
pub struct DemuxerContext {
  /// Pointer to AVFormatContext
  ptr: NonNull<AVFormatContext>,
  /// Custom I/O context (for buffer input)
  custom_io: Option<CustomIOContext>,
  /// Cached stream information
  streams: Vec<StreamInfo>,
}

impl DemuxerContext {
  /// Open a file for demuxing
  pub fn open_file(path: &str) -> Result<Self, CodecError> {
    let c_path =
      CString::new(path).map_err(|_| CodecError::InvalidConfig("Invalid path".to_string()))?;

    let mut ctx_ptr: *mut AVFormatContext = ptr::null_mut();
    let ret =
      unsafe { avformat_open_input(&mut ctx_ptr, c_path.as_ptr(), ptr::null(), ptr::null_mut()) };

    if ret < 0 || ctx_ptr.is_null() {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    let mut ctx = Self {
      ptr: unsafe { NonNull::new_unchecked(ctx_ptr) },
      custom_io: None,
      streams: Vec::new(),
    };

    // Find stream information
    ctx.find_stream_info()?;

    Ok(ctx)
  }

  /// Open a buffer for demuxing
  ///
  /// This method accepts any type implementing `BufferSource`, enabling
  /// zero-copy buffer loading from `Uint8Array` without intermediate copies.
  pub fn open_buffer(source: impl BufferSource + 'static) -> Result<Self, CodecError> {
    // Create custom I/O context for reading
    let custom_io = CustomIOContext::new_buffer_read(source).map_err(CodecError::InvalidConfig)?;

    // Allocate format context
    let ctx_ptr = unsafe {
      let ptr = crate::ffi::avformat::avformat_alloc_context();
      if ptr.is_null() {
        return Err(CodecError::AllocationFailed("AVFormatContext"));
      }
      ptr
    };

    // Set custom I/O
    unsafe {
      fffmt_set_pb(ctx_ptr, custom_io.as_ptr());
    }

    // Open input
    let mut ctx_ptr_mut = ctx_ptr;
    let ret =
      unsafe { avformat_open_input(&mut ctx_ptr_mut, ptr::null(), ptr::null(), ptr::null_mut()) };

    if ret < 0 {
      // On failure, avformat_open_input frees the context
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    let mut ctx = Self {
      ptr: unsafe { NonNull::new_unchecked(ctx_ptr_mut) },
      custom_io: Some(custom_io),
      streams: Vec::new(),
    };

    // Find stream information
    ctx.find_stream_info()?;

    Ok(ctx)
  }

  /// Find and parse stream information
  fn find_stream_info(&mut self) -> Result<(), CodecError> {
    let ret = unsafe { avformat_find_stream_info(self.ptr.as_ptr(), ptr::null_mut()) };

    if ret < 0 {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    // Parse stream information
    self.parse_streams();

    Ok(())
  }

  /// Parse stream information from format context
  fn parse_streams(&mut self) {
    let nb_streams = unsafe { fffmt_get_nb_streams(self.ptr.as_ptr()) };

    self.streams.clear();
    self.streams.reserve(nb_streams as usize);

    for i in 0..nb_streams {
      let stream = unsafe { fffmt_get_stream(self.ptr.as_ptr(), i) };
      if stream.is_null() {
        continue;
      }

      let codecpar = unsafe { ffstream_get_codecpar_const(stream) };
      if codecpar.is_null() {
        continue;
      }

      // Get codec type
      let codec_type_raw = unsafe { ffcodecpar_get_codec_type(codecpar) };
      let media_type = match MediaType::from_ffmpeg(codec_type_raw) {
        Some(t) => t,
        None => continue, // Skip unknown stream types
      };

      // Get basic info
      let index = unsafe { ffstream_get_index(stream) };
      let codec_id_raw = unsafe { ffcodecpar_get_codec_id(codecpar) };
      let codec_id = AVCodecID::from_raw(codec_id_raw);

      // Get time base
      let mut time_base_num = 0i32;
      let mut time_base_den = 0i32;
      unsafe {
        ffstream_get_time_base(stream, &mut time_base_num, &mut time_base_den);
      }

      // Get duration
      let duration_raw = unsafe { ffstream_get_duration(stream) };
      let duration = if duration_raw > 0 {
        Some(duration_raw)
      } else {
        None
      };

      // Get extradata
      let extradata_ptr = unsafe { ffcodecpar_get_extradata(codecpar) };
      let extradata_size = unsafe { ffcodecpar_get_extradata_size(codecpar) };
      let extradata = if !extradata_ptr.is_null() && extradata_size > 0 {
        Some(unsafe { std::slice::from_raw_parts(extradata_ptr, extradata_size as usize).to_vec() })
      } else {
        None
      };

      // Video-specific info
      let (width, height, pixel_format) = if media_type == MediaType::Video {
        let w = unsafe { ffcodecpar_get_width(codecpar) };
        let h = unsafe { ffcodecpar_get_height(codecpar) };
        let fmt = unsafe { ffcodecpar_get_format(codecpar) };
        (
          Some(w as u32),
          Some(h as u32),
          Some(AVPixelFormat::from_raw(fmt)),
        )
      } else {
        (None, None, None)
      };

      // Audio-specific info
      let (sample_rate, channels, sample_format) = if media_type == MediaType::Audio {
        let sr = unsafe { ffcodecpar_get_sample_rate(codecpar) };
        let ch = unsafe { ffcodecpar_get_channels(codecpar) };
        let fmt = unsafe { ffcodecpar_get_format(codecpar) };
        (
          Some(sr as u32),
          Some(ch as u32),
          Some(AVSampleFormat::from_raw(fmt)),
        )
      } else {
        (None, None, None)
      };

      self.streams.push(StreamInfo {
        index,
        media_type,
        codec_id,
        width,
        height,
        pixel_format,
        sample_rate,
        channels,
        sample_format,
        time_base: (time_base_num, time_base_den),
        duration,
        extradata,
      });
    }
  }

  /// Get all streams
  pub fn streams(&self) -> &[StreamInfo] {
    &self.streams
  }

  /// Find the best stream of a given type
  pub fn find_best_stream(&self, media_type: MediaType) -> Option<&StreamInfo> {
    let stream_index = unsafe {
      av_find_best_stream(
        self.ptr.as_ptr(),
        media_type.ffmpeg_type(),
        -1,
        -1,
        ptr::null_mut(),
        0,
      )
    };

    if stream_index < 0 {
      return None;
    }

    self.streams.iter().find(|s| s.index == stream_index)
  }

  /// Read the next packet from the container
  ///
  /// Returns `Ok(Some((packet, stream_index)))` if a packet was read,
  /// `Ok(None)` on EOF, or `Err` on error.
  pub fn read_packet(&mut self) -> Result<Option<(super::Packet, i32)>, CodecError> {
    let mut packet = super::Packet::new()?;

    let ret = unsafe { av_read_frame(self.ptr.as_ptr(), packet.as_mut_ptr()) };

    if ret == crate::ffi::error::AVERROR_EOF {
      return Ok(None);
    }

    if ret < 0 {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    let stream_index = packet.stream_index();
    Ok(Some((packet, stream_index)))
  }

  /// Seek to a timestamp in the stream
  ///
  /// # Arguments
  /// * `stream_index` - Stream index to seek (-1 for default)
  /// * `timestamp` - Timestamp in stream time base units
  /// * `backward` - If true, seek to keyframe before timestamp
  pub fn seek(
    &mut self,
    stream_index: i32,
    timestamp: i64,
    backward: bool,
  ) -> Result<(), CodecError> {
    let flags = if backward { seek_flag::BACKWARD } else { 0 };

    let ret = unsafe { av_seek_frame(self.ptr.as_ptr(), stream_index, timestamp, flags) };

    if ret < 0 {
      return Err(CodecError::Ffmpeg(crate::ffi::FFmpegError::from_code(ret)));
    }

    Ok(())
  }

  /// Get the container duration in AV_TIME_BASE units (microseconds)
  pub fn duration_us(&self) -> Option<i64> {
    let duration = unsafe { fffmt_get_duration(self.ptr.as_ptr()) };
    if duration > 0 { Some(duration) } else { None }
  }

  /// Get the number of streams
  pub fn num_streams(&self) -> usize {
    self.streams.len()
  }

  /// Get stream info by index
  pub fn get_stream(&self, index: i32) -> Option<&StreamInfo> {
    self.streams.iter().find(|s| s.index == index)
  }

  /// Get video stream info (first video stream)
  pub fn video_stream(&self) -> Option<&StreamInfo> {
    self
      .streams
      .iter()
      .find(|s| s.media_type == MediaType::Video)
  }

  /// Get audio stream info (first audio stream)
  pub fn audio_stream(&self) -> Option<&StreamInfo> {
    self
      .streams
      .iter()
      .find(|s| s.media_type == MediaType::Audio)
  }
}

impl Drop for DemuxerContext {
  fn drop(&mut self) {
    if self.custom_io.is_some() {
      // For custom I/O (buffer-based demuxing):
      // Clear pb first so FFmpeg doesn't try to close our custom AVIO context.
      // Use avformat_free_context instead of avformat_close_input to avoid
      // any I/O operations that could access the (now-null) pb.
      // CustomIOContext will properly free the AVIO buffer when dropped.
      unsafe {
        fffmt_set_pb(self.ptr.as_ptr(), ptr::null_mut());
        avformat_free_context(self.ptr.as_ptr());
      }
    } else {
      // For file-based demuxing:
      // Use avformat_close_input which properly closes the file handle.
      let mut ptr = self.ptr.as_ptr();
      unsafe {
        avformat_close_input(&mut ptr);
      }
    }

    // custom_io is dropped automatically, freeing the AVIO context and buffer
  }
}

// SAFETY: DemuxerContext owns all its resources and can be safely sent between threads
unsafe impl Send for DemuxerContext {}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_media_type_conversion() {
    assert_eq!(MediaType::Video.ffmpeg_type(), media_type::VIDEO);
    assert_eq!(MediaType::Audio.ffmpeg_type(), media_type::AUDIO);

    assert_eq!(
      MediaType::from_ffmpeg(media_type::VIDEO),
      Some(MediaType::Video)
    );
    assert_eq!(
      MediaType::from_ffmpeg(media_type::AUDIO),
      Some(MediaType::Audio)
    );
    assert_eq!(MediaType::from_ffmpeg(-1), None);
  }
}
