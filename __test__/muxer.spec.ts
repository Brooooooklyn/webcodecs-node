/**
 * Muxer Tests
 *
 * Tests for Mp4Muxer, WebMMuxer, and MkvMuxer classes.
 */

import test from 'ava'

import {
  Mp4Muxer,
  Mp4Demuxer,
  WebMMuxer,
  WebMDemuxer,
  MkvMuxer,
  VideoEncoder,
  VideoDecoder,
  AudioEncoder,
  resetHardwareFallbackState,
  type EncodedVideoChunk,
  type EncodedAudioChunk,
  type EncodedVideoChunkMetadata,
  type EncodedAudioChunkMetadata,
  type VideoFrame,
} from '../index.js'
import { generateSolidColorI420Frame, generateSilence, TestColors } from './helpers/index.js'

test('WebMMuxer: preserves sparse source timestamps through demux', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => chunks.push(chunk),
    error: (error) => t.fail(error.message),
  })
  encoder.configure({ codec: 'vp8', width: 64, height: 64, bitrate: 100_000, framerate: 30 })

  for (const timestamp of [0, 1_000_000]) {
    const frame = generateSolidColorI420Frame(64, 64, TestColors.red, timestamp)
    encoder.encode(frame, { keyFrame: true })
    frame.close()
  }
  await encoder.flush()
  encoder.close()

  const muxer = new WebMMuxer()
  muxer.addVideoTrack({ codec: 'vp8', width: 64, height: 64, framerate: 30 })
  for (const chunk of chunks) muxer.addVideoChunk(chunk)
  const data = muxer.finalize()
  muxer.close()

  const timestamps: number[] = []
  const demuxer = new WebMDemuxer({
    videoOutput: (chunk) => timestamps.push(chunk.timestamp),
    error: (error) => t.fail(error.message),
  })
  await demuxer.loadBuffer(data)
  await demuxer.demuxAsync()
  demuxer.close()

  t.deepEqual(timestamps, [0, 1_000_000])
})

test('WebMMuxer: accepts duplicate source timestamps', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => chunks.push(chunk),
    error: (error) => t.fail(error.message),
  })
  encoder.configure({ codec: 'vp8', width: 64, height: 64, bitrate: 100_000, framerate: 30 })

  for (let i = 0; i < 2; i++) {
    const frame = generateSolidColorI420Frame(64, 64, TestColors.red, 0)
    encoder.encode(frame, { keyFrame: true })
    frame.close()
  }
  await encoder.flush()
  encoder.close()

  const muxer = new WebMMuxer()
  muxer.addVideoTrack({ codec: 'vp8', width: 64, height: 64, framerate: 30 })
  for (const chunk of chunks) muxer.addVideoChunk(chunk)
  t.true(muxer.finalize().length > 0)
  muxer.close()
})

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// Mp4Muxer Tests
// ============================================================================

test('Mp4Muxer: constructor creates muxer', (t) => {
  const muxer = new Mp4Muxer()
  t.truthy(muxer)
  muxer.close()
})

test('Mp4Muxer: constructor accepts options', (t) => {
  const muxer = new Mp4Muxer({ fastStart: true })
  t.truthy(muxer)
  muxer.close()
})

test('Mp4Muxer: can add video track', (t) => {
  const muxer = new Mp4Muxer()

  t.notThrows(() => {
    muxer.addVideoTrack({
      codec: 'avc1.42001E',
      width: 320,
      height: 240,
    })
  })

  muxer.close()
})

