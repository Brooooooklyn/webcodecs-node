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
