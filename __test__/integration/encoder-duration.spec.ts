/**
 * Encoder Duration Propagation Integration Tests
 *
 * VideoFrame.duration must reach EncodedVideoChunk.duration. Software
 * encoders propagate frame->duration to pkt->duration themselves (gated on
 * AV_CODEC_FLAG_FRAME_DURATION); VideoToolbox never sets pkt->duration, so
 * the PTS-keyed input map fills it in as a fallback. Assertions join chunks
 * to input frames by timestamp, which is immune to B-frame reordering.
 */

import test from 'ava'
import type { ExecutionContext } from 'ava'

import {
  EncodedVideoChunk,
  Mp4Demuxer,
  Mp4Muxer,
  resetHardwareFallbackState,
  VideoEncoder,
} from '../../index.js'
import type { EncodedVideoChunkMetadata, VideoEncoderConfig } from '../../index.js'
import { generateGradientI420Frame, hasHardwareAcceleration } from '../helpers/index.js'

// Reset hardware fallback state before each test to ensure test isolation
test.beforeEach(() => {
  resetHardwareFallbackState()
})

const WIDTH = 320
const HEIGHT = 240
const FRAME_COUNT = 8
// Fixed 30fps timestamps with distinct per-frame durations (VFR content)
const TIMESTAMPS = Array.from({ length: FRAME_COUNT }, (_, i) => i * 33333)
const DURATION_CYCLE = [33333, 16667, 50000, 41667]
const FRAME_DURATIONS = Array.from({ length: FRAME_COUNT }, (_, i) => DURATION_CYCLE[i % DURATION_CYCLE.length])

const CODEC_STRINGS = ['avc1.42001E', 'hev1.1.6.L93.B0', 'vp09.00.10.08', 'av01.0.01M.08'] as const

function makeConfig(codec: string): VideoEncoderConfig {
  return {
    codec,
    width: WIDTH,
    height: HEIGHT,
    bitrate: 2_000_000,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
    latencyMode: 'quality',
  }
}

interface EncodeResult {
  chunks: EncodedVideoChunk[]
  metadatas: (EncodedVideoChunkMetadata | undefined)[]
  errors: Error[]
  decoderConfig?: EncodedVideoChunkMetadata['decoderConfig']
}

async function encodeFrames(
  config: VideoEncoderConfig,
  withDurations: number[] | null,
  timestamps: number[] = TIMESTAMPS,
): Promise<EncodeResult> {
  const chunks: EncodedVideoChunk[] = []
  const metadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const errors: Error[] = []
  let decoderConfig: EncodedVideoChunkMetadata['decoderConfig']

  const encoder = new VideoEncoder({
    output: (chunk, metadata?: EncodedVideoChunkMetadata) => {
      chunks.push(chunk)
      metadatas.push(metadata)
      if (!decoderConfig && metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig
      }
    },
    error: (e) => errors.push(e),
  })
  encoder.configure(config)

  for (let i = 0; i < FRAME_COUNT; i++) {
    const duration = withDurations ? withDurations[i] : undefined
    const frame = generateGradientI420Frame(WIDTH, HEIGHT, timestamps[i], duration)
    encoder.encode(frame, i === 0 ? { keyFrame: true } : undefined)
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  return { chunks, metadatas, errors, decoderConfig }
}

function assertDurationsMatchInputs(t: ExecutionContext, chunks: EncodedVideoChunk[], errors: Error[]) {
  t.is(errors.length, 0, `No encoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(chunks.length, FRAME_COUNT, 'One chunk per input frame')

  const durationByTimestamp = new Map(TIMESTAMPS.map((ts, i) => [ts, FRAME_DURATIONS[i]]))
  for (const chunk of chunks) {
    t.not(chunk.duration, null, `Chunk ts=${chunk.timestamp} must have a duration`)
    t.is(
      chunk.duration,
      durationByTimestamp.get(chunk.timestamp) ?? null,
      `Chunk ts=${chunk.timestamp} must carry its input frame's duration`,
    )
  }
}

for (const codec of CODEC_STRINGS) {
  test(`duration passthrough with distinct VFR durations: ${codec}`, async (t) => {
    const { chunks, errors } = await encodeFrames(makeConfig(codec), FRAME_DURATIONS)
    assertDurationsMatchInputs(t, chunks, errors)
  })
}

