/**
 * EncodedVideoChunk API Conformance Tests
 *
 * Tests WebCodecs EncodedVideoChunk specification compliance.
 */

import test from 'ava'

import { EncodedVideoChunk } from '../index.js'

// ============================================================================
// Constructor Tests
// ============================================================================

test('EncodedVideoChunk: constructor with key frame', (t) => {
  const data = new Uint8Array([0x00, 0x01, 0x02, 0x03])
  const timestamp = 1000
  const duration = 33333

  const chunk = new EncodedVideoChunk({
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

test('EncodedVideoChunk: constructor with delta frame', (t) => {
  const data = new Uint8Array([0x10, 0x20, 0x30])
  const timestamp = 2000

  const chunk = new EncodedVideoChunk({
    type: 'delta',
    timestamp,
    data,
  })

  t.is(chunk.type, 'delta')
  t.is(chunk.timestamp, timestamp)
  t.is(chunk.byteLength, data.length)
})

test('EncodedVideoChunk: constructor without duration', (t) => {
  const data = new Uint8Array([0x00])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.duration, null)
})

// ============================================================================
// Property Tests
// ============================================================================

test('EncodedVideoChunk: type property returns correct ChunkType', (t) => {
  const keyChunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })
  t.is(keyChunk.type, 'key')

  const deltaChunk = new EncodedVideoChunk({
    type: 'delta',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })
  t.is(deltaChunk.type, 'delta')
})

test('EncodedVideoChunk: timestamp property', (t) => {
  const timestamps = [0, 1000, 33333, 1000000]

  for (const ts of timestamps) {
    const chunk = new EncodedVideoChunk({
      type: 'key',
      timestamp: ts,
      data: new Uint8Array([0x00]),
    })
    t.is(chunk.timestamp, ts, `Timestamp ${ts} not preserved`)
  }
})

test('EncodedVideoChunk: duration property (optional)', (t) => {
  // Without duration
  const chunk1 = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })
  t.is(chunk1.duration, null)

  // With duration
  const chunk2 = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    duration: 33333,
    data: new Uint8Array([0x00]),
  })
  t.is(chunk2.duration, 33333)
})

test('EncodedVideoChunk: byteLength property', (t) => {
  const testCases = [{ size: 1 }, { size: 100 }, { size: 1024 }, { size: 65536 }]

  for (const { size } of testCases) {
    const data = new Uint8Array(size)
    const chunk = new EncodedVideoChunk({
      type: 'key',
      timestamp: 0,
      data,
    })
    t.is(chunk.byteLength, size, `byteLength mismatch for size ${size}`)
  }
})

// ============================================================================
// Method Tests
// ============================================================================

test('EncodedVideoChunk: copyTo() extracts data', (t) => {
  const sourceData = new Uint8Array([0x01, 0x02, 0x03, 0x04, 0x05])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: sourceData,
  })

  const destination = new Uint8Array(chunk.byteLength)
  chunk.copyTo(destination)

  for (let i = 0; i < sourceData.length; i++) {
    t.is(destination[i], sourceData[i], `Data mismatch at index ${i}`)
  }
})

test('EncodedVideoChunk: copyTo() with larger destination buffer', (t) => {
  const sourceData = new Uint8Array([0x0a, 0x0b, 0x0c])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: sourceData,
  })

  // Destination is larger than needed
  const destination = new Uint8Array(100)
  destination.fill(0xff) // Pre-fill to verify only relevant bytes are written

  chunk.copyTo(destination)

  // First bytes should match source
  t.is(destination[0], 0x0a)
  t.is(destination[1], 0x0b)
  t.is(destination[2], 0x0c)
})

test('EncodedVideoChunk: copyTo() extracts data from Uint8Array source', (t) => {
  const sourceData = new Uint8Array([0xde, 0xad, 0xbe, 0xef])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: sourceData,
  })

  const data = new Uint8Array(chunk.byteLength)
  chunk.copyTo(data)
  t.is(data.length, sourceData.length)

  for (let i = 0; i < sourceData.length; i++) {
    t.is(data[i], sourceData[i], `Data mismatch at index ${i}`)
  }
})

// ============================================================================
// Edge Case Tests
// ============================================================================

test('EncodedVideoChunk: empty data buffer', (t) => {
  const data = new Uint8Array(0)
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, 0)
})

test('EncodedVideoChunk: timestamp of 0 is valid', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([0x00]),
  })
  t.is(chunk.timestamp, 0)
})

test('EncodedVideoChunk: large data buffer', (t) => {
  // 1MB of data
  const size = 1024 * 1024
  const data = new Uint8Array(size)
  data.fill(0x42)

  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, size)

  const extracted = new Uint8Array(chunk.byteLength)
  chunk.copyTo(extracted)
  t.is(extracted.length, size)
  t.is(extracted[0], 0x42)
  t.is(extracted[size - 1], 0x42)
})

test('EncodedVideoChunk: data immutability', (t) => {
  const originalData = new Uint8Array([0x01, 0x02, 0x03])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: originalData,
  })

  // Modify original buffer
  originalData[0] = 0xff

  // Chunk data should be independent
  const extractedData = new Uint8Array(chunk.byteLength)
  chunk.copyTo(extractedData)
  t.is(extractedData[0], 0x01, 'Chunk data should be independent of original buffer')
})

test('EncodedVideoChunk: binary data preservation', (t) => {
  // Test all byte values 0-255
  const allBytes = new Uint8Array(256)
  for (let i = 0; i < 256; i++) {
    allBytes[i] = i
  }

  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: allBytes,
  })

  const extracted = new Uint8Array(chunk.byteLength)
  chunk.copyTo(extracted)
  for (let i = 0; i < 256; i++) {
    t.is(extracted[i], i, `Byte ${i} not preserved`)
  }
})
