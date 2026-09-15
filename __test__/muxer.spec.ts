/**
 * Muxer Tests
 *
 * Tests for Mp4Muxer, WebMMuxer, and MkvMuxer classes.
 */

import test from 'ava'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = dirname(fileURLToPath(import.meta.url))

import {
  Mp4Muxer,
  Mp4Demuxer,
  WebMMuxer,
  WebMDemuxer,
  MkvMuxer,
  VideoEncoder,
  VideoDecoder,
  AudioEncoder,
  EncodedVideoChunk,
  resetHardwareFallbackState,
  type EncodedAudioChunk,
  type EncodedVideoChunkMetadata,
  type EncodedAudioChunkMetadata,
  type VideoFrame,
} from '../index.js'
import { generateSolidColorI420Frame, generateSolidColorI420AFrame, generateSilence, TestColors } from './helpers/index.js'

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

/** NAL unit types of a length-prefixed (4-byte) HEVC sample buffer */
function hevcNalTypes(data: Uint8Array): number[] {
  const types: number[] = []
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength)
  let off = 0
  while (off + 5 <= data.length) {
    const len = view.getUint32(off)
    if (len <= 0 || off + 4 + len > data.length) break
    types.push((data[off + 4] >> 1) & 0x3f)
    off += 4 + len
  }
  return types
}

/** Payloads of all mdat boxes in an MP4 file */
function mdatPayloads(mp4: Uint8Array): Uint8Array[] {
  const view = new DataView(mp4.buffer, mp4.byteOffset, mp4.byteLength)
  const payloads: Uint8Array[] = []
  for (let i = 4; i + 8 <= mp4.length; i++) {
    // 'mdat' fourcc; box header starts 4 bytes earlier at the size field
    if (mp4[i] === 0x6d && mp4[i + 1] === 0x64 && mp4[i + 2] === 0x61 && mp4[i + 3] === 0x74) {
      const size = view.getUint32(i - 4)
      if (size >= 8 && i - 4 + size <= mp4.length) payloads.push(mp4.subarray(i + 4, i - 4 + size))
    }
  }
  return payloads
}

/** Extract VPS/SPS/PPS NAL units (types 32-34) from an hvcC description */
function extractHevcParameterSets(hvcc: Uint8Array): Uint8Array[] {
  const sets: Uint8Array[] = []
  const numArrays = hvcc[22]
  let off = 23
  for (let a = 0; a < numArrays; a++) {
    const nalType = hvcc[off] & 0x3f
    const numNalus = (hvcc[off + 1] << 8) | hvcc[off + 2]
    off += 3
    for (let n = 0; n < numNalus; n++) {
      const len = (hvcc[off] << 8) | hvcc[off + 1]
      off += 2
      if (nalType >= 32 && nalType <= 34) sets.push(hvcc.slice(off, off + len))
      off += len
    }
  }
  return sets
}

/** Copy chunks, prepending 4-byte length-prefixed VPS/SPS/PPS to the first sample */
function prependParameterSets(chunks: EncodedVideoChunk[], description: Uint8Array): EncodedVideoChunk[] {
  const psPrefix = extractHevcParameterSets(description)
  if (psPrefix.length === 0) throw new Error('hvcC description carries no parameter sets')
  return chunks.map((chunk, i) => {
    const data = new Uint8Array(chunk.byteLength)
    chunk.copyTo(data)
    if (i !== 0) return new EncodedVideoChunk({ type: chunk.type, timestamp: chunk.timestamp, data })
    const parts = psPrefix.map((nal) => {
      const lp = new Uint8Array(4 + nal.length)
      new DataView(lp.buffer).setUint32(0, nal.length)
      lp.set(nal, 4)
      return lp
    })
    parts.push(data)
    const combined = new Uint8Array(parts.reduce((sum, p) => sum + p.length, 0))
    let off = 0
    for (const p of parts) {
      combined.set(p, off)
      off += p.length
    }
    return new EncodedVideoChunk({ type: chunk.type, timestamp: chunk.timestamp, data: combined })
  })
}

