//! Custom I/O context wrapper for FFmpeg's AVIO system
//!
//! Provides safe wrappers for custom I/O operations (memory/streaming buffers).

use super::io_buffer::{BufferSource, MemoryBuffer, ReadOnlyBuffer, StreamingBuffer};
use crate::ffi::avformat::{
  AVIOContext, avio_alloc_context, avio_context_free, avio_flush, seek_whence,
};
use crate::ffi::avutil::av_malloc;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::raw::{c_int, c_void};
use std::ptr::NonNull;

/// Default buffer size for AVIO operations (32KB)
const DEFAULT_BUFFER_SIZE: usize = 32 * 1024;

/// I/O mode for the custom context
pub enum IoMode {
  /// Buffer-based output (muxer writes to memory buffer)
  BufferWrite(Box<MemoryBuffer>),
  /// Buffer-based input (demuxer reads from read-only buffer) - supports zero-copy
  BufferRead(Box<ReadOnlyBuffer>),
  /// Streaming output (muxer writes to streaming buffer)
  StreamingWrite(Box<StreamingBuffer>),
}

/// Custom I/O context wrapper
///
/// Wraps FFmpeg's AVIOContext for custom I/O operations.
/// Handles memory management and provides safe callbacks.
pub struct CustomIOContext {
  /// Pointer to the AVIOContext
  ptr: NonNull<AVIOContext>,
  /// FFmpeg I/O buffer (allocated with av_malloc, freed by avio_context_free)
  /// We keep this field for potential debugging, but don't manually free it.
  #[allow(dead_code)]
  buffer: *mut u8,
}

impl CustomIOContext {
  /// Create a new custom I/O context for writing to a memory buffer
  pub fn new_buffer_write() -> Result<Self, String> {
    Self::new_buffer_write_with_capacity(DEFAULT_BUFFER_SIZE)
  }

  /// Create a new custom I/O context for writing to a memory buffer with pre-allocated capacity
  pub fn new_buffer_write_with_capacity(capacity: usize) -> Result<Self, String> {
    let buffer = MemoryBuffer::with_capacity(capacity);
    Self::create_write_context(IoMode::BufferWrite(Box::new(buffer)))
  }

  /// Create a new custom I/O context for reading from a memory buffer
  ///
  /// This method accepts any type implementing `BufferSource`, enabling
  /// zero-copy buffer loading from `Uint8Array` without intermediate copies.
  pub fn new_buffer_read(source: impl BufferSource + 'static) -> Result<Self, String> {
    let buffer = ReadOnlyBuffer::new(source);
    Self::create_read_context(IoMode::BufferRead(Box::new(buffer)))
  }

  /// Create a new custom I/O context for streaming output
  pub fn new_streaming_write(capacity: usize) -> Result<Self, String> {
    let buffer = StreamingBuffer::new(capacity);
    Self::create_write_context(IoMode::StreamingWrite(Box::new(buffer)))
  }

  /// Create a write context with the given mode
  fn create_write_context(mode: IoMode) -> Result<Self, String> {
    let buffer_size = DEFAULT_BUFFER_SIZE;

    // Allocate FFmpeg buffer
    let buffer = unsafe { av_malloc(buffer_size) } as *mut u8;
    if buffer.is_null() {
      return Err("Failed to allocate AVIO buffer".to_string());
    }

    // Determine if we need read callback (for faststart support in buffer mode)
    // Check before boxing since Box::new() moves the value
    let needs_read = matches!(mode, IoMode::BufferWrite(_));

    // Box the mode to get a stable pointer
    let mut boxed_mode = Box::new(mode);
    let opaque = boxed_mode.as_mut() as *mut IoMode as *mut c_void;

    let read_cb: Option<crate::ffi::avformat::ReadPacketFn> = if needs_read {
      Some(read_callback)
    } else {
      None
    };

    // Create the AVIO context
    let ptr = unsafe {
      avio_alloc_context(
        buffer,
        buffer_size as c_int,
        1, // write_flag = 1 for writing
        opaque,
        read_cb,                   // read_packet - needed for faststart in buffer mode
        Some(write_callback),      // write_packet
        Some(seek_callback_write), // seek
      )
    };

    if ptr.is_null() {
      // Free the buffer on failure
      unsafe { crate::ffi::avutil::av_free(buffer as *mut c_void) };
      return Err("Failed to allocate AVIOContext".to_string());
    }

    // Set seekable flag for buffer mode (required for faststart in MP4)
    // BufferWrite mode supports seeking (and reading back), StreamingWrite does not
    if needs_read {
      unsafe { fffio_set_seekable(ptr, AVIO_SEEKABLE_NORMAL) };
    }

    // Leak the boxed mode - will be reclaimed in Drop
    let _ = Box::into_raw(boxed_mode);

    Ok(Self {
      ptr: unsafe { NonNull::new_unchecked(ptr) },
      buffer,
    })
  }

