/**
 * Roundtrip Integration Tests
 *
 * Tests encode-decode roundtrip for quality verification.
 */

import test from 'ava'

import { VideoEncoder, VideoDecoder, EncodedVideoChunkType } from '../../index.js'
import {
  generateSolidColorI420Frame,
  generateGradientI420Frame,
  generateFrameSequence,
  generateColorBarsI420Frame,
  TestColors,
  extractI420Data,
} from '../helpers/index.js'
import {
  compareFrames,
  compareBuffers,
  PSNRThresholds,
  formatPSNR,
  getQualityDescription,
} from '../helpers/frame-comparator.js'
import { createEncoderConfig, createDecoderConfig, type CodecType } from '../helpers/codec-matrix.js'

// ============================================================================
// Basic Roundtrip Tests
// ============================================================================

test('roundtrip: H.264 single frame encode-decode', (t) => {
  const width = 320
  const height = 240
  const timestamp = 0

  // Create original frame
  const originalFrame = generateSolidColorI420Frame(width, height, TestColors.red, timestamp)
  const originalData = extractI420Data(originalFrame)

  // Encode
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))
  encoder.encode(originalFrame, { keyFrame: true })
  originalFrame.close()
  encoder.flush()

  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length > 0, 'Should produce encoded chunks')
  encoder.close()

  // Decode
  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  decoder.flush()

  const decodedFrames = decoder.takeDecodedFrames()
  t.true(decodedFrames.length > 0, 'Should produce decoded frames')

  // Compare
  const decodedData = extractI420Data(decodedFrames[0])
  const comparison = compareBuffers(originalData, decodedData)

  t.log(`PSNR: ${formatPSNR(comparison.psnr)} (${getQualityDescription(comparison.psnr)})`)
  t.true(comparison.acceptable, `PSNR ${formatPSNR(comparison.psnr)} should be >= ${PSNRThresholds.acceptable} dB`)

  // Cleanup
  for (const frame of decodedFrames) {
    frame.close()
  }
  decoder.close()
})

test('roundtrip: H.264 multiple frames', (t) => {
  const width = 320
  const height = 240
  const frameCount = 5

  // Create original frames
  const originalFrames = generateFrameSequence(width, height, frameCount)
  const originalDataList = originalFrames.map(extractI420Data)

  // Encode
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  encoder.encode(originalFrames[0], { keyFrame: true })
  for (let i = 1; i < originalFrames.length; i++) {
    encoder.encode(originalFrames[i])
  }

  for (const frame of originalFrames) {
    frame.close()
  }

  encoder.flush()
  const chunks = encoder.takeEncodedChunks()
  encoder.close()

  t.true(chunks.length > 0, 'Should produce encoded chunks')

  // Decode
  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  decoder.flush()

  const decodedFrames = decoder.takeDecodedFrames()
  t.true(decodedFrames.length > 0, 'Should produce decoded frames')

  // Compare each frame (note: might have different count due to codec behavior)
  const framesToCompare = Math.min(originalDataList.length, decodedFrames.length)
  for (let i = 0; i < framesToCompare; i++) {
    const decodedData = extractI420Data(decodedFrames[i])
    const comparison = compareBuffers(originalDataList[i], decodedData)

    t.true(comparison.acceptable, `Frame ${i}: PSNR ${formatPSNR(comparison.psnr)} should be acceptable`)
  }

  // Cleanup
  for (const frame of decodedFrames) {
    frame.close()
  }
  decoder.close()
})

// ============================================================================
// Timestamp Preservation Tests
// ============================================================================

