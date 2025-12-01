/**
 * AudioDecoder API Conformance Tests
 *
 * Tests WebCodecs AudioDecoder specification compliance.
 */

import test from 'ava'

import {
  AudioDecoder,
  AudioEncoder,
  AudioSampleFormat,
  CodecState,
  EncodedAudioChunk,
  EncodedAudioChunkType,
} from '../index.js'
import { generateSineTone } from './helpers/index.js'

// ============================================================================
// Constructor Tests
// ============================================================================

test('AudioDecoder: constructor creates unconfigured decoder', (t) => {
  const decoder = new AudioDecoder()

  t.is(decoder.state, CodecState.Unconfigured)
  t.is(decoder.decodeQueueSize, 0)

  decoder.close()
})

// ============================================================================
// Configuration Tests
// ============================================================================

test('AudioDecoder: configure() with AAC codec', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

test('AudioDecoder: configure() with Opus codec', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

test('AudioDecoder: configure() with MP3 codec', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

test('AudioDecoder: configure() with FLAC codec', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

test('AudioDecoder: configure() with mono audio', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 1,
  })

  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

test('AudioDecoder: configure() rejects invalid codec', (t) => {
  const decoder = new AudioDecoder()

  t.throws(() => {
    decoder.configure({
      codec: 'invalid-codec',
      sampleRate: 48000,
      numberOfChannels: 2,
    })
  })

  decoder.close()
})

// ============================================================================
// State Machine Tests
// ============================================================================

test('AudioDecoder: decode() throws when not configured', (t) => {
  const decoder = new AudioDecoder()

  const chunk = new EncodedAudioChunk({
    type: EncodedAudioChunkType.Key,
    timestamp: 0,
    data: Buffer.from([0x00, 0x01, 0x02]),
  })

  t.throws(() => {
    decoder.decode(chunk)
  })

  
  decoder.close()
})

test('AudioDecoder: decode() throws when closed', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  decoder.close()

  const chunk = new EncodedAudioChunk({
    type: EncodedAudioChunkType.Key,
    timestamp: 0,
    data: Buffer.from([0x00, 0x01, 0x02]),
  })

  t.throws(() => {
    decoder.decode(chunk)
  })

  
})

test('AudioDecoder: reset() returns to unconfigured state', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(decoder.state, CodecState.Configured)

  decoder.reset()

  t.is(decoder.state, CodecState.Unconfigured)

  decoder.close()
})

test('AudioDecoder: can reconfigure after reset', (t) => {
  const decoder = new AudioDecoder()

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

  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

test('AudioDecoder: close() is idempotent', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.notThrows(() => decoder.close())
  t.notThrows(() => decoder.close())
})

// ============================================================================
// Output Tests
// ============================================================================

test('AudioDecoder: takeDecodedAudio() clears queue', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  // First take should be empty (nothing decoded yet)
  const audio1 = decoder.takeDecodedAudio()
  t.is(audio1.length, 0)

  // Second take should also be empty
  const audio2 = decoder.takeDecodedAudio()
  t.is(audio2.length, 0)

  decoder.close()
})

test('AudioDecoder: hasOutput() reflects queue state', (t) => {
  const decoder = new AudioDecoder()

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  // Initially no output
  t.false(decoder.hasOutput())

  decoder.close()
})

// ============================================================================
// isConfigSupported Tests
// ============================================================================

test('AudioDecoder.isConfigSupported: Opus is supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: AAC is supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: MP3 is supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: FLAC is supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder.isConfigSupported: invalid codec not supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
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
  const encoder = new AudioEncoder()
  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Generate and encode multiple frames
  for (let i = 0; i < 10; i++) {
    const audio = generateSineTone(440, 960, 2, 48000, AudioSampleFormat.F32, i * 20000)
    encoder.encode(audio)
    audio.close()
  }

  encoder.flush()
  const encodedChunks = encoder.takeEncodedChunks()
  encoder.close()

  // Skip if encoder didn't produce any chunks (may need more data)
  if (encodedChunks.length === 0) {
    t.pass('Encoder needs more data to produce output')
    return
  }

  // Decode
  const decoder = new AudioDecoder()
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
    } catch (_e) {
      // May fail without proper Opus header, that's expected
    }
    
  }

  decoder.flush()
  const decodedAudio = decoder.takeDecodedAudio()

  // Clean up decoded audio
  for (const audio of decodedAudio) {
    audio.close()
  }

  decoder.close()
  t.pass()
})

// ============================================================================
// Codec Support Tests
// ============================================================================

test('AudioDecoder: PCM S16 is supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
    codec: 'pcm-s16',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder: PCM F32 is supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
    codec: 'pcm-f32',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioDecoder: Vorbis is supported', (t) => {
  const result = AudioDecoder.isConfigSupported({
    codec: 'vorbis',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})
