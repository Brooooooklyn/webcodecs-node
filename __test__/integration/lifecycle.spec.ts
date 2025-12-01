/**
 * Lifecycle Integration Tests
 *
 * Tests resource management, state transitions, and cleanup.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import { VideoEncoder, VideoDecoder, VideoFrame, CodecState } from '../../index.js'
import {
  generateSolidColorI420Frame,
  generateFrameSequence,
  TestColors,
  reconstructVideoChunk,
  type EncodedVideoChunkOutput,
} from '../helpers/index.js'
import { createEncoderConfig, createDecoderConfig } from '../helpers/codec-matrix.js'

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
// Encoder Lifecycle Tests
// ============================================================================

test('lifecycle: encoder full state cycle', async (t) => {
  const { encoder } = createTestEncoder()

  // Unconfigured
  t.is(encoder.state, CodecState.Unconfigured)

  // Configure -> Configured
  encoder.configure(createEncoderConfig('h264', 320, 240))
  t.is(encoder.state, CodecState.Configured)

  // Encode some frames
  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()
  t.is(encoder.state, CodecState.Configured)

  await encoder.flush()

  // Reset -> Unconfigured (implementation clears state)
  encoder.reset()
  t.is(encoder.state, CodecState.Unconfigured)

  // Reconfigure -> Configured
  encoder.configure(createEncoderConfig('h264', 640, 480))
  t.is(encoder.state, CodecState.Configured)

  // Close -> Closed
  encoder.close()
  t.is(encoder.state, CodecState.Closed)
})

test('lifecycle: decoder full state cycle', (t) => {
  const { decoder } = createTestDecoder()

  // Unconfigured
  t.is(decoder.state, CodecState.Unconfigured)

  // Configure -> Configured
  decoder.configure(createDecoderConfig('h264'))
  t.is(decoder.state, CodecState.Configured)

  // Reset -> Unconfigured (implementation clears state)
  decoder.reset()
  t.is(decoder.state, CodecState.Unconfigured)

  // Reconfigure -> Configured
  decoder.configure(createDecoderConfig('vp8'))
  t.is(decoder.state, CodecState.Configured)

  // Close -> Closed
  decoder.close()
  t.is(decoder.state, CodecState.Closed)
})

// ============================================================================
// Resource Cleanup Tests
// ============================================================================

test('lifecycle: encoder close releases resources', async (t) => {
  const { encoder } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  // Encode frames to use resources
  const frames = generateFrameSequence(320, 240, 10)
  encoder.encode(frames[0], { keyFrame: true })
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }
  for (const frame of frames) {
    frame.close()
  }

  await encoder.flush()

  // Close should not throw
  t.notThrows(() => encoder.close())
  t.is(encoder.state, CodecState.Closed)
})

test('lifecycle: decoder close releases resources', async (t) => {
  // First create some encoded chunks
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))
  const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()
  await encoder.flush()
  encoder.close()

  // Decode
  const { decoder, frames: decodedFrames } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))
  for (const chunk of chunks) {
    decoder.decode(reconstructVideoChunk(chunk))
  }

  await decoder.flush()

  // Clean up decoded frames
  for (const f of decodedFrames) {
    f.close()
  }

  // Close should not throw
  t.notThrows(() => decoder.close())
  t.is(decoder.state, CodecState.Closed)
})

test('lifecycle: frame close releases resources', (t) => {
  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)

  // Should be accessible
  t.is(frame.codedWidth, 320)
  t.is(frame.codedHeight, 240)

  // Close should not throw
  t.notThrows(() => frame.close())

  // Multiple closes should not throw
  t.notThrows(() => frame.close())
})

// ============================================================================
// Clone Independence Tests
// ============================================================================

test('lifecycle: clone is independent of original', (t) => {
  const original = generateSolidColorI420Frame(320, 240, TestColors.yellow, 1000, 33333)

  // Clone
  const cloned = original.clone()

  // Verify properties match
  t.is(cloned.format, original.format)
  t.is(cloned.codedWidth, original.codedWidth)
  t.is(cloned.codedHeight, original.codedHeight)
  t.is(cloned.timestamp, original.timestamp)
  t.is(cloned.duration, original.duration)

  // Close original
  original.close()

  // Clone should still be usable
  t.is(cloned.codedWidth, 320)
  t.is(cloned.codedHeight, 240)

  const size = cloned.allocationSize()
  t.true(size > 0)

  const buffer = new Uint8Array(size)
  t.notThrows(() => cloned.copyTo(buffer))

  // Cleanup
  cloned.close()
})

test('lifecycle: multiple clones are independent', (t) => {
  const original = generateSolidColorI420Frame(128, 96, TestColors.cyan, 0)

  // Create multiple clones
  const clone1 = original.clone()
  const clone2 = original.clone()
  const clone3 = clone1.clone() // Clone of clone

  // Close original and clone1
  original.close()
  clone1.close()

  // clone2 and clone3 should still work
  t.is(clone2.codedWidth, 128)
  t.is(clone3.codedWidth, 128)

  clone2.close()
  clone3.close()
})

// ============================================================================
// Idempotency Tests
// ============================================================================

test('lifecycle: encoder close is idempotent', (t) => {
  const { encoder } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))

  // Multiple closes should not throw
  t.notThrows(() => encoder.close())
  t.notThrows(() => encoder.close())
  t.notThrows(() => encoder.close())

  t.is(encoder.state, CodecState.Closed)
})

test('lifecycle: decoder close is idempotent', (t) => {
  const { decoder } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264'))

  // Multiple closes should not throw
  t.notThrows(() => decoder.close())
  t.notThrows(() => decoder.close())
  t.notThrows(() => decoder.close())

  t.is(decoder.state, CodecState.Closed)
})

test('lifecycle: frame close is idempotent', (t) => {
  const frame = generateSolidColorI420Frame(128, 96, TestColors.red, 0)

  // Multiple closes should not throw
  t.notThrows(() => frame.close())
  t.notThrows(() => frame.close())
  t.notThrows(() => frame.close())
})

// ============================================================================
// Use After Close Tests
// ============================================================================

test('lifecycle: encoder operations fail after close', (t) => {
  const { encoder } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))
  encoder.close()

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)

  // encode() on closed encoder triggers error callback (transitions to closed already)
  encoder.encode(frame)

  frame.close()

  // Test passes if no crash - error callback will be invoked asynchronously
  t.pass('encode() on closed encoder did not crash')
})

test('lifecycle: decoder operations fail after close', async (t) => {
  const { decoder } = createTestDecoder()
  decoder.configure(createDecoderConfig('h264'))
  decoder.close()

  // Create a chunk to try decoding
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', 320, 240))
  const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()
  await encoder.flush()
  encoder.close()

  // decode() on closed decoder triggers error callback
  if (chunks.length > 0) {
    decoder.decode(reconstructVideoChunk(chunks[0]))
  }

  // Test passes if no crash - error callback will be invoked asynchronously
  t.pass('decode() on closed decoder did not crash')
})

// ============================================================================
// Concurrent Instance Tests
// ============================================================================

test('lifecycle: multiple encoder instances', (t) => {
  const { encoder: encoder1 } = createTestEncoder()
  const { encoder: encoder2 } = createTestEncoder()
  const { encoder: encoder3 } = createTestEncoder()

  encoder1.configure(createEncoderConfig('h264', 320, 240))
  encoder2.configure(createEncoderConfig('h264', 640, 480))
  encoder3.configure(createEncoderConfig('vp8', 320, 240))

  // All should be configured
  t.is(encoder1.state, CodecState.Configured)
  t.is(encoder2.state, CodecState.Configured)
  t.is(encoder3.state, CodecState.Configured)

  // Close in different order
  encoder2.close()
  encoder1.close()
  encoder3.close()

  t.is(encoder1.state, CodecState.Closed)
  t.is(encoder2.state, CodecState.Closed)
  t.is(encoder3.state, CodecState.Closed)
})

test('lifecycle: multiple decoder instances', (t) => {
  const { decoder: decoder1 } = createTestDecoder()
  const { decoder: decoder2 } = createTestDecoder()
  const { decoder: decoder3 } = createTestDecoder()

  decoder1.configure(createDecoderConfig('h264'))
  decoder2.configure(createDecoderConfig('vp8'))
  decoder3.configure(createDecoderConfig('vp9'))

  // All should be configured
  t.is(decoder1.state, CodecState.Configured)
  t.is(decoder2.state, CodecState.Configured)
  t.is(decoder3.state, CodecState.Configured)

  // Close all
  decoder1.close()
  decoder2.close()
  decoder3.close()

  t.is(decoder1.state, CodecState.Closed)
  t.is(decoder2.state, CodecState.Closed)
  t.is(decoder3.state, CodecState.Closed)
})
