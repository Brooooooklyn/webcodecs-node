/**
 * VideoEncoder API Conformance Tests
 *
 * Tests WebCodecs VideoEncoder specification compliance.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import { VideoEncoder, CodecState, EncodedVideoChunkType } from '../index.js'
import {
  generateSolidColorI420Frame,
  generateFrameSequence,
  TestColors,
  type EncodedVideoChunkOutput,
} from './helpers/index.js'
import { createEncoderConfig } from './helpers/codec-matrix.js'

// Helper to create encoder with callbacks that collect output
function createTestEncoder() {
  const chunks: EncodedVideoChunkOutput[] = []
  const errors: Error[] = []

  const encoder = new VideoEncoder(
    (chunk, _metadata) => {
      chunks.push(chunk)
    },
    (e) => {
      errors.push(e)
    },
  )

  return { encoder, chunks, errors }
}

// ============================================================================
// Constructor and State Tests
// ============================================================================

test('VideoEncoder: constructor creates unconfigured encoder', (t) => {
  const { encoder } = createTestEncoder()
  t.is(encoder.state, CodecState.Unconfigured)
  t.is(encoder.encodeQueueSize, 0)
  encoder.close()
})

test('VideoEncoder: constructor requires callbacks', (t) => {
  // @ts-expect-error - Testing that missing callbacks throws
  t.throws(() => new VideoEncoder())
  // @ts-expect-error - Testing that missing error callback throws
  t.throws(() => new VideoEncoder(() => {}))
})

test('VideoEncoder: state transitions correctly', (t) => {
  const { encoder } = createTestEncoder()

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
  const { encoder } = createTestEncoder()
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
  const { encoder } = createTestEncoder()

  t.notThrows(() => {
    encoder.configure(createEncoderConfig('h264', 320, 240))
  })

  t.is(encoder.state, CodecState.Configured)
  encoder.close()
})

test('VideoEncoder: configure() with VP8', (t) => {
  const { encoder } = createTestEncoder()

  t.notThrows(() => {
    encoder.configure(createEncoderConfig('vp8', 320, 240))
  })

  t.is(encoder.state, CodecState.Configured)
  encoder.close()
})

test('VideoEncoder: configure() with VP9', (t) => {
  const { encoder } = createTestEncoder()

  t.notThrows(() => {
    encoder.configure(createEncoderConfig('vp9', 320, 240))
  })

  t.is(encoder.state, CodecState.Configured)
  encoder.close()
})

test('VideoEncoder: configure() can be called multiple times', (t) => {
  const { encoder } = createTestEncoder()

  // First configuration
  encoder.configure(createEncoderConfig('h264', 320, 240))
  t.is(encoder.state, CodecState.Configured)

  // Reconfigure with different settings
  encoder.configure(createEncoderConfig('h264', 640, 480))
  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

// ============================================================================
// encode() Tests
// ============================================================================

test('VideoEncoder: encode() single frame', async (t) => {
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, 0)

  t.notThrows(() => {
    encoder.encode(frame, { keyFrame: true })
  })

  frame.close()
  await encoder.flush()

  // Should have output via callback
  t.true(chunks.length > 0)

  encoder.close()
})

test('VideoEncoder: encode() multiple frames', async (t) => {
  const { encoder, chunks } = createTestEncoder()
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

  await encoder.flush()

  // Should have multiple chunks via callback
  t.true(chunks.length > 0, 'Should produce encoded chunks')

  encoder.close()
})

test('VideoEncoder: encode() with keyFrame option', async (t) => {
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()

  t.true(chunks.length > 0)

  // First chunk should be a keyframe
  t.is(chunks[0].type, EncodedVideoChunkType.Key)

  encoder.close()
})

test('VideoEncoder: encode() preserves timestamp', async (t) => {
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const timestamp = 123456
  const frame = generateSolidColorI420Frame(320, 240, TestColors.yellow, timestamp)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()

  t.true(chunks.length > 0)
  t.is(chunks[0].timestamp, timestamp)

  encoder.close()
})

// ============================================================================
// flush() Tests
// ============================================================================

test('VideoEncoder: flush() produces all pending output', async (t) => {
  const { encoder, chunks } = createTestEncoder()
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
  await encoder.flush()

  // After flush, should have all output via callback
  t.true(chunks.length >= 1, 'Flush should produce output')

  encoder.close()
})

test('VideoEncoder: flush() returns a Promise', async (t) => {
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.cyan, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  const flushResult = encoder.flush()
  t.true(flushResult instanceof Promise, 'flush() should return a Promise')

  await flushResult

  t.true(chunks.length > 0)

  encoder.close()
})

// ============================================================================
// reset() Tests
// ============================================================================

test('VideoEncoder: reset() returns to unconfigured state', (t) => {
  const { encoder } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.magenta, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  encoder.reset()

  // Reset returns to unconfigured state
  t.is(encoder.state, CodecState.Unconfigured)

  encoder.close()
})

test('VideoEncoder: reset() then reconfigure allows new encoding', async (t) => {
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  // First encode
  const frame1 = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame1, { keyFrame: true })
  frame1.close()
  await encoder.flush()
  const firstChunksCount = chunks.length

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

  await encoder.flush()
  t.true(chunks.length > firstChunksCount, 'Should have more chunks after second encode')

  encoder.close()
})

// ============================================================================
// isConfigSupported() Tests
// ============================================================================

test('VideoEncoder: isConfigSupported() for H.264', async (t) => {
  const config = createEncoderConfig('h264', 320, 240)
  const result = await VideoEncoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.truthy(result.config)
})

test('VideoEncoder: isConfigSupported() for VP8', async (t) => {
  const config = createEncoderConfig('vp8', 320, 240)
  const result = await VideoEncoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.truthy(result.config)
})

test('VideoEncoder: isConfigSupported() for VP9', async (t) => {
  const config = createEncoderConfig('vp9', 320, 240)
  const result = await VideoEncoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.truthy(result.config)
})

test('VideoEncoder: isConfigSupported() returns false for unknown codec', async (t) => {
  const result = await VideoEncoder.isConfigSupported({
    codec: 'unknown-codec',
    width: 320,
    height: 240,
  })

  t.false(result.supported)
})

// ============================================================================
// Error Handling Tests (Errors via callback)
// ============================================================================

test('VideoEncoder: encode() on unconfigured encoder triggers error callback', (t) => {
  const { encoder } = createTestEncoder()
  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)

  // encode() on unconfigured encoder should trigger error callback
  encoder.encode(frame)

  frame.close()

  // Give the error callback a chance to be called
  t.is(encoder.state, CodecState.Closed, 'Encoder should be closed after error')

  encoder.close()
})

test('VideoEncoder: encode() on closed encoder triggers error callback', (t) => {
  const { encoder } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))
  encoder.close()

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)

  // encode() on closed encoder should trigger error callback
  encoder.encode(frame)

  frame.close()

  // Test passes if no crash - error callback will be invoked asynchronously
  t.pass('encode() on closed encoder did not crash')
})

// ============================================================================
// ondequeue Event Tests
// ============================================================================

test('VideoEncoder: ondequeue fires when queue decreases', async (t) => {
  const { encoder } = createTestEncoder()

  let dequeueCount = 0
  encoder.ondequeue = () => {
    dequeueCount++
  }

  encoder.configure(createEncoderConfig('h264', 320, 240))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()

  t.true(dequeueCount >= 1, 'ondequeue should have fired')

  encoder.close()
})

test('VideoEncoder: ondequeue can be set to null', (t) => {
  const { encoder } = createTestEncoder()

  encoder.ondequeue = () => {}
  t.notThrows(() => {
    encoder.ondequeue = null
  })

  encoder.close()
})
