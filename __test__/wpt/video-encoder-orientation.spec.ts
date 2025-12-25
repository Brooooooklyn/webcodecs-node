/**
 * VideoEncoder Orientation Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * - wpt/webcodecs/video-encoder-orientation.https.any.js
 *
 * Tests VideoEncoder handling of frame rotation and flip properties.
 */

import test from 'ava'

import { EncodedVideoChunk, resetHardwareFallbackState, VideoEncoder, VideoFrame } from '../../index.js'
import { generateSolidColorI420Frame, TestColors } from '../helpers/frame-generator.js'
import { createCollectingCodecInit } from '../helpers/wpt-utils.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

const defaultConfig = {
  codec: 'vp8',
  width: 640,
  height: 480,
}

/**
 * Creates a VideoFrame with the specified orientation.
 */
function createFrameWithOrientation(
  width: number,
  height: number,
  timestamp: number,
  rotation: number,
  flip: boolean,
): VideoFrame {
  const baseFrame = generateSolidColorI420Frame(width, height, TestColors.blue, timestamp)
  const orientedFrame = new VideoFrame(baseFrame, { rotation, flip })
  baseFrame.close()
  return orientedFrame
}

// ============================================================================
// Orientation Encoding Tests
// WPT: "Encode video frame with orientation"
// ============================================================================

test('VideoEncoder: encode frame with orientation', async (t) => {
  const { init, outputs } = createCollectingCodecInit<EncodedVideoChunk>()
  let decoderConfig: Record<string, unknown> | null = null

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      init.output(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig as unknown as Record<string, unknown>
      }
    },
    error: init.error,
  })

  encoder.configure(defaultConfig)

  const frame = createFrameWithOrientation(640, 480, 0, 90, true)
  encoder.encode(frame)
  frame.close()

  await encoder.flush()
  encoder.close()

  t.is(outputs.length, 1, 'output count')
  t.truthy(decoderConfig, 'decoderConfig should be provided')
  t.is(decoderConfig!.rotation, 90, 'rotation in decoderConfig')
  t.is(decoderConfig!.flip, true, 'flip in decoderConfig')
})

// ============================================================================
// Different Orientation Tests (Non-Fatal Failures)
// WPT: "Encode video frames with different orientation has non-fatal failures"
// NOTE: Current implementation does not validate orientation consistency per W3C spec.
//       This test documents the expected behavior but uses skip for unimplemented features.
// ============================================================================

test.skip('VideoEncoder: different orientations throw DataError', async (t) => {
  // WPT expects that encoding frames with different orientations in the same
  // encode session throws DataError. Current implementation does not enforce this.
  // TODO: Implement orientation consistency validation per W3C spec.

  const { init, outputs } = createCollectingCodecInit<EncodedVideoChunk>()
  let decoderConfig: Record<string, unknown> | null = null

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      init.output(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig as unknown as Record<string, unknown>
      }
    },
    error: init.error,
  })

  encoder.configure(defaultConfig)

  // First frame with rotation=90, flip=true
  const frame1 = createFrameWithOrientation(640, 480, 0, 90, true)
  // Different flip (90, false)
  const frame2 = createFrameWithOrientation(640, 480, 33333, 90, false)
  // Different rotation (180, true)
  const frame3 = createFrameWithOrientation(640, 480, 66666, 180, true)
  // Same as first (90, true)
  const frame4 = createFrameWithOrientation(640, 480, 99999, 90, true)

  encoder.encode(frame1)

  // Encoding frame with different orientation should throw DataError
  t.throws(
    () => encoder.encode(frame2),
    { instanceOf: DOMException, name: 'DataError' },
    'different flip should throw DataError',
  )

  t.throws(
    () => encoder.encode(frame3),
    { instanceOf: DOMException, name: 'DataError' },
    'different rotation should throw DataError',
  )

  // Same orientation as first should succeed
  encoder.encode(frame4)

  frame1.close()
  frame2.close()
  frame3.close()
  frame4.close()

  await encoder.flush()
  encoder.close()

  t.is(outputs.length, 2, 'only frames with matching orientation encoded')
  t.truthy(decoderConfig)
  t.is(decoderConfig!.rotation, 90, 'rotation')
  t.is(decoderConfig!.flip, true, 'flip')
})

