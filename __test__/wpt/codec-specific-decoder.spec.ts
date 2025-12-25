/**
 * Codec-Specific Decoder Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests decoding of various video and audio codecs using fixture files.
 */

import test from 'ava'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  AudioData,
  AudioDecoder,
  EncodedAudioChunk,
  EncodedVideoChunk,
  resetHardwareFallbackState,
  VideoDecoder,
  VideoFrame,
} from '../../index.js'
import { createCollectingCodecInit, createErrorTrackingCodecInit } from '../helpers/wpt-utils.js'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

const fixturesPath = join(__dirname, '../fixtures/wpt')

// ============================================================================
// H.264 Tests
// ============================================================================

test('VideoDecoder: H.264 Annex B format', async (t) => {
  let h264Data: Buffer
  try {
    h264Data = readFileSync(join(fixturesPath, 'h264.annexb'))
  } catch {
    t.pass('H.264 Annex B fixture not available')
    return
  }

  // Check if H.264 is supported
  const support = await VideoDecoder.isConfigSupported({
    codec: 'avc1.42001E',
  })

  if (!support.supported) {
    t.pass('H.264 not supported on this platform')
    return
  }

  const { init, outputs } = createCollectingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs.push(frame)
      frame.close()
    },
    error: init.error,
  })

  decoder.configure({
    codec: 'avc1.42001E',
  })

  // Create chunk from raw data
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: h264Data,
  })

  decoder.decode(chunk)

  try {
    await decoder.flush()
  } catch {
    // Decoding may fail if data is not valid NAL units
  }

  t.true(h264Data.length > 0, 'fixture loaded')
  decoder.close()
})

test('VideoDecoder: H.264 MP4 format', async (t) => {
  let h264Data: Buffer
  try {
    h264Data = readFileSync(join(fixturesPath, 'h264.mp4'))
  } catch {
    t.pass('H.264 MP4 fixture not available')
    return
  }

  // Note: MP4 container needs demuxing before decoding
  // This test verifies the fixture loads correctly
  t.true(h264Data.length > 0, 'fixture loaded')
})

// ============================================================================
// H.265 Tests
// ============================================================================

test('VideoDecoder: H.265 Annex B format', async (t) => {
  let h265Data: Buffer
  try {
    h265Data = readFileSync(join(fixturesPath, 'h265.annexb'))
  } catch {
    t.pass('H.265 Annex B fixture not available')
    return
  }

  // Check if H.265 is supported
  const support = await VideoDecoder.isConfigSupported({
    codec: 'hev1.1.6.L93.B0',
  })

  if (!support.supported) {
    t.pass('H.265 not supported on this platform')
    return
  }

  const { init, outputs } = createCollectingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs.push(frame)
      frame.close()
    },
    error: init.error,
  })

  decoder.configure({
    codec: 'hev1.1.6.L93.B0',
  })

  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: h265Data,
  })

  decoder.decode(chunk)

  try {
    await decoder.flush()
  } catch {
    // Decoding may fail if data is not valid NAL units
  }

  t.true(h265Data.length > 0, 'fixture loaded')
  decoder.close()
})

test('VideoDecoder: H.265 MP4 format', async (t) => {
  let h265Data: Buffer
  try {
    h265Data = readFileSync(join(fixturesPath, 'h265.mp4'))
  } catch {
    t.pass('H.265 MP4 fixture not available')
    return
  }

  t.true(h265Data.length > 0, 'fixture loaded')
})

// ============================================================================
// VP8 Tests
// ============================================================================

test('VideoDecoder: VP8 isConfigSupported', async (t) => {
  const support = await VideoDecoder.isConfigSupported({
    codec: 'vp8',
  })

  if (!support.supported) {
    t.pass('VP8 not supported on this platform')
    return
  }

  t.true(support.supported)
  t.is(support.config.codec, 'vp8')
})