  /// Create a read context with the given mode
  fn create_read_context(mode: IoMode) -> Result<Self, String> {
    let buffer_size = DEFAULT_BUFFER_SIZE;

    // Allocate FFmpeg buffer
    let buffer = unsafe { av_malloc(buffer_size) } as *mut u8;
    if buffer.is_null() {
      return Err("Failed to allocate AVIO buffer".to_string());
    }

    // Box the mode to get a stable pointer
    let mut boxed_mode = Box::new(mode);
    let opaque = boxed_mode.as_mut() as *mut IoMode as *mut c_void;

    // Create the AVIO context
    let ptr = unsafe {
      avio_alloc_context(
        buffer,
        buffer_size as c_int,
        0, // write_flag = 0 for reading
        opaque,
        Some(read_callback),      // read_packet
        None,                     // write_packet - not needed for reading
        Some(seek_callback_read), // seek
      )
    };

    if ptr.is_null() {
      // Free the buffer on failure
      unsafe { crate::ffi::avutil::av_free(buffer as *mut c_void) };
      return Err("Failed to allocate AVIOContext".to_string());
    }

    // Leak the boxed mode - will be reclaimed in Drop
    let _ = Box::into_raw(boxed_mode);

    Ok(Self {
      ptr: unsafe { NonNull::new_unchecked(ptr) },
      buffer,
    })
  }

  /// Get the raw AVIOContext pointer
  pub fn as_ptr(&self) -> *mut AVIOContext {
    self.ptr.as_ptr()
  }

  /// Flush the I/O context
  pub fn flush(&self) {
    unsafe { avio_flush(self.ptr.as_ptr()) };
  }

  /// Take the output buffer data (for buffer write mode)
  ///
  /// Returns the data and clears the buffer.
  /// Returns None if not in buffer write mode.
  pub fn take_buffer_data(&mut self) -> Option<Vec<u8>> {
    self.flush();

    // Get the opaque pointer from the AVIO context
    // Note: We need to access the mode through the opaque pointer
    // This is safe because we own the context and the mode
    unsafe {
      let avio_ptr = self.ptr.as_ptr();
      // The opaque is stored at the beginning of AVIOContext
      // We access it through the callback mechanism
      let opaque = get_avio_opaque(avio_ptr);
      if !opaque.is_null() {
        let mode = &mut *(opaque as *mut IoMode);
        match mode {
          IoMode::BufferWrite(buf) => Some(buf.take_data()),
          _ => None,
        }
      } else {
        None
      }
    }
  }

  /// Get a handle to the streaming buffer (for streaming write mode)
  ///
  /// Returns None if not in streaming write mode.
  pub fn get_streaming_handle(&self) -> Option<super::io_buffer::StreamingBufferHandle> {
    unsafe {
      let opaque = get_avio_opaque(self.ptr.as_ptr());
      if !opaque.is_null() {
        let mode = &*(opaque as *const IoMode);
        match mode {
          IoMode::StreamingWrite(buf) => Some(buf.clone_handle()),
          _ => None,
        }
      } else {
        None
      }
    }
  }