/** hvcC record with the PPS array removed (well-formed but incomplete) */
function hvccWithoutPps(hvcc: Uint8Array): Uint8Array {
  const header = hvcc.slice(0, 22)
  const numArrays = hvcc[22]
  const kept: Uint8Array[] = []
  let off = 23
  for (let a = 0; a < numArrays; a++) {
    const start = off
    const nalType = hvcc[off] & 0x3f
    const numNalus = (hvcc[off + 1] << 8) | hvcc[off + 2]
    off += 3
    for (let n = 0; n < numNalus; n++) {
      const len = (hvcc[off] << 8) | hvcc[off + 1]
      off += 2 + len
    }
    if (nalType !== 34) kept.push(hvcc.slice(start, off))
  }
  const out = new Uint8Array(23 + kept.reduce((sum, k) => sum + k.length, 0))
  out.set(header, 0)
  out[22] = kept.length
  let w = 23
  for (const k of kept) {
    out.set(k, w)
    w += k.length
  }
  return out
}

for (const fragmented of [false, true]) {
  test(`Mp4Muxer: strips in-band HEVC parameter sets under hvc1 (fragmented=${fragmented})`, async (t) => {
    const frameCount = 3
    const { chunks, metadatas } = await encodeHevcChunks(128, 128, frameCount)
    const description = metadatas[0]?.decoderConfig?.description
    t.truthy(description, 'Encoder should provide an hvcC description')
    if (!description) return

    // Samples carrying in-band VPS/SPS/PPS (allowed for hev1, forbidden for hvc1)
    const inBandChunks = prependParameterSets(chunks, new Uint8Array(description))

    const muxer = new Mp4Muxer(fragmented ? { fragmented: true } : {})
    muxer.addVideoTrack({ codec: 'hev1.1.6.L93.B0', width: 128, height: 128, framerate: 30, description })
    for (const chunk of inBandChunks) muxer.addVideoChunk(chunk)
    muxer.flush()
    const mp4Data = muxer.finalize()
    muxer.close()

    t.is(getMp4VideoSampleEntryTag(mp4Data), 'hvc1', 'Sample entry should be hvc1')
    const payloads = mdatPayloads(mp4Data)
    t.true(payloads.length > 0, 'Should have mdat payloads')
    const types = payloads.flatMap((p) => hevcNalTypes(p))
    t.true(types.includes(20), 'Samples should still carry slice NALs')
    t.false(
      types.some((nalType) => nalType >= 32 && nalType <= 34),
      `hvc1 samples must not carry VPS/SPS/PPS, got types ${JSON.stringify(types)}`,
    )

    if (fragmented) return

    // The stripped output must still decode (parameter sets come from the hvcC)
    const demuxedChunks: EncodedVideoChunk[] = []
    const demuxer = new Mp4Demuxer({
      videoOutput: (chunk) => demuxedChunks.push(chunk),
      error: (e) => t.fail(`Demuxer error: ${e.message}`),
    })
    await demuxer.loadBuffer(mp4Data)
    const config = demuxer.videoDecoderConfig
    t.truthy(config)
    if (!config) return
    await demuxer.demuxAsync()
    demuxer.close()
    t.is(demuxedChunks.length, frameCount, 'All chunks should demux')

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
    for (const chunk of demuxedChunks) decoder.decode(chunk)
    await decoder.flush()
    decoder.close()
    t.is(decodedFrames.length, frameCount, 'Every frame should decode after stripping')
    for (const frame of decodedFrames) frame.close()
  })
}

