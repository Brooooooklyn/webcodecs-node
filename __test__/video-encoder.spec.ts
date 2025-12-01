/**
 * VideoEncoder API Conformance Tests
 *
 * Tests WebCodecs VideoEncoder specification compliance.
 */

import test from 'ava'

import { VideoEncoder, VideoFrame, CodecState, EncodedVideoChunkType } from '../index.js'
import {
  generateSolidColorI420Frame,
  generateFrameSequence,
  TestColors,
} from './helpers/index.js'
import { createEncoderConfig, CodecRegistry } from './helpers/codec-matrix.js'

// ============================================================================
// Constructor and State Tests
// ============================================================================

test('VideoEncoder: constructor creates unconfigured encoder', (t) => {
  const encoder = new VideoEncoder()
  t.is(encoder.state, CodecState.Unconfigured)
  t.is(encoder.encodeQueueSize, 0)
  encoder.close()
})

test('VideoEncoder: state transitions correctly', (t) => {
  const encoder = new VideoEncoder()

  // Initial state
  t.is(encoder.state, CodecState.Unconfigured)

  // Configure
  encoder.configure(createEncoderConfig('h264', 320, 240))
  t.is(encoder.state, CodecState.Configured)

  // Close
  encoder.close()
  t.is(encoder.state, CodecState.Closed)
})

test('VideoEncoder: close() is idempotent', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  encoder.close()
  t.is(encoder.state, CodecState.Closed)

  // Second close should not throw
  t.notThrows(() => encoder.close())
  t.is(encoder.state, CodecState.Closed)
})

// ============================================================================
// configure() Tests
// ============================================================================

test('VideoEncoder: configure() with H.264', (t) => {
  const encoder = new VideoEncoder()

  t.notThrows(() => {
    encoder.configure(createEncoderConfig('h264', 320, 240))
  })

  t.is(encoder.state, CodecState.Configured)
  encoder.close()
})

test('VideoEncoder: configure() with VP8', (t) => {
  const encoder = new VideoEncoder()

  t.notThrows(() => {
    encoder.configure(createEncoderConfig('vp8', 320, 240))
  })

  t.is(encoder.state, CodecState.Configured)
  encoder.close()
})

test('VideoEncoder: configure() with VP9', (t) => {
  const encoder = new VideoEncoder()

  t.notThrows(() => {
    encoder.configure(createEncoderConfig('vp9', 320, 240))
  })

  t.is(encoder.state, CodecState.Configured)
  encoder.close()
})

test('VideoEncoder: configure() can be called multiple times', (t) => {
  const encoder = new VideoEncoder()

  // First configuration
  encoder.configure(createEncoderConfig('h264', 320, 240))
  t.is(encoder.state, CodecState.Configured)

  // Reconfigure with different settings
  encoder.configure(createEncoderConfig('h264', 640, 480))
  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

test('VideoEncoder: configure() clears output queue', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  // Encode a frame
  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  // Flush to get output
  encoder.flush()

  // Reconfigure should clear queue
  encoder.configure(createEncoderConfig('h264', 320, 240))

  // Queue should be empty after reconfigure
  t.false(encoder.hasOutput())

  encoder.close()
})

// ============================================================================
// encode() Tests
// ============================================================================

test('VideoEncoder: encode() single frame', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, 0)

  t.notThrows(() => {
    encoder.encode(frame, { keyFrame: true })
  })

  frame.close()
  encoder.flush()

  // Should have output
  t.true(encoder.hasOutput())

  encoder.close()
})

test('VideoEncoder: encode() multiple frames', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frames = generateFrameSequence(320, 240, 5)

  // First frame as keyframe
  encoder.encode(frames[0], { keyFrame: true })

  // Remaining frames as delta
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }

  // Clean up frames
  for (const frame of frames) {
    frame.close()
  }

  encoder.flush()

  // Should have multiple chunks
  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length > 0, 'Should produce encoded chunks')

  encoder.close()
})

test('VideoEncoder: encode() with keyFrame option', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  encoder.flush()

  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length > 0)

  // First chunk should be a keyframe
  t.is(chunks[0].type, EncodedVideoChunkType.Key)

  encoder.close()
})

test('VideoEncoder: encode() preserves timestamp', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const timestamp = 123456
  const frame = generateSolidColorI420Frame(320, 240, TestColors.yellow, timestamp)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  encoder.flush()

  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length > 0)
  t.is(chunks[0].timestamp, timestamp)

  encoder.close()
})

// ============================================================================
// flush() Tests
// ============================================================================

test('VideoEncoder: flush() produces all pending output', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frames = generateFrameSequence(320, 240, 10)

  encoder.encode(frames[0], { keyFrame: true })
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }

  for (const frame of frames) {
    frame.close()
  }

  // Before flush, may or may not have output
  encoder.flush()

  // After flush, should have all output
  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length >= 1, 'Flush should produce output')

  encoder.close()
})