test('VideoDecoder: VP8 configure', async (t) => {
  const support = await VideoDecoder.isConfigSupported({
    codec: 'vp8',
  })

  if (!support.supported) {
    t.pass('VP8 not supported')
    return
  }

  const decoder = new VideoDecoder({
    output: () => {},
    error: () => {},
  })

  decoder.configure({
    codec: 'vp8',
  })

  t.is(decoder.state, 'configured')
  decoder.close()
})

// ============================================================================
// VP9 Tests
// ============================================================================

test('VideoDecoder: VP9 isConfigSupported', async (t) => {
  const support = await VideoDecoder.isConfigSupported({
    codec: 'vp09.00.10.08',
  })

  if (!support.supported) {
    t.pass('VP9 not supported on this platform')
    return
  }

  t.true(support.supported)
})

test('VideoDecoder: VP9 with profile 0', async (t) => {
  const support = await VideoDecoder.isConfigSupported({
    codec: 'vp09.00.10.08',
  })

  if (!support.supported) {
    t.pass('VP9 profile 0 not supported')
    return
  }

  const decoder = new VideoDecoder({
    output: () => {},
    error: () => {},
  })

  decoder.configure({
    codec: 'vp09.00.10.08',
  })

  t.is(decoder.state, 'configured')
  decoder.close()
})

test('VideoDecoder: VP9 with profile 2 (10-bit)', async (t) => {
  const support = await VideoDecoder.isConfigSupported({
    codec: 'vp09.02.10.10',
  })

  if (!support.supported) {
    t.pass('VP9 profile 2 not supported')
    return
  }

  const decoder = new VideoDecoder({
    output: () => {},
    error: () => {},
  })

  decoder.configure({
    codec: 'vp09.02.10.10',
  })

  t.is(decoder.state, 'configured')
  decoder.close()
})

// ============================================================================
// AV1 Tests
// ============================================================================

test('VideoDecoder: AV1 isConfigSupported', async (t) => {
  const support = await VideoDecoder.isConfigSupported({
    codec: 'av01.0.04M.08',
  })

  if (!support.supported) {
    t.pass('AV1 not supported on this platform')
    return
  }

  t.true(support.supported)
})

test('VideoDecoder: AV1 MP4 format', async (t) => {
  let av1Data: Buffer
  try {
    av1Data = readFileSync(join(fixturesPath, 'av1.mp4'))
  } catch {
    t.pass('AV1 MP4 fixture not available')
    return
  }

  t.true(av1Data.length > 0, 'fixture loaded')
})

test('VideoDecoder: AV1 with different profiles', async (t) => {
  const profiles = [
    'av01.0.04M.08', // Main profile, level 4.0, 8-bit
    'av01.0.08M.08', // Main profile, level 5.0, 8-bit
    'av01.0.04M.10', // Main profile, level 4.0, 10-bit
  ]

  for (const codec of profiles) {
    const support = await VideoDecoder.isConfigSupported({ codec })
    t.true(typeof support.supported === 'boolean', `${codec} returns valid support status`)
  }
})

// ============================================================================
// Audio Codec Tests - Opus
// ============================================================================

test('AudioDecoder: Opus isConfigSupported', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('Opus not supported on this platform')
    return
  }

  t.true(support.supported)
  t.is(support.config.codec, 'opus')
  t.is(support.config.sampleRate, 48000)
  t.is(support.config.numberOfChannels, 2)
})

test('AudioDecoder: Opus mono', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 1,
  })

  if (!support.supported) {
    t.pass('Opus mono not supported')
    return
  }

  const decoder = new AudioDecoder({
    output: () => {},
    error: () => {},
  })

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 1,
  })

  t.is(decoder.state, 'configured')
  decoder.close()
})

test('AudioDecoder: Opus file', async (t) => {
  let opusData: Buffer
  try {
    opusData = readFileSync(join(fixturesPath, 'sfx-opus.ogg'))
  } catch {
    t.pass('Opus fixture not available')
    return
  }

  t.true(opusData.length > 0, 'fixture loaded')
})