test('roundtrip: timestamp preservation in chunks', (t) => {
  const width = 320
  const height = 240
  const timestamps = [0, 33333, 66666, 100000]

  // Create frames with specific timestamps
  const frames = timestamps.map((ts) =>
    generateSolidColorI420Frame(width, height, TestColors.blue, ts),
  )

  // Encode
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  encoder.encode(frames[0], { keyFrame: true })
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }

  for (const frame of frames) {
    frame.close()
  }

  encoder.flush()
  const chunks = encoder.takeEncodedChunks()
  encoder.close()

  // Verify chunk timestamps
  t.true(chunks.length > 0)

  // First chunk should have first timestamp
  t.is(chunks[0].timestamp, timestamps[0], 'First chunk should preserve timestamp')

  // All chunks should have timestamps from our input
  const chunkTimestamps = chunks.map((c) => c.timestamp)
  for (const ts of chunkTimestamps) {
    t.true(timestamps.includes(ts), `Chunk timestamp ${ts} should be from input timestamps`)
  }

  // Note: Decoded frame timestamps may differ from input due to codec reordering
  // This is expected behavior for some codecs (especially with B-frames)
})

// ============================================================================
// Keyframe Tests
// ============================================================================

test('roundtrip: keyframe generation', (t) => {
  const width = 320
  const height = 240

  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  // Encode with explicit keyframes
  const frame1 = generateSolidColorI420Frame(width, height, TestColors.red, 0)
  const frame2 = generateSolidColorI420Frame(width, height, TestColors.green, 33333)
  const frame3 = generateSolidColorI420Frame(width, height, TestColors.blue, 66666)

  encoder.encode(frame1, { keyFrame: true }) // Should be key
  encoder.encode(frame2) // Should be delta
  encoder.encode(frame3, { keyFrame: true }) // Request key

  frame1.close()
  frame2.close()
  frame3.close()

  encoder.flush()
  const chunks = encoder.takeEncodedChunks()
  encoder.close()

  // First chunk must be a keyframe
  t.is(chunks[0].type, EncodedVideoChunkType.Key, 'First chunk should be keyframe')

  // Count keyframes - note: codec may not honor all keyFrame requests
  // depending on GOP settings and B-frame configuration
  const keyframes = chunks.filter((c) => c.type === EncodedVideoChunkType.Key)
  t.true(keyframes.length >= 1, 'Should have at least 1 keyframe')
  t.log(`Generated ${keyframes.length} keyframes from ${chunks.length} chunks`)
})

// ============================================================================
// Pattern Tests
// ============================================================================

test('roundtrip: gradient pattern quality', (t) => {
  const width = 320
  const height = 240

  const originalFrame = generateGradientI420Frame(width, height, 0)
  const originalData = extractI420Data(originalFrame)

  // Encode
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', width, height, { quality: 'high' }))
  encoder.encode(originalFrame, { keyFrame: true })
  originalFrame.close()
  encoder.flush()

  const chunks = encoder.takeEncodedChunks()
  encoder.close()

  // Decode
  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  decoder.flush()

  const decodedFrames = decoder.takeDecodedFrames()
  t.true(decodedFrames.length > 0)

  // Compare
  const decodedData = extractI420Data(decodedFrames[0])
  const comparison = compareBuffers(originalData, decodedData)

  t.log(`Gradient PSNR: ${formatPSNR(comparison.psnr)}`)
  t.true(comparison.acceptable, 'Gradient pattern should survive roundtrip')

  for (const frame of decodedFrames) {
    frame.close()
  }
  decoder.close()
})

test('roundtrip: color bars pattern quality', (t) => {
  const width = 320
  const height = 240

  const originalFrame = generateColorBarsI420Frame(width, height, 0)
  const originalData = extractI420Data(originalFrame)

  // Encode
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', width, height, { quality: 'high' }))
  encoder.encode(originalFrame, { keyFrame: true })
  originalFrame.close()
  encoder.flush()

  const chunks = encoder.takeEncodedChunks()
  encoder.close()

  // Decode
  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  decoder.flush()

  const decodedFrames = decoder.takeDecodedFrames()
  t.true(decodedFrames.length > 0)

  // Compare
  const decodedData = extractI420Data(decodedFrames[0])
  const comparison = compareBuffers(originalData, decodedData)

  t.log(`Color bars PSNR: ${formatPSNR(comparison.psnr)}`)
  t.true(comparison.acceptable, 'Color bars pattern should survive roundtrip')

  for (const frame of decodedFrames) {
    frame.close()
  }
  decoder.close()
})

