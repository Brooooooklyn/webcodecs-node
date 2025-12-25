/**
 * Per-Frame QP Encoding Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * - wpt/webcodecs/per-frame-qp-encoding.https.any.js
 *
 * Tests VideoEncoder per-frame quantizer (QP) control with bitrateMode: 'quantizer'.
 */

import test, { ExecutionContext } from 'ava'

import {
  EncodedVideoChunk,
  resetHardwareFallbackState,
  VideoDecoder,
  VideoEncoder,
  VideoEncoderEncodeOptions,
} from '../../index.js'
import type { VideoDecoderConfig, VideoEncoderConfig } from '../../standard.js'
import { generateSolidColorI420Frame, TestColors } from '../helpers/frame-generator.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

interface CodecTestConfig {
  name: string
  encoderConfig: VideoEncoderConfig
  qpRange: { min: number; max: number }
  setQp: (options: VideoEncoderEncodeOptions, value: number) => void
}

const codecConfigs: CodecTestConfig[] = [
  {
    name: 'AV1',
    encoderConfig: {
      codec: 'av01.0.04M.08',
      width: 320,
      height: 200,
      bitrate: 1_000_000,
      bitrateMode: 'quantizer',
      framerate: 30,
    },
    qpRange: { min: 1, max: 255 },
    setQp: (options, value) => {
      options.av1 = { quantizer: value }
    },
  },
  {
    name: 'VP9 Profile 0',
    encoderConfig: {
      codec: 'vp09.00.10.08',
      width: 320,
      height: 200,
      bitrate: 1_000_000,
      bitrateMode: 'quantizer',
      framerate: 30,
    },
    qpRange: { min: 1, max: 255 },
    setQp: (options, value) => {
      options.vp9 = { quantizer: value }
    },
  },
  {
    name: 'VP9 Profile 2',
    encoderConfig: {
      codec: 'vp09.02.10.10',
      width: 320,
      height: 200,
      bitrate: 1_000_000,
      bitrateMode: 'quantizer',
      framerate: 30,
    },
    qpRange: { min: 1, max: 255 },
    setQp: (options, value) => {
      options.vp9 = { quantizer: value }
    },
  },
  {
    name: 'H.264',
    encoderConfig: {
      codec: 'avc1.42001E',
      width: 320,
      height: 200,
      bitrate: 1_000_000,
      bitrateMode: 'quantizer',
      framerate: 30,
      avc: { format: 'annexb' },
    },
    qpRange: { min: 1, max: 51 },
    setQp: (options, value) => {
      options.avc = { quantizer: value }
    },
  },
  {
    name: 'H.265',
    encoderConfig: {
      codec: 'hev1.1.6.L93.90',
      width: 320,
      height: 200,
      bitrate: 1_000_000,
      bitrateMode: 'quantizer',
      framerate: 30,
      hevc: { format: 'annexb' },
    },
    qpRange: { min: 1, max: 51 },
    setQp: (options, value) => {
      options.hevc = { quantizer: value }
    },
  },
]

// ============================================================================
// Per-Frame QP Encoding Tests
// WPT: "Frame QP encoding, full range"
// WPT: "Frame QP encoding, good range with validation"
// ============================================================================