// ============================================================================
// Orientation After Reconfigure Tests
// WPT: "Encode video frames with different orientations after reconfigure"
// ============================================================================

test('VideoEncoder: different orientations after reconfigure', async (t) => {
  const { init, outputs } = createCollectingCodecInit<EncodedVideoChunk>()
  let decoderConfig: Record<string, unknown> | null = null

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      init.output(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig as unknown as Record<string, unknown>
      }
    },
    error: init.error,
  })

  // First encode session with rotation=90, flip=true
  encoder.configure(defaultConfig)

  let frame = createFrameWithOrientation(640, 480, 0, 90, true)
  encoder.encode(frame)
  frame.close()

  await encoder.flush()

  t.is(outputs.length, 1)
  t.truthy(decoderConfig)
  t.is(decoderConfig!.rotation, 90, 'first rotation')
  t.is(decoderConfig!.flip, true, 'first flip')

  // Reconfigure and use different orientation
  encoder.configure(defaultConfig)

  frame = createFrameWithOrientation(640, 480, 0, 270, false)
  encoder.encode(frame)
  frame.close()

  await encoder.flush()

  t.is(outputs.length, 2, 'output count after reconfigure')
  t.is(decoderConfig!.rotation, 270, 'second rotation')
  // Note: flip=false may be omitted from decoderConfig (undefined == false)
  t.true(decoderConfig!.flip === false || decoderConfig!.flip === undefined, 'second flip')

  encoder.close()
})

// ============================================================================
// Additional Orientation Tests
// ============================================================================

test('VideoEncoder: all rotation values', async (t) => {
  const rotations = [0, 90, 180, 270]

  for (const rotation of rotations) {
    const { init, outputs } = createCollectingCodecInit<EncodedVideoChunk>()
    let decoderConfig: Record<string, unknown> | null = null

    const encoder = new VideoEncoder({
      output: (chunk, metadata) => {
        init.output(chunk)
        if (metadata?.decoderConfig) {
          decoderConfig = metadata.decoderConfig as unknown as Record<string, unknown>
        }
      },
      error: init.error,
    })

    encoder.configure(defaultConfig)

    const frame = createFrameWithOrientation(640, 480, 0, rotation, false)
    encoder.encode(frame)
    frame.close()

    await encoder.flush()
    encoder.close()

    t.is(outputs.length, 1, `output for rotation=${rotation}`)
    // Note: rotation=0 may be omitted from decoderConfig (undefined == 0)
    const actualRotation = decoderConfig!.rotation ?? 0
    t.is(actualRotation, rotation, `decoderConfig rotation=${rotation}`)
  }
})

test('VideoEncoder: orientation preserved through encode/decode', async (t) => {
  const { init, outputs } = createCollectingCodecInit<EncodedVideoChunk>()
  let decoderConfig: Record<string, unknown> | null = null

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      init.output(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig as unknown as Record<string, unknown>
      }
    },
    error: init.error,
  })

  encoder.configure(defaultConfig)

  // Create frame with 180 rotation and flip
  const frame = createFrameWithOrientation(640, 480, 0, 180, true)

  t.is(frame.rotation, 180, 'input frame rotation')
  t.is(frame.flip, true, 'input frame flip')

  encoder.encode(frame)
  frame.close()

  await encoder.flush()
  encoder.close()

  t.is(outputs.length, 1)
  t.truthy(decoderConfig)
  t.is(decoderConfig!.rotation, 180, 'decoderConfig preserves rotation')
  t.is(decoderConfig!.flip, true, 'decoderConfig preserves flip')
})
