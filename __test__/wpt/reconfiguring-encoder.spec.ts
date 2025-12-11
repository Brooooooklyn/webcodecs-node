/**
 * Reconfiguring Encoder Test (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests dynamic encoder reconfiguration with different resolutions and bitrates.
 * Validates that decoder config changes are correctly reflected in output.
 */

import test, { type ExecutionContext } from 'ava'

import {
  EncodedVideoChunk,
  resetHardwareFallbackState,
  VideoEncoder,
  type EncodedVideoChunkMetadata,
} from '../../index.js'
import type { VideoDecoderConfig, VideoEncoderConfig } from '../../standard.js'

import { checkEncoderSupport, createFrame } from '../helpers/wpt-frame-utils.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// Encoder configurations for reconfiguration tests
const RECONFIG_ENCODER_CONFIGS: Record<string, Partial<VideoEncoderConfig>> = {
  av1: { codec: 'av01.0.04M.08' },
  vp8: { codec: 'vp8' },
  vp9_p0: { codec: 'vp09.00.10.08' },
  vp9_p2: { codec: 'vp09.02.10.10' },
  h264_avc: { codec: 'avc1.42001F', avc: { format: 'avc' } },
  h264_annexb: { codec: 'avc1.42001F', avc: { format: 'annexb' } },
}

/**
 * Run reconfiguration test
 *
 * @param t - AVA execution context
 * @param codecKey - Key to encoder config
 */
async function runReconfigTest(t: ExecutionContext, codecKey: string): Promise<void> {
  const baseConfig = RECONFIG_ENCODER_CONFIGS[codecKey]
  if (!baseConfig) {
    t.fail(`Unknown codec key: ${codecKey}`)
    return
  }

  // Original configuration
  const originalW = 800
  const originalH = 600
  const originalBitrate = 3_000_000

  // New configuration after reconfigure
  const newW = 640
  const newH = 480
  const newBitrate = 2_000_000

  let nextTs = 0
  let reconfTs = 0
  const framesToEncode = 16
  let beforeReconfFrames = 0
  let afterReconfFrames = 0

  const params: VideoEncoderConfig = {
    ...baseConfig,
    hardwareAcceleration: 'prefer-software',
    bitrateMode: 'constant',
    framerate: 30,
    width: originalW,
    height: originalH,
    bitrate: originalBitrate,
  } as VideoEncoderConfig

  // Check support
  await checkEncoderSupport(t, params)

  const processVideoChunk = (chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata) => {
    const config = metadata?.decoderConfig as VideoDecoderConfig | undefined
    const afterReconf = reconfTs !== 0 && chunk.timestamp >= reconfTs

    if (afterReconf) {
      afterReconfFrames++
      if (config) {
        t.is(config.codedWidth, newW, 'after reconf: codedWidth')
        t.is(config.codedHeight, newH, 'after reconf: codedHeight')
      }
    } else {
      beforeReconfFrames++
      if (config) {
        t.is(config.codedWidth, originalW, 'before reconf: codedWidth')
        t.is(config.codedHeight, originalH, 'before reconf: codedHeight')
      }
    }
  }

  const encoder = new VideoEncoder({
    output: (chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata) => {
      try {
        processVideoChunk(chunk, metadata)
      } catch (e) {
        t.fail(`Chunk processing error: ${e instanceof Error ? e.message : String(e)}`)
      }
    },
    error: (e: Error) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(params)

  // Initial flush to ensure encoder is ready
  await encoder.flush()

  // Encode frames with original settings
  for (let i = 0; i < framesToEncode; i++) {
    const frame = createFrame(originalW, originalH, nextTs++)
    encoder.encode(frame, {})
    frame.close()
  }

  // Flush before reconfiguring to ensure all frames are processed
  await encoder.flush()

  // Reset and reconfigure encoder with new settings
  // Note: reset() is needed when changing dimensions to ensure clean state
  encoder.reset()
  params.width = newW
  params.height = newH
  params.bitrate = newBitrate

  encoder.configure(params)
  reconfTs = nextTs

  // Encode frames with new settings
  for (let i = 0; i < framesToEncode; i++) {
    const frame = createFrame(newW, newH, nextTs++)
    encoder.encode(frame, {})
    frame.close()
  }

  await encoder.flush()

  // Reset and configure back to original config to verify it works
  encoder.reset()
  params.width = originalW
  params.height = originalH
  params.bitrate = originalBitrate
  encoder.configure(params)
  await encoder.flush()

  encoder.close()

  // Validate results
  t.is(beforeReconfFrames, framesToEncode, 'before reconf frame count')
  t.is(afterReconfFrames, framesToEncode, 'after reconf frame count')
}

// ============================================================================
// AV1 Reconfiguration Tests
// ============================================================================

test.serial('Reconfiguring encoder: AV1', async (t) => {
  await runReconfigTest(t, 'av1')
})

// ============================================================================
// VP8 Reconfiguration Tests
// ============================================================================

test.serial('Reconfiguring encoder: VP8', async (t) => {
  await runReconfigTest(t, 'vp8')
})

// ============================================================================
// VP9 Reconfiguration Tests
// ============================================================================

test.serial('Reconfiguring encoder: VP9 Profile 0', async (t) => {
  await runReconfigTest(t, 'vp9_p0')
})

test.serial('Reconfiguring encoder: VP9 Profile 2', async (t) => {
  await runReconfigTest(t, 'vp9_p2')
})

// ============================================================================
// H.264 Reconfiguration Tests
// ============================================================================

test.serial('Reconfiguring encoder: H.264 AVC', async (t) => {
  await runReconfigTest(t, 'h264_avc')
})

test.serial('Reconfiguring encoder: H.264 Annex B', async (t) => {
  await runReconfigTest(t, 'h264_annexb')
})