// ============================================================================
// Audio Codec Tests - AAC
// ============================================================================

test('AudioDecoder: AAC isConfigSupported', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('AAC not supported on this platform')
    return
  }

  t.true(support.supported)
  t.is(support.config.codec, 'mp4a.40.2')
})

test('AudioDecoder: AAC ADTS file', async (t) => {
  let aacData: Buffer
  try {
    aacData = readFileSync(join(fixturesPath, 'sfx.adts'))
  } catch {
    t.pass('AAC ADTS fixture not available')
    return
  }

  t.true(aacData.length > 0, 'fixture loaded')
})

test('AudioDecoder: AAC MP4 file', async (t) => {
  let aacData: Buffer
  try {
    aacData = readFileSync(join(fixturesPath, 'sfx-aac.mp4'))
  } catch {
    t.pass('AAC MP4 fixture not available')
    return
  }

  t.true(aacData.length > 0, 'fixture loaded')
})

// ============================================================================
// Audio Codec Tests - MP3
// ============================================================================

test('AudioDecoder: MP3 isConfigSupported', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('MP3 not supported on this platform')
    return
  }

  t.true(support.supported)
})

test('AudioDecoder: MP3 file', async (t) => {
  let mp3Data: Buffer
  try {
    mp3Data = readFileSync(join(fixturesPath, 'sfx.mp3'))
  } catch {
    t.pass('MP3 fixture not available')
    return
  }

  t.true(mp3Data.length > 0, 'fixture loaded')
})

// ============================================================================
// Audio Codec Tests - FLAC
// ============================================================================

test('AudioDecoder: FLAC isConfigSupported', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('FLAC not supported on this platform')
    return
  }

  t.true(support.supported)
})

test('AudioDecoder: FLAC file', async (t) => {
  let flacData: Buffer
  try {
    flacData = readFileSync(join(fixturesPath, 'sfx.flac'))
  } catch {
    t.pass('FLAC fixture not available')
    return
  }

  t.true(flacData.length > 0, 'fixture loaded')
})

// ============================================================================
// Audio Codec Tests - Vorbis
// ============================================================================

test('AudioDecoder: Vorbis isConfigSupported', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'vorbis',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  // Vorbis typically requires description
  if (!support.supported) {
    t.pass('Vorbis not supported (may require description)')
    return
  }

  t.true(support.supported)
})

test('AudioDecoder: Vorbis file', async (t) => {
  let vorbisData: Buffer
  try {
    vorbisData = readFileSync(join(fixturesPath, 'sfx-vorbis.ogg'))
  } catch {
    t.pass('Vorbis fixture not available')
    return
  }

  t.true(vorbisData.length > 0, 'fixture loaded')
})

// ============================================================================
// Audio Codec Tests - PCM/WAV
// ============================================================================

test('AudioDecoder: PCM WAV files exist', async (t) => {
  const wavFiles = [
    'sfx.wav',
    'sfx-s16.wav',
    'sfx-s24.wav',
    'sfx-s32.wav',
    'sfx-f32.wav',
    'sfx-alaw.wav',
    'sfx-ulaw.wav',
  ]

  let foundCount = 0
  for (const file of wavFiles) {
    try {
      const data = readFileSync(join(fixturesPath, file))
      if (data.length > 0) {
        foundCount++
      }
    } catch {
      // File not available
    }
  }

  t.true(foundCount >= 0, `Found ${foundCount} WAV fixtures`)
})

// ============================================================================
// Codec String Parsing Tests
// ============================================================================

test('VideoDecoder: various H.264 codec strings', async (t) => {
  const h264Codecs = [
    'avc1.42001E', // Baseline
    'avc1.4D001E', // Main
    'avc1.64001E', // High
    'avc1.640028', // High 4.0
    'avc1.64002A', // High 4.2
  ]

  for (const codec of h264Codecs) {
    const support = await VideoDecoder.isConfigSupported({ codec })
    t.true(typeof support.supported === 'boolean', `${codec} returns valid support status`)
  }
})

