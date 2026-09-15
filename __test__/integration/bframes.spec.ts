/**
 * B-frame Timestamp Attribution Integration Tests
 *
 * With B-frames enabled (quality latencyMode), encoders emit chunks in decode
 * order. Each EncodedVideoChunk must carry the presentation timestamp of its
 * own frame (GitHub issue #28), not the Nth input timestamp in FIFO order.
 * Symmetrically, the decoder must pair output frames with the timestamp
 * propagated through the bitstream, not a FIFO of input chunk timestamps.
 */

import test from 'ava'
import type { ExecutionContext } from 'ava'

import {
  EncodedVideoChunk,
  resetHardwareFallbackState,
  VideoEncoder,
  VideoDecoder,
  VideoFrame,
} from '../../index.js'
import type { EncodedVideoChunkMetadata, VideoDecoderConfig, VideoEncoderConfig } from '../../index.js'
import { hasHardwareAcceleration } from '../helpers/index.js'

// Reset hardware fallback state before each test to ensure test isolation
test.beforeEach(() => {
  resetHardwareFallbackState()
})

const WIDTH = 320
const HEIGHT = 240
const FRAME_COUNT = 24
// 30fps timestamps in microseconds
const TIMESTAMPS = Array.from({ length: FRAME_COUNT }, (_, i) => Math.round((i * 1_000_000) / 30))

const HEVC_CONFIG = {
  codec: 'hev1.1.6.L93.B0',
  width: WIDTH,
  height: HEIGHT,
  bitrate: 2_000_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-software' as const,
  latencyMode: 'quality' as const,
}

/** Generate an I420 frame whose Y plane encodes the frame index (Y = i*10 % 256) */
function generateIndexFrame(index: number, timestamp: number): VideoFrame {
  const ySize = WIDTH * HEIGHT
  const uvSize = (WIDTH / 2) * (HEIGHT / 2)
  const buffer = new Uint8Array(ySize + uvSize * 2)
  buffer.fill((index * 10) % 256, 0, ySize)
  buffer.fill(128, ySize)

  return new VideoFrame(buffer, {
    format: 'I420',
    codedWidth: WIDTH,
    codedHeight: HEIGHT,
    timestamp,
  })
}

function createTestEncoder() {
  const chunks: EncodedVideoChunk[] = []
  const errors: Error[] = []
  let decoderConfig: VideoDecoderConfig | undefined

  const encoder = new VideoEncoder({
    output: (chunk, metadata?: EncodedVideoChunkMetadata) => {
      chunks.push(chunk)
      if (!decoderConfig && metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig as VideoDecoderConfig
      }
    },
    error: (e) => errors.push(e),
  })

  return { encoder, chunks, errors, getDecoderConfig: () => decoderConfig }
}

function createTestDecoder() {
  const frames: VideoFrame[] = []
  const errors: Error[] = []

  const decoder = new VideoDecoder({
    output: (frame) => frames.push(frame),
    error: (e) => errors.push(e),
  })

  return { decoder, frames, errors }
}

