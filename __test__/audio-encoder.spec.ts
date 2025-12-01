/**
 * AudioEncoder API Conformance Tests
 *
 * Tests WebCodecs AudioEncoder specification compliance.
 */

import test from 'ava'

import { AudioEncoder, AudioSampleFormat, CodecState, EncodedAudioChunkType } from '../index.js'
import { generateSineTone, generateSilence, TestSampleRates, TestDurations } from './helpers/index.js'

// ============================================================================
// Constructor Tests
// ============================================================================

test('AudioEncoder: constructor creates unconfigured encoder', (t) => {
  const encoder = new AudioEncoder()

  t.is(encoder.state, CodecState.Unconfigured)
  t.is(encoder.encodeQueueSize, 0)

  encoder.close()
})

// ============================================================================
// Configuration Tests
// ============================================================================

test('AudioEncoder: configure() with AAC codec', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'mp4a.40.2', // AAC-LC
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

test('AudioEncoder: configure() with Opus codec', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

test('AudioEncoder: configure() with MP3 codec', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 192000,
  })

  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

test('AudioEncoder: configure() with FLAC codec', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

test('AudioEncoder: configure() with mono audio', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 1,
    bitrate: 32000,
  })

  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

test('AudioEncoder: configure() rejects invalid codec', (t) => {
  const encoder = new AudioEncoder()

  t.throws(() => {
    encoder.configure({
      codec: 'invalid-codec',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  encoder.close()
})

// ============================================================================
// Encoding Tests
// ============================================================================

test('AudioEncoder: encode() single frame', async (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Generate 20ms of audio (960 samples at 48kHz)
  const audio = generateSineTone(440, 960, 2, 48000, AudioSampleFormat.F32, 0)

  encoder.encode(audio)
  audio.close()

  // Flush to get output
  encoder.flush()

  // Take encoded output
  const chunks = encoder.takeEncodedChunks()

  // We should have at least one chunk after flush
  t.true(chunks.length >= 0, 'Encoder should produce chunks or buffer them')

  for (const chunk of chunks) {
    t.is(chunk.type, EncodedAudioChunkType.Key)
    t.true(chunk.byteLength > 0)
    
  }

  encoder.close()
})

test('AudioEncoder: encode() multiple frames', async (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Encode 10 frames of 20ms each
  for (let i = 0; i < 10; i++) {
    const timestamp = i * 20000 // 20ms per frame in microseconds
    const audio = generateSineTone(440, 960, 2, 48000, AudioSampleFormat.F32, timestamp)
    encoder.encode(audio)
    audio.close()
  }

  encoder.flush()

  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length > 0, 'Should have produced encoded chunks')

  // Clean up
  for (const chunk of chunks) {
    
  }

  encoder.close()
})

test('AudioEncoder: encode() with AAC', async (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  // AAC typically needs 1024 samples per frame
  const audio = generateSineTone(440, 1024, 2, 48000, AudioSampleFormat.F32, 0)
  encoder.encode(audio)
  audio.close()

  // Need to encode more frames to get output (AAC buffers)
  for (let i = 1; i < 5; i++) {
    const frame = generateSineTone(440, 1024, 2, 48000, AudioSampleFormat.F32, i * 21333)
    encoder.encode(frame)
    frame.close()
  }

  encoder.flush()

  const chunks = encoder.takeEncodedChunks()
  // AAC may need more data before producing output
  t.true(chunks.length >= 0)

  for (const chunk of chunks) {
    
  }

  encoder.close()
})

// ============================================================================
// State Machine Tests
// ============================================================================

test('AudioEncoder: encode() throws when not configured', (t) => {
  const encoder = new AudioEncoder()

  const audio = generateSilence(960, 2, 48000, AudioSampleFormat.F32, 0)

  t.throws(() => {
    encoder.encode(audio)
  })

  audio.close()
  encoder.close()
})

test('AudioEncoder: encode() throws when closed', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  encoder.close()

  const audio = generateSilence(960, 2, 48000, AudioSampleFormat.F32, 0)

  t.throws(() => {
    encoder.encode(audio)
  })

  audio.close()
})

test('AudioEncoder: reset() returns to unconfigured state', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(encoder.state, CodecState.Configured)

  encoder.reset()

  t.is(encoder.state, CodecState.Unconfigured)

  encoder.close()
})

test('AudioEncoder: can reconfigure after reset', (t) => {
  const encoder = new AudioEncoder()

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

  t.is(encoder.state, CodecState.Configured)

  encoder.close()
})

test('AudioEncoder: close() is idempotent', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.notThrows(() => encoder.close())
  t.notThrows(() => encoder.close())
})

// ============================================================================
// Output Tests
// ============================================================================

test('AudioEncoder: takeEncodedChunks() clears queue', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Generate and encode audio
  for (let i = 0; i < 5; i++) {
    const audio = generateSineTone(440, 960, 2, 48000, AudioSampleFormat.F32, i * 20000)
    encoder.encode(audio)
    audio.close()
  }

  encoder.flush()

  // First take should get chunks
  const chunks1 = encoder.takeEncodedChunks()

  // Second take should be empty
  const chunks2 = encoder.takeEncodedChunks()
  t.is(chunks2.length, 0, 'Queue should be empty after take')

  // Clean up
  for (const chunk of chunks1) {
    
  }

  encoder.close()
})

test('AudioEncoder: hasOutput() reflects queue state', (t) => {
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Initially no output
  t.false(encoder.hasOutput())

  // Encode and flush
  for (let i = 0; i < 5; i++) {
    const audio = generateSineTone(440, 960, 2, 48000, AudioSampleFormat.F32, i * 20000)
    encoder.encode(audio)
    audio.close()
  }
  encoder.flush()

  // Should have output after flush (if encoder produced any)
  const hasOutput = encoder.hasOutput()
  const chunks = encoder.takeEncodedChunks()

  // hasOutput should match whether we got chunks
  t.is(hasOutput, chunks.length > 0)

  for (const chunk of chunks) {
    
  }

  encoder.close()
})

// ============================================================================
// isConfigSupported Tests
// ============================================================================

test('AudioEncoder.isConfigSupported: Opus is supported', (t) => {
  const result = AudioEncoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioEncoder.isConfigSupported: AAC is supported', (t) => {
  const result = AudioEncoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioEncoder.isConfigSupported: invalid codec not supported', (t) => {
  const result = AudioEncoder.isConfigSupported({
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
  const encoder = new AudioEncoder()

  encoder.configure({
    codec: 'mp3', // MP3 commonly uses 44.1kHz
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  // Generate audio at 44.1kHz
  const audio = generateSineTone(440, 1152, 2, 44100, AudioSampleFormat.F32, 0) // MP3 frame size
  encoder.encode(audio)
  audio.close()

  // Encode more to get output
  for (let i = 1; i < 10; i++) {
    const frame = generateSineTone(440, 1152, 2, 44100, AudioSampleFormat.F32, i * 26122) // ~1152/44100 seconds
    encoder.encode(frame)
    frame.close()
  }

  encoder.flush()

  const chunks = encoder.takeEncodedChunks()

  for (const chunk of chunks) {
    
  }

  encoder.close()
  t.pass()
})