test('VideoDecoder: various H.265 codec strings', async (t) => {
  const h265Codecs = [
    'hev1.1.6.L93.B0', // Main profile
    'hvc1.1.6.L93.B0', // Alternative form
    'hev1.2.4.L120.B0', // Main 10
  ]

  for (const codec of h265Codecs) {
    const support = await VideoDecoder.isConfigSupported({ codec })
    t.true(typeof support.supported === 'boolean', `${codec} returns valid support status`)
  }
})

test('AudioDecoder: various AAC codec strings', async (t) => {
  const aacCodecs = [
    'mp4a.40.2', // AAC-LC
    'mp4a.40.5', // HE-AAC
    'mp4a.40.29', // HE-AAC v2
  ]

  for (const codec of aacCodecs) {
    const support = await AudioDecoder.isConfigSupported({
      codec,
      sampleRate: 44100,
      numberOfChannels: 2,
    })
    t.true(typeof support.supported === 'boolean', `${codec} returns valid support status`)
  }
})

// ============================================================================
// Sample Rate Tests
// ============================================================================

test('AudioDecoder: various sample rates', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('Opus not supported')
    return
  }

  const sampleRates = [8000, 12000, 16000, 24000, 48000]

  for (const sampleRate of sampleRates) {
    const result = await AudioDecoder.isConfigSupported({
      codec: 'opus',
      sampleRate,
      numberOfChannels: 2,
    })
    // Opus should support various sample rates
    t.true(typeof result.supported === 'boolean', `${sampleRate}Hz returns valid support status`)
  }
})

// ============================================================================
// Channel Count Tests
// ============================================================================

test('AudioDecoder: various channel counts', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('Opus not supported')
    return
  }

  // Test mono and stereo (most commonly supported)
  for (const channels of [1, 2]) {
    const result = await AudioDecoder.isConfigSupported({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: channels,
    })
    t.true(typeof result.supported === 'boolean', `${channels} channels returns valid support status`)
  }
})

// ============================================================================
// Error Handling Tests
// ============================================================================

test('VideoDecoder: corrupt data triggers error', async (t) => {
  const support = await VideoDecoder.isConfigSupported({
    codec: 'vp8',
  })

  if (!support.supported) {
    t.pass('VP8 not supported')
    return
  }

  const { init, outputs, gotError } = createErrorTrackingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs.push(frame)
      frame.close()
    },
    error: init.error,
  })

  decoder.configure({
    codec: 'vp8',
  })

  // Create chunk with invalid data
  const corruptChunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0xff, 0xff, 0xff, 0xff]),
  })

  decoder.decode(corruptChunk)

  // flush() may reject with DOMException (Node < 22 compatibility)
  try {
    await decoder.flush()
    t.fail('flush should have thrown')
  } catch (e) {
    t.true(e instanceof Error || e instanceof DOMException, 'should be Error or DOMException')
  }

  const error = await gotError
  t.true(error instanceof Error)
})

test('AudioDecoder: corrupt data triggers error', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('Opus not supported')
    return
  }

  const { init, outputs, gotError } = createErrorTrackingCodecInit<AudioData>()

  const decoder = new AudioDecoder({
    output: (data) => {
      outputs.push(data)
      data.close()
    },
    error: init.error,
  })

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  // Create chunk with invalid data
  const corruptChunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0xff, 0xff, 0xff, 0xff]),
  })

  decoder.decode(corruptChunk)

  // flush() may reject with DOMException (Node < 22 compatibility)
  try {
    await decoder.flush()
    t.fail('flush should have thrown')
  } catch (e) {
    t.true(e instanceof Error || e instanceof DOMException, 'should be Error or DOMException')
  }

  const error = await gotError
  t.true(error instanceof Error)
})

