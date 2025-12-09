/**
 * EncodedVideoChunk and EncodedAudioChunk Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/nicosurjana/nicosurjana.git
 *
 * Tests chunk construction, copyTo, and properties.
 */

import test from 'ava'

import { EncodedAudioChunk, EncodedVideoChunk, resetHardwareFallbackState } from '../../index.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// EncodedVideoChunk Construction Tests
// ============================================================================

test('EncodedVideoChunk: construction with key frame', (t) => {
  const data = new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 1000,
    data,
  })

  t.is(chunk.type, 'key')
  t.is(chunk.timestamp, 1000)
  t.is(chunk.byteLength, 8)
  t.is(chunk.duration, null)
})

test('EncodedVideoChunk: construction with delta frame', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'delta',
    timestamp: 2000,
    data: new Uint8Array([1, 2, 3, 4]),
  })

  t.is(chunk.type, 'delta')
  t.is(chunk.timestamp, 2000)
  t.is(chunk.byteLength, 4)
})

test('EncodedVideoChunk: construction with duration', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 1000,
    duration: 33333,
    data: new Uint8Array([0, 1, 2, 3]),
  })

  t.is(chunk.duration, 33333)
})

test('EncodedVideoChunk: construction with negative timestamp', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: -5000,
    data: new Uint8Array([0, 1]),
  })

  t.is(chunk.timestamp, -5000)
})

test('EncodedVideoChunk: construction requires type', (t) => {
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new EncodedVideoChunk({
        timestamp: 1000,
        data: new Uint8Array([0, 1]),
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedVideoChunk: construction requires timestamp', (t) => {
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new EncodedVideoChunk({
        type: 'key',
        data: new Uint8Array([0, 1]),
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedVideoChunk: construction requires data', (t) => {
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new EncodedVideoChunk({
        type: 'key',
        timestamp: 1000,
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedVideoChunk: invalid type throws', (t) => {
  t.throws(
    () => {
      new EncodedVideoChunk({
        type: 'invalid' as any,
        timestamp: 1000,
        data: new Uint8Array([0, 1]),
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedVideoChunk: construction with zero-size data', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new ArrayBuffer(0),
  })

  t.is(chunk.byteLength, 0)
})

test('EncodedVideoChunk: construction with ArrayBuffer', (t) => {
  const buffer = new ArrayBuffer(8)
  const view = new Uint8Array(buffer)
  view.set([1, 2, 3, 4, 5, 6, 7, 8])

  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: buffer,
  })

  t.is(chunk.byteLength, 8)
})

test('EncodedVideoChunk: construction with DataView', (t) => {
  const buffer = new ArrayBuffer(8)
  const dataView = new DataView(buffer)
  dataView.setUint8(0, 42)

  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: dataView,
  })

  t.is(chunk.byteLength, 8)
})

// ============================================================================
// EncodedVideoChunk copyTo Tests
// ============================================================================

test('EncodedVideoChunk: copyTo', (t) => {
  const sourceData = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: sourceData,
  })

  const dest = new Uint8Array(8)
  chunk.copyTo(dest)

  t.deepEqual(Array.from(dest), Array.from(sourceData))
})

test('EncodedVideoChunk: copyTo destination too small throws', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]),
  })

  t.throws(
    () => {
      chunk.copyTo(new Uint8Array(4))
    },
    { instanceOf: TypeError },
  )
})

test('EncodedVideoChunk: copyTo with ArrayBuffer', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1, 2, 3, 4]),
  })

  const dest = new ArrayBuffer(4)
  chunk.copyTo(dest)

  const view = new Uint8Array(dest)
  t.deepEqual(Array.from(view), [1, 2, 3, 4])
})

test('EncodedVideoChunk: copyTo empty chunk', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new ArrayBuffer(0),
  })

  const dest = new Uint8Array(0)
  chunk.copyTo(dest) // Should not throw
  t.pass()
})