async function encodeIndexFrames(config: VideoEncoderConfig) {
  const { encoder, chunks, errors, getDecoderConfig } = createTestEncoder()
  encoder.configure(config)

  for (let i = 0; i < FRAME_COUNT; i++) {
    const frame = generateIndexFrame(i, TIMESTAMPS[i])
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  return { chunks, errors, decoderConfig: getDecoderConfig() }
}

function assertChunkTimestampsAttributable(
  t: ExecutionContext,
  chunks: EncodedVideoChunk[],
  errors: Error[],
) {
  t.is(errors.length, 0, `No encoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(chunks.length, FRAME_COUNT, 'One chunk per input frame')

  // Every chunk must carry the presentation timestamp of its own frame:
  // the multiset of chunk timestamps equals the multiset of input timestamps.
  const chunkTimestamps = chunks.map((c) => c.timestamp)
  t.deepEqual(
    [...chunkTimestamps].sort((a, b) => a - b),
    [...TIMESTAMPS].sort((a, b) => a - b),
    'Chunk timestamps must be a permutation of the input timestamps',
  )

  // Emission is decode order, so under B-frame reordering the per-frame
  // timestamps are NOT monotonically non-decreasing. A FIFO-labeled encoder
  // would emit strictly increasing timestamps attached to the wrong frames.
  const monotonic = chunkTimestamps.every((ts, i) => i === 0 || ts >= chunkTimestamps[i - 1])
  t.false(monotonic, 'B-frame reordering should emit chunks out of presentation order')

  // Decode order starts with the keyframe (presentation timestamp 0)
  t.is(chunks[0].timestamp, 0)
  t.is(chunks[0].type, 'key')
}

test('encoder labels each chunk with its own PTS under B-frame reordering', async (t) => {
  const { chunks, errors } = await encodeIndexFrames(HEVC_CONFIG)
  assertChunkTimestampsAttributable(t, chunks, errors)
})

test('roundtrip preserves presentation order and content with B-frames', async (t) => {
  const { chunks, errors: encErrors, decoderConfig } = await encodeIndexFrames(HEVC_CONFIG)
  t.is(encErrors.length, 0, 'No encoder errors')
  t.is(chunks.length, FRAME_COUNT, 'One chunk per input frame')

  const { decoder, frames, errors } = createTestDecoder()
  decoder.configure({
    codec: 'hev1.1.6.L93.B0',
    codedWidth: WIDTH,
    codedHeight: HEIGHT,
    hardwareAcceleration: 'prefer-software',
    description: decoderConfig?.description,
  })

  // Feed chunks in emission (decode) order
  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  await decoder.flush()
  decoder.close()

  t.is(errors.length, 0, `No decoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(frames.length, FRAME_COUNT, 'One output frame per input frame')

  // Decoder outputs in presentation order: timestamps strictly increasing,
  // exactly equal to the input timestamps.
  t.deepEqual(
    frames.map((f) => f.timestamp),
    [...TIMESTAMPS],
    'Output frame timestamps must equal input timestamps in presentation order',
  )

  // Verify content: average Y matches the frame index encoded in the Y plane.
  // x265 is lossy; at 2 Mbps for 320x240 the error stays well within +-2.
  for (const frame of frames) {
    const frameIndex = Math.round((frame.timestamp * 30) / 1_000_000)
    const data = new Uint8Array(frame.allocationSize())
    await frame.copyTo(data)

    const yPlane = data.subarray(0, WIDTH * HEIGHT)
    let sum = 0
    for (const value of yPlane) {
      sum += value
    }
    const averageY = sum / yPlane.length
    const expectedY = (frameIndex * 10) % 256

    t.true(
      Math.abs(averageY - expectedY) <= 2,
      `Frame ${frameIndex} (ts=${frame.timestamp}) average Y ${averageY.toFixed(2)} should be ~${expectedY}`,
    )
    frame.close()
  }
})

test('decoder pairs duration with the same frame as its timestamp', async (t) => {
  const { chunks, errors: encErrors, decoderConfig } = await encodeIndexFrames(HEVC_CONFIG)
  t.is(encErrors.length, 0, 'No encoder errors')
  t.is(chunks.length, FRAME_COUNT, 'One chunk per input frame')

  // Re-wrap chunks through the public API with per-chunk durations:
  // explicit 0, explicit 33333µs, and omitted — the pattern external
  // consumers produce when relaying encoder output.
  const durationByTimestamp = new Map<number, number | undefined>()
  const rewrapped = chunks.map((chunk, i) => {
    const data = new Uint8Array(chunk.byteLength)
    chunk.copyTo(data)
    const duration = i % 3 === 0 ? 0 : i % 3 === 1 ? 33333 : undefined
    durationByTimestamp.set(chunk.timestamp, duration)
    return new EncodedVideoChunk({
      type: chunk.type,
      timestamp: chunk.timestamp,
      ...(duration !== undefined ? { duration } : {}),
      data,
    })
  })

  const { decoder, frames, errors } = createTestDecoder()
  decoder.configure({
    codec: 'hev1.1.6.L93.B0',
    codedWidth: WIDTH,
    codedHeight: HEIGHT,
    hardwareAcceleration: 'prefer-software',
    description: decoderConfig?.description,
  })

  // Decode order: chunks arrive reordered, frames exit in presentation order.
  for (const chunk of rewrapped) {
    decoder.decode(chunk)
  }
  await decoder.flush()
  decoder.close()

  t.is(errors.length, 0, `No decoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(frames.length, FRAME_COUNT, 'One output frame per input frame')

  for (const frame of frames) {
    const expected = durationByTimestamp.get(frame.timestamp)
    t.is(
      frame.duration ?? undefined,
      expected,
      `Frame ts=${frame.timestamp} must carry its own chunk's duration (${expected}), not a neighbor's`,
    )
    frame.close()
  }
})

test('decoder pairs duration with the exact chunk under duplicate timestamps', async (t) => {
  const { chunks, errors: encErrors, decoderConfig } = await encodeIndexFrames(HEVC_CONFIG)
  t.is(encErrors.length, 0, 'No encoder errors')
  t.is(chunks.length, FRAME_COUNT, 'One chunk per input frame')

  // Force the chunks carrying content frames 3 (Y≈30) and 4 (Y≈40) to share
  // one timestamp, with different durations (0 and 50000). Identity through
  // reordering is then unrecoverable from PTS alone — this is the SVC/field
  // picture case. Chunks arrive in decode order: ts 100000 is content 3,
  // ts 133333 is content 4.
  const DUP_TS = TIMESTAMPS[3]
  const expectedByContent = new Map<number, { ts: number; duration: number | undefined }>()
  const rewrapped = chunks.map((chunk) => {
    const data = new Uint8Array(chunk.byteLength)
    chunk.copyTo(data)
    const contentIndex = Math.round((chunk.timestamp * 30) / 1_000_000)
    let ts = chunk.timestamp
    let duration: number | undefined = 33333
    if (chunk.timestamp === TIMESTAMPS[3]) {
      ts = DUP_TS
      duration = 0 // content frame 3
    } else if (chunk.timestamp === TIMESTAMPS[4]) {
      ts = DUP_TS
      duration = 50000 // content frame 4
    }
    expectedByContent.set(contentIndex, { ts, duration })
    return new EncodedVideoChunk({
      type: chunk.type,
      timestamp: ts,
      duration,
      data,
    })
  })

  const { decoder, frames, errors } = createTestDecoder()
  decoder.configure({
    codec: 'hev1.1.6.L93.B0',
    codedWidth: WIDTH,
    codedHeight: HEIGHT,
    hardwareAcceleration: 'prefer-software',
    description: decoderConfig?.description,
  })

  for (const chunk of rewrapped) {
    decoder.decode(chunk)
  }
  await decoder.flush()
  decoder.close()

  t.is(errors.length, 0, `No decoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(frames.length, FRAME_COUNT, 'One output frame per input frame')

  // Correlate by pixel content, not timestamp: the two duplicate-timestamp
  // frames are indistinguishable by ts, which is the point of this test.
  for (const frame of frames) {
    const data = new Uint8Array(frame.allocationSize())
    await frame.copyTo(data)
    const yPlane = data.subarray(0, WIDTH * HEIGHT)
    let sum = 0
    for (const value of yPlane) {
      sum += value
    }
    const averageY = sum / yPlane.length
    const contentIndex = Math.round(averageY / 10)
    const expected = expectedByContent.get(contentIndex)
    t.truthy(expected, `frame with Y≈${averageY.toFixed(1)} maps to content frame ${contentIndex}`)
    t.is(frame.timestamp, expected!.ts, `content frame ${contentIndex} timestamp`)
    t.is(
      frame.duration ?? undefined,
      expected!.duration,
      `content frame ${contentIndex} (Y≈${averageY.toFixed(1)}) must carry duration ${expected!.duration}, not the other duplicate's`,
    )
    frame.close()
  }
})

// VideoToolbox B-frame attribution (macOS only)
const testOnDarwin = process.platform === 'darwin' ? test : test.skip

testOnDarwin('VideoToolbox hardware encoder B-frame attribution', async (t) => {
  if (!hasHardwareAcceleration()) {
    t.pass('No usable hardware accelerator available, skipping')
    return
  }

  const { chunks, errors } = await encodeIndexFrames({
    ...HEVC_CONFIG,
    codec: 'avc1.640028',
    hardwareAcceleration: 'prefer-hardware',
  })

  assertChunkTimestampsAttributable(t, chunks, errors)
})