test('VideoEncoder: flush() produces output once', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.cyan, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  encoder.flush()
  const chunks1 = encoder.takeEncodedChunks()

  t.true(chunks1.length > 0)

  // After flush without new encodes, queue should be empty
  const chunks2 = encoder.takeEncodedChunks()
  t.is(chunks2.length, 0, 'No more output after taking chunks')

  encoder.close()
})

// ============================================================================
// reset() Tests
// ============================================================================

test('VideoEncoder: reset() returns to unconfigured state', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.magenta, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  encoder.reset()

  // Reset returns to unconfigured state (implementation detail)
  t.is(encoder.state, CodecState.Unconfigured)

  // Output should be cleared
  t.false(encoder.hasOutput())

  encoder.close()
})

test('VideoEncoder: reset() then reconfigure allows new encoding', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  // First encode
  const frame1 = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame1, { keyFrame: true })
  frame1.close()
  encoder.flush()
  encoder.takeEncodedChunks()

  // Reset (goes to unconfigured)
  encoder.reset()
  t.is(encoder.state, CodecState.Unconfigured)

  // Reconfigure
  encoder.configure(createEncoderConfig('h264', 320, 240))
  t.is(encoder.state, CodecState.Configured)

  // Second encode should work
  const frame2 = generateSolidColorI420Frame(320, 240, TestColors.blue, 1000)
  t.notThrows(() => {
    encoder.encode(frame2, { keyFrame: true })
  })
  frame2.close()

  encoder.flush()
  t.true(encoder.hasOutput())

  encoder.close()
})

// ============================================================================
// Output Queue Tests
// ============================================================================

test('VideoEncoder: hasOutput() reflects queue state', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  // Initially empty
  t.false(encoder.hasOutput())

  const frame = generateSolidColorI420Frame(320, 240, TestColors.white, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  encoder.flush()

  // After flush, should have output
  t.true(encoder.hasOutput())

  // Take all chunks
  encoder.takeEncodedChunks()

  // Should be empty again
  t.false(encoder.hasOutput())

  encoder.close()
})

test('VideoEncoder: takeEncodedChunks() empties queue', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.black, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  encoder.flush()

  const chunks1 = encoder.takeEncodedChunks()
  t.true(chunks1.length > 0)

  // Second call should return empty array
  const chunks2 = encoder.takeEncodedChunks()
  t.is(chunks2.length, 0)

  encoder.close()
})

test('VideoEncoder: takeNextChunk() returns chunks one at a time', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frames = generateFrameSequence(320, 240, 3)
  encoder.encode(frames[0], { keyFrame: true })
  encoder.encode(frames[1])
  encoder.encode(frames[2])

  for (const frame of frames) {
    frame.close()
  }

  encoder.flush()

  // Take chunks one at a time
  let count = 0
  while (encoder.hasOutput()) {
    const chunk = encoder.takeNextChunk()
    t.truthy(chunk)
    count++
    if (count > 100) break // Safety limit
  }

  t.true(count > 0, 'Should have taken at least one chunk')

  // No more chunks
  const finalChunk = encoder.takeNextChunk()
  t.is(finalChunk, null)

  encoder.close()
})

// ============================================================================
// isConfigSupported() Tests
// ============================================================================

test('VideoEncoder: isConfigSupported() for H.264', (t) => {
  const config = createEncoderConfig('h264', 320, 240)
  const result = VideoEncoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.truthy(result.config)
})

test('VideoEncoder: isConfigSupported() for VP8', (t) => {
  const config = createEncoderConfig('vp8', 320, 240)
  const result = VideoEncoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.truthy(result.config)
})

test('VideoEncoder: isConfigSupported() for VP9', (t) => {
  const config = createEncoderConfig('vp9', 320, 240)
  const result = VideoEncoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.truthy(result.config)
})

test('VideoEncoder: isConfigSupported() returns false for unknown codec', (t) => {
  const result = VideoEncoder.isConfigSupported({
    codec: 'unknown-codec',
    width: 320,
    height: 240,
  })

  t.false(result.supported)
})

// ============================================================================
// Error Handling Tests
// ============================================================================

test('VideoEncoder: encode() on unconfigured encoder throws', (t) => {
  const encoder = new VideoEncoder()
  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)

  t.throws(() => {
    encoder.encode(frame)
  })

  frame.close()
  encoder.close()
})

test('VideoEncoder: encode() on closed encoder throws', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))
  encoder.close()

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)

  t.throws(() => {
    encoder.encode(frame)
  })

  frame.close()
})

test('VideoEncoder: flush() on unconfigured encoder throws', (t) => {
  const encoder = new VideoEncoder()

  t.throws(() => {
    encoder.flush()
  })

  encoder.close()
})

test('VideoEncoder: flush() on closed encoder throws', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))
  encoder.close()

  t.throws(() => {
    encoder.flush()
  })
})