/** Rebuild an hvcC with a different configurationVersion, mapping each NAL payload */
function rebuildHvcc(
  hvcc: Uint8Array,
  version: number,
  mapNal: (nalType: number, nal: Uint8Array) => Uint8Array,
): Uint8Array {
  const parts: Uint8Array[] = [hvcc.slice(0, 22)]
  parts[0][0] = version
  const numArrays = hvcc[22]
  const rebuiltArrays: Uint8Array[] = []
  let off = 23
  for (let a = 0; a < numArrays; a++) {
    const headerByte = hvcc[off]
    const nalType = headerByte & 0x3f
    const numNalus = (hvcc[off + 1] << 8) | hvcc[off + 2]
    off += 3
    const nals: Uint8Array[] = []
    for (let n = 0; n < numNalus; n++) {
      const len = (hvcc[off] << 8) | hvcc[off + 1]
      off += 2
      nals.push(mapNal(nalType, hvcc.slice(off, off + len)))
      off += len
    }
    rebuiltArrays.push(Uint8Array.of(headerByte, 0, nals.length))
    for (const nal of nals) {
      rebuiltArrays.push(Uint8Array.of(0, nal.length))
      rebuiltArrays.push(nal)
    }
  }
  parts.push(Uint8Array.of(numArrays), ...rebuiltArrays)
  const out = new Uint8Array(parts.reduce((sum, p) => sum + p.length, 0))
  let w = 0
  for (const p of parts) {
    out.set(p, w)
    w += p.length
  }
  return out
}

test('Mp4Muxer: rejects in-band HEVC parameter set updates under hvc1', async (t) => {
  // Track description comes from a 128x128 session; samples carry the 64x64
  // session's parameter sets in-band — a legal hev1 update that hvc1 cannot
  // represent, so the muxer must reject rather than silently drop it.
  const big = await encodeHevcChunks(128, 128, 3)
  const small = await encodeHevcChunks(64, 64, 3)
  const bigDescription = big.metadatas[0]?.decoderConfig?.description
  const smallDescription = small.metadatas[0]?.decoderConfig?.description
  t.truthy(bigDescription)
  t.truthy(smallDescription)
  if (!bigDescription || !smallDescription) return

  const inBandChunks = prependParameterSets(small.chunks, new Uint8Array(smallDescription))

  const muxer = new Mp4Muxer()
  muxer.addVideoTrack({ codec: 'hev1.1.6.L93.B0', width: 64, height: 64, framerate: 30, description: bigDescription })
  const err = t.throws(() => muxer.addVideoChunk(inBandChunks[0]), { instanceOf: Error })
  t.regex(err?.message ?? '', /parameter set update/)
  muxer.close()
})

test('Mp4Muxer: malformed hvcC variants keep hev1 and preserve parameter sets', async (t) => {
  const { chunks, metadatas } = await encodeHevcChunks(128, 128, 3)
  const description = metadatas[0]?.decoderConfig?.description
  t.truthy(description, 'Encoder should provide an hvcC description')
  if (!description) return
  const hvcc = new Uint8Array(description)
  const inBandChunks = prependParameterSets(chunks, hvcc)

  const variants: [string, Uint8Array][] = [
    ['configurationVersion 0', rebuildHvcc(hvcc, 0, (_nalType, nal) => nal)],
    // VPS array entry shorter than the 2-byte NAL header
    ['one-byte VPS payload', rebuildHvcc(hvcc, 1, (nalType, nal) => (nalType === 32 ? Uint8Array.of(0x40) : nal))],
    // PPS payload (header type 34) inside the VPS array (declared type 32)
    [
      'payload type mismatch',
      rebuildHvcc(hvcc, 1, (nalType, nal) => {
        if (nalType !== 32) return nal
        const pps = extractHevcParameterSets(hvcc).find((n) => ((n[0] >> 1) & 0x3f) === 34)
        return pps ?? nal
      }),
    ],
  ]

  for (const [name, desc] of variants) {
    const muxer = new Mp4Muxer()
    muxer.addVideoTrack({ codec: 'hev1.1.6.L93.B0', width: 128, height: 128, framerate: 30, description: desc })
    for (const chunk of inBandChunks) muxer.addVideoChunk(chunk)
    muxer.flush()
    const mp4Data = muxer.finalize()
    muxer.close()

    t.is(getMp4VideoSampleEntryTag(mp4Data), 'hev1', `${name}: malformed hvcC must not select hvc1`)
    const types = mdatPayloads(mp4Data).flatMap((p) => hevcNalTypes(p))
    t.true(
      types.some((nalType) => nalType >= 32 && nalType <= 34),
      `${name}: hev1 samples should keep in-band parameter sets, got ${JSON.stringify(types)}`,
    )
  }
})

