/**
 * Performance and Stress Tests
 *
 * Tests for memory management, throughput, and stability under load.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import { resetHardwareFallbackState, VideoEncoder, VideoDecoder, VideoFrame } from '../../index.js'
import type { EncodedVideoChunkMetadata, VideoDecoderConfig } from '../../index.js'
import { generateSolidColorI420Frame, TestColors, calculateI420Size, type EncodedVideoChunk } from '../helpers/index.js'
import { createEncoderConfig, createDecoderConfig } from '../helpers/codec-matrix.js'

// Reset hardware fallback state before each test to ensure test isolation
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// Helper to create test encoder with callbacks
// Captures decoderConfig from first chunk's metadata for proper AVCC format handling
function createTestEncoder() {
  const chunks: EncodedVideoChunk[] = []
  const errors: Error[] = []
  let decoderConfig: VideoDecoderConfig | undefined

  const encoder = new VideoEncoder({
    output: (chunk, metadata?: EncodedVideoChunkMetadata) => {
      chunks.push(chunk)
      // Capture decoderConfig from first chunk (contains description for AVCC format)
      if (!decoderConfig && metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig as VideoDecoderConfig
      }
    },
    error: (e) => errors.push(e),
  })

  return { encoder, chunks, errors, getDecoderConfig: () => decoderConfig }
}

// Helper to create test decoder with callbacks
function createTestDecoder() {
  const frames: VideoFrame[] = []
  const errors: Error[] = []

  const decoder = new VideoDecoder({
    output: (frame) => frames.push(frame),
    error: (e) => errors.push(e),
  })

  return { decoder, frames, errors }
}

// ============================================================================
// Frame Count Stress Tests
// ============================================================================

test('stress: encode 100 frames', async (t) => {
  const width = 320
  const height = 240
  const frameCount = 100

  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  // Encode frames
  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.red, i * 33333)
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()

  t.true(chunks.length > 0, 'Should produce encoded chunks')
  t.log(`Encoded ${frameCount} frames into ${chunks.length} chunks`)

  encoder.close()
})

test('stress: encode 500 frames', async (t) => {
  const width = 320
  const height = 240
  const frameCount = 500

  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  const startTime = Date.now()

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.green, i * 33333)
    encoder.encode(frame, i % 30 === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()

  const elapsed = Date.now() - startTime
  const fps = (frameCount / elapsed) * 1000

  t.true(chunks.length > 0)
  t.log(`Encoded ${frameCount} frames in ${elapsed}ms (${fps.toFixed(1)} fps)`)

  encoder.close()
})

test('stress: decode 100 chunks', async (t) => {
  const width = 320
  const height = 240
  const frameCount = 100

  // First encode
  const { encoder, chunks, getDecoderConfig } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.blue, i * 33333)
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(chunks.length > 0)

  // Now decode - use captured decoderConfig that includes description (SPS/PPS for H.264)
  const { decoder, frames } = createTestDecoder()
  const decoderConfig = getDecoderConfig() || createDecoderConfig('h264', { codedWidth: width, codedHeight: height })
  decoder.configure(decoderConfig)

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  await decoder.flush()

  t.true(frames.length > 0, 'Should decode frames')
  t.log(`Decoded ${chunks.length} chunks into ${frames.length} frames`)

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

// ============================================================================
// Resolution Stress Tests
// ============================================================================

test('stress: 720p encoding (1280x720)', async (t) => {
  const width = 1280
  const height = 720
  const frameCount = 10

  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height, { quality: 'medium' }))

  const startTime = Date.now()

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.cyan, i * 33333)
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()

  const elapsed = Date.now() - startTime

  t.true(chunks.length > 0)
  t.log(`720p: Encoded ${frameCount} frames in ${elapsed}ms`)

  encoder.close()
})

test('stress: 1080p encoding (1920x1080)', async (t) => {
  const width = 1920
  const height = 1080
  const frameCount = 5

  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height, { quality: 'low' }))

  const startTime = Date.now()

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.magenta, i * 33333)
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()

  const elapsed = Date.now() - startTime

  t.true(chunks.length > 0)
  t.log(`1080p: Encoded ${frameCount} frames in ${elapsed}ms`)

  encoder.close()
})

// ============================================================================
// Concurrent Encoder Stress Tests
// ============================================================================

test('stress: 4 concurrent encoders', async (t) => {
  const width = 320
  const height = 240
  const frameCount = 20

  const encoderData = [createTestEncoder(), createTestEncoder(), createTestEncoder(), createTestEncoder()]

  // Configure all
  for (const { encoder } of encoderData) {
    encoder.configure(createEncoderConfig('h264', width, height))
  }

  // Encode frames on each
  for (let i = 0; i < frameCount; i++) {
    for (const { encoder } of encoderData) {
      const frame = generateSolidColorI420Frame(width, height, TestColors.yellow, i * 33333)
      encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
      frame.close()
    }
  }

  // Flush and get results
  const allChunks = await Promise.all(
    encoderData.map(async ({ encoder, chunks }) => {
      await encoder.flush()
      return chunks
    }),
  )

  // Verify all produced output
  for (let j = 0; j < encoderData.length; j++) {
    t.true(allChunks[j].length > 0, `Encoder ${j} should produce chunks`)
  }

  // Close all
  for (const { encoder } of encoderData) {
    encoder.close()
  }

  t.log(`4 concurrent encoders each produced ${allChunks.map((c) => c.length).join(', ')} chunks`)
})

// ============================================================================
// Memory Management Tests
// ============================================================================

test('stress: frame creation and cleanup loop', (t) => {
  const width = 640
  const height = 480
  const iterations = 100

  // Create and immediately close many frames
  for (let i = 0; i < iterations; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.white, i)
    t.is(frame.codedWidth, width)
    frame.close()
  }

  // If we get here without OOM, the test passes
  t.pass(`Created and closed ${iterations} frames without memory issues`)
})

test('stress: encoder reconfigure loop', async (t) => {
  const { encoder, chunks } = createTestEncoder()
  const iterations = 20

  for (let i = 0; i < iterations; i++) {
    const width = 320 + (i % 4) * 64
    const height = 240 + (i % 3) * 48

    encoder.configure(createEncoderConfig('h264', width, height))
    t.is(encoder.state, 'configured')

    // Encode one frame
    const frame = generateSolidColorI420Frame(width, height, TestColors.black, 0)
    encoder.encode(frame, { keyFrame: true })
    frame.close()

    await encoder.flush()
    // Chunks are collected via callback, clear the array for next iteration
    chunks.length = 0
  }

  encoder.close()
  t.pass(`Reconfigured encoder ${iterations} times`)
})

test('stress: decoder reconfigure loop', (t) => {
  const { decoder } = createTestDecoder()
  const iterations = 20

  for (let i = 0; i < iterations; i++) {
    decoder.configure(createDecoderConfig('h264'))
    t.is(decoder.state, 'configured')
    decoder.reset()
  }

  decoder.close()
  t.pass(`Reconfigured decoder ${iterations} times`)
})

// ============================================================================
// Throughput Tests
// ============================================================================

test('throughput: H.264 320x240 FPS measurement', async (t) => {
  const width = 320
  const height = 240
  const frameCount = 100

  const { encoder } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  const startTime = Date.now()

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.gray, i * 33333)
    encoder.encode(frame, i % 30 === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()

  const elapsed = Date.now() - startTime
  const fps = (frameCount / elapsed) * 1000

  t.log(`H.264 ${width}x${height}: ${fps.toFixed(1)} fps (${frameCount} frames in ${elapsed}ms)`)
  t.true(fps > 0, 'FPS should be positive')

  encoder.close()
})

test('throughput: H.264 640x480 FPS measurement', async (t) => {
  const width = 640
  const height = 480
  const frameCount = 50

  const { encoder } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  const startTime = Date.now()

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.red, i * 33333)
    encoder.encode(frame, i % 30 === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()

  const elapsed = Date.now() - startTime
  const fps = (frameCount / elapsed) * 1000

  t.log(`H.264 ${width}x${height}: ${fps.toFixed(1)} fps (${frameCount} frames in ${elapsed}ms)`)
  t.true(fps > 0, 'FPS should be positive')

  encoder.close()
})

// ============================================================================
// Queue Size Tests
// ============================================================================

test('stress: encoder queue size under load', async (t) => {
  const width = 320
  const height = 240
  const frameCount = 50

  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  let maxQueueSize = 0

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.blue, i * 33333)
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()

    maxQueueSize = Math.max(maxQueueSize, encoder.encodeQueueSize)
  }

  await encoder.flush()

  t.log(`Max encode queue size: ${maxQueueSize}`)
  t.true(chunks.length > 0, 'Should produce encoded chunks')
  t.true(maxQueueSize >= 0, 'Queue size should be non-negative')

  encoder.close()
})

test('stress: decoder queue size under load', async (t) => {
  const width = 320
  const height = 240
  const frameCount = 50

  // First encode
  const { encoder, chunks, getDecoderConfig } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.green, i * 33333)
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(chunks.length > 0, 'Should have encoded chunks')

  // Decode - use captured decoderConfig that includes description (SPS/PPS for H.264)
  const { decoder, frames } = createTestDecoder()
  const decoderConfig = getDecoderConfig() || createDecoderConfig('h264', { codedWidth: width, codedHeight: height })
  decoder.configure(decoderConfig)

  let maxQueueSize = 0

  for (const chunk of chunks) {
    decoder.decode(chunk)
    maxQueueSize = Math.max(maxQueueSize, decoder.decodeQueueSize)
  }

  await decoder.flush()

  t.log(`Max decode queue size: ${maxQueueSize}`)
  t.true(maxQueueSize >= 0, 'Queue size should be non-negative')

  t.true(frames.length > 0, 'Should produce decoded frames')

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

// ============================================================================
// Edge Case Stress Tests
// ============================================================================

test('stress: multiple encode-reconfigure cycles', async (t) => {
  const width = 320
  const height = 240
  const cycles = 10

  for (let i = 0; i < cycles; i++) {
    const { encoder, chunks, errors } = createTestEncoder()

    if (errors.length) {
      for (const error of errors) {
        console.error(error)
      }
      t.fail()
    }

    encoder.configure(createEncoderConfig('h264', width, height))

    const frame = generateSolidColorI420Frame(width, height, TestColors.yellow, i * 33333)
    encoder.encode(frame, { keyFrame: true })
    await encoder.flush()
    frame.close()
    t.true(chunks.length > 0, `Cycle ${i} should produce output`)

    encoder.close()
  }

  t.pass(`Completed ${cycles} encode cycles with fresh encoders`)
})

test('stress: rapid reconfigure cycles', async (t) => {
  const width = 320
  const height = 240
  const cycles = 20

  const { encoder, chunks } = createTestEncoder()

  for (let i = 0; i < cycles; i++) {
    encoder.configure(createEncoderConfig('h264', width, height))

    const frame = generateSolidColorI420Frame(width, height, TestColors.cyan, 0)
    encoder.encode(frame, { keyFrame: true })
    frame.close()

    await encoder.flush()
    // Clear chunks for next iteration
    chunks.length = 0
  }

  encoder.close()
  t.pass(`Completed ${cycles} reconfigure cycles`)
})

// ============================================================================
// Large Data Tests
// ============================================================================

test('stress: large frame data handling', async (t) => {
  // 4K frame would be very slow, use 1080p
  const width = 1920
  const height = 1080
  const frameSize = calculateI420Size(width, height)

  t.log(`Frame size: ${(frameSize / 1024 / 1024).toFixed(2)} MB`)

  const frame = generateSolidColorI420Frame(width, height, TestColors.white, 0)
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)

  const buffer = new Uint8Array(frame.allocationSize())
  await frame.copyTo(buffer)

  t.is(buffer.length, frameSize)

  frame.close()
  t.pass('Successfully created and extracted 1080p frame data')
})
