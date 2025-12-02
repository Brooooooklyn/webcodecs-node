/**
 * Audio test data generators
 *
 * Provides utilities for generating audio test data.
 */

import { AudioData, type AudioSampleFormat } from '../../index.js'

// ============================================================================
// Audio Test Constants
// ============================================================================

/** Common test sample rates */
export const TestSampleRates = {
  cd: 44100,
  dvd: 48000,
  hd: 96000,
  phone: 8000,
  voip: 16000,
} as const

/** Common test channel configurations */
export const TestChannels = {
  mono: 1,
  stereo: 2,
  surround51: 6,
} as const

/** Standard audio test durations (in samples at 48kHz) */
export const TestDurations = {
  short: 960, // 20ms at 48kHz (Opus frame)
  medium: 1024, // ~21ms, common frame size
  long: 4800, // 100ms
  oneSecond: 48000,
} as const

// ============================================================================
// Audio Data Generation
// ============================================================================

/**
 * Generate silence audio data (all zeros)
 */
export function generateSilence(
  numberOfFrames: number,
  numberOfChannels: number,
  sampleRate: number,
  format: AudioSampleFormat = 'f32',
  timestamp: number = 0
): AudioData {
  const bytesPerSample = getBytesPerSample(format)
  const dataSize = numberOfFrames * numberOfChannels * bytesPerSample
  const buffer = new Uint8Array(dataSize)

  return new AudioData({
    format,
    sampleRate,
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data: buffer,
  })
}

/**
 * Generate a sine wave tone
 */
export function generateSineTone(
  frequency: number,
  numberOfFrames: number,
  numberOfChannels: number,
  sampleRate: number,
  format: AudioSampleFormat = 'f32',
  timestamp: number = 0,
  amplitude: number = 0.5
): AudioData {
  const bytesPerSample = getBytesPerSample(format)
  const dataSize = numberOfFrames * numberOfChannels * bytesPerSample
  const buffer = Buffer.alloc(dataSize)

  for (let frame = 0; frame < numberOfFrames; frame++) {
    const t = frame / sampleRate
    const value = Math.sin(2 * Math.PI * frequency * t) * amplitude

    for (let ch = 0; ch < numberOfChannels; ch++) {
      const offset = (frame * numberOfChannels + ch) * bytesPerSample
      writeSample(buffer, offset, value, format)
    }
  }

  return new AudioData({
    format,
    sampleRate,
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data: new Uint8Array(buffer),
  })
}

/**
 * Generate white noise
 */
export function generateWhiteNoise(
  numberOfFrames: number,
  numberOfChannels: number,
  sampleRate: number,
  format: AudioSampleFormat = 'f32',
  timestamp: number = 0,
  amplitude: number = 0.3
): AudioData {
  const bytesPerSample = getBytesPerSample(format)
  const dataSize = numberOfFrames * numberOfChannels * bytesPerSample
  const buffer = Buffer.alloc(dataSize)

  for (let frame = 0; frame < numberOfFrames; frame++) {
    for (let ch = 0; ch < numberOfChannels; ch++) {
      const offset = (frame * numberOfChannels + ch) * bytesPerSample
      const value = (Math.random() * 2 - 1) * amplitude
      writeSample(buffer, offset, value, format)
    }
  }

  return new AudioData({
    format,
    sampleRate,
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data: new Uint8Array(buffer),
  })
}

/**
 * Generate a chirp (frequency sweep)
 */
export function generateChirp(
  startFreq: number,
  endFreq: number,
  numberOfFrames: number,
  numberOfChannels: number,
  sampleRate: number,
  format: AudioSampleFormat = 'f32',
  timestamp: number = 0
): AudioData {
  const bytesPerSample = getBytesPerSample(format)
  const dataSize = numberOfFrames * numberOfChannels * bytesPerSample
  const buffer = Buffer.alloc(dataSize)

  const duration = numberOfFrames / sampleRate
  const k = (endFreq - startFreq) / duration

  for (let frame = 0; frame < numberOfFrames; frame++) {
    const t = frame / sampleRate
    const phase = 2 * Math.PI * (startFreq * t + (k * t * t) / 2)
    const value = Math.sin(phase) * 0.5

    for (let ch = 0; ch < numberOfChannels; ch++) {
      const offset = (frame * numberOfChannels + ch) * bytesPerSample
      writeSample(buffer, offset, value, format)
    }
  }

  return new AudioData({
    format,
    sampleRate,
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data: new Uint8Array(buffer),
  })
}

// ============================================================================
// Utility Functions
// ============================================================================

/**
 * Get bytes per sample for a given format
 */
export function getBytesPerSample(format: AudioSampleFormat): number {
  switch (format) {
    case 'u8':
    case 'u8-planar':
      return 1
    case 's16':
    case 's16-planar':
      return 2
    case 's32':
    case 's32-planar':
    case 'f32':
    case 'f32-planar':
      return 4
    default:
      throw new Error(`Unknown format: ${format}`)
  }
}

/**
 * Check if a format is planar
 */
export function isPlanarFormat(format: AudioSampleFormat): boolean {
  return [
    'u8-planar',
    's16-planar',
    's32-planar',
    'f32-planar',
  ].includes(format)
}

/**
 * Calculate total audio buffer size
 */
export function calculateAudioSize(
  numberOfFrames: number,
  numberOfChannels: number,
  format: AudioSampleFormat
): number {
  return numberOfFrames * numberOfChannels * getBytesPerSample(format)
}

/**
 * Write a sample value to a buffer
 */
function writeSample(buffer: Buffer, offset: number, value: number, format: AudioSampleFormat): void {
  switch (format) {
    case 'u8':
    case 'u8-planar':
      // Use 128 as center point for symmetric round-trip: 0 → 128 → 0
      buffer.writeUInt8(Math.min(255, Math.max(0, Math.round(value * 127.5 + 128))), offset)
      break
    case 's16':
    case 's16-planar':
      buffer.writeInt16LE(Math.round(value * 32767), offset)
      break
    case 's32':
    case 's32-planar':
      buffer.writeInt32LE(Math.round(value * 2147483647), offset)
      break
    case 'f32':
    case 'f32-planar':
      buffer.writeFloatLE(value, offset)
      break
  }
}

/**
 * Read a sample value from a buffer
 */
export function readSample(buffer: Buffer | Uint8Array, offset: number, format: AudioSampleFormat): number {
  const buf = Buffer.isBuffer(buffer) ? buffer : Buffer.from(buffer)
  switch (format) {
    case 'u8':
    case 'u8-planar':
      // Inverse of write: (byte - 128) / 127.5 gives symmetric round-trip
      return (buf.readUInt8(offset) - 128) / 127.5
    case 's16':
    case 's16-planar':
      return buf.readInt16LE(offset) / 32767
    case 's32':
    case 's32-planar':
      return buf.readInt32LE(offset) / 2147483647
    case 'f32':
    case 'f32-planar':
      return buf.readFloatLE(offset)
    default:
      throw new Error(`Unknown format: ${format}`)
  }
}

/**
 * Calculate RMS (Root Mean Square) of audio data
 */
export function calculateRMS(data: Uint8Array, format: AudioSampleFormat): number {
  const bytesPerSample = getBytesPerSample(format)
  const numSamples = data.length / bytesPerSample
  let sumSquares = 0

  for (let i = 0; i < numSamples; i++) {
    const sample = readSample(data, i * bytesPerSample, format)
    sumSquares += sample * sample
  }

  return Math.sqrt(sumSquares / numSamples)
}
