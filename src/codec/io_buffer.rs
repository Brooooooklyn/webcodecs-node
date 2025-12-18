//! I/O buffer implementations for muxing and demuxing
//!
//! Provides memory and streaming buffers for FFmpeg's custom I/O system.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Condvar, Mutex};

// ============================================================================
// Buffer Source Trait (for zero-copy demuxing)
// ============================================================================

/// Object-safe trait for read-only buffer access.
///
/// This trait enables zero-copy buffer loading by allowing different buffer
/// types (Vec<u8>, Uint8Array, etc.) to be used without copying data.
pub trait BufferSource: Send + Sync {
  /// Get pointer and length of the buffer data.
  ///
  /// # Safety
  /// The returned pointer must remain valid for the lifetime of the BufferSource.
  fn buffer_data(&self) -> (*const u8, usize);
}

impl BufferSource for Vec<u8> {
  fn buffer_data(&self) -> (*const u8, usize) {
    (self.as_ptr(), self.len())
  }
}

impl BufferSource for Box<[u8]> {
  fn buffer_data(&self) -> (*const u8, usize) {
    (self.as_ptr(), self.len())
  }
}

// ============================================================================
// Read-Only Buffer (for demuxing with zero-copy support)
// ============================================================================

/// Read-only buffer for demuxing operations.
///
/// This buffer wraps any `BufferSource` implementation, enabling zero-copy
/// buffer loading from JavaScript `Uint8Array` without intermediate copies.
pub struct ReadOnlyBuffer {
  source: Box<dyn BufferSource>,
  position: usize,
}

impl ReadOnlyBuffer {
  /// Create a new read-only buffer from any BufferSource.
  pub fn new(source: impl BufferSource + 'static) -> Self {
    Self {
      source: Box::new(source),
      position: 0,
    }
  }

  /// Get the buffer data as a slice.
  #[inline]
  pub fn as_slice(&self) -> &[u8] {
    let (ptr, len) = self.source.buffer_data();
    if ptr.is_null() || len == 0 {
      &[]
    } else {
      // SAFETY: BufferSource guarantees the pointer is valid for its lifetime
      unsafe { std::slice::from_raw_parts(ptr, len) }
    }
  }

  /// Get the total length of the buffer.
  #[inline]
  pub fn len(&self) -> usize {
    self.source.buffer_data().1
  }

  /// Check if the buffer is empty.
  #[inline]
  pub fn is_empty(&self) -> bool {
    self.len() == 0
  }

  /// Get the current read position.
  #[inline]
  pub fn position(&self) -> usize {
    self.position
  }

  /// Get remaining bytes from current position.
  #[inline]
  pub fn remaining(&self) -> usize {
    self.len().saturating_sub(self.position)
  }
}

impl Read for ReadOnlyBuffer {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    let data = self.as_slice();
    if self.position >= data.len() {
      return Ok(0); // EOF
    }

    let available = data.len() - self.position;
    let to_read = available.min(buf.len());

    buf[..to_read].copy_from_slice(&data[self.position..self.position + to_read]);
    self.position += to_read;

    Ok(to_read)
  }
}

impl Seek for ReadOnlyBuffer {
  fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
    let len = self.len();
    let new_pos = match pos {
      SeekFrom::Start(offset) => offset as i64,
      SeekFrom::End(offset) => len as i64 + offset,
      SeekFrom::Current(offset) => self.position as i64 + offset,
    };

    if new_pos < 0 {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "Attempted to seek before start of buffer",
      ));
    }

    // Allow seeking past end - will return EOF on next read
    self.position = new_pos as usize;
    Ok(self.position as u64)
  }
}

// Implement Debug manually since Box<dyn BufferSource> doesn't implement Debug
impl std::fmt::Debug for ReadOnlyBuffer {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ReadOnlyBuffer")
      .field("len", &self.len())
      .field("position", &self.position)
      .finish()
  }
}

/// Growable memory buffer for buffer-based muxing/demuxing
///
/// This buffer supports:
/// - Writing encoded data during muxing
/// - Reading data during demuxing
/// - Seeking for container format requirements (e.g., MP4 moov atom)
#[derive(Debug)]
pub struct MemoryBuffer {
  data: Vec<u8>,
  position: usize,
  /// Maximum allowed size (0 = unlimited)
  max_size: usize,
}

impl MemoryBuffer {
  /// Create a new empty memory buffer
  pub fn new() -> Self {
    Self {
      data: Vec::new(),
      position: 0,
      max_size: 0,
    }
  }