test('EncodedVideoChunk: data is copied on construction', (t) => {
  const sourceData = new Uint8Array([1, 2, 3, 4])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: sourceData,
  })

  // Modify source
  sourceData[0] = 255

  // Chunk should have original value
  const dest = new Uint8Array(4)
  chunk.copyTo(dest)
  t.is(dest[0], 1)
})

// ============================================================================
// EncodedAudioChunk Construction Tests
// ============================================================================

test('EncodedAudioChunk: construction with key type', (t) => {
  const data = new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7])
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 1000,
    data,
  })

  t.is(chunk.type, 'key')
  t.is(chunk.timestamp, 1000)
  t.is(chunk.byteLength, 8)
  t.is(chunk.duration, null)
})

test('EncodedAudioChunk: construction with delta type', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'delta',
    timestamp: 2000,
    data: new Uint8Array([1, 2, 3, 4]),
  })

  t.is(chunk.type, 'delta')
  t.is(chunk.timestamp, 2000)
  t.is(chunk.byteLength, 4)
})

test('EncodedAudioChunk: construction with duration', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 1000,
    duration: 20000,
    data: new Uint8Array([0, 1, 2, 3]),
  })

  t.is(chunk.duration, 20000)
})

test('EncodedAudioChunk: construction with negative timestamp', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: -10000,
    data: new Uint8Array([0, 1]),
  })

  t.is(chunk.timestamp, -10000)
})

test('EncodedAudioChunk: construction requires type', (t) => {
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new EncodedAudioChunk({
        timestamp: 1000,
        data: new Uint8Array([0, 1]),
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedAudioChunk: construction requires timestamp', (t) => {
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new EncodedAudioChunk({
        type: 'key',
        data: new Uint8Array([0, 1]),
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedAudioChunk: construction requires data', (t) => {
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new EncodedAudioChunk({
        type: 'key',
        timestamp: 1000,
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedAudioChunk: invalid type throws', (t) => {
  t.throws(
    () => {
      new EncodedAudioChunk({
        type: 'invalid' as any,
        timestamp: 1000,
        data: new Uint8Array([0, 1]),
      })
    },
    { instanceOf: TypeError },
  )
})

test('EncodedAudioChunk: construction with zero-size data', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new ArrayBuffer(0),
  })

  t.is(chunk.byteLength, 0)
})

// ============================================================================
// EncodedAudioChunk copyTo Tests
// ============================================================================

test('EncodedAudioChunk: copyTo', (t) => {
  const sourceData = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: sourceData,
  })

  const dest = new Uint8Array(8)
  chunk.copyTo(dest)

  t.deepEqual(Array.from(dest), Array.from(sourceData))
})

test('EncodedAudioChunk: copyTo destination too small throws', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]),
  })

  t.throws(
    () => {
      chunk.copyTo(new Uint8Array(4))
    },
    { instanceOf: TypeError },
  )
})

test('EncodedAudioChunk: copyTo with ArrayBuffer', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1, 2, 3, 4]),
  })

  const dest = new ArrayBuffer(4)
  chunk.copyTo(dest)

  const view = new Uint8Array(dest)
  t.deepEqual(Array.from(view), [1, 2, 3, 4])
})

test('EncodedAudioChunk: copyTo empty chunk', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new ArrayBuffer(0),
  })

  const dest = new Uint8Array(0)
  chunk.copyTo(dest) // Should not throw
  t.pass()
})

test('EncodedAudioChunk: data is copied on construction', (t) => {
  const sourceData = new Uint8Array([1, 2, 3, 4])
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: sourceData,
  })

  // Modify source
  sourceData[0] = 255

  // Chunk should have original value
  const dest = new Uint8Array(4)
  chunk.copyTo(dest)
  t.is(dest[0], 1)
})

// ============================================================================
// Large Data Tests
// ============================================================================

