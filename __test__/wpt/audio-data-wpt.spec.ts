/**
 * AudioData Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests AudioData construction, copying, cloning, and format conversion.
 */

import test from 'ava'

import { AudioData, resetHardwareFallbackState } from '../../index.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// Construction Tests
// ============================================================================

test('AudioData: construction with valid init', (t) => {
  const data = new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7])
  const audioData = new AudioData({
    data,
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 1234,
  })

  t.is(audioData.format, 'u8')
  t.is(audioData.sampleRate, 44100)
  t.is(audioData.numberOfFrames, 4)
  t.is(audioData.numberOfChannels, 2)
  t.is(audioData.timestamp, 1234)

  audioData.close()
})

test('AudioData: construction requires all fields', (t) => {
  // Missing data
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new AudioData({
        format: 'u8',
        sampleRate: 44100,
        numberOfFrames: 4,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )

  // Missing format
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new AudioData({
        data: new Uint8Array(8),
        sampleRate: 44100,
        numberOfFrames: 4,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )

  // Missing sampleRate
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new AudioData({
        data: new Uint8Array(8),
        format: 'u8',
        numberOfFrames: 4,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )

  // Missing numberOfFrames
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new AudioData({
        data: new Uint8Array(8),
        format: 'u8',
        sampleRate: 44100,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )

  // Missing numberOfChannels
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new AudioData({
        data: new Uint8Array(8),
        format: 'u8',
        sampleRate: 44100,
        numberOfFrames: 4,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )

  // Missing timestamp
  t.throws(
    () => {
      // @ts-expect-error - Testing missing field
      new AudioData({
        data: new Uint8Array(8),
        format: 'u8',
        sampleRate: 44100,
        numberOfFrames: 4,
        numberOfChannels: 2,
      })
    },
    { instanceOf: TypeError },
  )
})

test('AudioData: invalid format throws', (t) => {
  t.throws(
    () => {
      new AudioData({
        data: new Uint8Array(8),
        format: 'bogus' as any,
        sampleRate: 44100,
        numberOfFrames: 4,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('AudioData: zero sampleRate throws', (t) => {
  t.throws(
    () => {
      new AudioData({
        data: new Uint8Array(8),
        format: 'u8',
        sampleRate: 0,
        numberOfFrames: 4,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('AudioData: zero numberOfFrames throws', (t) => {
  t.throws(
    () => {
      new AudioData({
        data: new Uint8Array(8),
        format: 'u8',
        sampleRate: 44100,
        numberOfFrames: 0,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('AudioData: zero numberOfChannels throws', (t) => {
  t.throws(
    () => {
      new AudioData({
        data: new Uint8Array(8),
        format: 'u8',
        sampleRate: 44100,
        numberOfFrames: 4,
        numberOfChannels: 0,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('AudioData: data too small throws', (t) => {
  // u8 format: 4 frames * 2 channels = 8 bytes needed, only 4 provided
  t.throws(
    () => {
      new AudioData({
        data: new Uint8Array(4),
        format: 'u8',
        sampleRate: 44100,
        numberOfFrames: 4,
        numberOfChannels: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('AudioData: negative timestamp', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: -12345,
  })

  t.is(audioData.timestamp, -12345)
  audioData.close()
})

// ============================================================================
// Format Tests
// ============================================================================

const formats = ['u8', 's16', 's32', 'f32', 'u8-planar', 's16-planar', 's32-planar', 'f32-planar'] as const

for (const format of formats) {
  test(`AudioData: construction with format ${format}`, (t) => {
    const bytesPerSample = format.startsWith('u8') ? 1 : format.startsWith('s16') ? 2 : 4
    const frames = 4
    const channels = 2
    const dataSize = frames * channels * bytesPerSample

    const audioData = new AudioData({
      data: new Uint8Array(dataSize),
      format,
      sampleRate: 44100,
      numberOfFrames: frames,
      numberOfChannels: channels,
      timestamp: 0,
    })

    t.is(audioData.format, format)
    t.is(audioData.numberOfFrames, frames)
    t.is(audioData.numberOfChannels, channels)

    audioData.close()
  })
}

// ============================================================================
// Close Tests
// ============================================================================

test('AudioData: close', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  t.is(audioData.format, 'u8')

  audioData.close()

  // After close, format should be null
  t.is(audioData.format, null)
})

test('AudioData: double close is safe', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  audioData.close()
  audioData.close() // Should not throw
  t.pass()
})

// ============================================================================
// Clone Tests
// ============================================================================

test('AudioData: clone', (t) => {
  const original = new AudioData({
    data: new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 1000,
  })

  const cloned = original.clone()

  t.is(cloned.format, original.format)
  t.is(cloned.sampleRate, original.sampleRate)
  t.is(cloned.numberOfFrames, original.numberOfFrames)
  t.is(cloned.numberOfChannels, original.numberOfChannels)
  t.is(cloned.timestamp, original.timestamp)

  // Closing original should not affect clone
  original.close()
  t.is(cloned.format, 'u8')

  cloned.close()
})

test('AudioData: clone closed throws', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  audioData.close()

  try {
    audioData.clone()
    t.fail('clone should throw InvalidStateError')
  } catch (error) {
    t.true(error instanceof DOMException, 'clone error should be DOMException')
    t.is((error as DOMException).name, 'InvalidStateError')
  }
})

// ============================================================================
// copyTo Tests
// ============================================================================

test('AudioData: copyTo interleaved', (t) => {
  const sourceData = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])
  const audioData = new AudioData({
    data: sourceData,
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  const dest = new Uint8Array(8)
  audioData.copyTo(dest, { planeIndex: 0 })

  // Check data was copied
  t.deepEqual(Array.from(dest), Array.from(sourceData))

  audioData.close()
})

test('AudioData: copyTo planar', (t) => {
  // Planar: first 4 bytes are channel 0, next 4 are channel 1
  const sourceData = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])
  const audioData = new AudioData({
    data: sourceData,
    format: 'u8-planar',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // Copy plane 0 (channel 0)
  const dest0 = new Uint8Array(4)
  audioData.copyTo(dest0, { planeIndex: 0 })
  t.deepEqual(Array.from(dest0), [1, 2, 3, 4])

  // Copy plane 1 (channel 1)
  const dest1 = new Uint8Array(4)
  audioData.copyTo(dest1, { planeIndex: 1 })
  t.deepEqual(Array.from(dest1), [5, 6, 7, 8])

  audioData.close()
})

test('AudioData: copyTo with invalid planeIndex throws', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // For interleaved format, only planeIndex 0 is valid
  t.throws(
    () => {
      audioData.copyTo(new Uint8Array(8), { planeIndex: 1 })
    },
    { instanceOf: RangeError },
  )

  audioData.close()
})

test('AudioData: copyTo closed throws', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  audioData.close()

  try {
    audioData.copyTo(new Uint8Array(8), { planeIndex: 0 })
    t.fail('copyTo should throw InvalidStateError')
  } catch (error) {
    t.true(error instanceof DOMException, 'copyTo error should be DOMException')
    t.is((error as DOMException).name, 'InvalidStateError')
  }
})

test('AudioData: copyTo destination too small throws', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  t.throws(
    () => {
      audioData.copyTo(new Uint8Array(4), { planeIndex: 0 })
    },
    { instanceOf: RangeError },
  )

  audioData.close()
})

// ============================================================================
// allocationSize Tests
// ============================================================================

test('AudioData: allocationSize for interleaved', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // For interleaved u8: 4 frames * 2 channels * 1 byte = 8
  const size = audioData.allocationSize({ planeIndex: 0 })
  t.is(size, 8)

  audioData.close()
})

test('AudioData: allocationSize for planar', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8-planar',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // For planar u8: 4 frames * 1 byte per plane
  const size0 = audioData.allocationSize({ planeIndex: 0 })
  t.is(size0, 4)

  const size1 = audioData.allocationSize({ planeIndex: 1 })
  t.is(size1, 4)

  audioData.close()
})

test('AudioData: allocationSize closed throws', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  audioData.close()

  try {
    audioData.allocationSize({ planeIndex: 0 })
    t.fail('allocationSize should throw InvalidStateError')
  } catch (error) {
    t.true(error instanceof DOMException, 'allocationSize error should be DOMException')
    t.is((error as DOMException).name, 'InvalidStateError')
  }
})

test('AudioData: allocationSize invalid planeIndex throws', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // For interleaved, only planeIndex 0 is valid
  t.throws(
    () => {
      audioData.allocationSize({ planeIndex: 1 })
    },
    { instanceOf: RangeError },
  )

  audioData.close()
})

// ============================================================================
// Duration Tests
// ============================================================================

test('AudioData: duration calculation', (t) => {
  // u8 format, 2 channels, 44100 frames = 88200 bytes
  const audioData = new AudioData({
    data: new Uint8Array(88200),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 44100, // 1 second worth of frames
    numberOfChannels: 2,
    timestamp: 0,
  })

  // Duration in microseconds: frames / sampleRate * 1_000_000
  // 44100 / 44100 * 1_000_000 = 1_000_000 microseconds = 1 second
  t.is(audioData.duration, 1_000_000)

  audioData.close()
})

test('AudioData: duration for 48kHz', (t) => {
  const audioData = new AudioData({
    data: new Float32Array(48000 * 2), // 1 second stereo
    format: 'f32',
    sampleRate: 48000,
    numberOfFrames: 48000,
    numberOfChannels: 2,
    timestamp: 0,
  })

  t.is(audioData.duration, 1_000_000) // 1 second in microseconds

  audioData.close()
})

// ============================================================================
// Data Copy on Construction Tests
// ============================================================================

test('AudioData: input data is copied on construction', (t) => {
  const sourceData = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])
  const audioData = new AudioData({
    data: sourceData,
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // Modify source data
  sourceData[0] = 255

  // Copy from AudioData
  const dest = new Uint8Array(8)
  audioData.copyTo(dest, { planeIndex: 0 })

  // AudioData should have original value, not modified value
  t.is(dest[0], 1)

  audioData.close()
})

// ============================================================================
// Different Array Types Tests
// ============================================================================

test('AudioData: construction with different typed arrays', (t) => {
  // Uint8Array
  const u8Data = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })
  t.is(u8Data.format, 'u8')
  u8Data.close()

  // Int16Array
  const s16Data = new AudioData({
    data: new Int16Array(8),
    format: 's16',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })
  t.is(s16Data.format, 's16')
  s16Data.close()

  // Int32Array
  const s32Data = new AudioData({
    data: new Int32Array(8),
    format: 's32',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })
  t.is(s32Data.format, 's32')
  s32Data.close()

  // Float32Array
  const f32Data = new AudioData({
    data: new Float32Array(8),
    format: 'f32',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })
  t.is(f32Data.format, 'f32')
  f32Data.close()
})

// ============================================================================
// numberOfPlanes Tests
// ============================================================================

test('AudioData: numberOfPlanes for interleaved', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // Interleaved formats have 1 plane
  t.is(audioData.numberOfPlanes, 1)

  audioData.close()
})

test('AudioData: numberOfPlanes for planar', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array(8),
    format: 'u8-planar',
    sampleRate: 44100,
    numberOfFrames: 4,
    numberOfChannels: 2,
    timestamp: 0,
  })

  // Planar formats have numberOfChannels planes
  t.is(audioData.numberOfPlanes, 2)

  audioData.close()
})

// ============================================================================
// Large Data Tests
// ============================================================================

test('AudioData: large data handling', (t) => {
  const frames = 48000 * 10 // 10 seconds at 48kHz
  const channels = 2
  const data = new Float32Array(frames * channels)

  // Fill with some data
  for (let i = 0; i < data.length; i++) {
    data[i] = Math.sin(i / 100)
  }

  const audioData = new AudioData({
    data,
    format: 'f32',
    sampleRate: 48000,
    numberOfFrames: frames,
    numberOfChannels: channels,
    timestamp: 0,
  })

  t.is(audioData.numberOfFrames, frames)
  t.is(audioData.duration, 10_000_000) // 10 seconds in microseconds

  audioData.close()
})

// ============================================================================
// Edge Case Tests
// ============================================================================

test('AudioData: single sample', (t) => {
  const audioData = new AudioData({
    data: new Uint8Array([42]),
    format: 'u8',
    sampleRate: 44100,
    numberOfFrames: 1,
    numberOfChannels: 1,
    timestamp: 0,
  })

  t.is(audioData.numberOfFrames, 1)
  t.is(audioData.numberOfChannels, 1)

  const dest = new Uint8Array(1)
  audioData.copyTo(dest, { planeIndex: 0 })
  t.is(dest[0], 42)

  audioData.close()
})

test('AudioData: many channels', (t) => {
  const channels = 8
  const frames = 4
  const data = new Float32Array(channels * frames)

  const audioData = new AudioData({
    data,
    format: 'f32',
    sampleRate: 48000,
    numberOfFrames: frames,
    numberOfChannels: channels,
    timestamp: 0,
  })

  t.is(audioData.numberOfChannels, channels)

  audioData.close()
})