  /// Create a memory buffer with pre-allocated capacity
  pub fn with_capacity(capacity: usize) -> Self {
    Self {
      data: Vec::with_capacity(capacity),
      position: 0,
      max_size: 0,
    }
  }

  /// Create a memory buffer from existing data (for demuxing)
  pub fn from_data(data: Vec<u8>) -> Self {
    Self {
      data,
      position: 0,
      max_size: 0,
    }
  }

  /// Set maximum allowed size (0 = unlimited)
  pub fn set_max_size(&mut self, max_size: usize) {
    self.max_size = max_size;
  }

  /// Get current position
  pub fn position(&self) -> usize {
    self.position
  }

  /// Get total size of buffer
  pub fn len(&self) -> usize {
    self.data.len()
  }

  /// Check if buffer is empty
  pub fn is_empty(&self) -> bool {
    self.data.is_empty()
  }

  /// Get reference to underlying data
  pub fn data(&self) -> &[u8] {
    &self.data
  }

  /// Take ownership of the buffer data
  pub fn take_data(&mut self) -> Vec<u8> {
    self.position = 0;
    std::mem::take(&mut self.data)
  }

  /// Clear the buffer
  pub fn clear(&mut self) {
    self.data.clear();
    self.position = 0;
  }

  /// Get remaining bytes from current position
  pub fn remaining(&self) -> usize {
    self.data.len().saturating_sub(self.position)
  }
}

impl Default for MemoryBuffer {
  fn default() -> Self {
    Self::new()
  }
}

impl Write for MemoryBuffer {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    if buf.is_empty() {
      return Ok(0);
    }

    let required_len = self.position.saturating_add(buf.len());

    // Check max size limit
    if self.max_size > 0 && required_len > self.max_size {
      return Err(io::Error::new(
        io::ErrorKind::WriteZero,
        format!(
          "Buffer would exceed maximum size ({} > {})",
          required_len, self.max_size
        ),
      ));
    }

    // Extend buffer if necessary
    if required_len > self.data.len() {
      self.data.resize(required_len, 0);
    }

    // Write data at current position
    self.data[self.position..self.position + buf.len()].copy_from_slice(buf);
    self.position += buf.len();

    Ok(buf.len())
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

impl Read for MemoryBuffer {
  fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
    if self.position >= self.data.len() {
      return Ok(0); // EOF
    }

    let available = self.data.len() - self.position;
    let to_read = available.min(buf.len());

    buf[..to_read].copy_from_slice(&self.data[self.position..self.position + to_read]);
    self.position += to_read;

    Ok(to_read)
  }
}

impl Seek for MemoryBuffer {
  fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
    let new_pos = match pos {
      SeekFrom::Start(offset) => offset as i64,
      SeekFrom::End(offset) => self.data.len() as i64 + offset,
      SeekFrom::Current(offset) => self.position as i64 + offset,
    };

    if new_pos < 0 {
      return Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "Attempted to seek before start of buffer",
      ));
    }

    // Allow seeking past end - will create gap filled with zeros on next write
    self.position = new_pos as usize;
    Ok(self.position as u64)
  }
}

/// Internal state for streaming buffer
struct StreamingBufferState {
  buffer: Vec<u8>,
  write_pos: usize,
  read_pos: usize,
  /// Total bytes written (for tracking)
  total_written: u64,
  /// Total bytes read (for tracking)
  total_read: u64,
  /// Whether the writer has finished
  finished: bool,
  /// Whether the buffer is closed
  closed: bool,
}

/// Ring buffer for streaming output with backpressure support
///
/// This buffer is designed for streaming muxer output:
/// - Producer (muxer) writes encoded data
/// - Consumer reads data via ReadableStream
/// - Backpressure when buffer is full
/// - Thread-safe via mutex + condvars
pub struct StreamingBuffer {
  inner: Arc<Mutex<StreamingBufferState>>,
  not_full: Arc<Condvar>,
  not_empty: Arc<Condvar>,
  capacity: usize,
}

impl StreamingBuffer {
  /// Create a new streaming buffer with specified capacity
  pub fn new(capacity: usize) -> Self {
    Self {
      inner: Arc::new(Mutex::new(StreamingBufferState {
        buffer: vec![0; capacity],
        write_pos: 0,
        read_pos: 0,
        total_written: 0,
        total_read: 0,
        finished: false,
        closed: false,
      })),
      not_full: Arc::new(Condvar::new()),
      not_empty: Arc::new(Condvar::new()),
      capacity,
    }
  }

