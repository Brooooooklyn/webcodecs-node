/**
 * AudioData API Conformance Tests
 *
 * Tests WebCodecs AudioData specification compliance.
 */

import test from 'ava'

import { AudioData } from '../index.js'
import {
  generateSilence,
  generateSineTone,
  getBytesPerSample,
  calculateAudioSize,
  TestSampleRates,
  TestChannels,
  calculateRMS,
} from './helpers/index.js'

// ============================================================================
// Constructor Tests
// ============================================================================

test('AudioData: constructor with F32 interleaved data', (t) => {
  const numberOfFrames = 1024
  const numberOfChannels = 2
  const sampleRate = 48000
  const timestamp = 1000

  const audio = generateSilence(numberOfFrames, numberOfChannels, sampleRate, 'f32', timestamp)

  t.is(audio.format, 'f32')
  t.is(audio.numberOfFrames, numberOfFrames)
  t.is(audio.numberOfChannels, numberOfChannels)
  t.is(audio.sampleRate, sampleRate)
  t.is(audio.timestamp, timestamp)

  audio.close()
})

test('AudioData: constructor with S16 data', (t) => {
  const numberOfFrames = 512
  const numberOfChannels = 1
  const sampleRate = 44100
  const timestamp = 2000

  const audio = generateSilence(numberOfFrames, numberOfChannels, sampleRate, 's16', timestamp)

  t.is(audio.format, 's16')
  t.is(audio.numberOfFrames, numberOfFrames)
  t.is(audio.numberOfChannels, numberOfChannels)
  t.is(audio.sampleRate, sampleRate)

  audio.close()
})

test('AudioData: constructor calculates duration', (t) => {
  const numberOfFrames = 960
  const numberOfChannels = 2
  const sampleRate = 48000
  const timestamp = 0
  // Duration should be calculated: 960/48000 * 1000000 = 20000 microseconds

  const bytesPerSample = getBytesPerSample('f32')
  const dataSize = numberOfFrames * numberOfChannels * bytesPerSample
  const buffer = new Uint8Array(dataSize)

  const audio = new AudioData({
    format: 'f32',
    sampleRate,
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data: buffer,
  })

  t.is(audio.timestamp, timestamp)
  // Duration is calculated from frames and sample rate
  t.truthy(audio.duration)

  audio.close()
})

// ============================================================================
// Property Tests
// ============================================================================

test('AudioData: format property returns correct sample format', (t) => {
  const formats = ['u8', 's16', 's32', 'f32'] as const

  for (const format of formats) {
    const audio = generateSilence(256, 1, 48000, format, 0)
    t.is(audio.format, format, `Format ${format} not preserved`)
    audio.close()
  }
})

test('AudioData: numberOfFrames is correct', (t) => {
  const frameCounts = [256, 512, 960, 1024, 2048, 4096]

  for (const numberOfFrames of frameCounts) {
    const audio = generateSilence(numberOfFrames, 2, 48000, 'f32', 0)
    t.is(audio.numberOfFrames, numberOfFrames, `Frame count ${numberOfFrames} not preserved`)
    audio.close()
  }
})

test('AudioData: numberOfChannels is correct', (t) => {
  for (const [name, channels] of Object.entries(TestChannels)) {
    const audio = generateSilence(1024, channels, 48000, 'f32', 0)
    t.is(audio.numberOfChannels, channels, `Channel count mismatch for ${name}`)
    audio.close()
  }
})

test('AudioData: sampleRate property', (t) => {
  for (const [name, rate] of Object.entries(TestSampleRates)) {
    const audio = generateSilence(1024, 2, rate, 'f32', 0)
    t.is(audio.sampleRate, rate, `Sample rate mismatch for ${name}`)
    audio.close()
  }
})

test('AudioData: timestamp property', (t) => {
  const timestamps = [0, 1000, 33333, 1000000, 9007199254740991]

  for (const ts of timestamps) {
    const audio = generateSilence(1024, 2, 48000, 'f32', ts)
    t.is(audio.timestamp, ts, `Timestamp ${ts} not preserved`)
    audio.close()
  }
})

test('AudioData: duration property (optional)', (t) => {
  // Without duration - should be calculated from frames and sample rate
  const audio1 = generateSilence(48000, 2, 48000, 'f32', 0)
  // Duration should be ~1 second (1,000,000 microseconds)
  t.true(audio1.duration !== null, 'Duration should be calculated')
  audio1.close()
})

// ============================================================================
// Method Tests
// ============================================================================

test('AudioData: allocationSize() returns correct size', (t) => {
  const testCases = [
    { frames: 256, channels: 1, format: 'u8' as const },
    { frames: 512, channels: 2, format: 's16' as const },
    { frames: 1024, channels: 2, format: 's32' as const },
    { frames: 960, channels: 2, format: 'f32' as const },
  ]

  for (const { frames, channels, format } of testCases) {
    const audio = generateSilence(frames, channels, 48000, format, 0)
    const expectedSize = calculateAudioSize(frames, channels, format)
    t.is(
      audio.allocationSize({ planeIndex: 0 }),
      expectedSize,
      `allocationSize mismatch for ${frames}x${channels} ${format}`,
    )
    audio.close()
  }
})

