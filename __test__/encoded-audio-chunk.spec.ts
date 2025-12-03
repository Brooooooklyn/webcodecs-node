/**
 * EncodedAudioChunk API Conformance Tests
 *
 * Tests WebCodecs EncodedAudioChunk specification compliance.
 * Per W3C spec, EncodedAudioChunk has no close() method - it's immutable encoded data.
 */

import test from 'ava'

import { EncodedAudioChunk } from '../index.js'

// ============================================================================
// Constructor Tests
// ============================================================================

test('EncodedAudioChunk: constructor with key chunk', (t) => {
  const data = new Uint8Array([0x00, 0x01, 0x02, 0x03])
  const timestamp = 1000
  const duration = 20000

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp,
    duration,
    data,
  })

  t.is(chunk.type, 'key')
  t.is(chunk.timestamp, timestamp)
  t.is(chunk.duration, duration)
  t.is(chunk.byteLength, data.length)
})

test('EncodedAudioChunk: constructor with delta chunk', (t) => {
  const data = new Uint8Array([0x10, 0x20, 0x30])
  const timestamp = 2000

  const chunk = new EncodedAudioChunk({
    type: 'delta',
    timestamp,
    data,
  })

  t.is(chunk.type, 'delta')
  t.is(chunk.timestamp, timestamp)
  t.is(chunk.byteLength, data.length)
})

test('EncodedAudioChunk: constructor without duration', (t) => {
  const data = new Uint8Array([0x01])
  const timestamp = 0

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp,
    data,
  })

  t.is(chunk.duration, null)
})

// ============================================================================
// Property Tests
// ============================================================================

test('EncodedAudioChunk: type property', (t) => {
  const data = new Uint8Array([0x00])

  const keyChunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data,
  })
  t.is(keyChunk.type, 'key')

  const deltaChunk = new EncodedAudioChunk({
    type: 'delta',
    timestamp: 0,
    data,
  })
  t.is(deltaChunk.type, 'delta')
})

test('EncodedAudioChunk: timestamp property', (t) => {
  const timestamps = [0, 1000, 33333, 1000000, 9007199254740991]

  for (const ts of timestamps) {
    const chunk = new EncodedAudioChunk({
      type: 'key',
      timestamp: ts,
      data: new Uint8Array([0x00]),
    })
    t.is(chunk.timestamp, ts, `Timestamp ${ts} not preserved`)
  }
})

test('EncodedAudioChunk: duration property', (t) => {
  const durations = [1000, 20000, 33333, 100000]

  for (const dur of durations) {
    const chunk = new EncodedAudioChunk({
      type: 'key',
      timestamp: 0,
      duration: dur,
      data: new Uint8Array([0x00]),
    })
    t.is(chunk.duration, dur, `Duration ${dur} not preserved`)
  }
})

test('EncodedAudioChunk: byteLength property', (t) => {
  const sizes = [1, 10, 100, 1000, 10000]

  for (const size of sizes) {
    const data = new Uint8Array(size)
    data.fill(0x42)
    const chunk = new EncodedAudioChunk({
      type: 'key',
      timestamp: 0,
      data,
    })
    t.is(chunk.byteLength, size, `byteLength ${size} not preserved`)
  }
})

// ============================================================================
// Method Tests
// ============================================================================

test('EncodedAudioChunk: copyTo() extracts chunk data', (t) => {
  const originalData = new Uint8Array([0x01, 0x02, 0x03, 0x04, 0x05])

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: originalData,
  })

  const extractedData = new Uint8Array(chunk.byteLength)
  chunk.copyTo(extractedData)

  for (let i = 0; i < originalData.length; i++) {
    t.is(extractedData[i], originalData[i], `Data mismatch at index ${i}`)
  }
})

test('EncodedAudioChunk: copyTo() with larger buffer', (t) => {
  const originalData = new Uint8Array([0xaa, 0xbb, 0xcc])

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: originalData,
  })

  // Use a larger buffer than needed
  const extractedData = new Uint8Array(100)
  chunk.copyTo(extractedData)

  // First bytes should match original data
  for (let i = 0; i < originalData.length; i++) {
    t.is(extractedData[i], originalData[i], `Data mismatch at index ${i}`)
  }
})

test('EncodedAudioChunk: can be created and accessed', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })

  // Should be able to access properties
  t.is(chunk.type, 'key')
  t.is(chunk.timestamp, 0)
  t.is(chunk.byteLength, 1)
})

// ============================================================================
// Edge Case Tests
// ============================================================================

test('EncodedAudioChunk: minimum data size (1 byte)', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })

  t.is(chunk.byteLength, 1)
})

test('EncodedAudioChunk: large data size', (t) => {
  const size = 100000
  const data = new Uint8Array(size)
  data.fill(0x55)

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, size)
})

test('EncodedAudioChunk: timestamp of 0 is valid', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })

  t.is(chunk.timestamp, 0)
})

test('EncodedAudioChunk: duration of 0 is valid', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    duration: 0,
    data: new Uint8Array([0x00]),
  })

  t.is(chunk.duration, 0)
})

// ============================================================================
// Simulated Codec Data Tests
// ============================================================================

test('EncodedAudioChunk: AAC-like data structure', (t) => {
  // Simulated AAC ADTS frame header (not real AAC)
  const fakeAdtsHeader = new Uint8Array([
    0xff,
    0xf1, // Sync word + MPEG-4, Layer 0
    0x50,
    0x80, // AAC-LC, 48kHz, stereo
    0x00,
    0x1f,
    0xfc, // Frame length header
  ])
  const frameData = new Uint8Array(100) // Fake audio data
  const data = new Uint8Array(fakeAdtsHeader.length + frameData.length)
  data.set(fakeAdtsHeader, 0)
  data.set(frameData, fakeAdtsHeader.length)

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    duration: 21333, // ~1024 samples at 48kHz
    data,
  })

  t.is(chunk.byteLength, data.length)
  t.is(chunk.type, 'key')
})

test('EncodedAudioChunk: Opus-like data structure', (t) => {
  // Simulated Opus TOC byte (not real Opus)
  const tocByte = 0xfc // Config 31, stereo
  const tocData = new Uint8Array([tocByte])
  const payload = new Uint8Array(50)
  const data = new Uint8Array(tocData.length + payload.length)
  data.set(tocData, 0)
  data.set(payload, tocData.length)

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    duration: 20000, // 20ms Opus frame
    data,
  })

  t.is(chunk.byteLength, data.length)
})