test('Mp4Muxer: multi-layer HEVC alpha keeps hev1 and preserves alpha pixels', async (t) => {
  // Alpha HEVC carries its alpha layer as nonzero nuh_layer_id arrays in the
  // hvcC. movenc drops those arrays when re-writing hvcC (both tags), so the
  // muxer must select hev1 and keep parameter sets in-band — the decoder then
  // recovers the alpha layer from the samples instead of the description.
  const width = 128
  const height = 128
  const alpha = 64
  const chunks: EncodedVideoChunk[] = []
  const metadatas: (EncodedVideoChunkMetadata | undefined)[] = []
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      metadatas.push(metadata)
    },
    error: (e) => t.fail(e.message),
  })
  encoder.configure({
    codec: 'hev1.1.6.L93.B0',
    width,
    height,
    bitrate: 500_000,
    framerate: 30,
    alpha: 'keep',
    hardwareAcceleration: 'prefer-software',
  })
  for (let i = 0; i < 3; i++) {
    const frame = generateSolidColorI420AFrame(width, height, TestColors.green, alpha, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }
  await encoder.flush()
  encoder.close()

  const description = metadatas[0]?.decoderConfig?.description
  t.truthy(description, 'HEVC alpha encoder should provide an hvcC description')
  if (!description) return

  const inBandChunks = prependParameterSets(chunks, new Uint8Array(description))

  for (const fragmented of [false, true]) {
    const muxer = new Mp4Muxer(fragmented ? { fragmented: true } : {})
    muxer.addVideoTrack({ codec: 'hev1.1.6.L93.B0', width, height, framerate: 30, description })
    for (const chunk of inBandChunks) muxer.addVideoChunk(chunk)
    muxer.flush()
    const mp4Data = muxer.finalize()
    muxer.close()

    t.is(
      getMp4VideoSampleEntryTag(mp4Data),
      'hev1',
      `fragmented=${fragmented}: multi-layer hvcC must not select hvc1`,
    )

    const demuxedChunks: EncodedVideoChunk[] = []
    const demuxer = new Mp4Demuxer({
      videoOutput: (chunk) => demuxedChunks.push(chunk),
      error: (e) => t.fail(`Demuxer error: ${e.message}`),
    })
    await demuxer.loadBuffer(mp4Data)
    const config = demuxer.videoDecoderConfig
    t.truthy(config)
    if (!config) {
      demuxer.close()
      continue
    }
    await demuxer.demuxAsync()
    demuxer.close()

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
    for (const chunk of demuxedChunks) decoder.decode(chunk)
    await decoder.flush()
    decoder.close()

    t.is(decodedFrames.length, 3, `fragmented=${fragmented}: all frames decode`)
    t.is(decodedFrames[0]?.format, 'I420A', `fragmented=${fragmented}: alpha format survives`)
    if (decodedFrames[0]?.format === 'I420A') {
      const plane = new Uint8Array(decodedFrames[0].allocationSize({ format: 'I420A' }))
      await decodedFrames[0].copyTo(plane, { format: 'I420A' })
      const alphaOffset = width * height * 1.5
      t.is(plane[alphaOffset], alpha, `fragmented=${fragmented}: alpha pixel preserved`)
    }
    for (const frame of decodedFrames) frame.close()
  }
})

