/**
 * Full Cycle Test (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/nicosurjana/nicosurjana.git
 *
 * Tests end-to-end encoding and decoding cycles for all supported codecs.
 * Validates that frames can be encoded and then decoded with correct properties.
 */

import test from 'ava'

import { EncodedVideoChunk, resetHardwareFallbackState, VideoDecoder, VideoEncoder, VideoFrame } from '../../index.js'
import type {
  EncodedVideoChunkMetadata,
  VideoDecoderConfig,
  VideoEncoderConfig,
  VideoEncoderEncodeOptions,
} from '../../standard.js'

import {
  checkEncoderSupport,
  createDottedFrame,
  createEncoderConfig,
  ENCODER_CONFIGS,
  validateBlackDots,
} from '../helpers/wpt-frame-utils.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

interface FullCycleOptions {
  realTimeLatencyMode?: boolean
  stripDecoderConfigColorSpace?: boolean
  rateControl?: boolean
}

/**
 * Run a full encode/decode cycle test
 */
async function runFullCycleTest(
  t: test.ExecutionContext,
  encoderConfig: VideoEncoderConfig & { hasEmbeddedColorSpace?: boolean },
  options: FullCycleOptions = {},
): Promise<void> {
  // Check support first
  await checkEncoderSupport(t, encoderConfig)

  const config = { ...encoderConfig }
  if (options.realTimeLatencyMode) {
    config.latencyMode = 'realtime'
  }

  const w = config.width!
  const h = config.height!
  const framesToEncode = 16
  let framesEncoded = 0
  let framesDecoded = 0
  let nextTs = 0
  let encoderColorSpace: {
    primaries?: string | null
    transfer?: string | null
    matrix?: string | null
    fullRange?: boolean | null
  } = {}

  // Track frames for cleanup
  const framesToClose: VideoFrame[] = []
  let decoderConfigured = false

  // Create decoder
  const decoder = new VideoDecoder({
    output: (frame: VideoFrame) => {
      framesToClose.push(frame)

      t.is(frame.visibleRect?.width, w, 'visibleRect.width')
      t.is(frame.visibleRect?.height, h, 'visibleRect.height')

      if (!options.realTimeLatencyMode) {
        t.is(frame.timestamp, nextTs++, 'decode timestamp')
      }

      // Verify color space matches encoder output
      if (encoderColorSpace.primaries !== undefined) {
        t.is(frame.colorSpace.primaries, encoderColorSpace.primaries, 'colorSpace.primaries')
        t.is(frame.colorSpace.transfer, encoderColorSpace.transfer, 'colorSpace.transfer')
        t.is(frame.colorSpace.matrix, encoderColorSpace.matrix, 'colorSpace.matrix')
        t.is(frame.colorSpace.fullRange, encoderColorSpace.fullRange, 'colorSpace.fullRange')
      }

      framesDecoded++
    },
    error: (e: Error) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  let nextEncodeTs = 0
  const encoder = new VideoEncoder({
    output: (chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata) => {
      const decoderConfig = metadata?.decoderConfig as VideoDecoderConfig | undefined

      // Configure decoder on first chunk or when config changes
      if (decoderConfig) {
        const configWithHw: VideoDecoderConfig = {
          ...decoderConfig,
          hardwareAcceleration: config.hardwareAcceleration,
        }

        encoderColorSpace = decoderConfig.colorSpace || {}

        // Strip color space if testing embedded bitstream color space
        if (options.stripDecoderConfigColorSpace) {
          configWithHw.colorSpace = {}
        }

        decoder.configure(configWithHw)
        decoderConfigured = true
      } else if (!decoderConfigured) {
        // Fallback: configure decoder from encoder config if no decoderConfig provided
        const fallbackConfig: VideoDecoderConfig = {
          codec: config.codec,
          codedWidth: w,
          codedHeight: h,
          hardwareAcceleration: config.hardwareAcceleration,
        }
        decoder.configure(fallbackConfig)
        decoderConfigured = true
      }

      decoder.decode(chunk)
      framesEncoded++

      if (!options.realTimeLatencyMode) {
        t.is(chunk.timestamp, nextEncodeTs++, 'encode timestamp')
      }
    },
    error: (e: Error) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(config)

  // Encode frames
  for (let i = 0; i < framesToEncode; i++) {
    const frame = createDottedFrame(w, h, i)

    // Verify frame has valid color space
    t.not(frame.colorSpace.primaries, null, 'frame colorSpace.primaries')
    t.not(frame.colorSpace.transfer, null, 'frame colorSpace.transfer')
    t.not(frame.colorSpace.matrix, null, 'frame colorSpace.matrix')
    t.not(frame.colorSpace.fullRange, null, 'frame colorSpace.fullRange')

    const keyframe = i % 5 === 0
    const encodeOptions: VideoEncoderEncodeOptions = { keyFrame: keyframe }

    encoder.encode(frame, encodeOptions)

    // Apply rate control if requested
    if (i % 3 === 0 && options.rateControl) {
      config.bitrate = Math.floor((config.bitrate || 1_000_000) * 0.9)
      encoder.configure(config)
    }

    frame.close()
  }

  await encoder.flush()
  await decoder.flush()

  encoder.close()
  decoder.close()

  // Cleanup frames
  for (const frame of framesToClose) {
    frame.close()
  }

  // Validate results
  if (options.realTimeLatencyMode) {
    t.true(framesEncoded > 0, 'frames_encoded > 0')
  } else {
    t.is(framesEncoded, framesToEncode, 'frames_encoded')
  }
  t.is(framesDecoded, framesEncoded, 'frames_decoded')
}

// ============================================================================
// AV1 Tests
// ============================================================================

test.serial('Full cycle: AV1 - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('av1')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.av1.hasEmbeddedColorSpace })
})

test.serial('Full cycle: AV1 - realtime latency mode', async (t) => {
  const config = createEncoderConfig('av1')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { realTimeLatencyMode: true })
})

test.serial('Full cycle: AV1 - stripped color space', async (t) => {
  const config = createEncoderConfig('av1')
  if (ENCODER_CONFIGS.av1.hasEmbeddedColorSpace) {
    await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { stripDecoderConfigColorSpace: true })
  } else {
    t.pass('Skipped: codec does not have embedded color space')
  }
})

// Skip: Rate control with mid-stream reconfiguration is covered in reconfiguring-encoder.spec.ts
test.skip('Full cycle: AV1 - rate control', async (t) => {
  const config = createEncoderConfig('av1')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { rateControl: true })
})