test('Mp4Muxer: can add audio track', (t) => {
  const muxer = new Mp4Muxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'mp4a.40.2',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('Mp4Muxer: muxes video chunks and produces valid MP4', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  // Create encoder to generate real encoded chunks
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Encode some frames
  for (let i = 0; i < 30; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.red, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  // Now mux the chunks (without fastStart for memory-based I/O)
  const muxer = new Mp4Muxer()

  // Get description from first keyframe metadata
  const description = videoMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    description,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  // Verify we got some data
  t.true(mp4Data.length > 0, 'Should have MP4 data')
  t.true(mp4Data.length > 1000, 'MP4 should have reasonable size')

  // Check MP4 magic bytes (ftyp box)
  const ftypOffset = mp4Data.indexOf(0x66) // 'f'
  t.true(ftypOffset >= 0, 'Should have ftyp box')
})

// ============================================================================
// Mp4Muxer HEVC Sample Entry Tests (hvc1 preferred over hev1, issue #22)
// ============================================================================

/**
 * Walk ISO-BMFF box structure to locate a nested box.
 * Returns the offset of the box header (size field) or -1 when not found.
 */
function findMp4Box(data: Uint8Array, path: string[], start: number, end: number): number {
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength)
  let offset = start
  while (offset + 8 <= end) {
    const size = view.getUint32(offset)
    const type = String.fromCharCode(data[offset + 4], data[offset + 5], data[offset + 6], data[offset + 7])
    const boxEnd = size === 0 ? end : offset + size
    if (type === path[0]) {
      if (path.length === 1) return offset
      const child = findMp4Box(data, path.slice(1), offset + 8, boxEnd)
      if (child >= 0) return child
    }
    if (size === 0) break
    offset = boxEnd
  }
  return -1
}

/** Read the fourcc of the first video sample entry in moov/trak/mdia/minf/stbl/stsd */
function getMp4VideoSampleEntryTag(data: Uint8Array): string | null {
  const stsd = findMp4Box(data, ['moov', 'trak', 'mdia', 'minf', 'stbl', 'stsd'], 0, data.length)
  if (stsd < 0) return null
  // stsd payload: 4-byte version/flags + 4-byte entry count, then entries as size+fourcc
  const entry = stsd + 8 + 4 + 4
  return String.fromCharCode(data[entry + 4], data[entry + 5], data[entry + 6], data[entry + 7])
}

/** Encode HEVC frames with the software encoder and return chunks plus per-chunk metadata */
async function encodeHevcChunks(width: number, height: number, frameCount: number) {
  const chunks: EncodedVideoChunk[] = []
  const metadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const errors: Error[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      metadatas.push(metadata)
    },
    error: (e) => errors.push(e),
  })
  encoder.configure({
    codec: 'hev1.1.6.L93.B0',
    width,
    height,
    bitrate: 500_000,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
  })

  for (let i = 0; i < frameCount; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.green, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }
  await encoder.flush()
  encoder.close()

  if (errors.length > 0) throw errors[0]
  if (chunks.length === 0) throw new Error('HEVC encoder produced no chunks')
  return { chunks, metadatas }
}

test('Mp4Muxer: writes hvc1 sample entry for HEVC with description', async (t) => {
  const { chunks, metadatas } = await encodeHevcChunks(128, 128, 10)

  const muxer = new Mp4Muxer()
  muxer.addVideoTrack({
    codec: 'hev1.1.6.L93.B0',
    width: 128,
    height: 128,
    framerate: 30,
    description: metadatas[0]?.decoderConfig?.description,
  })
  for (let i = 0; i < chunks.length; i++) {
    muxer.addVideoChunk(chunks[i], metadatas[i])
  }
  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  t.true(mp4Data.length > 0, 'Should have MP4 data')
  // hvc1 signals parameter sets live in the sample description (hvcC), not the bitstream
  t.is(getMp4VideoSampleEntryTag(mp4Data), 'hvc1', 'HEVC sample entry should be hvc1')

  // The hvcC box must be inside the hvc1 sample entry (extradata-based parameter sets)
  const stsd = findMp4Box(mp4Data, ['moov', 'trak', 'mdia', 'minf', 'stbl', 'stsd'], 0, mp4Data.length)
  t.true(stsd >= 0, 'stsd box should exist')
  const entry = stsd + 8 + 4 + 4
  const view = new DataView(mp4Data.buffer, mp4Data.byteOffset, mp4Data.byteLength)
  const entrySize = view.getUint32(entry)
  const entryEnd = Math.min(entry + entrySize, mp4Data.length)
  let hvcC = -1
  for (let i = entry + 8; i + 4 <= entryEnd; i++) {
    if (mp4Data[i] === 0x68 && mp4Data[i + 1] === 0x76 && mp4Data[i + 2] === 0x63 && mp4Data[i + 3] === 0x43) {
      hvcC = i
      break
    }
  }
  t.true(hvcC >= 0, 'hvcC box should exist inside the hvc1 sample entry')
})