  /// Finish streaming (for streaming write mode)
  ///
  /// Signals that no more data will be written.
  pub fn finish_streaming(&self) {
    self.flush();
    unsafe {
      let opaque = get_avio_opaque(self.ptr.as_ptr());
      if !opaque.is_null() {
        let mode = &*(opaque as *const IoMode);
        if let IoMode::StreamingWrite(buf) = mode {
          buf.finish();
        }
      }
    }
  }

  /// Get the current size of the buffer (for buffer modes)
  pub fn buffer_size(&self) -> Option<usize> {
    unsafe {
      let opaque = get_avio_opaque(self.ptr.as_ptr());
      if !opaque.is_null() {
        let mode = &*(opaque as *const IoMode);
        match mode {
          IoMode::BufferWrite(buf) => Some(buf.len()),
          IoMode::BufferRead(buf) => Some(buf.len()),
          IoMode::StreamingWrite(_) => None,
        }
      } else {
        None
      }
    }
  }
}

impl Drop for CustomIOContext {
  fn drop(&mut self) {
    unsafe {
      // Get the opaque pointer before freeing the context
      let opaque = get_avio_opaque(self.ptr.as_ptr());

      // Free the AVIO context - this DOES free the buffer according to FFmpeg docs:
      // "The buffer is freed automatically on avio_context_free()"
      // So we must NOT call av_free(self.buffer) afterwards to avoid double-free.
      let mut ptr = self.ptr.as_ptr();
      avio_context_free(&mut ptr);

      // Note: self.buffer is now invalid - avio_context_free freed it.
      // Do NOT call av_free(self.buffer) here!

      // Reclaim and drop the boxed mode
      if !opaque.is_null() {
        let _ = Box::from_raw(opaque as *mut IoMode);
      }
    }
  }
}

// SAFETY: The CustomIOContext owns all its resources and can be safely sent
// between threads. The FFmpeg context is only accessed through our safe API.
unsafe impl Send for CustomIOContext {}

// ============================================================================
// FFmpeg Callbacks
// ============================================================================

/// Get the opaque pointer from an AVIOContext
///
/// # Safety
/// This accesses the internal structure of AVIOContext which is opaque.
/// We rely on the fact that 'opaque' is stored at a known offset.
#[inline]
unsafe fn get_avio_opaque(ctx: *mut AVIOContext) -> *mut c_void {
  // The opaque pointer is stored in the AVIOContext structure
  // We need to access it through the C accessor or by knowing the offset
  // For now, we'll use a simple approach: store it separately
  // This is implemented via a C accessor function
  unsafe { fffio_get_opaque(ctx) }
}

// Declare the C accessor functions
unsafe extern "C" {
  fn fffio_get_opaque(ctx: *mut AVIOContext) -> *mut c_void;
  fn fffio_set_seekable(ctx: *mut AVIOContext, seekable: c_int);
}

/// AVIO_SEEKABLE_NORMAL - indicates normal seekable I/O
const AVIO_SEEKABLE_NORMAL: c_int = 1;

/// Write callback for FFmpeg custom I/O
unsafe extern "C" fn write_callback(opaque: *mut c_void, buf: *const u8, buf_size: c_int) -> c_int {
  if opaque.is_null() || buf.is_null() || buf_size <= 0 {
    return -1;
  }

  // SAFETY: opaque was checked for null above, and it was set by us to point to a valid IoMode
  let mode = unsafe { &mut *(opaque as *mut IoMode) };
  // SAFETY: buf was checked for null above, buf_size is valid
  let data = unsafe { std::slice::from_raw_parts(buf, buf_size as usize) };

  let result = match mode {
    IoMode::BufferWrite(buffer) => buffer.write(data),
    IoMode::StreamingWrite(buffer) => buffer.write_blocking(data),
    IoMode::BufferRead(_) => return -1, // Can't write to read buffer
  };

  match result {
    Ok(n) => n as c_int,
    Err(_) => -1,
  }
}