test('AudioData: copyTo() extracts audio data', (t) => {
  const audio = generateSineTone(440, 1024, 2, 48000, 'f32', 0)

  const size = audio.allocationSize({ planeIndex: 0 })
  const buffer = new Uint8Array(size)

  audio.copyTo(buffer, { planeIndex: 0 })

  // Sine tone should have non-zero data
  const rms = calculateRMS(buffer, 'f32')
  t.true(rms > 0.1, 'Sine tone should have significant amplitude')

  audio.close()
})

test('AudioData: copyTo() preserves data round-trip', (t) => {
  const numberOfFrames = 256
  const numberOfChannels = 2
  const format = 'f32'
  const bytesPerSample = getBytesPerSample(format)

  // Create source data with a pattern
  const sourceSize = numberOfFrames * numberOfChannels * bytesPerSample
  const sourceData = new Uint8Array(sourceSize)
  const view = new DataView(sourceData.buffer)

  for (let i = 0; i < numberOfFrames * numberOfChannels; i++) {
    const value = Math.sin((i / (numberOfFrames * numberOfChannels)) * Math.PI * 2)
    view.setFloat32(i * bytesPerSample, value, true) // little-endian
  }

  const audio = new AudioData({
    format,
    sampleRate: 48000,
    numberOfFrames,
    numberOfChannels,
    timestamp: 0,
    data: sourceData,
  })

  // Extract and compare
  const extractedData = new Uint8Array(sourceSize)
  audio.copyTo(extractedData, { planeIndex: 0 })

  for (let i = 0; i < sourceSize; i++) {
    t.is(extractedData[i], sourceData[i], `Data mismatch at byte ${i}`)
  }

  audio.close()
})

test('AudioData: clone() creates independent copy', (t) => {
  const audio = generateSineTone(440, 1024, 2, 48000, 'f32', 12345, 0.5)

  const cloned = audio.clone()

  // Properties should match
  t.is(cloned.format, audio.format)
  t.is(cloned.numberOfFrames, audio.numberOfFrames)
  t.is(cloned.numberOfChannels, audio.numberOfChannels)
  t.is(cloned.sampleRate, audio.sampleRate)
  t.is(cloned.timestamp, audio.timestamp)

  // Close original - clone should still work
  audio.close()

  // Clone should still be accessible
  t.is(cloned.numberOfFrames, 1024)

  const size = cloned.allocationSize({ planeIndex: 0 })
  const buffer = new Uint8Array(size)
  t.notThrows(() => cloned.copyTo(buffer, { planeIndex: 0 }))

  cloned.close()
})

test('AudioData: close() releases resources', (t) => {
  const audio = generateSilence(1024, 2, 48000, 'f32', 0)

  // Should not throw
  t.notThrows(() => audio.close())

  // Idempotent - calling close again should not throw
  t.notThrows(() => audio.close())
})

// ============================================================================
// Edge Case Tests
// ============================================================================

test('AudioData: minimum frame count (1 frame)', (t) => {
  const format = 'f32'
  const bytesPerSample = getBytesPerSample(format)
  const buffer = new Uint8Array(1 * 1 * bytesPerSample)

  const audio = new AudioData({
    format,
    sampleRate: 48000,
    numberOfFrames: 1,
    numberOfChannels: 1,
    timestamp: 0,
    data: buffer,
  })

  t.is(audio.numberOfFrames, 1)
  t.is(audio.numberOfChannels, 1)

  audio.close()
})

test('AudioData: timestamp of 0 is valid', (t) => {
  const audio = generateSilence(1024, 2, 48000, 'f32', 0)
  t.is(audio.timestamp, 0)
  audio.close()
})

test('AudioData: large timestamp values', (t) => {
  // 1 hour in microseconds
  const oneHourUs = 3600 * 1000000
  const audio = generateSilence(1024, 2, 48000, 'f32', oneHourUs)
  t.is(audio.timestamp, oneHourUs)
  audio.close()
})

test('AudioData: different sample rates', (t) => {
  const rates = [8000, 16000, 22050, 32000, 44100, 48000, 96000]

  for (const rate of rates) {
    const audio = generateSilence(1024, 2, rate, 'f32', 0)
    t.is(audio.sampleRate, rate, `Sample rate ${rate} not preserved`)
    audio.close()
  }
})

// ============================================================================
// Audio Content Tests
// ============================================================================

test('AudioData: silence has near-zero amplitude', (t) => {
  const audio = generateSilence(1024, 2, 48000, 'f32', 0)

  const size = audio.allocationSize({ planeIndex: 0 })
  const buffer = new Uint8Array(size)
  audio.copyTo(buffer, { planeIndex: 0 })

  const rms = calculateRMS(buffer, 'f32')
  t.true(rms < 0.001, 'Silence should have near-zero amplitude')

  audio.close()
})

test('AudioData: sine tone has expected amplitude', (t) => {
  const amplitude = 0.5
  const audio = generateSineTone(440, 4096, 1, 48000, 'f32', 0, amplitude)

  const size = audio.allocationSize({ planeIndex: 0 })
  const buffer = new Uint8Array(size)
  audio.copyTo(buffer, { planeIndex: 0 })

  const rms = calculateRMS(buffer, 'f32')
  // RMS of sine wave is amplitude / sqrt(2) ~= 0.707 * amplitude
  const expectedRms = amplitude / Math.sqrt(2)
  t.true(Math.abs(rms - expectedRms) < 0.05, `RMS ${rms} should be close to ${expectedRms}`)

  audio.close()
})
