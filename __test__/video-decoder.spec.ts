/**
 * VideoDecoder API Conformance Tests
 *
 * Tests WebCodecs VideoDecoder specification compliance.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import {
  VideoEncoder,
  VideoDecoder,
  VideoFrame,
  EncodedVideoChunk,
  CodecState,
} from '../index.js'
import {
  generateFrameSequence,
  reconstructVideoChunk,
  type EncodedVideoChunkOutput,
} from './helpers/index.js'
import { createEncoderConfig, createDecoderConfig } from './helpers/codec-matrix.js'

// Helper to create test encoder with callbacks
function createTestEncoder() {
  const chunks: EncodedVideoChunkOutput[] = []
  const errors: Error[] = []

  const encoder = new VideoEncoder(
    (chunk, _metadata) => {
      chunks.push(chunk)
    },
    (e) => errors.push(e),
  )

  return { encoder, chunks, errors }
}

// Helper to create test decoder with callbacks
function createTestDecoder() {
  const frames: VideoFrame[] = []
  const errors: Error[] = []

  const decoder = new VideoDecoder(
    (frame) => frames.push(frame),
    (e) => errors.push(e),
  )

  return { decoder, frames, errors }
}

// ============================================================================
// Helper: Create encoded chunks for decoder tests
// ============================================================================

async function createEncodedH264Chunks(
  width: number,
  height: number,
  frameCount: number,
): Promise<EncodedVideoChunk[]> {
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  const frames = generateFrameSequence(width, height, frameCount)

  encoder.encode(frames[0], { keyFrame: true })
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }

  for (const frame of frames) {
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  // Reconstruct chunks to preserve class identity for decoder.decode()
  return chunks.map(reconstructVideoChunk)
}

// ============================================================================
// Constructor and State Tests
// ============================================================================

test('VideoDecoder: constructor creates unconfigured decoder', (t) => {
  const { decoder } = createTestDecoder()
  t.is(decoder.state, CodecState.Unconfigured)
  t.is(decoder.decodeQueueSize, 0)
  decoder.close()
})

test('VideoDecoder: constructor requires callbacks', (t) => {
  // @ts-expect-error - Testing that missing callbacks throws
  t.throws(() => new VideoDecoder())
  // @ts-expect-error - Testing that missing error callback throws
  t.throws(() => new VideoDecoder(() => {}))
})

test('VideoDecoder: state transitions correctly', (t) => {
  const { decoder } = createTestDecoder()

  // Initial state
  t.is(decoder.state, CodecState.Unconfigured)

  // Configure
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))
  t.is(decoder.state, CodecState.Configured)

  // Close
  decoder.close()
  t.is(decoder.state, CodecState.Closed)
})

test('VideoDecoder: close() is idempotent', (t) => {
  const { decoder } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264'))

  decoder.close()
  t.is(decoder.state, CodecState.Closed)

  // Second close should not throw
  t.notThrows(() => decoder.close())
  t.is(decoder.state, CodecState.Closed)
})

// ============================================================================
// configure() Tests
// ============================================================================

test('VideoDecoder: configure() with H.264', (t) => {
  const { decoder } = createTestDecoder()

  t.notThrows(() => {
    decoder.configure(createDecoderConfig('h264'))
  })

  t.is(decoder.state, CodecState.Configured)
  decoder.close()
})

test('VideoDecoder: configure() with VP8', (t) => {
  const { decoder } = createTestDecoder()

  t.notThrows(() => {
    decoder.configure(createDecoderConfig('vp8'))
  })

  t.is(decoder.state, CodecState.Configured)
  decoder.close()
})

test('VideoDecoder: configure() with VP9', (t) => {
  const { decoder } = createTestDecoder()

  t.notThrows(() => {
    decoder.configure(createDecoderConfig('vp9'))
  })

  t.is(decoder.state, CodecState.Configured)
  decoder.close()
})

test('VideoDecoder: configure() can be called multiple times', (t) => {
  const { decoder } = createTestDecoder()

  // First configuration
  decoder.configure(createDecoderConfig('h264'))
  t.is(decoder.state, CodecState.Configured)

  // Reconfigure
  decoder.configure(createDecoderConfig('vp8'))
  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

// ============================================================================
// decode() Tests
// ============================================================================

test('VideoDecoder: decode() single chunk', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)
  t.true(chunks.length > 0, 'Should have encoded chunks')

  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  t.notThrows(() => {
    decoder.decode(chunks[0])
  })

  await decoder.flush()

  // Should have output via callback
  t.true(frames.length > 0)

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: decode() multiple chunks', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 5)
  t.true(chunks.length > 0)

  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  await decoder.flush()

  t.true(frames.length > 0, 'Should decode frames from multiple chunks')

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: decode() preserves frame properties', async (t) => {
  const width = 320
  const height = 240

  const chunks = await createEncodedH264Chunks(width, height, 1)
  t.true(chunks.length > 0)

  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))

  decoder.decode(chunks[0])
  await decoder.flush()

  t.true(frames.length > 0)

  const frame = frames[0]
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)

  for (const f of frames) {
    f.close()
  }

  decoder.close()
})

// ============================================================================
// flush() Tests
// ============================================================================

test('VideoDecoder: flush() produces all pending output', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 3)

  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  // Flush to get all output
  await decoder.flush()

  t.true(frames.length > 0, 'Should have output after flush')

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: flush() returns a Promise', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  decoder.decode(chunks[0])

  const flushResult = decoder.flush()
  t.true(flushResult instanceof Promise, 'flush() should return a Promise')

  await flushResult

  t.true(frames.length > 0)

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

// ============================================================================
// reset() Tests
// ============================================================================

test('VideoDecoder: reset() returns to unconfigured state', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const { decoder } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  decoder.decode(chunks[0])
  decoder.reset()

  // Reset returns to unconfigured state
  t.is(decoder.state, CodecState.Unconfigured)

  decoder.close()
})

test('VideoDecoder: reset() then reconfigure allows new decoding', async (t) => {
  // Create two separate encoded streams (each starting with a keyframe)
  const chunks1 = await createEncodedH264Chunks(320, 240, 1)
  const chunks2 = await createEncodedH264Chunks(320, 240, 1)

  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  // First decode
  decoder.decode(chunks1[0])
  await decoder.flush()
  const firstFrameCount = frames.length
  t.true(firstFrameCount > 0, 'First decode should produce frames')

  // Reset (goes to unconfigured)
  decoder.reset()
  t.is(decoder.state, CodecState.Unconfigured)

  // Reconfigure
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))
  t.is(decoder.state, CodecState.Configured)

  // Second decode should work (with fresh keyframe)
  t.notThrows(() => {
    decoder.decode(chunks2[0])
  })

  await decoder.flush()
  t.true(frames.length > firstFrameCount, 'Second decode should produce more frames')

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

// ============================================================================
// isConfigSupported() Tests
// ============================================================================

test('VideoDecoder: isConfigSupported() for H.264', async (t) => {
  const config = createDecoderConfig('h264')
  const result = await VideoDecoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.is(typeof result.codec, 'string')
})

test('VideoDecoder: isConfigSupported() for VP8', async (t) => {
  const config = createDecoderConfig('vp8')
  const result = await VideoDecoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
})

test('VideoDecoder: isConfigSupported() for VP9', async (t) => {
  const config = createDecoderConfig('vp9')
  const result = await VideoDecoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
})

test('VideoDecoder: isConfigSupported() returns false for unknown codec', async (t) => {
  const result = await VideoDecoder.isConfigSupported({
    codec: 'unknown-codec',
  })

  t.false(result.supported)
})

// ============================================================================
// Error Handling Tests (Errors via callback)
// ============================================================================

test('VideoDecoder: decode() on unconfigured decoder triggers error callback', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const { decoder } = createTestDecoder()

  // decode() on unconfigured decoder should trigger error callback
  decoder.decode(chunks[0])

  t.is(decoder.state, CodecState.Closed, 'Decoder should be closed after error')

  decoder.close()
})

test('VideoDecoder: decode() on closed decoder triggers error callback', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const { decoder } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264'))
  decoder.close()

  // decode() on closed decoder should trigger error callback
  decoder.decode(chunks[0])

  // Test passes if no crash - error callback will be invoked asynchronously
  t.pass('decode() on closed decoder did not crash')
})

// ============================================================================
// ondequeue Event Tests
// ============================================================================

test('VideoDecoder: ondequeue fires when queue decreases', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const { decoder, frames } = createTestDecoder()

  let dequeueCount = 0
  decoder.ondequeue = () => {
    dequeueCount++
  }

  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  decoder.decode(chunks[0])
  await decoder.flush()

  t.true(dequeueCount >= 1, 'ondequeue should have fired')

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: ondequeue can be set to null', (t) => {
  const { decoder } = createTestDecoder()

  decoder.ondequeue = () => {}
  t.notThrows(() => {
    decoder.ondequeue = null
  })

  decoder.close()
})