/// Read callback for FFmpeg custom I/O
///
/// This callback supports reading from:
/// - BufferRead: normal read mode for demuxing
/// - BufferWrite: allows reading back written data for faststart support
unsafe extern "C" fn read_callback(opaque: *mut c_void, buf: *mut u8, buf_size: c_int) -> c_int {
  if opaque.is_null() || buf.is_null() || buf_size <= 0 {
    return -1;
  }

  // SAFETY: opaque was checked for null above, and it was set by us to point to a valid IoMode
  let mode = unsafe { &mut *(opaque as *mut IoMode) };
  // SAFETY: buf was checked for null above, buf_size is valid
  let data = unsafe { std::slice::from_raw_parts_mut(buf, buf_size as usize) };

  let result = match mode {
    IoMode::BufferRead(buffer) => buffer.read(data),
    // BufferWrite also supports reading for faststart (FFmpeg needs to read back written data)
    IoMode::BufferWrite(buffer) => buffer.read(data),
    IoMode::StreamingWrite(_) => return -1, // Streaming doesn't support read-back
  };

  match result {
    Ok(0) => crate::ffi::error::AVERROR_EOF, // EOF
    Ok(n) => n as c_int,
    Err(_) => -1,
  }
}

/// Seek callback for write mode
unsafe extern "C" fn seek_callback_write(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
  if opaque.is_null() {
    return -1;
  }

  // Handle AVSEEK_SIZE - return total size
  if whence == seek_whence::AVSEEK_SIZE {
    // SAFETY: opaque was checked for null above
    let mode = unsafe { &*(opaque as *const IoMode) };
    return match mode {
      IoMode::BufferWrite(buffer) => buffer.len() as i64,
      IoMode::StreamingWrite(_) => -1, // Streaming doesn't support size query
      IoMode::BufferRead(_) => -1,
    };
  }

  // SAFETY: opaque was checked for null above
  let mode = unsafe { &mut *(opaque as *mut IoMode) };

  let seek_from = match whence {
    0 => SeekFrom::Start(offset as u64), // SEEK_SET
    1 => SeekFrom::Current(offset),      // SEEK_CUR
    2 => SeekFrom::End(offset),          // SEEK_END
    _ => return -1,
  };

  match mode {
    IoMode::BufferWrite(buffer) => match buffer.seek(seek_from) {
      Ok(pos) => pos as i64,
      Err(_) => -1,
    },
    IoMode::StreamingWrite(_) => -1, // Streaming doesn't support seeking
    IoMode::BufferRead(_) => -1,
  }
}

/// Seek callback for read mode
unsafe extern "C" fn seek_callback_read(opaque: *mut c_void, offset: i64, whence: c_int) -> i64 {
  if opaque.is_null() {
    return -1;
  }

  // Handle AVSEEK_SIZE - return total size
  if whence == seek_whence::AVSEEK_SIZE {
    // SAFETY: opaque was checked for null above
    let mode = unsafe { &*(opaque as *const IoMode) };
    return match mode {
      IoMode::BufferRead(buffer) => buffer.len() as i64,
      _ => -1,
    };
  }

  // SAFETY: opaque was checked for null above
  let mode = unsafe { &mut *(opaque as *mut IoMode) };

  let seek_from = match whence {
    0 => SeekFrom::Start(offset as u64), // SEEK_SET
    1 => SeekFrom::Current(offset),      // SEEK_CUR
    2 => SeekFrom::End(offset),          // SEEK_END
    _ => return -1,
  };

  match mode {
    IoMode::BufferRead(buffer) => match buffer.seek(seek_from) {
      Ok(pos) => pos as i64,
      Err(_) => -1,
    },
    _ => -1,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_buffer_write_creation() {
    let ctx = CustomIOContext::new_buffer_write();
    assert!(ctx.is_ok());
  }

  #[test]
  fn test_buffer_read_creation() {
    let data = vec![1, 2, 3, 4, 5];
    let ctx = CustomIOContext::new_buffer_read(data);
    assert!(ctx.is_ok());
  }
}