test('EncodedVideoChunk: large data', (t) => {
  const size = 1024 * 1024 // 1MB
  const data = new Uint8Array(size)
  for (let i = 0; i < size; i++) {
    data[i] = i & 0xff
  }

  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, size)

  const dest = new Uint8Array(size)
  chunk.copyTo(dest)
  t.is(dest[0], 0)
  t.is(dest[255], 255)
  t.is(dest[256], 0)
})

test('EncodedAudioChunk: large data', (t) => {
  const size = 512 * 1024 // 512KB
  const data = new Uint8Array(size)
  for (let i = 0; i < size; i++) {
    data[i] = i & 0xff
  }

  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, size)

  const dest = new Uint8Array(size)
  chunk.copyTo(dest)
  t.is(dest[0], 0)
  t.is(dest[255], 255)
})

// ============================================================================
// Timestamp Edge Cases Tests
// ============================================================================

test('EncodedVideoChunk: zero timestamp', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1]),
  })

  t.is(chunk.timestamp, 0)
})

test('EncodedVideoChunk: large timestamp', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: Number.MAX_SAFE_INTEGER,
    data: new Uint8Array([1]),
  })

  t.is(chunk.timestamp, Number.MAX_SAFE_INTEGER)
})

test('EncodedVideoChunk: large negative timestamp', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: Number.MIN_SAFE_INTEGER,
    data: new Uint8Array([1]),
  })

  t.is(chunk.timestamp, Number.MIN_SAFE_INTEGER)
})

test('EncodedAudioChunk: zero timestamp', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1]),
  })

  t.is(chunk.timestamp, 0)
})

test('EncodedAudioChunk: large timestamp', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: Number.MAX_SAFE_INTEGER,
    data: new Uint8Array([1]),
  })

  t.is(chunk.timestamp, Number.MAX_SAFE_INTEGER)
})

// ============================================================================
// Duration Edge Cases Tests
// ============================================================================

test('EncodedVideoChunk: zero duration', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    duration: 0,
    data: new Uint8Array([1]),
  })

  t.is(chunk.duration, 0)
})

test('EncodedVideoChunk: large duration', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    duration: 1000000000, // 1000 seconds in microseconds
    data: new Uint8Array([1]),
  })

  t.is(chunk.duration, 1000000000)
})

test('EncodedAudioChunk: zero duration', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    duration: 0,
    data: new Uint8Array([1]),
  })

  t.is(chunk.duration, 0)
})

// ============================================================================
// Different Array Types Tests
// ============================================================================

test('EncodedVideoChunk: construction with Int8Array', (t) => {
  const data = new Int8Array([1, 2, 3, 4])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, 4)
})

test('EncodedVideoChunk: construction with Uint16Array', (t) => {
  const data = new Uint16Array([1, 2, 3, 4])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, 8) // 4 * 2 bytes
})

test('EncodedVideoChunk: construction with Float32Array', (t) => {
  const data = new Float32Array([1.0, 2.0, 3.0, 4.0])
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, 16) // 4 * 4 bytes
})

test('EncodedAudioChunk: construction with Int8Array', (t) => {
  const data = new Int8Array([1, 2, 3, 4])
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data,
  })

  t.is(chunk.byteLength, 4)
})

// ============================================================================
// copyTo Offset Tests
// ============================================================================

test('EncodedVideoChunk: copyTo with larger destination', (t) => {
  const chunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1, 2, 3, 4]),
  })

  const dest = new Uint8Array(10)
  chunk.copyTo(dest)

  // First 4 bytes should be filled, rest should be 0
  t.deepEqual(Array.from(dest), [1, 2, 3, 4, 0, 0, 0, 0, 0, 0])
})

test('EncodedAudioChunk: copyTo with larger destination', (t) => {
  const chunk = new EncodedAudioChunk({
    type: 'key',
    timestamp: 0,
    data: new Uint8Array([1, 2, 3, 4]),
  })

  const dest = new Uint8Array(10)
  chunk.copyTo(dest)

  // First 4 bytes should be filled, rest should be 0
  t.deepEqual(Array.from(dest), [1, 2, 3, 4, 0, 0, 0, 0, 0, 0])
})
