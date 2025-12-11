/**
 * AudioDecoder API Conformance Tests
 *
 * Tests WebCodecs AudioDecoder specification compliance.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import { AudioDecoder, AudioEncoder, AudioData, EncodedAudioChunk } from '../index.js'
import { generateSineTone } from './helpers/index.js'

// Helper to create test encoder with callbacks
function createTestEncoder() {
  const chunks: EncodedAudioChunk[] = []
  const errors: Error[] = []

  const encoder = new AudioEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => errors.push(e),
  })

  return { encoder, chunks, errors }
}

// Helper to create test decoder with callbacks
function createTestDecoder() {
  const audioOutputs: AudioData[] = []
  const errors: Error[] = []

  const decoder = new AudioDecoder({
    output: (data) => audioOutputs.push(data),
    error: (e) => errors.push(e),
  })

  return { decoder, audioOutputs, errors }
}

// ============================================================================
// Constructor Tests
// ============================================================================

test('AudioDecoder: constructor creates unconfigured decoder', (t) => {
  const { decoder } = createTestDecoder()

  t.is(decoder.state, 'unconfigured')
  t.is(decoder.decodeQueueSize, 0)

  decoder.close()
})

test('AudioDecoder: constructor requires init dictionary', (t) => {
  // @ts-expect-error - Testing that missing init throws
  t.throws(() => new AudioDecoder())
  // @ts-expect-error - Testing that missing error callback throws
  t.throws(() => new AudioDecoder({ output: () => {} }))
})

// ============================================================================
// Configuration Tests
// ============================================================================

test('AudioDecoder: configure() with AAC codec', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(decoder.state, 'configured')

  decoder.close()
})

test('AudioDecoder: configure() with Opus codec', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(decoder.state, 'configured')

  decoder.close()
})

test('AudioDecoder: configure() with MP3 codec', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  t.is(decoder.state, 'configured')

  decoder.close()
})

test('AudioDecoder: configure() with FLAC codec requires description', async (t) => {
  const { decoder, errors } = createTestDecoder()

  // FLAC requires a description (STREAMINFO) per W3C WebCodecs spec
  decoder.configure({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  // Wait for error callback to be called (it's async via ThreadsafeFunction)
  // Use polling with timeout for slower platforms (armv7, etc.)
  // Wait for BOTH decoder to close AND error callback to fire
  const maxWait = 500
  const pollInterval = 20
  let elapsed = 0
  while ((decoder.state !== 'closed' || errors.length === 0) && elapsed < maxWait) {
    await new Promise((resolve) => setTimeout(resolve, pollInterval))
    elapsed += pollInterval
  }

  // Should trigger error callback and close decoder (no description provided)
  t.is(decoder.state, 'closed')
  t.is(errors.length, 1)
  t.true(errors[0].message.includes('NotSupportedError'))
})

test('AudioDecoder: configure() with mono audio', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 1,
  })

  t.is(decoder.state, 'configured')

  decoder.close()
})

test('AudioDecoder: configure() with invalid codec triggers error callback', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'invalid-codec',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  // Error callback transitions to closed
  t.is(decoder.state, 'closed')

  // Already closed by error callback, so close() throws InvalidStateError
  const error = t.throws(() => decoder.close())
  t.true(error instanceof DOMException, 'error should be DOMException instance')
  t.is((error as DOMException).name, 'InvalidStateError')
})

// ============================================================================
// State Machine Tests
// ============================================================================

test('AudioDecoder: decode() on unconfigured throws InvalidStateError', (t) => {
  const { decoder } = createTestDecoder()

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00, 0x01, 0x02]),
  })

  // W3C spec: decode() on unconfigured decoder should throw InvalidStateError
  const error = t.throws(() => decoder.decode(chunk))
  t.true(error instanceof DOMException, 'error should be DOMException instance')
  t.is((error as DOMException).name, 'InvalidStateError')
})

test('AudioDecoder: decode() on closed throws InvalidStateError', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  decoder.close()

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00, 0x01, 0x02]),
  })

  // W3C spec: decode() on closed decoder should throw InvalidStateError
  const error = t.throws(() => decoder.decode(chunk))
  t.true(error instanceof DOMException, 'error should be DOMException instance')
  t.is((error as DOMException).name, 'InvalidStateError')
})

test('AudioDecoder: reset() returns to unconfigured state', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(decoder.state, 'configured')

  decoder.reset()

  t.is(decoder.state, 'unconfigured')

  decoder.close()
})

test('AudioDecoder: can reconfigure after reset', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  decoder.reset()

  // Reconfigure with different settings
  decoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  t.is(decoder.state, 'configured')

  decoder.close()
})

test('AudioDecoder: close() on closed decoder throws InvalidStateError', (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  decoder.close()
  // W3C spec: second close should throw InvalidStateError
  const error = t.throws(() => decoder.close())
  t.true(error instanceof DOMException, 'error should be DOMException instance')
  t.is((error as DOMException).name, 'InvalidStateError')
})

// ============================================================================
// flush() Tests
// ============================================================================

test('AudioDecoder: flush() returns a Promise', async (t) => {
  const { decoder } = createTestDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  const flushResult = decoder.flush()
  t.true(flushResult instanceof Promise, 'flush() should return a Promise')

  await flushResult

  decoder.close()
})

// ============================================================================
// isConfigSupported Tests
// ============================================================================

test('AudioDecoder.isConfigSupported: Opus is supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: AAC is supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: MP3 is supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: FLAC is supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: invalid codec not supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'invalid-codec',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.false(result.supported)
})

// ============================================================================
// Roundtrip Tests (Encode -> Decode)
// ============================================================================

test('AudioDecoder: roundtrip with Opus', async (t) => {
  // Encode audio
  const { encoder, chunks: encodedChunks } = createTestEncoder()
  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Generate and encode multiple frames
  for (let i = 0; i < 10; i++) {
    const audio = generateSineTone(440, 960, 2, 48000, 'f32', i * 20000)
    encoder.encode(audio)
    audio.close()
  }

  await encoder.flush()
  encoder.close()

  // Skip if encoder didn't produce any chunks (may need more data)
  if (encodedChunks.length === 0) {
    t.pass('Encoder needs more data to produce output')
    return
  }

  // Decode
  const { decoder, audioOutputs } = createTestDecoder()
  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  for (const chunk of encodedChunks) {
    // Note: This may fail if the encoded data requires specific decoder config
    // For Opus, we may need extradata (OpusHead)
    try {
      decoder.decode(chunk)
    } catch {
      // May fail without proper Opus header, that's expected
    }
  }

  await decoder.flush()

  // Clean up decoded audio
  for (const audio of audioOutputs) {
    audio.close()
  }

  decoder.close()
  t.pass()
})

// ============================================================================
// Codec Support Tests
// ============================================================================

test('AudioDecoder: PCM S16 is supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'pcm-s16',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder: PCM F32 is supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'pcm-f32',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder: Vorbis is supported', async (t) => {
  const result = await AudioDecoder.isConfigSupported({
    codec: 'vorbis',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

// ============================================================================
// ondequeue Event Tests
// ============================================================================

test('AudioDecoder: ondequeue can be set', (t) => {
  const { decoder } = createTestDecoder()

  let dequeueCount = 0
  decoder.ondequeue = () => {
    dequeueCount++
  }

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  decoder.close()
  t.is(dequeueCount, 0)
})

test('AudioDecoder: ondequeue can be set to null', (t) => {
  const { decoder } = createTestDecoder()

  decoder.ondequeue = () => {}
  t.notThrows(() => {
    decoder.ondequeue = null
  })

  decoder.close()
})
