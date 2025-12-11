/**
 * VideoFrame Orientation Tests
 *
 * Tests W3C WebCodecs VideoFrame rotation and flip properties.
 */

import test from 'ava'

import { VideoFrame, VideoEncoder, VideoDecoder, EncodedVideoChunk } from '../index.js'
import { generateSolidColorI420Frame, TestColors } from './helpers/index.js'
import type { VideoFrameBufferInit, VideoDecoderConfig } from '../standard.js'

// Extended VideoFrameBufferInit with rotation and flip properties (Node.js extension)
type VideoFrameBufferInitWithOrientation = VideoFrameBufferInit & {
  rotation?: number
  flip?: boolean
}

// Extended VideoDecoderConfig with rotation and flip properties (Node.js extension)
type VideoDecoderConfigWithOrientation = VideoDecoderConfig & {
  rotation?: number
  flip?: boolean
}

// ============================================================================
// VideoFrame Rotation Property Tests
// ============================================================================

test('VideoFrame: rotation property defaults to 0', (t) => {
  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 1000)
  t.is(frame.rotation, 0)
  frame.close()
})

test('VideoFrame: constructor accepts rotation 0', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 0,
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.rotation, 0)
  frame.close()
})

test('VideoFrame: constructor accepts rotation 90', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 90,
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.rotation, 90)
  // Display dimensions are swapped for 90 degree rotation
  t.is(frame.displayWidth, 240)
  t.is(frame.displayHeight, 320)
  frame.close()
})

test('VideoFrame: constructor accepts rotation 180', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 180,
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.rotation, 180)
  // Display dimensions are NOT swapped for 180 degree rotation
  t.is(frame.displayWidth, 320)
  t.is(frame.displayHeight, 240)
  frame.close()
})

test('VideoFrame: constructor accepts rotation 270', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 270,
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.rotation, 270)
  // Display dimensions are swapped for 270 degree rotation
  t.is(frame.displayWidth, 240)
  t.is(frame.displayHeight, 320)
  frame.close()
})

test('VideoFrame: rotation is normalized to 0-359 range', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 360, // Should normalize to 0
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.rotation, 0)
  frame.close()
})

test('VideoFrame: rotation rounds to nearest 90 (ties to positive)', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  // 45 / 90 = 0.5, round(0.5) = 1 (ties towards positive infinity per W3C spec)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 45, // Should round to 90 (ties towards positive)
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.rotation, 90)
  frame.close()
})

test('VideoFrame: rotation rounds down for values below midpoint', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 44, // Should round to 0
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.rotation, 0)
  frame.close()
})

// ============================================================================
// VideoFrame Flip Property Tests
// ============================================================================

test('VideoFrame: flip property defaults to false', (t) => {
  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 1000)
  t.is(frame.flip, false)
  frame.close()
})

test('VideoFrame: constructor accepts flip true', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    flip: true,
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.flip, true)
  frame.close()
})

test('VideoFrame: constructor accepts flip false', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    flip: false,
  } as VideoFrameBufferInitWithOrientation)
  t.is(frame.flip, false)
  frame.close()
})

// ============================================================================
// VideoFrame constructor (from VideoFrame) Rotation Combination Tests
// ============================================================================

test('VideoFrame constructor (from VideoFrame): preserves rotation from source', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const source = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 90,
  } as VideoFrameBufferInitWithOrientation)

  const cloned = new VideoFrame(source, { timestamp: 2000 })
  t.is(cloned.rotation, 90)

  source.close()
  cloned.close()
})

test('VideoFrame constructor (from VideoFrame): combines rotations (add)', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const source = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 90,
  } as VideoFrameBufferInitWithOrientation)

  const cloned = new VideoFrame(source, {
    timestamp: 2000,
    rotation: 90, // 90 + 90 = 180
  } as VideoFrameBufferInitWithOrientation)
  t.is(cloned.rotation, 180)

  source.close()
  cloned.close()
})

test('VideoFrame constructor (from VideoFrame): rotation wraps at 360', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const source = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    rotation: 270,
  } as VideoFrameBufferInitWithOrientation)

  const cloned = new VideoFrame(source, {
    timestamp: 2000,
    rotation: 180, // 270 + 180 = 450 -> 90
  } as VideoFrameBufferInitWithOrientation)
  t.is(cloned.rotation, 90)

  source.close()
  cloned.close()
})

// ============================================================================
// VideoFrame constructor (from VideoFrame) Flip Combination Tests
// ============================================================================

test('VideoFrame constructor (from VideoFrame): preserves flip from source', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const source = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    flip: true,
  } as VideoFrameBufferInitWithOrientation)

  const cloned = new VideoFrame(source, { timestamp: 2000 })
  t.is(cloned.flip, true)

  source.close()
  cloned.close()
})

test('VideoFrame constructor (from VideoFrame): flip XOR logic (false XOR true = true)', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const source = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    flip: false,
  } as VideoFrameBufferInitWithOrientation)

  const cloned = new VideoFrame(source, {
    timestamp: 2000,
    flip: true,
  } as VideoFrameBufferInitWithOrientation)
  t.is(cloned.flip, true)

  source.close()
  cloned.close()
})

