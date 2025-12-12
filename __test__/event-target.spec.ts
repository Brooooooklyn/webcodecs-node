/**
 * EventTarget Interface Tests
 *
 * Tests that addEventListener('dequeue', ...) fires correctly when the
 * encode/decode queue decreases. This is per W3C WebCodecs spec which
 * requires EventTarget interface on all codecs.
 */

import test from 'ava'

import {
  AudioData,
  AudioDecoder,
  AudioEncoder,
  EncodedVideoChunkMetadata,
  resetHardwareFallbackState,
  VideoDecoder,
  VideoEncoder,
} from '../index.js'
import {
  generateSolidColorI420Frame,
  generateFrameSequence,
  TestColors,
  type EncodedVideoChunk,
  type EncodedAudioChunk,
} from './helpers/index.js'
import { createEncoderConfig } from './helpers/codec-matrix.js'

// Helper: Create encoded chunks for decoder tests
interface EncodedChunkWithMetadata {
  chunk: EncodedVideoChunk
  metadata?: EncodedVideoChunkMetadata
}

async function createEncodedH264Chunks(
  width: number,
  height: number,
  frameCount: number,
): Promise<EncodedChunkWithMetadata[]> {
  const chunks: EncodedChunkWithMetadata[] = []
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push({ chunk, metadata })
    },
    error: () => {},
  })
  encoder.configure(createEncoderConfig('h264', width, height))

  const frames = generateFrameSequence(width, height, frameCount)

  encoder.encode(frames[0], { keyFrame: true })
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }

  for (const frame of frames) {
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  return chunks
}

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// VideoEncoder EventTarget Tests
// ============================================================================

test('VideoEncoder: addEventListener dequeue fires when queue decreases', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  // Set up the listener BEFORE encoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => encoder.addEventListener('dequeue', resolve, { once: true }))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame)
  frame.close()

  // Wait for dequeue event
  await dequeuePromise

  t.is(encoder.encodeQueueSize, 0)

  await encoder.flush()
  encoder.close()
})

test('VideoEncoder: multiple dequeue listeners all fire', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  let listener1Count = 0
  let listener2Count = 0

  encoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  encoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)
  encoder.encode(frame)
  frame.close()

  await encoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  encoder.close()
})

test('VideoEncoder: dequeue once:true fires only once', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  let onceCount = 0
  let regularCount = 0

  encoder.addEventListener(
    'dequeue',
    () => {
      onceCount++
    },
    { once: true },
  )
  encoder.addEventListener('dequeue', () => {
    regularCount++
  })

  // Encode 3 frames
  for (let i = 0; i < 3; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame)
    frame.close()
  }

  await encoder.flush()

  t.is(onceCount, 1, 'once listener should fire exactly once')
  t.true(regularCount >= 1, 'regular listener should fire for each dequeue')

  encoder.close()
})

// ============================================================================
// VideoDecoder EventTarget Tests
// ============================================================================

test('VideoDecoder: addEventListener dequeue fires when queue decreases', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const frames: unknown[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      frames.push(frame)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: 'avc1.42001E',
    codedWidth: 320,
    codedHeight: 240,
    description: chunks[0].metadata?.decoderConfig?.description,
  })

  // Set up the listener BEFORE decoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => decoder.addEventListener('dequeue', resolve, { once: true }))

  decoder.decode(chunks[0].chunk)

  // Wait for dequeue event
  await dequeuePromise

  t.is(decoder.decodeQueueSize, 0)

  await decoder.flush()
  decoder.close()

  // Clean up frames
  for (const frame of frames) {
    ;(frame as { close: () => void }).close()
  }
})

test('VideoDecoder: multiple dequeue listeners all fire', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const frames: unknown[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      frames.push(frame)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: 'avc1.42001E',
    codedWidth: 320,
    codedHeight: 240,
    description: chunks[0].metadata?.decoderConfig?.description,
  })

  let listener1Count = 0
  let listener2Count = 0

  decoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  decoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  decoder.decode(chunks[0].chunk)

  await decoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  decoder.close()

  // Clean up frames
  for (const frame of frames) {
    ;(frame as { close: () => void }).close()
  }
})

