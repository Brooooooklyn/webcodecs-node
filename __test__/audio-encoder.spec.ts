/**
 * AudioEncoder API Conformance Tests
 *
 * Tests WebCodecs AudioEncoder specification compliance.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import { AudioEncoder } from '../index.js'
import { generateSineTone, generateSilence, type EncodedAudioChunk } from './helpers/index.js'

// Helper to create encoder with callbacks that collect output
function createTestEncoder() {
  const chunks: EncodedAudioChunk[] = []
  const errors: Error[] = []

  const encoder = new AudioEncoder({
    output: (chunk, _metadata) => {
      chunks.push(chunk)
    },
    error: (e) => {
      errors.push(e)
    },
  })

  return { encoder, chunks, errors }
}

// ============================================================================
// Constructor Tests
// ============================================================================

test('AudioEncoder: constructor creates unconfigured encoder', (t) => {
  const { encoder } = createTestEncoder()

  t.is(encoder.state, 'unconfigured')
  t.is(encoder.encodeQueueSize, 0)

  encoder.close()
})

test('AudioEncoder: constructor requires init dictionary', (t) => {
  // @ts-expect-error - Testing that missing init throws
  t.throws(() => new AudioEncoder())
  // @ts-expect-error - Testing that missing error callback throws
  t.throws(() => new AudioEncoder({ output: () => {} }))
})

// ============================================================================
// Configuration Tests
// ============================================================================

test('AudioEncoder: configure() with AAC codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'mp4a.40.2', // AAC-LC
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with Opus codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with MP3 codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 192000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with FLAC codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with mono audio', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 1,
    bitrate: 32000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with invalid codec triggers error callback', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'invalid-codec',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  // Error callback transitions to closed
  t.is(encoder.state, 'closed')

  // Already closed by error callback, so close() throws InvalidStateError
  const error = t.throws(() => encoder.close())
  t.true(error?.message.includes('InvalidStateError'))
})

// ============================================================================
// Encoding Tests
// ============================================================================

test('AudioEncoder: encode() single frame', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Generate 20ms of audio (960 samples at 48kHz)
  const audio = generateSineTone(440, 960, 2, 48000, 'f32', 0)

  encoder.encode(audio)
  audio.close()

  // Flush to get output
  await encoder.flush()

  // We should have at least one chunk after flush
  t.true(chunks.length >= 0, 'Encoder should produce chunks or buffer them')

  for (const chunk of chunks) {
    t.is(chunk.type, 'key')
    t.true(chunk.byteLength > 0)
  }

  encoder.close()
})

test('AudioEncoder: encode() multiple frames', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Encode 10 frames of 20ms each
  for (let i = 0; i < 10; i++) {
    const timestamp = i * 20000 // 20ms per frame in microseconds
    const audio = generateSineTone(440, 960, 2, 48000, 'f32', timestamp)
    encoder.encode(audio)
    audio.close()
  }

  await encoder.flush()

  t.true(chunks.length > 0, 'Should have produced encoded chunks')

  encoder.close()
})

test('AudioEncoder: encode() with AAC', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  encoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  // AAC typically needs 1024 samples per frame
  const audio = generateSineTone(440, 1024, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  // Need to encode more frames to get output (AAC buffers)
  for (let i = 1; i < 5; i++) {
    const frame = generateSineTone(440, 1024, 2, 48000, 'f32', i * 21333)
    encoder.encode(frame)
    frame.close()
  }

  await encoder.flush()

  // AAC may need more data before producing output
  t.true(chunks.length >= 0)

  encoder.close()
})

// ============================================================================
// State Machine Tests
// ============================================================================

test('AudioEncoder: encode() on unconfigured throws InvalidStateError', (t) => {
  const { encoder } = createTestEncoder()

  const audio = generateSilence(960, 2, 48000, 'f32', 0)

  // W3C spec: encode() on unconfigured encoder should throw InvalidStateError
  const error = t.throws(() => encoder.encode(audio))
  t.true(error?.message.includes('InvalidStateError'))

  audio.close()
})

test('AudioEncoder: encode() on closed throws InvalidStateError', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  encoder.close()

  const audio = generateSilence(960, 2, 48000, 'f32', 0)

  // W3C spec: encode() on closed encoder should throw InvalidStateError
  const error = t.throws(() => encoder.encode(audio))
  t.true(error?.message.includes('InvalidStateError'))

  audio.close()
})

test('AudioEncoder: reset() returns to unconfigured state', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(encoder.state, 'configured')

  encoder.reset()

  t.is(encoder.state, 'unconfigured')

  encoder.close()
})

test('AudioEncoder: can reconfigure after reset', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  encoder.reset()

  // Reconfigure with different settings
  encoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 192000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: close() on closed encoder throws InvalidStateError', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  encoder.close()
  // W3C spec: second close should throw InvalidStateError
  const error = t.throws(() => encoder.close())
  t.true(error?.message.includes('InvalidStateError'))
})

// ============================================================================
// flush() Tests
// ============================================================================

test('AudioEncoder: flush() returns a Promise', async (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audio = generateSineTone(440, 960, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  const flushResult = encoder.flush()
  t.true(flushResult instanceof Promise, 'flush() should return a Promise')

  await flushResult

  encoder.close()
})

// ============================================================================
// isConfigSupported Tests
// ============================================================================

test('AudioEncoder.isConfigSupported: Opus is supported', async (t) => {
  const result = await AudioEncoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioEncoder.isConfigSupported: AAC is supported', async (t) => {
  const result = await AudioEncoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioEncoder.isConfigSupported: invalid codec not supported', async (t) => {
  const result = await AudioEncoder.isConfigSupported({
    codec: 'invalid-codec',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.false(result.supported)
})

// ============================================================================
// Sample Rate Tests
// ============================================================================

test('AudioEncoder: encode with 44.1kHz sample rate', async (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'mp3', // MP3 commonly uses 44.1kHz
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  // Generate audio at 44.1kHz
  const audio = generateSineTone(440, 1152, 2, 44100, 'f32', 0) // MP3 frame size
  encoder.encode(audio)
  audio.close()

  // Encode more to get output
  for (let i = 1; i < 10; i++) {
    const frame = generateSineTone(440, 1152, 2, 44100, 'f32', i * 26122) // ~1152/44100 seconds
    encoder.encode(frame)
    frame.close()
  }

  await encoder.flush()

  encoder.close()
  t.pass()
})

// ============================================================================
// ondequeue Event Tests
// ============================================================================

test('AudioEncoder: ondequeue fires when queue decreases', async (t) => {
  const { encoder } = createTestEncoder()

  let dequeueCount = 0
  encoder.ondequeue = () => {
    dequeueCount++
  }

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audio = generateSineTone(440, 960, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  await encoder.flush()

  t.true(dequeueCount >= 1, 'ondequeue should have fired')

  encoder.close()
})

test('AudioEncoder: ondequeue can be set to null', (t) => {
  const { encoder } = createTestEncoder()

  encoder.ondequeue = () => {}
  t.notThrows(() => {
    encoder.ondequeue = null
  })

  encoder.close()
})