test('Mp4Muxer: rejects malformed HEVC samples under hvc1', async (t) => {
  const { chunks, metadatas } = await encodeHevcChunks(128, 128, 3)
  const description = metadatas[0]?.decoderConfig?.description
  t.truthy(description, 'Encoder should provide an hvcC description')
  if (!description) return

  const data = new Uint8Array(chunks[0].byteLength)
  chunks[0].copyTo(data)

  // One stray trailing byte: the length-prefix walk cannot finish cleanly
  const trailing = new Uint8Array(data.length + 1)
  trailing.set(data)
  trailing[trailing.length - 1] = 0xaa

  // A zero-length NAL prefix before the real data
  const zeroNal = new Uint8Array(4 + data.length)
  zeroNal.set(data, 4)

  for (const [name, bad] of [
    ['trailing byte', trailing],
    ['zero-length NAL', zeroNal],
  ] as const) {
    const muxer = new Mp4Muxer()
    muxer.addVideoTrack({ codec: 'hev1.1.6.L93.B0', width: 128, height: 128, framerate: 30, description })
    const err = t.throws(
      () => muxer.addVideoChunk(new EncodedVideoChunk({ type: chunks[0].type, timestamp: chunks[0].timestamp, data: bad })),
      { instanceOf: Error },
      name,
    )
    t.regex(err?.message ?? '', /malformed HEVC sample/, name)
    muxer.close()
  }
})

/** hvcC with one parameter-set array padded to `count` entries (first repeated) */
function hvccWithPsCount(hvcc: Uint8Array, nalType: number, count: number): Uint8Array {
  const header = hvcc.slice(0, 22)
  const numArrays = hvcc[22]
  const arrays: Uint8Array[] = []
  let off = 23
  for (let a = 0; a < numArrays; a++) {
    const start = off
    const type = hvcc[off] & 0x3f
    const numNalus = (hvcc[off + 1] << 8) | hvcc[off + 2]
    off += 3
    const nals: Uint8Array[] = []
    for (let n = 0; n < numNalus; n++) {
      const len = (hvcc[off] << 8) | hvcc[off + 1]
      off += 2
      nals.push(hvcc.slice(off, off + len))
      off += len
    }
    if (type === nalType) {
      while (nals.length < count) nals.push(nals[0])
    }
    arrays.push(Uint8Array.of(hvcc[start], 0, nals.length))
    for (const nal of nals) {
      arrays.push(Uint8Array.of(0, nal.length))
      arrays.push(nal)
    }
  }
  const parts = [header, Uint8Array.of(numArrays), ...arrays]
  const out = new Uint8Array(parts.reduce((sum, p) => sum + p.length, 0))
  let w = 0
  for (const p of parts) {
    out.set(p, w)
    w += p.length
  }
  return out
}

test('Mp4Muxer: hvc1 honors movenc parameter-set count limits', async (t) => {
  const { chunks, metadatas } = await encodeHevcChunks(128, 128, 3)
  const description = metadatas[0]?.decoderConfig?.description
  t.truthy(description, 'Encoder should provide an hvcC description')
  if (!description) return
  const hvcc = new Uint8Array(description)

  // movenc's hvcc_write rejects records over 16 VPS / 16 SPS / 64 PPS and
  // writes an empty hvcC box; the gate must fall back to hev1 past the limit
  for (const [nalType, atLimit, overLimit] of [
    [32, 16, 17],
    [33, 16, 17],
    [34, 64, 65],
  ] as const) {
    for (const [count, expect] of [
      [atLimit, 'hvc1'],
      [overLimit, 'hev1'],
    ] as const) {
      const muxer = new Mp4Muxer()
      muxer.addVideoTrack({
        codec: 'hev1.1.6.L93.B0',
        width: 128,
        height: 128,
        framerate: 30,
        description: hvccWithPsCount(hvcc, nalType, count),
      })
      for (const chunk of chunks) muxer.addVideoChunk(chunk)
      muxer.flush()
      const mp4Data = muxer.finalize()
      muxer.close()
      t.is(
        getMp4VideoSampleEntryTag(mp4Data),
        expect,
        `NAL type ${nalType} x ${count} should select ${expect}`,
      )
    }
  }
})

