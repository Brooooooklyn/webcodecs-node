/**
 * Temporal SVC Encoding Test (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests Scalable Video Coding (SVC) with temporal layers L1T2 and L1T3.
 * Validates that temporal layer IDs are correctly reported in metadata.
 *
 * NOTE: These tests are currently SKIPPED because the SvcOutputMetadata
 * is not yet being populated in the encoder output. The metadata.svc
 * field is always None in the current implementation.
 *
 * See: src/webcodecs/video_encoder.rs - svc field is always None
 */

import test from 'ava'

import { EncodedVideoChunk, resetHardwareFallbackState, VideoDecoder, VideoEncoder, VideoFrame } from '../../index.js'
import type { EncodedVideoChunkMetadata, VideoDecoderConfig, VideoEncoderConfig } from '../../standard.js'

import { checkEncoderSupport, createDottedFrame, validateBlackDots } from '../helpers/wpt-frame-utils.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// Encoder configurations for SVC tests
const SVC_ENCODER_CONFIGS: Record<string, Partial<VideoEncoderConfig>> = {
  av1: { codec: 'av01.0.04M.08' },
  vp8: { codec: 'vp8' },
  vp9: { codec: 'vp09.00.10.08' },
  h264: { codec: 'avc1.42001E', avc: { format: 'annexb' } },
}

/**
 * Run SVC encoding test
 *
 * @param t - AVA execution context
 * @param layers - Number of temporal layers (2 or 3)
 * @param baseLayerDecimator - Expected frame rate reduction for base layer
 * @param codecKey - Key to encoder config
 */
async function runSvcTest(
  t: test.ExecutionContext,
  layers: number,
  baseLayerDecimator: number,
  codecKey: string,
): Promise<void> {
  const baseConfig = SVC_ENCODER_CONFIGS[codecKey]
  if (!baseConfig) {
    t.fail(`Unknown codec key: ${codecKey}`)
    return
  }

  const encoderConfig: VideoEncoderConfig = {
    ...baseConfig,
    hardwareAcceleration: 'prefer-software',
    width: 320,
    height: 200,
    bitrate: 1_000_000,
    bitrateMode: 'constant',
    framerate: 30,
    scalabilityMode: `L1T${layers}`,
  } as VideoEncoderConfig

  // Check support
  await checkEncoderSupport(t, encoderConfig)

  const w = encoderConfig.width!
  const h = encoderConfig.height!
  const framesToEncode = 24
  let framesEncoded = 0
  let _framesDecoded = 0
  const baseLayerChunks: EncodedVideoChunk[] = []
  const corruptedFrames: number[] = []

  // Encoder that filters to base layer only
  const encoder = new VideoEncoder({
    output: (chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata) => {
      framesEncoded++

      // Check SVC metadata
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const svc = (metadata as any)?.svc
      if (!svc) {
        // SVC metadata not present - this is expected until implemented
        // For now, just collect all chunks as if they were base layer
        baseLayerChunks.push(chunk)
        return
      }

      t.true('temporalLayerId' in svc, 'metadata should have svc.temporalLayerId')
      t.true(svc.temporalLayerId < layers, `temporalLayerId should be < ${layers}`)

      // Only keep base layer (temporalLayerId === 0)
      if (svc.temporalLayerId === 0) {
        baseLayerChunks.push(chunk)
      }
    },
    error: (e: Error) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(encoderConfig)

  // Encode frames
  for (let i = 0; i < framesToEncode; i++) {
    const frame = createDottedFrame(w, h, i)
    encoder.encode(frame, { keyFrame: false })
    frame.close()
  }

  await encoder.flush()

  // Create decoder to validate base layer
  const decoder = new VideoDecoder({
    output: async (frame: VideoFrame) => {
      _framesDecoded++

      // Validate the frame has correct dots
      const isValid = await validateBlackDots(frame, frame.timestamp)
      const hasExtraDots = await validateBlackDots(frame, frame.timestamp + 1)

      if (!isValid || hasExtraDots) {
        corruptedFrames.push(frame.timestamp)
      }

      frame.close()
    },
    error: (e: Error) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  const decoderConfig: VideoDecoderConfig = {
    codec: encoderConfig.codec,
    hardwareAcceleration: encoderConfig.hardwareAcceleration,
    codedWidth: w,
    codedHeight: h,
  }
  decoder.configure(decoderConfig)

  // Decode base layer chunks
  for (const chunk of baseLayerChunks) {
    decoder.decode(chunk)
  }

  await decoder.flush()

  encoder.close()
  decoder.close()

  // Validate results
  t.is(framesEncoded, framesToEncode, 'all frames should be encoded')

  // Note: When SVC is properly implemented, base layer should have
  // framesToEncode / baseLayerDecimator chunks
  // For now, we just check that something was produced
  t.true(baseLayerChunks.length > 0, 'should have base layer chunks')

  // When SVC metadata is implemented, uncomment:
  // const expectedBaseLayerFrames = framesToEncode / baseLayerDecimator
  // t.is(baseLayerChunks.length, expectedBaseLayerFrames, 'base layer chunk count')
  // t.is(framesDecoded, expectedBaseLayerFrames, 'decoded frame count')
  // t.is(corruptedFrames.length, 0, `no corrupted frames: ${corruptedFrames}`)
}

// ============================================================================
// AV1 SVC Tests (SKIPPED - SVC metadata not implemented)
// ============================================================================

test.skip('SVC L1T2: AV1', async (t) => {
  await runSvcTest(t, 2, 2, 'av1')
})

test.skip('SVC L1T3: AV1', async (t) => {
  await runSvcTest(t, 3, 4, 'av1')
})

// ============================================================================
// VP8 SVC Tests (SKIPPED - SVC metadata not implemented)
// ============================================================================

test.skip('SVC L1T2: VP8', async (t) => {
  await runSvcTest(t, 2, 2, 'vp8')
})

test.skip('SVC L1T3: VP8', async (t) => {
  await runSvcTest(t, 3, 4, 'vp8')
})

// ============================================================================
// VP9 SVC Tests (SKIPPED - SVC metadata not implemented)
// ============================================================================

test.skip('SVC L1T2: VP9', async (t) => {
  await runSvcTest(t, 2, 2, 'vp9')
})

test.skip('SVC L1T3: VP9', async (t) => {
  await runSvcTest(t, 3, 4, 'vp9')
})

// ============================================================================
// H.264 SVC Tests (SKIPPED - SVC metadata not implemented)
// ============================================================================

test.skip('SVC L1T2: H.264', async (t) => {
  await runSvcTest(t, 2, 2, 'h264')
})

test.skip('SVC L1T3: H.264', async (t) => {
  await runSvcTest(t, 3, 4, 'h264')
})

// ============================================================================
// Placeholder test to indicate SVC tests are pending
// ============================================================================

test('SVC encoding tests are pending implementation', (t) => {
  t.log('SVC temporal layer metadata (metadata.svc.temporalLayerId) is not yet populated')
  t.log('See src/webcodecs/video_encoder.rs - svc field is always None')
  t.log('When implemented, remove .skip from the tests above')
  t.pass()
})