test('VideoFrame constructor (from VideoFrame): flip XOR logic (true XOR true = false)', (t) => {
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const source = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 1000,
    flip: true,
  } as VideoFrameBufferInitWithOrientation)

  const cloned = new VideoFrame(source, {
    timestamp: 2000,
    flip: true,
  } as VideoFrameBufferInitWithOrientation)
  t.is(cloned.flip, false)

  source.close()
  cloned.close()
})

// ============================================================================
// VideoDecoder Config Rotation/Flip Tests
// ============================================================================

test('VideoDecoder: config with rotation applies to output frames', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const frames: VideoFrame[] = []

  // First, encode some frames to get valid chunks
  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()
  encoder.close()

  t.true(chunks.length > 0, 'Should have encoded chunks')

  // Now decode with rotation config
  const decoder = new VideoDecoder({
    output: (f) => {
      frames.push(f)
    },
    error: (e) => t.fail(`Decoder error: ${e.message}`),
  })

  decoder.configure({
    codec: 'vp8',
    rotation: 90, // Set rotation in config
  } as VideoDecoderConfigWithOrientation)

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  await decoder.flush()
  decoder.close()

  t.true(frames.length > 0, 'Should have decoded frames')
  t.is(frames[0].rotation, 90, 'Decoded frame should have rotation from config')

  frames.forEach((f) => f.close())
})

test('VideoDecoder: config with flip applies to output frames', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const frames: VideoFrame[] = []

  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()
  encoder.close()

  const decoder = new VideoDecoder({
    output: (f) => {
      frames.push(f)
    },
    error: (e) => t.fail(`Decoder error: ${e.message}`),
  })

  decoder.configure({
    codec: 'vp8',
    flip: true, // Set flip in config
  } as VideoDecoderConfigWithOrientation)

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  await decoder.flush()
  decoder.close()

  t.true(frames.length > 0, 'Should have decoded frames')
  t.is(frames[0].flip, true, 'Decoded frame should have flip from config')

  frames.forEach((f) => f.close())
})

// ============================================================================
// VideoEncoder Rotation/Flip Metadata Tests
// ============================================================================

test('VideoEncoder: outputs rotation in metadata from input frame', async (t) => {
  let metadata: { rotation?: number; flip?: boolean } | undefined

  const encoder = new VideoEncoder({
    output: (_chunk, meta) => {
      if (meta?.decoderConfig) {
        metadata = {
          rotation: meta.decoderConfig.rotation,
          flip: meta.decoderConfig.flip,
        }
      }
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Create frame with rotation
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 0,
    rotation: 90,
  } as VideoFrameBufferInitWithOrientation)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()
  encoder.close()

  t.is(metadata?.rotation, 90, 'Metadata should include rotation from input frame')
})

test('VideoEncoder: outputs flip in metadata from input frame', async (t) => {
  let metadata: { rotation?: number; flip?: boolean } | undefined

  const encoder = new VideoEncoder({
    output: (_chunk, meta) => {
      if (meta?.decoderConfig) {
        metadata = {
          rotation: meta.decoderConfig.rotation,
          flip: meta.decoderConfig.flip,
        }
      }
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Create frame with flip
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 0,
    flip: true,
  } as VideoFrameBufferInitWithOrientation)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()
  encoder.close()

  t.is(metadata?.flip, true, 'Metadata should include flip from input frame')
})

// ============================================================================
// Roundtrip Tests
// ============================================================================

test('Roundtrip: rotation preserved through encode/decode cycle', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const decodedFrames: VideoFrame[] = []
  let decoderConfig: { rotation?: number } | undefined

  const encoder = new VideoEncoder({
    output: (chunk, meta) => {
      chunks.push(chunk)
      if (meta?.decoderConfig) {
        decoderConfig = { rotation: meta.decoderConfig.rotation }
      }
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Encode frame with rotation 180
  const data = new Uint8Array((320 * 240 * 3) / 2)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 320,
    codedHeight: 240,
    timestamp: 0,
    rotation: 180,
  } as VideoFrameBufferInitWithOrientation)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()
  encoder.close()

  t.true(chunks.length > 0, 'Should have encoded chunks')
  t.is(decoderConfig?.rotation, 180, 'Encoder should output rotation in metadata')

  // Decode with rotation from metadata
  const decoder = new VideoDecoder({
    output: (f) => {
      decodedFrames.push(f)
    },
    error: (e) => t.fail(`Decoder error: ${e.message}`),
  })

  decoder.configure({
    codec: 'vp8',
    rotation: decoderConfig?.rotation,
  } as VideoDecoderConfigWithOrientation)

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  await decoder.flush()
  decoder.close()

  t.true(decodedFrames.length > 0, 'Should have decoded frames')
  t.is(decodedFrames[0].rotation, 180, 'Decoded frame rotation should match original')

  decodedFrames.forEach((f) => f.close())
})