  /// Get capacity of the buffer
  pub fn capacity(&self) -> usize {
    self.capacity
  }

  /// Clone for sharing between producer and consumer
  pub fn clone_handle(&self) -> StreamingBufferHandle {
    StreamingBufferHandle {
      inner: Arc::clone(&self.inner),
      not_full: Arc::clone(&self.not_full),
      not_empty: Arc::clone(&self.not_empty),
      capacity: self.capacity,
    }
  }

  /// Write data to the buffer, blocking if full
  ///
  /// Returns the number of bytes written, or an error if closed.
  pub fn write_blocking(&self, data: &[u8]) -> io::Result<usize> {
    if data.is_empty() {
      return Ok(0);
    }

    let mut written = 0;

    while written < data.len() {
      let mut state = self.inner.lock().unwrap();

      // Check if closed
      if state.closed {
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "Buffer closed"));
      }

      // Calculate available space
      let used = if state.write_pos >= state.read_pos {
        state.write_pos - state.read_pos
      } else {
        self.capacity - state.read_pos + state.write_pos
      };
      let available = self.capacity - used - 1; // -1 to distinguish full from empty

      if available == 0 {
        // Buffer full, wait for consumer
        state = self.not_full.wait(state).unwrap();
        continue;
      }

      // Write as much as possible
      let to_write = available.min(data.len() - written);

      // Handle wrap-around - extract positions first to avoid borrow issues
      let write_pos = state.write_pos;
      let first_part = (self.capacity - write_pos).min(to_write);
      state.buffer[write_pos..write_pos + first_part]
        .copy_from_slice(&data[written..written + first_part]);

      if first_part < to_write {
        let second_part = to_write - first_part;
        state.buffer[..second_part]
          .copy_from_slice(&data[written + first_part..written + to_write]);
      }

      state.write_pos = (write_pos + to_write) % self.capacity;
      state.total_written += to_write as u64;
      written += to_write;

      // Notify consumer
      self.not_empty.notify_one();
    }

    Ok(written)
  }

  /// Signal that writing is complete
  pub fn finish(&self) {
    let mut state = self.inner.lock().unwrap();
    state.finished = true;
    self.not_empty.notify_all();
  }

  /// Close the buffer (cancels pending operations)
  pub fn close(&self) {
    let mut state = self.inner.lock().unwrap();
    state.closed = true;
    state.finished = true;
    self.not_full.notify_all();
    self.not_empty.notify_all();
  }

  /// Check if the buffer is finished (no more data will be written)
  pub fn is_finished(&self) -> bool {
    let state = self.inner.lock().unwrap();
    state.finished
  }

  /// Get total bytes written
  pub fn total_written(&self) -> u64 {
    let state = self.inner.lock().unwrap();
    state.total_written
  }
}

impl Write for StreamingBuffer {
  fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
    self.write_blocking(buf)
  }

  fn flush(&mut self) -> io::Result<()> {
    Ok(())
  }
}

/// Handle for the consumer side of a streaming buffer
pub struct StreamingBufferHandle {
  inner: Arc<Mutex<StreamingBufferState>>,
  not_full: Arc<Condvar>,
  not_empty: Arc<Condvar>,
  capacity: usize,
}

impl StreamingBufferHandle {
  /// Read available data without blocking
  ///
  /// Returns None if buffer is empty but not finished (no data ready yet).
  /// Returns Some(empty vec) if buffer is empty and finished (EOF).
  /// Returns Some(data) if data is available.
  pub fn read_available(&self) -> Option<Vec<u8>> {
    let mut state = self.inner.lock().unwrap();

    // Calculate used space
    let used = if state.write_pos >= state.read_pos {
      state.write_pos - state.read_pos
    } else {
      self.capacity - state.read_pos + state.write_pos
    };

    if used == 0 {
      if state.finished {
        return Some(Vec::new()); // EOF - empty array signals finished
      }
      return None; // No data yet, but not finished
    }

    // Read all available data
    let mut data = Vec::with_capacity(used);

    if state.write_pos > state.read_pos {
      data.extend_from_slice(&state.buffer[state.read_pos..state.write_pos]);
    } else {
      data.extend_from_slice(&state.buffer[state.read_pos..]);
      data.extend_from_slice(&state.buffer[..state.write_pos]);
    }

    state.read_pos = state.write_pos;
    state.total_read += data.len() as u64;

    // Notify producer
    self.not_full.notify_one();

    Some(data)
  }