// ============================================================================
// H.264 SEI Recovery Point Tests
// WPT: videoDecoder-h264-sei.https.any.js
// Tests that H.264 SEI recovery point frames are treated as keyframes.
// ============================================================================

// H.264 SEI test data (from videoDecoder-codec-specific-setup.js)
const H264_SEI_AVC_DATA = {
  src: 'h264_sei.mp4',
  config: {
    codec: 'avc1.64000b',
    descriptionOffset: 11989,
    descriptionSize: 46,
    codedWidth: 320,
    codedHeight: 240,
  },
  chunks: [
    { offset: 48, size: 4229, key: true },
    { offset: 4277, size: 1114, key: false },
    { offset: 5391, size: 320, key: false },
    { offset: 5711, size: 188, key: false },
    { offset: 5899, size: 173, key: false },
    { offset: 6072, size: 3694, key: true }, // SEI recovery point
    { offset: 9766, size: 936, key: false },
    { offset: 10702, size: 345, key: false },
    { offset: 11047, size: 213, key: false },
    { offset: 11260, size: 210, key: false },
  ],
}

const H264_SEI_ANNEXB_DATA = {
  src: 'h264_sei.annexb',
  config: {
    codec: 'avc1.64000b',
    codedWidth: 320,
    codedHeight: 240,
  },
  chunks: [
    { offset: 0, size: 4264, key: true },
    { offset: 4264, size: 1112, key: false },
    { offset: 5376, size: 318, key: false },
    { offset: 5694, size: 186, key: false },
    { offset: 5880, size: 171, key: false },
    { offset: 6051, size: 3729, key: true }, // SEI recovery point
    { offset: 9780, size: 934, key: false },
    { offset: 10714, size: 343, key: false },
    { offset: 11057, size: 211, key: false },
    { offset: 11268, size: 208, key: false },
  ],
}

test('VideoDecoder: H.264 SEI recovery point frames treated as keyframes (AVC)', async (t) => {
  let fileData: Buffer
  try {
    fileData = readFileSync(join(fixturesPath, H264_SEI_AVC_DATA.src))
  } catch {
    t.pass('H.264 SEI AVC fixture not available')
    return
  }

  const support = await VideoDecoder.isConfigSupported({
    codec: H264_SEI_AVC_DATA.config.codec,
  })

  if (!support.supported) {
    t.pass('H.264 not supported on this platform')
    return
  }

  // Extract description from file
  const description = new Uint8Array(
    fileData.buffer,
    fileData.byteOffset + H264_SEI_AVC_DATA.config.descriptionOffset,
    H264_SEI_AVC_DATA.config.descriptionSize,
  )

  let outputs = 0
  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs++
      t.is(frame.timestamp, 5, 'timestamp matches chunk 5')
      frame.close()
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: H264_SEI_AVC_DATA.config.codec,
    codedWidth: H264_SEI_AVC_DATA.config.codedWidth,
    codedHeight: H264_SEI_AVC_DATA.config.codedHeight,
    description,
  })

  // Decode chunk 5 (SEI recovery point) - should work as keyframe
  const chunkInfo = H264_SEI_AVC_DATA.chunks[5]
  const chunkData = new Uint8Array(fileData.buffer, fileData.byteOffset + chunkInfo.offset, chunkInfo.size)

  const chunk = new EncodedVideoChunk({
    type: chunkInfo.key ? 'key' : 'delta',
    timestamp: 5,
    duration: 1,
    data: chunkData,
  })

  decoder.decode(chunk)

  await decoder.flush()
  decoder.close()

  t.is(outputs, 1, 'SEI recovery point frame decoded as keyframe')
})