test('Mp4Muxer: rejects mid-stream HEVC description changes under hvc1', async (t) => {
  // Fixture: 6 samples at 128x128 then samples referencing a second sample
  // description at 64x64, so mov.c emits AV_PKT_DATA_NEW_EXTRADATA on the
  // first boundary sample. The muxer must reject the change under hvc1
  // instead of dropping the side data and writing the stale description.
  const fixture = readFileSync(join(__dirname, 'fixtures/hevc-midstream-description-change.mp4'))

  const demuxedChunks: EncodedVideoChunk[] = []
  const demuxer = new Mp4Demuxer({
    videoOutput: (chunk) => demuxedChunks.push(chunk),
    error: (e) => t.fail(`Demuxer error: ${e.message}`),
  })
  await demuxer.loadBuffer(fixture)
  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Fixture should expose a video decoder config')
  if (!config) {
    demuxer.close()
    return
  }
  await demuxer.demuxAsync()
  demuxer.close()
  t.true(demuxedChunks.length > 6, 'Fixture should demux the pre-change samples plus the boundary sample')

  const muxer = new Mp4Muxer()
  muxer.addVideoTrack({
    codec: config.codec,
    width: config.codedWidth,
    height: config.codedHeight,
    framerate: 30,
    description: config.description,
  })
  // Pre-change samples carry no side data and mux normally
  for (let i = 0; i < demuxedChunks.length - 1; i++) {
    muxer.addVideoChunk(demuxedChunks[i])
  }
  // The boundary sample carries the new description and must be rejected
  const err = t.throws(
    () => muxer.addVideoChunk(demuxedChunks[demuxedChunks.length - 1]),
    { instanceOf: Error },
  )
  t.regex(err?.message ?? '', /description changed mid-stream/)
  muxer.close()
})

/** elst segment_duration in milliseconds (movie timescale from mvhd) */
function elstSegmentDurationMs(mp4: Uint8Array): number {
  const text = Buffer.from(mp4).toString('latin1')
  const view = new DataView(mp4.buffer, mp4.byteOffset)
  const elst = text.indexOf('elst')
  if (elst < 0) throw new Error('no elst box')
  const mvhd = text.indexOf('mvhd')
  const timescale = view.getUint32(mvhd + 16)
  return (view.getUint32(elst + 12) / timescale) * 1000
}

test('Mp4Muxer: hvc1 strip preserves edit-list trimming of discarded tail samples', async (t) => {
  // Fixture: 6 samples trimmed to 3 by its edit list, with duplicate in-band
  // parameter sets in sample 4 (a discarded delta sample). Stripping must not
  // lose AV_PKT_FLAG_DISCARD, or movenc would re-extend the output edit list
  // to the trimmed tail.
  const fixture = readFileSync(join(__dirname, 'fixtures/hevc-trimmed-tail.mp4'))
  const demuxedChunks: EncodedVideoChunk[] = []
  const demuxer = new Mp4Demuxer({
    videoOutput: (chunk) => demuxedChunks.push(chunk),
    error: (e) => t.fail(`Demuxer error: ${e.message}`),
  })
  await demuxer.loadBuffer(fixture)
  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Fixture should expose a video decoder config')
  t.truthy(config?.description, 'Fixture should carry an hvcC description')
  if (!config?.description) {
    demuxer.close()
    return
  }
  await demuxer.demuxAsync()
  demuxer.close()
  t.is(demuxedChunks.length, 6, 'Fixture should demux all six samples')

  const muxWith = (description?: Uint8Array) => {
    const muxer = new Mp4Muxer()
    muxer.addVideoTrack({
      codec: config.codec,
      width: config.codedWidth,
      height: config.codedHeight,
      framerate: 30,
      description,
    })
    for (const chunk of demuxedChunks) muxer.addVideoChunk(chunk)
    muxer.flush()
    const out = muxer.finalize()
    muxer.close()
    return out
  }

  const hvcc = new Uint8Array(config.description)
  // hev1 control (incomplete hvcC keeps the strip inactive): flags untouched
  const control = elstSegmentDurationMs(muxWith(hvccWithoutPps(hvcc)))
  // hvc1 path strips sample 4 and must keep its DISCARD flag
  const hvc1 = elstSegmentDurationMs(muxWith(hvcc))

  t.true(control > 0 && control < 150, `control edit list should keep the 3-frame trim, got ${control}ms`)
  t.is(hvc1, control, 'hvc1 strip must preserve the trimmed edit list')
})

