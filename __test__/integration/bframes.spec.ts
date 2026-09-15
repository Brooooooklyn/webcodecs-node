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

// VP9 show_existing_frame attribution. A show_existing packet re-displays a
// stored reference frame; FFmpeg hands back a copy of that reference carrying
// the reference's inherited metadata. The decoder must attribute the output
// to the display chunk (its own timestamp and duration), not the reference.

const VP9_CONFIG = {
  codec: 'vp09.00.10.08',
  width: WIDTH,
  height: HEIGHT,
  bitrate: 1_000_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-software' as const,
}

async function encodeSingleVp9Keyframe(t: ExecutionContext): Promise<Uint8Array> {
  const { encoder, chunks, errors } = createTestEncoder()
  encoder.configure(VP9_CONFIG)
  const frame = generateIndexFrame(20, 0) // Y = 200
  encoder.encode(frame, { keyFrame: true })
  frame.close()
  await encoder.flush()
  encoder.close()

  t.is(errors.length, 0, `No encoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(chunks.length, 1, 'Single keyframe chunk')
  t.is(chunks[0].type, 'key')

  const data = new Uint8Array(chunks[0].byteLength)
  chunks[0].copyTo(data)

  // VP9 uncompressed header, byte 0 (MSB first): frame_marker(2)=2,
  // profile(2)=0, show_existing_frame(1)=0, frame_type(1)=0 (key),
  // show_frame(1)=1, error_resilient(1).
  t.is(data[0] & 0xc0, 0x80, 'VP9 frame marker')
  t.is(data[0] & 0x30, 0, 'VP9 profile 0')
  t.is(data[0] & 0x0c, 0, 'not show_existing, keyframe')
  t.is(data[0] & 0x02, 0x02, 'visible keyframe (show_frame=1)')
  return data
}

/** Same keyframe with show_frame cleared: decodes into the reference buffer without producing output */
function invisibleVariant(keyframe: Uint8Array): Uint8Array {
  const data = new Uint8Array(keyframe)
  data[0] &= ~0x02
  return data
}

/** Raw show_existing_frame packet: marker=2, profile=0, show_existing_frame=1, frame_to_show_map_idx */
function showExistingPacket(refIndex = 0): Uint8Array {
  return new Uint8Array([0x88 | (refIndex & 0x7)])
}

/**
 * VP9 superframe: constituent frames followed by the index —
 * marker, little-endian sizes, marker again. The marker byte holds 0b110
 * in bits 7-5, (length_size-1) in bits 4-3 and (nb_frames-1) in bits 2-0.
 */
function makeSuperframe(...parts: Uint8Array[]): Uint8Array {
  const sizeBytes = 2
  const marker = 0xc0 | ((sizeBytes - 1) << 3) | (parts.length - 1)
  const total = parts.reduce((a, p) => a + p.length, 0)
  const out = new Uint8Array(total + 2 + parts.length * sizeBytes)
  let off = 0
  for (const p of parts) {
    out.set(p, off)
    off += p.length
  }
  out[off++] = marker
  for (const p of parts) {
    for (let j = 0; j < sizeBytes; j++) {
      out[off++] = (p.length >> (j * 8)) & 0xff
    }
  }
  out[off] = marker
  return out
}

interface ChunkSpec {
  type: 'key' | 'delta'
  timestamp: number
  duration?: number
  data: Uint8Array
}

async function decodeChunkSpecs(specs: ChunkSpec[]) {
  const { decoder, frames, errors } = createTestDecoder()
  decoder.configure({
    codec: VP9_CONFIG.codec,
    codedWidth: WIDTH,
    codedHeight: HEIGHT,
    hardwareAcceleration: 'prefer-software',
  })
  for (const spec of specs) {
    decoder.decode(
      new EncodedVideoChunk({
        type: spec.type,
        timestamp: spec.timestamp,
        ...(spec.duration !== undefined ? { duration: spec.duration } : {}),
        data: spec.data,
      }),
    )
  }
  await decoder.flush()
  decoder.close()
  return { frames, errors }
}

async function assertRepeatedFrames(
  t: ExecutionContext,
  frames: VideoFrame[],
  errors: Error[],
  expected: Array<{ ts: number; duration: number | undefined }>,
) {
  t.is(errors.length, 0, `No decoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(frames.length, expected.length, 'One output frame per display event')

  for (const [i, frame] of frames.entries()) {
    t.is(frame.timestamp, expected[i].ts, `frame ${i} timestamp`)
    t.is(
      frame.duration ?? undefined,
      expected[i].duration,
      `frame ${i} must carry the display chunk's duration (${expected[i].duration}), not the reference's`,
    )

    // Every output is the stored reference image (Y ≈ 200)
    const data = new Uint8Array(frame.allocationSize())
    await frame.copyTo(data)
    const yPlane = data.subarray(0, WIDTH * HEIGHT)
    let sum = 0
    for (const value of yPlane) {
      sum += value
    }
    const averageY = sum / yPlane.length
    t.true(Math.abs(averageY - 200) <= 2, `frame ${i} average Y ${averageY.toFixed(2)} should be ~200`)
    frame.close()
  }
}

test('decoder attributes show_existing_frame repeats to the display chunk (visible reference)', async (t) => {
  const keyframe = await encodeSingleVp9Keyframe(t)
  const showExisting = showExistingPacket()

  const { frames, errors } = await decodeChunkSpecs([
    { type: 'key', timestamp: 100, duration: 111, data: keyframe },
    { type: 'delta', timestamp: 200, duration: 222, data: showExisting },
    { type: 'delta', timestamp: 300, duration: 0, data: showExisting },
    { type: 'delta', timestamp: 400, data: showExisting },
  ])

  await assertRepeatedFrames(t, frames, errors, [
    { ts: 100, duration: 111 },
    { ts: 200, duration: 222 },
    { ts: 300, duration: 0 },
    { ts: 400, duration: undefined },
  ])
})

test('decoder attributes show_existing_frame repeats to the display chunk (invisible reference)', async (t) => {
  const keyframe = await encodeSingleVp9Keyframe(t)
  const showExisting = showExistingPacket()

  // The invisible reference produces no output itself; each show_existing
  // packet re-displays it and must carry its own chunk's metadata.
  const { frames, errors } = await decodeChunkSpecs([
    { type: 'key', timestamp: 100, duration: 111, data: invisibleVariant(keyframe) },
    { type: 'delta', timestamp: 200, duration: 222, data: showExisting },
    { type: 'delta', timestamp: 300, duration: 0, data: showExisting },
    { type: 'delta', timestamp: 400, data: showExisting },
  ])

  await assertRepeatedFrames(t, frames, errors, [
    { ts: 200, duration: 222 },
    { ts: 300, duration: 0 },
    { ts: 400, duration: undefined },
  ])
})

test('decoder attributes show_existing_frame repeats when the display ts collides with the reference ts', async (t) => {
  const keyframe = await encodeSingleVp9Keyframe(t)
  const showExisting = showExistingPacket()

  // The first repeat shares the invisible reference's timestamp, so the
  // frame's inherited opaque tag does not contradict its PTS. The display
  // chunk's own metadata must still win.
  const { frames, errors } = await decodeChunkSpecs([
    { type: 'key', timestamp: 200, duration: 111, data: invisibleVariant(keyframe) },
    { type: 'delta', timestamp: 200, duration: 0, data: showExisting },
    { type: 'delta', timestamp: 300, duration: 333, data: showExisting },
  ])

  await assertRepeatedFrames(t, frames, errors, [
    { ts: 200, duration: 0 },
    { ts: 300, duration: 333 },
  ])
})

test('decoder ignores invisible-reference entries when a later frame occupies the reference slots', async (t) => {
  const keyframe = await encodeSingleVp9Keyframe(t)
  const showExisting = showExistingPacket()

  // The visible keyframe refreshes every reference slot, so the repeat
  // re-displays it (not the earlier invisible frame). The invisible chunk's
  // lingering entry must not win the PTS match against the display chunk.
  const { frames, errors } = await decodeChunkSpecs([
    { type: 'key', timestamp: 200, duration: 111, data: invisibleVariant(keyframe) },
    { type: 'key', timestamp: 100, duration: 222, data: keyframe },
    { type: 'delta', timestamp: 200, duration: 0, data: showExisting },
  ])

  await assertRepeatedFrames(t, frames, errors, [
    { ts: 100, duration: 222 },
    { ts: 200, duration: 0 },
  ])
})

// Superframes: libvpx packs invisible frames before a visible frame (or a
// show_existing repeat) in one chunk. The chunk's metadata must be kept as
// long as any constituent produces a display event.
const DURATION_VARIANTS = [777, 0, undefined] as const

test('decoder keeps metadata for superframes packing an invisible frame before a visible frame', async (t) => {
  const keyframe = await encodeSingleVp9Keyframe(t)
  const invisible = invisibleVariant(keyframe)

  for (const duration of DURATION_VARIANTS) {
    const { frames, errors } = await decodeChunkSpecs([
      { type: 'key', timestamp: 500, ...(duration !== undefined ? { duration } : {}), data: makeSuperframe(invisible, keyframe) },
    ])
    await assertRepeatedFrames(t, frames, errors, [{ ts: 500, duration }])
  }
})

test('decoder keeps metadata for superframes packing an invisible frame before a show_existing repeat', async (t) => {
  const keyframe = await encodeSingleVp9Keyframe(t)
  const invisible = invisibleVariant(keyframe)
  const showExisting = showExistingPacket()

  for (const duration of DURATION_VARIANTS) {
    const { frames, errors } = await decodeChunkSpecs([
      { type: 'key', timestamp: 600, ...(duration !== undefined ? { duration } : {}), data: makeSuperframe(invisible, showExisting) },
    ])
    await assertRepeatedFrames(t, frames, errors, [{ ts: 600, duration }])
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