test('HEVC B-frame reordering keeps duration paired with its own frame', async (t) => {
  const { chunks, errors } = await encodeFrames(makeConfig('hev1.1.6.L93.B0'), FRAME_DURATIONS)

  // Chunks are emitted in decode order under B-frame reordering, so a
  // FIFO-labeled duration stream would misattribute them. The timestamp join
  // in assertDurationsMatchInputs is what proves per-frame pairing.
  const chunkTimestamps = chunks.map((c) => c.timestamp)
  const monotonic = chunkTimestamps.every((ts, i) => i === 0 || ts >= chunkTimestamps[i - 1])
  t.false(monotonic, 'B-frame reordering should emit chunks out of presentation order')

  assertDurationsMatchInputs(t, chunks, errors)
})

test('frames without duration produce chunks with null duration', async (t) => {
  const { chunks, errors } = await encodeFrames(makeConfig('avc1.42001E'), null)

  t.is(errors.length, 0, `No encoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(chunks.length, FRAME_COUNT, 'One chunk per input frame')
  for (const chunk of chunks) {
    t.is(chunk.duration, null, `Chunk ts=${chunk.timestamp} must have null duration`)
  }
})

// VideoToolbox never sets pkt->duration; this exercises the fallback that
// fills chunk duration from the input frame via the PTS-keyed map.
const testOnDarwin = process.platform === 'darwin' ? test : test.skip

testOnDarwin('VideoToolbox hardware encoder duration passthrough', async (t) => {
  if (!hasHardwareAcceleration()) {
    t.pass('No usable hardware accelerator available, skipping')
    return
  }

  const { chunks, errors } = await encodeFrames(
    { ...makeConfig('avc1.640028'), hardwareAcceleration: 'prefer-hardware' },
    FRAME_DURATIONS,
  )
  assertDurationsMatchInputs(t, chunks, errors)
})

test('Mp4Muxer roundtrip preserves chunk durations', async (t) => {
  // FFmpeg's MP4 muxer derives stts sample durations from DTS deltas, so
  // distinct durations only survive the container when timestamps are the
  // cumulative sum of prior durations (genuine VFR timing).
  const vfrTimestamps: number[] = [0]
  for (let i = 1; i < FRAME_COUNT; i++) {
    vfrTimestamps.push(vfrTimestamps[i - 1] + FRAME_DURATIONS[i - 1])
  }

  const { chunks, metadatas, errors, decoderConfig } = await encodeFrames(
    makeConfig('avc1.42001E'),
    FRAME_DURATIONS,
    vfrTimestamps,
  )
  t.is(errors.length, 0, `No encoder errors, got: ${errors.map((e) => e.message).join(', ')}`)
  t.is(chunks.length, FRAME_COUNT, 'One chunk per input frame')

  // framerate 0 selects the muxer's µs track timescale (1/1_000_000)
  // fallback, so µs durations round-trip exactly; an fps-derived timescale
  // quantizes durations to ticks.
  const muxer = new Mp4Muxer({ fastStart: true })
  muxer.addVideoTrack({
    codec: 'avc1.42001E',
    width: WIDTH,
    height: HEIGHT,
    framerate: 0,
    description: decoderConfig?.description,
  })
  for (let i = 0; i < chunks.length; i++) {
    muxer.addVideoChunk(chunks[i], metadatas[i])
  }
  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  t.true(mp4Data.length > 0, 'Should have MP4 data')

  const demuxedChunks: EncodedVideoChunk[] = []
  const demuxErrors: Error[] = []
  const demuxer = new Mp4Demuxer({
    videoOutput: (chunk) => demuxedChunks.push(chunk),
    error: (e) => demuxErrors.push(e),
  })
  await demuxer.loadBuffer(mp4Data)
  await demuxer.demuxAsync()
  demuxer.close()

  t.is(demuxErrors.length, 0, `No demuxer errors, got: ${demuxErrors.map((e) => e.message).join(', ')}`)
  t.is(demuxedChunks.length, chunks.length, 'One demuxed chunk per encoded chunk')

  const encodedDurationByTimestamp = new Map(chunks.map((c) => [c.timestamp, c.duration]))
  for (const chunk of demuxedChunks) {
    t.is(
      chunk.duration,
      encodedDurationByTimestamp.get(chunk.timestamp) ?? null,
      `Demuxed chunk ts=${chunk.timestamp} must keep the encoded duration`,
    )
  }
})