test('Mp4Muxer: hvc1 strip preserves edit-list trimming of a discarded keyframe', async (t) => {
  // Fixture: head-trimmed edit list (media_time skips one frame) with
  // duplicate in-band parameter sets in sample 1 — the skipped keyframe.
  // Stripping must not lose KEY|DISCARD: keyframe marking must OR, not
  // assign, or the trimmed head frame is restored in the output edit list.
  const fixture = readFileSync(join(__dirname, 'fixtures/hevc-trimmed-head.mp4'))
  const demuxedChunks: EncodedVideoChunk[] = []
  const demuxer = new Mp4Demuxer({
    videoOutput: (chunk) => demuxedChunks.push(chunk),
    error: (e) => t.fail(`Demuxer error: ${e.message}`),
  })
  await demuxer.loadBuffer(fixture)
  const config = demuxer.videoDecoderConfig
  t.truthy(config, 'Fixture should expose a video decoder config')
  t.truthy(config?.description, 'Fixture should carry an hvcC description')
  if (!config?.description) {
    demuxer.close()
    return
  }
  await demuxer.demuxAsync()
  demuxer.close()

  const muxWith = (description?: Uint8Array) => {
    const muxer = new Mp4Muxer()
    muxer.addVideoTrack({
      codec: config.codec,
      width: config.codedWidth,
      height: config.codedHeight,
      framerate: 30,
      description,
    })
    for (const chunk of demuxedChunks) muxer.addVideoChunk(chunk)
    muxer.flush()
    const out = muxer.finalize()
    muxer.close()
    return out
  }

  const hvcc = new Uint8Array(config.description)
  const elstMediaTime = (mp4: Uint8Array): number => {
    const text = Buffer.from(mp4).toString('latin1')
    const view = new DataView(mp4.buffer, mp4.byteOffset)
    return view.getUint32(text.indexOf('elst') + 16)
  }

  const control = elstMediaTime(muxWith(hvccWithoutPps(hvcc)))
  const hvc1 = elstMediaTime(muxWith(hvcc))
  t.is(control, 512, 'control should keep the one-frame head trim')
  t.is(hvc1, control, 'hvc1 strip must preserve the trimmed head keyframe')
})

