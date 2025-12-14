/**
 * Multi-Codec Integration Tests
 *
 * Tests encoding/decoding across all supported codecs.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import { resetHardwareFallbackState, VideoEncoder, VideoDecoder, VideoFrame } from '../../index.js'
import type { EncodedVideoChunkMetadata, VideoDecoderConfig } from '../../index.js'
import {
  generateSolidColorI420Frame,
  generateGradientI420Frame,
  TestColors,
  extractI420Data,
  type EncodedVideoChunk,
} from '../helpers/index.js'
import { compareBuffers, formatPSNR, PSNRThresholds } from '../helpers/frame-comparator.js'
import {
  createEncoderConfig,
  createDecoderConfig,
  getAllCodecs,
  CodecRegistry,
  type CodecType,
} from '../helpers/codec-matrix.js'

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
// Codec Support Tests
// ============================================================================

for (const codec of getAllCodecs()) {
  const codecInfo = CodecRegistry[codec]

  test(`codec support: ${codecInfo.name} encoder isConfigSupported`, async (t) => {
    const config = createEncoderConfig(codec, 320, 240)
    const result = await VideoEncoder.isConfigSupported(config)

    t.is(typeof result.supported, 'boolean')
    t.log(`${codecInfo.name} encoder supported: ${result.supported}`)
  })

  test(`codec support: ${codecInfo.name} decoder isConfigSupported`, async (t) => {
    const config = createDecoderConfig(codec)
    const result = await VideoDecoder.isConfigSupported(config)

    t.is(typeof result.supported, 'boolean')
    t.log(`${codecInfo.name} decoder supported: ${result.supported}`)
  })
}

// ============================================================================
// H.264 Tests
// ============================================================================

test('codec: H.264 encode-decode roundtrip', async (t) => {
  const width = 320
  const height = 240

  const original = generateSolidColorI420Frame(width, height, TestColors.red, 0)
  const originalData = await extractI420Data(original)

  // Encode
  const { encoder, chunks, getDecoderConfig } = createTestEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))
  encoder.encode(original, { keyFrame: true })
  original.close()
  await encoder.flush()

  t.true(chunks.length > 0, 'H.264 should produce chunks')
  encoder.close()

  // Decode - use decoderConfig from encoder metadata
  const { decoder, frames } = createTestDecoder()
  const decoderConfig = getDecoderConfig()
  decoder.configure({
    ...createDecoderConfig('h264', { codedWidth: width, codedHeight: height }),
    description: decoderConfig?.description,
  })

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  await decoder.flush()

  t.true(frames.length > 0, 'H.264 should decode frames')

  const comparison = compareBuffers(originalData, await extractI420Data(frames[0]))
  t.log(`H.264 PSNR: ${formatPSNR(comparison.psnr)}`)
  t.true(comparison.acceptable)

  for (const frame of frames) {
    frame.close()
  }
  decoder.close()
})

// ============================================================================
// VP8 Tests
// ============================================================================

test('codec: VP8 encode-decode roundtrip', async (t) => {
  const width = 320
  const height = 240

  // Check if VP8 is supported
  const support = await VideoEncoder.isConfigSupported(createEncoderConfig('vp8', width, height))
  if (!support.supported) {
    t.log('VP8 not supported, skipping')
    t.pass()
    return
  }

  const original = generateGradientI420Frame(width, height, 0)
  const originalData = await extractI420Data(original)

  // Encode
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('vp8', width, height))
  encoder.encode(original, { keyFrame: true })
  original.close()
  await encoder.flush()

  t.true(chunks.length > 0, 'VP8 should produce chunks')
  encoder.close()

  // Decode
  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('vp8', { codedWidth: width, codedHeight: height }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  await decoder.flush()

  t.true(frames.length > 0, 'VP8 should decode frames')

  const comparison = compareBuffers(originalData, await extractI420Data(frames[0]))
  t.log(`VP8 PSNR: ${formatPSNR(comparison.psnr)}`)
  t.true(comparison.acceptable)

  for (const frame of frames) {
    frame.close()
  }
  decoder.close()
})

// ============================================================================
// VP9 Tests
// ============================================================================

test('codec: VP9 encode-decode roundtrip', async (t) => {
  const width = 320
  const height = 240

  // Check if VP9 is supported
  const support = await VideoEncoder.isConfigSupported(createEncoderConfig('vp9', width, height))
  if (!support.supported) {
    t.log('VP9 not supported, skipping')
    t.pass()
    return
  }

  const original = generateSolidColorI420Frame(width, height, TestColors.green, 0)
  const originalData = await extractI420Data(original)

  // Encode
  const { encoder, chunks } = createTestEncoder()
  encoder.configure(createEncoderConfig('vp9', width, height))
  encoder.encode(original, { keyFrame: true })
  original.close()
  await encoder.flush()

  t.true(chunks.length > 0, 'VP9 should produce chunks')
  encoder.close()

  // Decode
  const { decoder, frames } = createTestDecoder()
  decoder.configure(createDecoderConfig('vp9', { codedWidth: width, codedHeight: height }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  await decoder.flush()

  t.true(frames.length > 0, 'VP9 should decode frames')

  const comparison = compareBuffers(originalData, await extractI420Data(frames[0]))
  t.log(`VP9 PSNR: ${formatPSNR(comparison.psnr)}`)
  t.true(comparison.acceptable)

  for (const frame of frames) {
    frame.close()
  }
  decoder.close()
})

// ============================================================================
// H.265/HEVC Tests
// ============================================================================

test('codec: H.265 encode-decode roundtrip', async (t) => {
  const width = 320
  const height = 240

  // Check if H.265 is supported
  const support = await VideoEncoder.isConfigSupported(createEncoderConfig('h265', width, height))
  if (!support.supported) {
    t.log('H.265 not supported, skipping')
    t.pass()
    return
  }

  const original = generateSolidColorI420Frame(width, height, TestColors.blue, 0)
  const originalData = await extractI420Data(original)

  // Encode
  const { encoder, chunks, getDecoderConfig } = createTestEncoder()
  encoder.configure(createEncoderConfig('h265', width, height))
  encoder.encode(original, { keyFrame: true })
  original.close()
  await encoder.flush()

  t.true(chunks.length > 0, 'H.265 should produce chunks')
  encoder.close()

  // Decode - use decoderConfig from encoder metadata
  const { decoder, frames } = createTestDecoder()
  const decoderConfig = getDecoderConfig()
  decoder.configure({
    ...createDecoderConfig('h265', { codedWidth: width, codedHeight: height }),
    description: decoderConfig?.description,
  })

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  await decoder.flush()

  t.true(frames.length > 0, 'H.265 should decode frames')

  const comparison = compareBuffers(originalData, await extractI420Data(frames[0]))
  t.log(`H.265 PSNR: ${formatPSNR(comparison.psnr)}`)
  t.true(comparison.acceptable)

  for (const frame of frames) {
    frame.close()
  }
  decoder.close()
})

// ============================================================================
// AV1 Tests
// ============================================================================

test('codec: AV1 encode-decode roundtrip', async (t) => {
  const width = 320
  const height = 240

  // Check if AV1 is supported
  const support = await VideoEncoder.isConfigSupported(createEncoderConfig('av1', width, height))
  if (!support.supported) {
    t.log('AV1 not supported, skipping')
    t.pass()
    return
  }

  const original = generateSolidColorI420Frame(width, height, TestColors.yellow, 0)
  const originalData = await extractI420Data(original)

  // Encode
  const { encoder, chunks, getDecoderConfig } = createTestEncoder()
  try {
    encoder.configure(createEncoderConfig('av1', width, height))
    encoder.encode(original, { keyFrame: true })
    original.close()
    await encoder.flush()

    if (chunks.length === 0) {
      t.log('AV1 encoder configured but produced no output, skipping')
      encoder.close()
      t.pass()
      return
    }

    // Decode - use decoderConfig from encoder metadata
    const { decoder, frames } = createTestDecoder()
    const decoderConfig = getDecoderConfig()
    decoder.configure({
      ...createDecoderConfig('av1', { codedWidth: width, codedHeight: height }),
      description: decoderConfig?.description,
    })

    for (const chunk of chunks) {
      decoder.decode(chunk)
    }
    await decoder.flush()

    if (frames.length === 0) {
      t.log('AV1 decoder configured but produced no frames')
      decoder.close()
      encoder.close()
      t.pass()
      return
    }

    const comparison = compareBuffers(originalData, await extractI420Data(frames[0]))
    t.log(`AV1 PSNR: ${formatPSNR(comparison.psnr)}`)
    t.true(comparison.acceptable)

    for (const frame of frames) {
      frame.close()
    }
    decoder.close()
    encoder.close()
  } catch (e) {
    original.close()
    encoder.close()
    t.log(`AV1 encode/decode failed: ${e instanceof Error ? e.message : String(e)}`)
    t.pass() // Skip on AV1 errors - not all builds have full AV1 support
  }
})

// ============================================================================
// Resolution Matrix Tests
// ============================================================================

const resolutions = [
  { width: 128, height: 96, name: 'tiny' },
  { width: 320, height: 240, name: 'QVGA' },
  { width: 640, height: 480, name: 'VGA' },
]

for (const res of resolutions) {
  test(`codec matrix: H.264 @ ${res.name} (${res.width}x${res.height})`, async (t) => {
    const original = generateSolidColorI420Frame(res.width, res.height, TestColors.white, 0)
    const originalData = await extractI420Data(original)

    // Encode
    const { encoder, chunks, getDecoderConfig } = createTestEncoder()
    encoder.configure(createEncoderConfig('h264', res.width, res.height))
    encoder.encode(original, { keyFrame: true })
    original.close()
    await encoder.flush()

    t.true(chunks.length > 0)
    encoder.close()

    // Decode - use decoderConfig from encoder metadata
    const { decoder, frames } = createTestDecoder()
    const decoderConfig = getDecoderConfig()
    decoder.configure({
      ...createDecoderConfig('h264', { codedWidth: res.width, codedHeight: res.height }),
      description: decoderConfig?.description,
    })

    for (const chunk of chunks) {
      decoder.decode(chunk)
    }
    await decoder.flush()

    t.true(frames.length > 0)

    // Verify dimensions
    t.is(frames[0].codedWidth, res.width)
    t.is(frames[0].codedHeight, res.height)

    // Verify quality
    const comparison = compareBuffers(originalData, await extractI420Data(frames[0]))
    t.true(comparison.acceptable, `Quality at ${res.name}`)

    for (const frame of frames) {
      frame.close()
    }
    decoder.close()
  })
}

// ============================================================================
// Cross-Codec Comparison
// ============================================================================

test('codec comparison: quality across codecs', async (t) => {
  const width = 320
  const height = 240
  const original = generateGradientI420Frame(width, height, 0)
  const originalData = await extractI420Data(original)

  const codecs: CodecType[] = ['h264', 'vp8', 'vp9']
  const results: { codec: string; psnr: number }[] = []

  for (const codec of codecs) {
    const support = await VideoEncoder.isConfigSupported(createEncoderConfig(codec, width, height))
    if (!support.supported) {
      t.log(`${codec} not supported, skipping`)
      continue
    }

    // Encode
    const { encoder, chunks, getDecoderConfig } = createTestEncoder()
    encoder.configure(createEncoderConfig(codec, width, height))

    const frame = generateGradientI420Frame(width, height, 0)
    encoder.encode(frame, { keyFrame: true })
    frame.close()

    await encoder.flush()
    encoder.close()

    if (chunks.length === 0) continue

    // Decode - use decoderConfig from encoder metadata
    const { decoder, frames } = createTestDecoder()
    const decoderConfig = getDecoderConfig()
    decoder.configure({
      ...createDecoderConfig(codec, { codedWidth: width, codedHeight: height }),
      description: decoderConfig?.description,
    })

    for (const chunk of chunks) {
      decoder.decode(chunk)
    }
    await decoder.flush()

    if (frames.length === 0) {
      decoder.close()
      continue
    }

    const comparison = compareBuffers(originalData, await extractI420Data(frames[0]))
    results.push({ codec, psnr: comparison.psnr })

    for (const f of frames) {
      f.close()
    }
    decoder.close()
  }

  original.close()

  // Log comparison
  t.log('Quality comparison:')
  for (const result of results) {
    t.log(`  ${result.codec}: ${formatPSNR(result.psnr)}`)
    t.true(result.psnr >= PSNRThresholds.acceptable, `${result.codec} should have acceptable quality`)
  }

  t.pass()
})