// ============================================================================
// VP8 Tests
// ============================================================================

test.serial('Full cycle: VP8 - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('vp8')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.vp8.hasEmbeddedColorSpace })
})

test.serial('Full cycle: VP8 - realtime latency mode', async (t) => {
  const config = createEncoderConfig('vp8')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: false }, { realTimeLatencyMode: true })
})

// Skip: Rate control with mid-stream reconfiguration is covered in reconfiguring-encoder.spec.ts
test.skip('Full cycle: VP8 - rate control', async (t) => {
  const config = createEncoderConfig('vp8')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: false }, { rateControl: true })
})

// ============================================================================
// VP9 Tests
// ============================================================================

test.serial('Full cycle: VP9 Profile 0 - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('vp9_p0')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.vp9_p0.hasEmbeddedColorSpace })
})

test.serial('Full cycle: VP9 Profile 0 - realtime latency mode', async (t) => {
  const config = createEncoderConfig('vp9_p0')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { realTimeLatencyMode: true })
})

test.serial('Full cycle: VP9 Profile 0 - stripped color space', async (t) => {
  const config = createEncoderConfig('vp9_p0')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { stripDecoderConfigColorSpace: true })
})

// Skip: Rate control with mid-stream reconfiguration is covered in reconfiguring-encoder.spec.ts
test.skip('Full cycle: VP9 Profile 0 - rate control', async (t) => {
  const config = createEncoderConfig('vp9_p0')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { rateControl: true })
})

test.serial('Full cycle: VP9 Profile 2 (10-bit) - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('vp9_p2')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.vp9_p2.hasEmbeddedColorSpace })
})

// ============================================================================
// H.264 Tests
// ============================================================================

test.serial('Full cycle: H.264 AVC - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('h264_avc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.h264_avc.hasEmbeddedColorSpace })
})

test.serial('Full cycle: H.264 AVC - realtime latency mode', async (t) => {
  const config = createEncoderConfig('h264_avc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { realTimeLatencyMode: true })
})

test.serial('Full cycle: H.264 AVC - stripped color space', async (t) => {
  const config = createEncoderConfig('h264_avc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { stripDecoderConfigColorSpace: true })
})

// Skip: Rate control with mid-stream reconfiguration is covered in reconfiguring-encoder.spec.ts
test.skip('Full cycle: H.264 AVC - rate control', async (t) => {
  const config = createEncoderConfig('h264_avc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { rateControl: true })
})

test.serial('Full cycle: H.264 Annex B - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('h264_annexb')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.h264_annexb.hasEmbeddedColorSpace })
})

test.serial('Full cycle: H.264 Annex B - realtime latency mode', async (t) => {
  const config = createEncoderConfig('h264_annexb')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { realTimeLatencyMode: true })
})

// ============================================================================
// H.265 Tests
// ============================================================================

test.serial('Full cycle: H.265 HEVC - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('h265_hevc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.h265_hevc.hasEmbeddedColorSpace })
})

test.serial('Full cycle: H.265 HEVC - realtime latency mode', async (t) => {
  const config = createEncoderConfig('h265_hevc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { realTimeLatencyMode: true })
})

test.serial('Full cycle: H.265 HEVC - stripped color space', async (t) => {
  const config = createEncoderConfig('h265_hevc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { stripDecoderConfigColorSpace: true })
})

// Skip: Rate control with mid-stream reconfiguration is covered in reconfiguring-encoder.spec.ts
test.skip('Full cycle: H.265 HEVC - rate control', async (t) => {
  const config = createEncoderConfig('h265_hevc')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { rateControl: true })
})

test.serial('Full cycle: H.265 Annex B - basic encoding and decoding', async (t) => {
  const config = createEncoderConfig('h265_annexb')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: ENCODER_CONFIGS.h265_annexb.hasEmbeddedColorSpace })
})

test.serial('Full cycle: H.265 Annex B - realtime latency mode', async (t) => {
  const config = createEncoderConfig('h265_annexb')
  await runFullCycleTest(t, { ...config, hasEmbeddedColorSpace: true }, { realTimeLatencyMode: true })
})