  /// Read data, blocking until data is available or EOF
  ///
  /// Returns None on EOF, Some(data) otherwise.
  pub fn read_blocking(&self) -> Option<Vec<u8>> {
    loop {
      let mut state = self.inner.lock().unwrap();

      // Calculate used space
      let used = if state.write_pos >= state.read_pos {
        state.write_pos - state.read_pos
      } else {
        self.capacity - state.read_pos + state.write_pos
      };

      if used > 0 {
        // Data available
        let mut data = Vec::with_capacity(used);

        if state.write_pos > state.read_pos {
          data.extend_from_slice(&state.buffer[state.read_pos..state.write_pos]);
        } else {
          data.extend_from_slice(&state.buffer[state.read_pos..]);
          data.extend_from_slice(&state.buffer[..state.write_pos]);
        }

        state.read_pos = state.write_pos;
        state.total_read += data.len() as u64;

        // Notify producer
        self.not_full.notify_one();

        return Some(data);
      }

      if state.finished {
        return None; // EOF
      }

      // Wait for data
      state = self.not_empty.wait(state).unwrap();

      if state.closed {
        return None;
      }
    }
  }

  /// Check if the buffer is finished and empty
  pub fn is_eof(&self) -> bool {
    let state = self.inner.lock().unwrap();
    let used = if state.write_pos >= state.read_pos {
      state.write_pos - state.read_pos
    } else {
      self.capacity - state.read_pos + state.write_pos
    };
    state.finished && used == 0
  }

  /// Get total bytes read
  pub fn total_read(&self) -> u64 {
    let state = self.inner.lock().unwrap();
    state.total_read
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_memory_buffer_write_read() {
    let mut buf = MemoryBuffer::new();

    // Write some data
    buf.write_all(b"Hello, ").unwrap();
    buf.write_all(b"World!").unwrap();

    assert_eq!(buf.len(), 13);
    assert_eq!(buf.position(), 13);

    // Seek to start and read
    buf.seek(SeekFrom::Start(0)).unwrap();
    let mut output = vec![0u8; 13];
    buf.read_exact(&mut output).unwrap();
    assert_eq!(&output, b"Hello, World!");
  }

  #[test]
  fn test_memory_buffer_seek_write() {
    let mut buf = MemoryBuffer::new();

    // Write at position 0
    buf.write_all(b"AAAA").unwrap();

    // Seek to position 2 and overwrite
    buf.seek(SeekFrom::Start(2)).unwrap();
    buf.write_all(b"BB").unwrap();

    assert_eq!(buf.data(), b"AABB");
  }

  #[test]
  fn test_memory_buffer_seek_past_end() {
    let mut buf = MemoryBuffer::new();

    // Seek past end
    buf.seek(SeekFrom::Start(5)).unwrap();
    buf.write_all(b"X").unwrap();

    // Should have zeros before the X
    assert_eq!(buf.data(), &[0, 0, 0, 0, 0, b'X']);
  }

  #[test]
  fn test_memory_buffer_take_data() {
    let mut buf = MemoryBuffer::new();
    buf.write_all(b"test").unwrap();

    let data = buf.take_data();
    assert_eq!(&data, b"test");
    assert!(buf.is_empty());
    assert_eq!(buf.position(), 0);
  }

  #[test]
  fn test_streaming_buffer_basic() {
    let buf = StreamingBuffer::new(1024);
    let handle = buf.clone_handle();

    // Write some data
    buf.write_blocking(b"Hello").unwrap();

    // Read it back
    let data = handle.read_available().unwrap();
    assert_eq!(&data, b"Hello");

    // No more data available
    let data = handle.read_available().unwrap();
    assert!(data.is_empty());

    // Finish and check EOF
    buf.finish();
    assert!(handle.read_available().is_none());
  }

  #[test]
  fn test_streaming_buffer_wrap_around() {
    let buf = StreamingBuffer::new(8);
    let handle = buf.clone_handle();

    // Write and read to advance positions
    buf.write_blocking(b"1234").unwrap();
    handle.read_available().unwrap();

    // Now write more to cause wrap-around
    buf.write_blocking(b"5678").unwrap();

    let data = handle.read_available().unwrap();
    assert_eq!(&data, b"5678");
  }
}