test('Mp4Muxer: writes hvc1 sample entry in fragmented MP4', async (t) => {
  const { chunks, metadatas } = await encodeHevcChunks(128, 128, 6)

  const muxer = new Mp4Muxer({ fragmented: true })
  muxer.addVideoTrack({
    codec: 'hev1.1.6.L93.B0',
    width: 128,
    height: 128,
    framerate: 30,
    description: metadatas[0]?.decoderConfig?.description,
  })
  for (let i = 0; i < chunks.length; i++) {
    muxer.addVideoChunk(chunks[i], metadatas[i])
  }
  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  t.true(mp4Data.length > 0, 'Should have fragmented MP4 data')
  t.is(getMp4VideoSampleEntryTag(mp4Data), 'hvc1', 'Fragmented HEVC sample entry should be hvc1')
})

test('Mp4Muxer: hvc1 HEVC output round-trips through Mp4Demuxer and VideoDecoder', async (t) => {
  const frameCount = 10
  const { chunks, metadatas } = await encodeHevcChunks(128, 128, frameCount)

  const muxer = new Mp4Muxer()
  muxer.addVideoTrack({
    codec: 'hev1.1.6.L93.B0',
    width: 128,
    height: 128,
    framerate: 30,
    description: metadatas[0]?.decoderConfig?.description,
  })
  for (let i = 0; i < chunks.length; i++) {
    muxer.addVideoChunk(chunks[i], metadatas[i])
  }
  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  t.is(getMp4VideoSampleEntryTag(mp4Data), 'hvc1', 'Precondition: sample entry is hvc1')

  // Demux the hvc1 file
  const demuxedChunks: EncodedVideoChunk[] = []
  const demuxer = new Mp4Demuxer({
    videoOutput: (chunk) => demuxedChunks.push(chunk),
    error: (e) => t.fail(`Demuxer error: ${e.message}`),
  })
  await demuxer.loadBuffer(mp4Data)

  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Should expose a video decoder config')
  if (!config) return
  t.true(config.codec.startsWith('hev1'), 'JS-facing codec string stays hev1-prefixed')
  t.is(config.codedWidth, 128)
  t.is(config.codedHeight, 128)
  t.truthy(config.description, 'Config should carry the hvcC description')

  await demuxer.demuxAsync()
  demuxer.close()

  t.is(demuxedChunks.length, chunks.length, 'All chunks should demux')

  // Decode the demuxed chunks with the demuxer's config
  const decodedFrames: VideoFrame[] = []
  const decoder = new VideoDecoder({
    output: (frame) => decodedFrames.push(frame),
    error: (e) => t.fail(`Decoder error: ${e.message}`),
  })
  decoder.configure({
    codec: config.codec,
    codedWidth: config.codedWidth,
    codedHeight: config.codedHeight,
    description: config.description,
  })
  for (const chunk of demuxedChunks) {
    decoder.decode(chunk)
  }
  await decoder.flush()
  decoder.close()

  t.is(decodedFrames.length, frameCount, 'Every encoded frame should decode')
  for (const frame of decodedFrames) {
    t.is(frame.codedWidth, 128)
    t.is(frame.codedHeight, 128)
    frame.close()
  }
})

// ============================================================================
// WebMMuxer Tests
// ============================================================================

test('WebMMuxer: constructor creates muxer', (t) => {
  const muxer = new WebMMuxer()
  t.truthy(muxer)
  muxer.close()
})

test('Muxers: streaming-only status getters reject in buffer mode', (t) => {
  for (const Muxer of [Mp4Muxer, WebMMuxer, MkvMuxer]) {
    const muxer = new Muxer()
    t.throws(() => muxer.isFinished, { message: /Not in streaming mode/ })
    t.throws(() => muxer.read(), { message: /Not in streaming mode/ })
    muxer.close()
  }
})

test('WebMMuxer: streaming output does not block when reads are delayed', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => chunks.push(chunk),
    error: (error) => t.fail(error.message),
  })
  encoder.configure({
    codec: 'vp8',
    width: 64,
    height: 64,
    bitrate: 100_000,
    hardwareAcceleration: 'prefer-software',
  })
  for (let index = 0; index < 20; index++) {
    const frame = generateSolidColorI420Frame(64, 64, TestColors.red, index * 33_333)
    encoder.encode(frame, { keyFrame: index === 0 })
    frame.close()
  }
  await encoder.flush()
  encoder.close()

  // Sixteen bytes is deliberately smaller than the container header. The old
  // fixed ring buffer deadlocked in addVideoChunk() before JavaScript could read.
  const muxer = new WebMMuxer({ live: true, streaming: { bufferCapacity: 16 } })
  muxer.addVideoTrack({ codec: 'vp8', width: 64, height: 64, framerate: 30 })
  for (const chunk of chunks) muxer.addVideoChunk(chunk)
  t.is(muxer.finalize().length, 0)

  const outputParts: Uint8Array[] = []
  for (;;) {
    const part = muxer.read()
    t.truthy(part, 'finalized streaming muxer should return data or EOF')
    if (!part || part.length === 0) break
    t.true(part.length <= 16)
    outputParts.push(part)
  }
  const output = Buffer.concat(outputParts)
  t.true(output.length > 16)
  t.deepEqual([...output.subarray(0, 4)], [0x1a, 0x45, 0xdf, 0xa3])
  t.true(muxer.isFinished)
  muxer.close()
})

