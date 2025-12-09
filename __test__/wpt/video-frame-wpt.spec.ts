/**
 * VideoFrame Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/nicosurjana/nicosurjana.git
 *
 * Tests VideoFrame construction, properties, copyTo, and cloning.
 * Note: Canvas-based tests and ImageBitmap tests are skipped (not available in Node.js).
 */

import test from 'ava'

import { DOMRectReadOnly, resetHardwareFallbackState, VideoColorSpace, VideoFrame } from '../../index.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// Construction Tests - Buffer Based
// ============================================================================

test('VideoFrame: construction from I420 buffer', (t) => {
  const width = 4
  const height = 2
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  const data = new Uint8Array(ySize + uvSize * 2)

  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'I420')
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)
  t.is(frame.timestamp, 0)

  frame.close()
})

test('VideoFrame: construction from I420A buffer', (t) => {
  const width = 4
  const height = 2
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  const aSize = width * height
  const data = new Uint8Array(ySize + uvSize * 2 + aSize)

  const frame = new VideoFrame(data, {
    format: 'I420A',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'I420A')
  frame.close()
})

test('VideoFrame: construction from I422 buffer', (t) => {
  const width = 4
  const height = 2
  const ySize = width * height
  const uvSize = (width / 2) * height
  const data = new Uint8Array(ySize + uvSize * 2)

  const frame = new VideoFrame(data, {
    format: 'I422',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'I422')
  frame.close()
})

test('VideoFrame: construction from I444 buffer', (t) => {
  const width = 4
  const height = 2
  const ySize = width * height
  const uvSize = width * height
  const data = new Uint8Array(ySize + uvSize * 2)

  const frame = new VideoFrame(data, {
    format: 'I444',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'I444')
  frame.close()
})

test('VideoFrame: construction from NV12 buffer', (t) => {
  const width = 4
  const height = 2
  const ySize = width * height
  const uvSize = width * (height / 2)
  const data = new Uint8Array(ySize + uvSize)

  const frame = new VideoFrame(data, {
    format: 'NV12',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'NV12')
  frame.close()
})

test('VideoFrame: construction from RGBA buffer', (t) => {
  const width = 4
  const height = 2
  const data = new Uint8Array(width * height * 4)

  const frame = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'RGBA')
  frame.close()
})

test('VideoFrame: construction from BGRA buffer', (t) => {
  const width = 4
  const height = 2
  const data = new Uint8Array(width * height * 4)

  const frame = new VideoFrame(data, {
    format: 'BGRA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'BGRA')
  frame.close()
})

test('VideoFrame: construction from RGBX buffer', (t) => {
  const width = 4
  const height = 2
  const data = new Uint8Array(width * height * 4)

  const frame = new VideoFrame(data, {
    format: 'RGBX',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'RGBX')
  frame.close()
})

test('VideoFrame: construction from BGRX buffer', (t) => {
  const width = 4
  const height = 2
  const data = new Uint8Array(width * height * 4)

  const frame = new VideoFrame(data, {
    format: 'BGRX',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.format, 'BGRX')
  frame.close()
})

// ============================================================================
// Construction Validation Tests
// ============================================================================

test('VideoFrame: missing format throws', (t) => {
  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      new VideoFrame(new Uint8Array(16), {
        codedWidth: 4,
        codedHeight: 2,
        timestamp: 0,
      } as any)
    },
    { instanceOf: TypeError },
  )
})

test('VideoFrame: missing codedWidth throws', (t) => {
  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      new VideoFrame(new Uint8Array(16), {
        format: 'RGBA',
        codedHeight: 2,
        timestamp: 0,
      } as any)
    },
    { instanceOf: TypeError },
  )
})

test('VideoFrame: missing codedHeight throws', (t) => {
  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      new VideoFrame(new Uint8Array(16), {
        format: 'RGBA',
        codedWidth: 4,
        timestamp: 0,
      } as any)
    },
    { instanceOf: TypeError },
  )
})

test('VideoFrame: missing timestamp throws', (t) => {
  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      new VideoFrame(new Uint8Array(16), {
        format: 'RGBA',
        codedWidth: 4,
        codedHeight: 2,
      } as any)
    },
    { instanceOf: TypeError },
  )
})