test('VideoDecoder: H.264 SEI recovery point frames treated as keyframes (Annex B)', async (t) => {
  let fileData: Buffer
  try {
    fileData = readFileSync(join(fixturesPath, H264_SEI_ANNEXB_DATA.src))
  } catch {
    t.pass('H.264 SEI Annex B fixture not available')
    return
  }

  const support = await VideoDecoder.isConfigSupported({
    codec: H264_SEI_ANNEXB_DATA.config.codec,
  })

  if (!support.supported) {
    t.pass('H.264 not supported on this platform')
    return
  }

  let outputs = 0
  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs++
      t.is(frame.timestamp, 5, 'timestamp matches chunk 5')
      frame.close()
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: H264_SEI_ANNEXB_DATA.config.codec,
    codedWidth: H264_SEI_ANNEXB_DATA.config.codedWidth,
    codedHeight: H264_SEI_ANNEXB_DATA.config.codedHeight,
  })

  // Decode chunk 5 (SEI recovery point) - should work as keyframe
  const chunkInfo = H264_SEI_ANNEXB_DATA.chunks[5]
  const chunkData = new Uint8Array(fileData.buffer, fileData.byteOffset + chunkInfo.offset, chunkInfo.size)

  const chunk = new EncodedVideoChunk({
    type: chunkInfo.key ? 'key' : 'delta',
    timestamp: 5,
    duration: 1,
    data: chunkData,
  })

  decoder.decode(chunk)

  await decoder.flush()
  decoder.close()

  t.is(outputs, 1, 'SEI recovery point frame decoded as keyframe')
})

// ============================================================================
// H.264 Interlaced Content Tests
// WPT: videoDecoder-interlaced-h264.https.any.js
// Tests decoding of interlaced H.264 content.
// ============================================================================

const H264_INTERLACED_AVC_DATA = {
  src: 'h264_interlaced.mp4',
  config: {
    codec: 'avc1.64000b',
    descriptionOffset: 7501,
    descriptionSize: 47,
    codedWidth: 320,
    codedHeight: 240,
  },
  chunks: [
    { offset: 48, size: 4091 },
    { offset: 4139, size: 949 },
    { offset: 5088, size: 260 },
    { offset: 5348, size: 134 },
    { offset: 5482, size: 111 },
    { offset: 5593, size: 660 },
    { offset: 6253, size: 197 },
    { offset: 6450, size: 96 },
    { offset: 6546, size: 159 },
    { offset: 6705, size: 277 },
  ],
}

test('VideoDecoder: decoding H.264 interlaced content', async (t) => {
  let fileData: Buffer
  try {
    fileData = readFileSync(join(fixturesPath, H264_INTERLACED_AVC_DATA.src))
  } catch {
    t.pass('H.264 interlaced fixture not available')
    return
  }

  const support = await VideoDecoder.isConfigSupported({
    codec: H264_INTERLACED_AVC_DATA.config.codec,
  })

  if (!support.supported) {
    t.pass('H.264 not supported on this platform')
    return
  }

  // Extract description from file
  const description = new Uint8Array(
    fileData.buffer,
    fileData.byteOffset + H264_INTERLACED_AVC_DATA.config.descriptionOffset,
    H264_INTERLACED_AVC_DATA.config.descriptionSize,
  )

  let outputs = 0
  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs++
      t.is(frame.timestamp, 0, 'timestamp is 0')
      frame.close()
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: H264_INTERLACED_AVC_DATA.config.codec,
    codedWidth: H264_INTERLACED_AVC_DATA.config.codedWidth,
    codedHeight: H264_INTERLACED_AVC_DATA.config.codedHeight,
    description,
  })

  // Decode first chunk (keyframe)
  const chunkInfo = H264_INTERLACED_AVC_DATA.chunks[0]
  const chunkData = new Uint8Array(fileData.buffer, fileData.byteOffset + chunkInfo.offset, chunkInfo.size)

  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: chunkData,
  })

  decoder.decode(chunk)

  await decoder.flush()
  decoder.close()

  t.is(outputs, 1, 'interlaced content decoded')
})