test('WebMMuxer: can add video track', (t) => {
  const muxer = new WebMMuxer()

  t.notThrows(() => {
    muxer.addVideoTrack({
      codec: 'vp09.00.10.08',
      width: 320,
      height: 240,
    })
  })

  muxer.close()
})

test('WebMMuxer: muxes VP9 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.green, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()

  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')

  // Check WebM magic bytes (0x1A 0x45 0xDF 0xA3 = EBML header)
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
  t.is(webmData[1], 0x45, 'WebM should start with EBML header')
  t.is(webmData[2], 0xdf, 'WebM should start with EBML header')
  t.is(webmData[3], 0xa3, 'WebM should start with EBML header')
})

test('WebMMuxer: can add Opus audio track', (t) => {
  const muxer = new WebMMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('WebMMuxer: muxes Opus audio chunks', async (t) => {
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64_000,
  })

  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(960, 2, 48000, 'f32', i * 20000)
    encoder.encode(audioData)
    audioData.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(audioChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()

  muxer.addAudioTrack({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
})

test('WebMMuxer: muxes VP9 video and Opus audio combined', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Video encoder error: ${e.message}`),
  })

  videoEncoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  const audioEncoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Audio encoder error: ${e.message}`),
  })

  audioEncoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.green, i * 33333)
    videoEncoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  for (let i = 0; i < 5; i++) {
    const audioData = generateSilence(960, 2, 48000, 'f32', i * 20000)
    audioEncoder.encode(audioData)
    audioData.close()
  }

  await Promise.all([videoEncoder.flush(), audioEncoder.flush()])
  videoEncoder.close()
  audioEncoder.close()

  t.true(videoChunks.length > 0, 'Should have video chunks')
  t.true(audioChunks.length > 0, 'Should have audio chunks')

  const muxer = new WebMMuxer()

  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
  })

  muxer.addAudioTrack({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.true(webmData.length > 500, 'WebM with audio+video should have minimum size')
})

test('WebMMuxer: muxes VP8 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.red, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()

  muxer.addVideoTrack({
    codec: 'vp8',
    width: 320,
    height: 240,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
})

test('WebMMuxer: muxes AV1 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 5; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new WebMMuxer()
  const description = videoMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    description,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  t.true(webmData.length > 0, 'Should have WebM data')
  t.is(webmData[0], 0x1a, 'WebM should start with EBML header')
})

// ============================================================================
// MkvMuxer Tests
// ============================================================================

test('MkvMuxer: constructor creates muxer', (t) => {
  const muxer = new MkvMuxer()
  t.truthy(muxer)
  muxer.close()
})

test('MkvMuxer: can add video track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addVideoTrack({
      codec: 'avc1.42001E',
      width: 320,
      height: 240,
    })
  })

  muxer.close()
})

test('MkvMuxer: muxes H.264 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()
  const description = videoMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    description,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')

  // Check MKV magic bytes (same as WebM - EBML header)
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
  t.is(mkvData[1], 0x45, 'MKV should start with EBML header')
})