test('VideoFrame: zero codedWidth throws', (t) => {
  t.throws(
    () => {
      new VideoFrame(new Uint8Array(0), {
        format: 'RGBA',
        codedWidth: 0,
        codedHeight: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('VideoFrame: zero codedHeight throws', (t) => {
  t.throws(
    () => {
      new VideoFrame(new Uint8Array(0), {
        format: 'RGBA',
        codedWidth: 4,
        codedHeight: 0,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('VideoFrame: invalid format throws', (t) => {
  t.throws(
    () => {
      new VideoFrame(new Uint8Array(16), {
        format: 'INVALID' as any,
        codedWidth: 4,
        codedHeight: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

test('VideoFrame: buffer too small throws', (t) => {
  t.throws(
    () => {
      new VideoFrame(new Uint8Array(4), {
        format: 'RGBA',
        codedWidth: 4,
        codedHeight: 2,
        timestamp: 0,
      })
    },
    { instanceOf: TypeError },
  )
})

// ============================================================================
// Timestamp Tests
// ============================================================================

test('VideoFrame: negative timestamp', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: -12345,
  })

  t.is(frame.timestamp, -12345)
  frame.close()
})

test('VideoFrame: large timestamp', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: Number.MAX_SAFE_INTEGER,
  })

  t.is(frame.timestamp, Number.MAX_SAFE_INTEGER)
  frame.close()
})

// ============================================================================
// Duration Tests
// ============================================================================

test('VideoFrame: with duration', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
    duration: 33333,
  })

  t.is(frame.duration, 33333)
  frame.close()
})

test('VideoFrame: without duration', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  t.is(frame.duration, null)
  frame.close()
})

// ============================================================================
// Display Dimensions Tests
// ============================================================================

test('VideoFrame: displayWidth and displayHeight default to coded', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  t.is(frame.displayWidth, 4)
  t.is(frame.displayHeight, 2)
  frame.close()
})

test('VideoFrame: explicit displayWidth and displayHeight', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
    displayWidth: 8,
    displayHeight: 4,
  })

  t.is(frame.codedWidth, 4)
  t.is(frame.codedHeight, 2)
  t.is(frame.displayWidth, 8)
  t.is(frame.displayHeight, 4)
  frame.close()
})

// ============================================================================
// Close Tests
// ============================================================================

test('VideoFrame: close', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  t.is(frame.format, 'RGBA')

  frame.close()

  // After close, format should be null
  t.is(frame.format, null)
})

test('VideoFrame: double close is safe', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  frame.close()
  frame.close() // Should not throw
  t.pass()
})

test('VideoFrame: properties after close', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
    duration: 1000,
  })

  frame.close()

  t.is(frame.format, null)
  t.is(frame.codedWidth, 0)
  t.is(frame.codedHeight, 0)
  t.is(frame.displayWidth, 0)
  t.is(frame.displayHeight, 0)
})

// ============================================================================
// Clone Tests
// ============================================================================

test('VideoFrame: clone', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 1000,
    duration: 33333,
  })

  const cloned = frame.clone()

  t.is(cloned.format, frame.format)
  t.is(cloned.codedWidth, frame.codedWidth)
  t.is(cloned.codedHeight, frame.codedHeight)
  t.is(cloned.timestamp, frame.timestamp)
  t.is(cloned.duration, frame.duration)

  // Closing original should not affect clone
  frame.close()
  t.is(cloned.format, 'RGBA')

  cloned.close()
})

test('VideoFrame: clone closed throws', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  frame.close()

  t.throws(
    () => {
      frame.clone()
    },
    { message: /InvalidStateError/ },
  )
})

// ============================================================================
// copyTo Tests
// ============================================================================