test('Mp4Muxer: rejects metadata description changes under hvc1', async (t) => {
  // Encoder chunks carry their description via metadata.decoderConfig; a
  // changed description mid-stream must be rejected under hvc1 just like
  // AV_PKT_DATA_NEW_EXTRADATA, instead of silently updating codecpar after
  // movenc captured the sample description.
  async function encodeHevcAt(width: number, height: number) {
    const chunks: EncodedVideoChunk[] = []
    const metadatas: (EncodedVideoChunkMetadata | undefined)[] = []
    const encoder = new VideoEncoder({
      output: (chunk, metadata) => {
        chunks.push(chunk)
        metadatas.push(metadata)
      },
      error: (e) => t.fail(e.message),
    })
    encoder.configure({
      codec: 'hev1.1.6.L93.B0',
      width,
      height,
      bitrate: 500_000,
      framerate: 30,
      hardwareAcceleration: 'prefer-software',
    })
    for (let i = 0; i < 3; i++) {
      const frame = generateSolidColorI420Frame(width, height, TestColors.green, i * 33333)
      encoder.encode(frame, { keyFrame: i === 0 })
      frame.close()
    }
    await encoder.flush()
    encoder.close()
    return { chunks, metadatas }
  }

  const big = await encodeHevcAt(128, 128)
  const small = await encodeHevcAt(64, 64)
  const description = big.metadatas[0]?.decoderConfig?.description
  t.truthy(description)
  if (!description) return

  const muxer = new Mp4Muxer()
  muxer.addVideoTrack({
    codec: 'hev1.1.6.L93.B0',
    width: 128,
    height: 128,
    framerate: 30,
    description,
  })
  for (let i = 0; i < 3; i++) muxer.addVideoChunk(big.chunks[i], big.metadatas[i])
  const err = t.throws(
    () => muxer.addVideoChunk(small.chunks[0], small.metadatas[0]),
    { instanceOf: Error },
  )
  t.regex(err?.message ?? '', /description changed mid-stream/)

  // The rejection must not corrupt muxer timing state: chunks added after
  // the failed call keep their own timestamps rather than extending the
  // rejected chunk's position.
  for (let i = 1; i < 3; i++) muxer.addVideoChunk(big.chunks[i], big.metadatas[i])
  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()
  const text = Buffer.from(mp4Data).toString('latin1')
  const view = new DataView(mp4Data.buffer, mp4Data.byteOffset)
  const mvhd = text.indexOf('mvhd')
  const durationMs = (view.getUint32(mvhd + 20) / view.getUint32(mvhd + 16)) * 1000
  t.true(durationMs > 50 && durationMs < 150, `output should stay 3 frames (~100ms), got ${durationMs}ms`)

  // A rejected FIRST chunk must not commit the header: the muxer stays in
  // the configuring state, streaming output stays empty, and tracks can
  // still be added.
  const streaming = new Mp4Muxer({ fragmented: true, streaming: { bufferCapacity: 64 * 1024 } })
  streaming.addVideoTrack({
    codec: 'hev1.1.6.L93.B0',
    width: 128,
    height: 128,
    framerate: 30,
    description,
  })
  t.throws(
    () => streaming.addVideoChunk(small.chunks[0], small.metadatas[0]),
    { instanceOf: Error },
  )
  t.is(streaming.state, 'configuring', 'rejected first chunk must leave the muxer configuring')
  t.falsy(streaming.read(), 'no header bytes may be emitted before a chunk is accepted')
  t.notThrows(() => {
    streaming.addAudioTrack({ codec: 'mp4a.40.2', sampleRate: 48000, numberOfChannels: 2 })
  }, 'tracks must still be addable after a rejected first chunk')
  streaming.close()
})

test('Mp4Muxer: keeps hev1 and preserves in-band parameter sets when hvcC is incomplete', async (t) => {
  const { chunks, metadatas } = await encodeHevcChunks(128, 128, 3)
  const description = metadatas[0]?.decoderConfig?.description
  t.truthy(description, 'Encoder should provide an hvcC description')
  if (!description) return

  const incomplete = hvccWithoutPps(new Uint8Array(description))
  const inBandChunks = prependParameterSets(chunks, new Uint8Array(description))

  const muxer = new Mp4Muxer()
  muxer.addVideoTrack({ codec: 'hev1.1.6.L93.B0', width: 128, height: 128, framerate: 30, description: incomplete })
  for (const chunk of inBandChunks) muxer.addVideoChunk(chunk)
  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  // Without a complete VPS/SPS/PPS set in the hvcC the muxer must not claim hvc1
  t.is(getMp4VideoSampleEntryTag(mp4Data), 'hev1', 'Sample entry should stay hev1')
  const types = mdatPayloads(mp4Data).flatMap((p) => hevcNalTypes(p))
  t.true(
    types.some((nalType) => nalType >= 32 && nalType <= 34),
    `hev1 samples should keep their in-band parameter sets, got types ${JSON.stringify(types)}`,
  )
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