// ============================================================================
// Resolution Tests
// ============================================================================

test('roundtrip: various resolutions', (t) => {
  const resolutions = [
    { width: 128, height: 96 },
    { width: 320, height: 240 },
    { width: 640, height: 480 },
  ]

  for (const { width, height } of resolutions) {
    const originalFrame = generateSolidColorI420Frame(width, height, TestColors.cyan, 0)
    const originalData = extractI420Data(originalFrame)

    // Encode
    const encoder = new VideoEncoder()
    encoder.configure(createEncoderConfig('h264', width, height))
    encoder.encode(originalFrame, { keyFrame: true })
    originalFrame.close()
    encoder.flush()

    const chunks = encoder.takeEncodedChunks()
    encoder.close()

    // Decode
    const decoder = new VideoDecoder()
    decoder.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))

    for (const chunk of chunks) {
      decoder.decode(chunk)
    }
    decoder.flush()

    const decodedFrames = decoder.takeDecodedFrames()
    t.true(decodedFrames.length > 0, `Should decode at ${width}x${height}`)

    // Verify dimensions
    const decodedFrame = decodedFrames[0]
    t.is(decodedFrame.codedWidth, width, `Width should match at ${width}x${height}`)
    t.is(decodedFrame.codedHeight, height, `Height should match at ${width}x${height}`)

    // Verify quality
    const decodedData = extractI420Data(decodedFrame)
    const comparison = compareBuffers(originalData, decodedData)
    t.true(comparison.acceptable, `Quality should be acceptable at ${width}x${height}`)

    for (const frame of decodedFrames) {
      frame.close()
    }
    decoder.close()
  }
})

// ============================================================================
// Re-encoding Tests
// ============================================================================

test('roundtrip: re-encoding (double roundtrip)', (t) => {
  const width = 320
  const height = 240

  // First pass
  const original = generateSolidColorI420Frame(width, height, TestColors.magenta, 0)
  const originalData = extractI420Data(original)

  const encoder1 = new VideoEncoder()
  encoder1.configure(createEncoderConfig('h264', width, height))
  encoder1.encode(original, { keyFrame: true })
  original.close()
  encoder1.flush()

  const chunks1 = encoder1.takeEncodedChunks()
  encoder1.close()

  const decoder1 = new VideoDecoder()
  decoder1.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))
  for (const chunk of chunks1) {
    decoder1.decode(chunk)
  }
  decoder1.flush()

  const pass1Frames = decoder1.takeDecodedFrames()
  t.true(pass1Frames.length > 0)

  // Second pass (re-encode the decoded frame)
  const encoder2 = new VideoEncoder()
  encoder2.configure(createEncoderConfig('h264', width, height))
  encoder2.encode(pass1Frames[0], { keyFrame: true })

  for (const frame of pass1Frames) {
    frame.close()
  }
  decoder1.close()

  encoder2.flush()
  const chunks2 = encoder2.takeEncodedChunks()
  encoder2.close()

  const decoder2 = new VideoDecoder()
  decoder2.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))
  for (const chunk of chunks2) {
    decoder2.decode(chunk)
  }
  decoder2.flush()

  const pass2Frames = decoder2.takeDecodedFrames()
  t.true(pass2Frames.length > 0)

  // Compare final output to original
  const finalData = extractI420Data(pass2Frames[0])
  const comparison = compareBuffers(originalData, finalData)

  t.log(`Double roundtrip PSNR: ${formatPSNR(comparison.psnr)}`)
  // Quality will degrade with re-encoding, but should still be acceptable
  t.true(comparison.psnr >= 25, 'Double roundtrip should maintain reasonable quality')

  for (const frame of pass2Frames) {
    frame.close()
  }
  decoder2.close()
})