test('VideoFrame: copyTo RGBA', async (t) => {
  const sourceData = new Uint8Array(32)
  for (let i = 0; i < sourceData.length; i++) {
    sourceData[i] = i
  }

  const frame = new VideoFrame(sourceData, {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  const dest = new Uint8Array(32)
  const layout = await frame.copyTo(dest)

  t.true(layout.length >= 1)
  t.deepEqual(Array.from(dest), Array.from(sourceData))

  frame.close()
})

test('VideoFrame: copyTo I420', async (t) => {
  const width = 4
  const height = 2
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  const totalSize = ySize + uvSize * 2

  const sourceData = new Uint8Array(totalSize)
  for (let i = 0; i < sourceData.length; i++) {
    sourceData[i] = i
  }

  const frame = new VideoFrame(sourceData, {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  const dest = new Uint8Array(totalSize)
  const layout = await frame.copyTo(dest)

  t.true(layout.length >= 3) // Y, U, V planes
  t.deepEqual(Array.from(dest), Array.from(sourceData))

  frame.close()
})

test('VideoFrame: copyTo closed throws', async (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  frame.close()

  await t.throwsAsync(frame.copyTo(new Uint8Array(32)), { message: /InvalidStateError/ })
})

test('VideoFrame: copyTo destination too small throws', async (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  await t.throwsAsync(frame.copyTo(new Uint8Array(16)), { message: /TypeError/ })

  frame.close()
})

// ============================================================================
// allocationSize Tests
// ============================================================================

test('VideoFrame: allocationSize RGBA', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  const size = frame.allocationSize()
  t.is(size, 32) // 4 * 2 * 4 bytes per pixel

  frame.close()
})

test('VideoFrame: allocationSize I420', (t) => {
  const width = 4
  const height = 2
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  const totalSize = ySize + uvSize * 2

  const frame = new VideoFrame(new Uint8Array(totalSize), {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  const size = frame.allocationSize()
  t.is(size, totalSize)

  frame.close()
})

test('VideoFrame: allocationSize closed throws', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  frame.close()

  t.throws(
    () => {
      frame.allocationSize()
    },
    { message: /InvalidStateError/ },
  )
})

// ============================================================================
// colorSpace Tests
// ============================================================================

test('VideoFrame: default colorSpace', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  t.truthy(frame.colorSpace)
  t.true(frame.colorSpace instanceof VideoColorSpace)

  frame.close()
})

test('VideoFrame: explicit colorSpace', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
    colorSpace: {
      primaries: 'bt709',
      transfer: 'srgb',
      matrix: 'rgb',
      fullRange: true,
    },
  })

  t.is(frame.colorSpace.primaries, 'bt709')
  t.is(frame.colorSpace.transfer, 'srgb')
  t.is(frame.colorSpace.matrix, 'rgb')
  t.is(frame.colorSpace.fullRange, true)

  frame.close()
})

// ============================================================================
// codedRect Tests
// ============================================================================

test('VideoFrame: codedRect', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  const rect = frame.codedRect
  t.truthy(rect)
  t.true(rect instanceof DOMRectReadOnly)
  t.is(rect.x, 0)
  t.is(rect.y, 0)
  t.is(rect.width, 4)
  t.is(rect.height, 2)

  frame.close()
})

test('VideoFrame: codedRect closed throws', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  frame.close()

  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-expressions
      frame.codedRect
    },
    { message: /InvalidStateError/ },
  )
})

// ============================================================================
// visibleRect Tests
// ============================================================================

test('VideoFrame: visibleRect', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  const rect = frame.visibleRect
  t.truthy(rect)
  t.true(rect instanceof DOMRectReadOnly)

  frame.close()
})

test('VideoFrame: visibleRect closed throws', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  frame.close()

  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-unused-expressions
      frame.visibleRect
    },
    { message: /InvalidStateError/ },
  )
})

// ============================================================================
// fromVideoFrame Tests
// ============================================================================