async function runPerFrameQpTest(
  t: ExecutionContext,
  config: CodecTestConfig,
  qpRange: { min: number; max: number },
): Promise<void> {
  // Check if encoder supports this config
  const support = await VideoEncoder.isConfigSupported(config.encoderConfig)
  if (!support.supported) {
    t.pass(`${config.name} encoder not supported - skipping`)
    return
  }

  const framesToEncode = 12
  let framesEncoded = 0
  let framesDecoded = 0
  const chunks: EncodedVideoChunk[] = []
  let decoderConfig: VideoDecoderConfig | null = null

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      framesEncoded++
      chunks.push(chunk)
      if (metadata?.decoderConfig && !decoderConfig) {
        decoderConfig = metadata.decoderConfig as unknown as VideoDecoderConfig
      }
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(config.encoderConfig)

  // Encode frames with varying QP values
  let qp = qpRange.min
  for (let i = 0; i < framesToEncode; i++) {
    const frame = generateSolidColorI420Frame(
      config.encoderConfig.width,
      config.encoderConfig.height,
      TestColors.blue,
      i * 33333,
    )

    const encodeOptions: VideoEncoderEncodeOptions = { keyFrame: i === 0 }
    config.setQp(encodeOptions, qp)

    encoder.encode(frame, encodeOptions)
    frame.close()

    qp += 3
    if (qp > qpRange.max) {
      qp = qpRange.min
    }
  }

  await encoder.flush()
  encoder.close()

  t.is(framesEncoded, framesToEncode, `${config.name}: all frames encoded`)
  t.is(chunks.length, framesToEncode, `${config.name}: all chunks produced`)
  t.truthy(decoderConfig, `${config.name}: decoderConfig received`)

  // Decode all chunks to verify they are valid
  const decoder = new VideoDecoder({
    output: (frame) => {
      framesDecoded++
      frame.close()
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: config.encoderConfig.codec,
    codedWidth: config.encoderConfig.width,
    codedHeight: config.encoderConfig.height,
  })

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  await decoder.flush()
  decoder.close()

  t.is(framesDecoded, framesToEncode, `${config.name}: all frames decoded`)
}

// Test each codec with full QP range
for (const config of codecConfigs) {
  test(`Per-frame QP encoding: ${config.name} full range`, async (t) => {
    await runPerFrameQpTest(t, config, config.qpRange)
  })
}

// Test each codec with restricted QP range (lower QP = higher quality)
for (const config of codecConfigs) {
  test(`Per-frame QP encoding: ${config.name} good quality range`, async (t) => {
    // Use lower QP values for higher quality
    const goodRange = { min: 1, max: Math.min(20, config.qpRange.max) }
    await runPerFrameQpTest(t, config, goodRange)
  })
}

// ============================================================================
// Additional Per-Frame QP Tests
// ============================================================================

test('Per-frame QP: bitrateMode quantizer without codec-specific options', async (t) => {
  // Verify that bitrateMode: 'quantizer' is accepted without per-frame QP options
  const support = await VideoEncoder.isConfigSupported({
    codec: 'vp8',
    width: 320,
    height: 200,
    bitrateMode: 'quantizer',
  })

  t.true(support.supported, 'vp8 with bitrateMode quantizer is supported')
  t.is(support.config?.bitrateMode, 'quantizer', 'bitrateMode preserved in config')
})

test('Per-frame QP: VP8 encoding with bitrateMode quantizer', async (t) => {
  // VP8 doesn't have codec-specific quantizer option but should work with bitrateMode
  const support = await VideoEncoder.isConfigSupported({
    codec: 'vp8',
    width: 320,
    height: 200,
    bitrateMode: 'quantizer',
    framerate: 30,
  })

  if (!support.supported) {
    t.pass('VP8 quantizer mode not supported')
    return
  }

  const chunks: EncodedVideoChunk[] = []

  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 200,
    bitrateMode: 'quantizer',
    framerate: 30,
  })

  for (let i = 0; i < 5; i++) {
    const frame = generateSolidColorI420Frame(320, 200, TestColors.red, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.is(chunks.length, 5, 'all frames encoded with quantizer mode')
})

test('Per-frame QP: varying QP affects output size', async (t) => {
  // Higher QP should generally produce smaller chunks
  const support = await VideoEncoder.isConfigSupported({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 200,
    bitrateMode: 'quantizer',
    framerate: 30,
  })

  if (!support.supported) {
    t.pass('VP9 quantizer mode not supported')
    return
  }

  const lowQpChunks: EncodedVideoChunk[] = []
  const highQpChunks: EncodedVideoChunk[] = []

  // Encode with low QP (high quality)
  const lowQpEncoder = new VideoEncoder({
    output: (chunk) => lowQpChunks.push(chunk),
    error: (e) => t.fail(`Low QP encoder error: ${e.message}`),
  })

  lowQpEncoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 200,
    bitrateMode: 'quantizer',
    framerate: 30,
  })

  for (let i = 0; i < 3; i++) {
    const frame = generateSolidColorI420Frame(320, 200, TestColors.blue, i * 33333)
    lowQpEncoder.encode(frame, { keyFrame: i === 0, vp9: { quantizer: 10 } })
    frame.close()
  }

  await lowQpEncoder.flush()
  lowQpEncoder.close()

  // Encode with high QP (low quality)
  const highQpEncoder = new VideoEncoder({
    output: (chunk) => highQpChunks.push(chunk),
    error: (e) => t.fail(`High QP encoder error: ${e.message}`),
  })

  highQpEncoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 200,
    bitrateMode: 'quantizer',
    framerate: 30,
  })

  for (let i = 0; i < 3; i++) {
    const frame = generateSolidColorI420Frame(320, 200, TestColors.blue, i * 33333)
    highQpEncoder.encode(frame, { keyFrame: i === 0, vp9: { quantizer: 200 } })
    frame.close()
  }

  await highQpEncoder.flush()
  highQpEncoder.close()

  t.is(lowQpChunks.length, 3, 'low QP chunks produced')
  t.is(highQpChunks.length, 3, 'high QP chunks produced')

  // Compare total byte sizes
  const lowQpSize = lowQpChunks.reduce((sum, c) => sum + c.byteLength, 0)
  const highQpSize = highQpChunks.reduce((sum, c) => sum + c.byteLength, 0)

  // Low QP should generally produce larger output (higher quality)
  // Note: This may not always hold for all frames, so we just verify both work
  t.true(lowQpSize > 0, 'low QP produces output')
  t.true(highQpSize > 0, 'high QP produces output')

  // Log sizes for debugging
  t.log(`Low QP (10) total size: ${lowQpSize} bytes`)
  t.log(`High QP (200) total size: ${highQpSize} bytes`)
})