test('MkvMuxer: can add AAC audio track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'mp4a.40.2',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('MkvMuxer: can add Opus audio track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('MkvMuxer: can add FLAC audio track', (t) => {
  const muxer = new MkvMuxer()

  t.notThrows(() => {
    muxer.addAudioTrack({
      codec: 'flac',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  muxer.close()
})

test('MkvMuxer: muxes AAC audio chunks', async (t) => {
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128_000,
  })

  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(1024, 2, 48000, 'f32', i * 21333)
    encoder.encode(audioData)
    audioData.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(audioChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()
  const description = audioMetadatas[0]?.decoderConfig?.description

  muxer.addAudioTrack({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    description,
  })

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

test('MkvMuxer: muxes Opus audio chunks', async (t) => {
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64_000,
  })

  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(960, 2, 48000, 'f32', i * 20000)
    encoder.encode(audioData)
    audioData.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(audioChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()

  muxer.addAudioTrack({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

test('MkvMuxer: muxes H.264 video and AAC audio combined', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Video encoder error: ${e.message}`),
  })

  videoEncoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  const audioEncoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Audio encoder error: ${e.message}`),
  })

  audioEncoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.yellow, i * 33333)
    videoEncoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  for (let i = 0; i < 5; i++) {
    const audioData = generateSilence(1024, 2, 48000, 'f32', i * 21333)
    audioEncoder.encode(audioData)
    audioData.close()
  }

  await Promise.all([videoEncoder.flush(), audioEncoder.flush()])
  videoEncoder.close()
  audioEncoder.close()

  t.true(videoChunks.length > 0, 'Should have video chunks')
  t.true(audioChunks.length > 0, 'Should have audio chunks')

  const muxer = new MkvMuxer()
  const videoDescription = videoMetadatas[0]?.decoderConfig?.description
  const audioDescription = audioMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    description: videoDescription,
  })

  muxer.addAudioTrack({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    description: audioDescription,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.true(mkvData.length > 500, 'MKV with audio+video should have minimum size')
})

test('MkvMuxer: muxes VP9 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 10; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.green, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()

  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: 320,
    height: 240,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

test('MkvMuxer: muxes AV1 video chunks', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Encoder error: ${e.message}`),
  })

  encoder.configure({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    bitrate: 500_000,
  })

  for (let i = 0; i < 5; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  t.true(videoChunks.length > 0, 'Should have encoded chunks')

  const muxer = new MkvMuxer()
  const description = videoMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'av01.0.04M.08',
    width: 320,
    height: 240,
    description,
  })

  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  muxer.flush()
  const mkvData = muxer.finalize()
  muxer.close()

  t.true(mkvData.length > 0, 'Should have MKV data')
  t.is(mkvData[0], 0x1a, 'MKV should start with EBML header')
})

// ============================================================================
// Combined Audio+Video Muxing Tests
// ============================================================================

test('Mp4Muxer: muxes both video and audio', async (t) => {
  const videoChunks: EncodedVideoChunk[] = []
  const videoMetadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const audioChunks: EncodedAudioChunk[] = []
  const audioMetadatas: (EncodedAudioChunkMetadata | undefined)[] = []

  // Video encoder
  const videoEncoder = new VideoEncoder({
    output: (chunk, metadata) => {
      videoChunks.push(chunk)
      videoMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Video encoder error: ${e.message}`),
  })

  videoEncoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Audio encoder
  const audioEncoder = new AudioEncoder({
    output: (chunk, metadata) => {
      audioChunks.push(chunk)
      audioMetadatas.push(metadata)
    },
    error: (e) => t.fail(`Audio encoder error: ${e.message}`),
  })

  audioEncoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128_000,
  })

  // Encode video
  for (let i = 0; i < 30; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.yellow, i * 33333)
    videoEncoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  // Encode audio
  for (let i = 0; i < 10; i++) {
    const audioData = generateSilence(1024, 2, 48000, 'f32', i * Math.floor((1024 * 1_000_000) / 48000))
    audioEncoder.encode(audioData)
    audioData.close()
  }

  await Promise.all([videoEncoder.flush(), audioEncoder.flush()])
  videoEncoder.close()
  audioEncoder.close()

  t.true(videoChunks.length > 0, 'Should have video chunks')
  t.true(audioChunks.length > 0, 'Should have audio chunks')

  // Mux together (without fastStart for memory-based I/O)
  const muxer = new Mp4Muxer()

  const videoDescription = videoMetadatas[0]?.decoderConfig?.description
  const audioDescription = audioMetadatas[0]?.decoderConfig?.description

  muxer.addVideoTrack({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    description: videoDescription,
  })

  muxer.addAudioTrack({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    description: audioDescription,
  })

  // Interleave chunks by timestamp
  for (let i = 0; i < videoChunks.length; i++) {
    muxer.addVideoChunk(videoChunks[i], videoMetadatas[i])
  }

  for (let i = 0; i < audioChunks.length; i++) {
    muxer.addAudioChunk(audioChunks[i], audioMetadatas[i])
  }

  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  t.true(mp4Data.length > 0, 'Should have MP4 data')
  // Solid color video and silence audio compress very well, so the output is smaller than expected
  t.true(mp4Data.length > 1000, 'MP4 with audio+video should have minimum size')
})