test('VideoFrame.fromVideoFrame: basic clone', (t) => {
  const original = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 1000,
  })

  const cloned = VideoFrame.fromVideoFrame(original)

  t.is(cloned.format, original.format)
  t.is(cloned.codedWidth, original.codedWidth)
  t.is(cloned.codedHeight, original.codedHeight)
  t.is(cloned.timestamp, original.timestamp)

  original.close()
  cloned.close()
})

test('VideoFrame.fromVideoFrame: with new timestamp', (t) => {
  const original = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 1000,
  })

  const cloned = VideoFrame.fromVideoFrame(original, { timestamp: 2000 })

  t.is(cloned.timestamp, 2000)
  t.is(original.timestamp, 1000)

  original.close()
  cloned.close()
})

test('VideoFrame.fromVideoFrame: with new duration', (t) => {
  const original = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 1000,
    duration: 1000,
  })

  const cloned = VideoFrame.fromVideoFrame(original, { duration: 2000 })

  t.is(cloned.duration, 2000)
  t.is(original.duration, 1000)

  original.close()
  cloned.close()
})

test('VideoFrame.fromVideoFrame: from closed throws', (t) => {
  const original = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 1000,
  })

  original.close()

  t.throws(
    () => {
      VideoFrame.fromVideoFrame(original)
    },
    { message: /InvalidStateError/ },
  )
})

// ============================================================================
// Data Copy Tests
// ============================================================================

test('VideoFrame: data is copied on construction', async (t) => {
  const sourceData = new Uint8Array(32)
  sourceData[0] = 42

  const frame = new VideoFrame(sourceData, {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  // Modify source
  sourceData[0] = 99

  // Frame should have original value
  const dest = new Uint8Array(32)
  await frame.copyTo(dest)
  t.is(dest[0], 42)

  frame.close()
})

// ============================================================================
// Large Frame Tests
// ============================================================================

test('VideoFrame: large 1080p frame', (t) => {
  const width = 1920
  const height = 1080
  const data = new Uint8Array(width * height * 4)

  const frame = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)

  frame.close()
})

test('VideoFrame: large 4K frame', (t) => {
  const width = 3840
  const height = 2160
  const data = new Uint8Array(width * height * 4)

  const frame = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)

  frame.close()
})

// ============================================================================
// Odd Dimensions Tests
// ============================================================================

test('VideoFrame: RGBA odd dimensions', (t) => {
  const frame = new VideoFrame(new Uint8Array(3 * 3 * 4), {
    format: 'RGBA',
    codedWidth: 3,
    codedHeight: 3,
    timestamp: 0,
  })

  t.is(frame.codedWidth, 3)
  t.is(frame.codedHeight, 3)

  frame.close()
})

// ============================================================================
// numberOfPlanes Tests
// ============================================================================

test('VideoFrame: numberOfPlanes RGBA', (t) => {
  const frame = new VideoFrame(new Uint8Array(32), {
    format: 'RGBA',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  // RGBA has 1 plane
  t.is(frame.numberOfPlanes, 1)

  frame.close()
})

test('VideoFrame: numberOfPlanes I420', (t) => {
  const frame = new VideoFrame(new Uint8Array(12), {
    format: 'I420',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  // I420 has 3 planes (Y, U, V)
  t.is(frame.numberOfPlanes, 3)

  frame.close()
})

test('VideoFrame: numberOfPlanes I420A', (t) => {
  const frame = new VideoFrame(new Uint8Array(20), {
    format: 'I420A',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  // I420A has 4 planes (Y, U, V, A)
  t.is(frame.numberOfPlanes, 4)

  frame.close()
})

test('VideoFrame: numberOfPlanes NV12', (t) => {
  const frame = new VideoFrame(new Uint8Array(12), {
    format: 'NV12',
    codedWidth: 4,
    codedHeight: 2,
    timestamp: 0,
  })

  // NV12 has 2 planes (Y, UV interleaved)
  t.is(frame.numberOfPlanes, 2)

  frame.close()
})