// ============================================================================
// AudioEncoder EventTarget Tests
// ============================================================================

function createTestAudioData(timestamp: number): AudioData {
  const sampleRate = 48000
  const numberOfChannels = 2
  const numberOfFrames = 1024
  const data = new Float32Array(numberOfFrames * numberOfChannels)

  // Generate sine wave
  const frequency = 440
  for (let i = 0; i < numberOfFrames; i++) {
    const t = i / sampleRate
    const sample = Math.sin(2 * Math.PI * frequency * t) * 0.5
    data[i * numberOfChannels] = sample
    data[i * numberOfChannels + 1] = sample
  }

  return new AudioData({
    format: 'f32',
    sampleRate,
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data,
  })
}

test('AudioEncoder: addEventListener dequeue fires when queue decreases', async (t) => {
  const chunks: EncodedAudioChunk[] = []
  const encoder = new AudioEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Set up the listener BEFORE encoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => encoder.addEventListener('dequeue', resolve, { once: true }))

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()

  // Wait for dequeue event
  await dequeuePromise

  t.is(encoder.encodeQueueSize, 0)

  await encoder.flush()
  encoder.close()
})

test('AudioEncoder: multiple dequeue listeners all fire', async (t) => {
  const encoder = new AudioEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  let listener1Count = 0
  let listener2Count = 0

  encoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  encoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()

  await encoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  encoder.close()
})

// ============================================================================
// AudioDecoder EventTarget Tests
// ============================================================================

test('AudioDecoder: addEventListener dequeue fires when queue decreases', async (t) => {
  // First encode some audio to get encoded chunks
  const encodedChunks: EncodedAudioChunk[] = []
  let decoderConfig: unknown = null

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      encodedChunks.push(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig
      }
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()
  await encoder.flush()
  encoder.close()

  // Now decode
  const decodedAudio: unknown[] = []
  const decoder = new AudioDecoder({
    output: (audio) => {
      decodedAudio.push(audio)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  const config = decoderConfig as {
    codec: string
    sampleRate: number
    numberOfChannels: number
    description?: Uint8Array
  }
  decoder.configure({
    codec: config.codec,
    sampleRate: config.sampleRate,
    numberOfChannels: config.numberOfChannels,
    description: config.description,
  })

  // Set up the listener BEFORE decoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => decoder.addEventListener('dequeue', resolve, { once: true }))

  decoder.decode(encodedChunks[0])

  // Wait for dequeue event
  await dequeuePromise

  t.is(decoder.decodeQueueSize, 0)

  await decoder.flush()
  decoder.close()

  // Clean up
  for (const audio of decodedAudio) {
    ;(audio as { close: () => void }).close()
  }
})

test('AudioDecoder: multiple dequeue listeners all fire', async (t) => {
  // First encode some audio
  const encodedChunks: EncodedAudioChunk[] = []
  let decoderConfig: unknown = null

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      encodedChunks.push(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig
      }
    },
    error: () => {},
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()
  await encoder.flush()
  encoder.close()

  // Now decode
  const decodedAudio: unknown[] = []
  const decoder = new AudioDecoder({
    output: (audio) => {
      decodedAudio.push(audio)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  const config = decoderConfig as {
    codec: string
    sampleRate: number
    numberOfChannels: number
    description?: Uint8Array
  }
  decoder.configure({
    codec: config.codec,
    sampleRate: config.sampleRate,
    numberOfChannels: config.numberOfChannels,
    description: config.description,
  })

  let listener1Count = 0
  let listener2Count = 0

  decoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  decoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  decoder.decode(encodedChunks[0])

  await decoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  decoder.close()

  // Clean up
  for (const audio of decodedAudio) {
    ;(audio as { close: () => void }).close()
  }
})
