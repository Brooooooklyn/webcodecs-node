/**
 * VideoFrame Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
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

test('VideoFrame: construction with visibleRect crops correctly', (t) => {
  const width = 8
  const height = 8
  const data = new Uint8Array(width * height * 4) // RGBA

  const frame = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
    visibleRect: { x: 2, y: 2, width: 4, height: 4 },
  })

  t.is(frame.codedWidth, 8)
  t.is(frame.codedHeight, 8)
  t.is(frame.visibleRect?.x, 2)
  t.is(frame.visibleRect?.y, 2)
  t.is(frame.visibleRect?.width, 4)
  t.is(frame.visibleRect?.height, 4)
  t.is(frame.displayWidth, 4) // Defaults to visible dimensions
  t.is(frame.displayHeight, 4)

  frame.close()
})

test('VideoFrame: I420 with even visibleRect offset succeeds', (t) => {
  const data = new Uint8Array(64 * 64 * 1.5) // I420

  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: 64,
    codedHeight: 64,
    timestamp: 0,
    visibleRect: { x: 2, y: 4, width: 8, height: 8 },
  })

  t.is(frame.visibleRect?.x, 2)
  t.is(frame.visibleRect?.y, 4)
  frame.close()
})

test('VideoFrame: I420 with odd x offset throws alignment error', (t) => {
  const data = new Uint8Array(64 * 64 * 1.5) // I420

  const error = t.throws(() => {
    new VideoFrame(data, {
      format: 'I420',
      codedWidth: 64,
      codedHeight: 64,
      timestamp: 0,
      visibleRect: { x: 1, y: 0, width: 4, height: 4 }, // x=1 is odd
    })
  })

  t.true(error?.message.includes('alignment'))
})

test('VideoFrame: I420 with odd y offset throws alignment error', (t) => {
  const data = new Uint8Array(64 * 64 * 1.5) // I420

  const error = t.throws(() => {
    new VideoFrame(data, {
      format: 'I420',
      codedWidth: 64,
      codedHeight: 64,
      timestamp: 0,
      visibleRect: { x: 0, y: 1, width: 4, height: 4 }, // y=1 is odd
    })
  })

  t.true(error?.message.includes('alignment'))
})

test('VideoFrame: RGBA allows any visibleRect offset', (t) => {
  const data = new Uint8Array(64 * 64 * 4) // RGBA

  const frame = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: 64,
    codedHeight: 64,
    timestamp: 0,
    visibleRect: { x: 1, y: 3, width: 10, height: 10 }, // Odd offsets OK for RGBA
  })

  t.is(frame.visibleRect?.x, 1)
  t.is(frame.visibleRect?.y, 3)
  frame.close()
})

test('VideoFrame: visibleRect exceeding bounds throws TypeError', (t) => {
  const data = new Uint8Array(64 * 64 * 4) // RGBA

  const error = t.throws(() => {
    new VideoFrame(data, {
      format: 'RGBA',
      codedWidth: 64,
      codedHeight: 64,
      timestamp: 0,
      visibleRect: { x: 60, y: 0, width: 10, height: 10 }, // 60+10 > 64
    })
  })

  t.true(error?.message.includes('exceeds'))
})

test('VideoFrame.fromVideoFrame: with visibleRect crops', (t) => {
  const width = 8
  const height = 8
  const data = new Uint8Array(width * height * 4) // RGBA

  const source = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  const cropped = VideoFrame.fromVideoFrame(source, {
    visibleRect: { x: 2, y: 2, width: 4, height: 4 },
  })

  t.is(cropped.visibleRect?.x, 2)
  t.is(cropped.visibleRect?.y, 2)
  t.is(cropped.visibleRect?.width, 4)
  t.is(cropped.visibleRect?.height, 4)
  t.is(cropped.displayWidth, 4)
  t.is(cropped.displayHeight, 4)

  source.close()
  cropped.close()
})

test('VideoFrame: copyTo with rect copies subregion', async (t) => {
  const width = 8
  const height = 8
  const data = new Uint8Array(width * height * 4) // RGBA
  // Fill with pattern
  for (let i = 0; i < data.length; i++) {
    data[i] = i % 256
  }

  const frame = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  // Copy a 4x4 subregion starting at (2, 2)
  const subSize = frame.allocationSize({ rect: { x: 2, y: 2, width: 4, height: 4 } })
  t.is(subSize, 4 * 4 * 4) // 4x4 pixels * 4 bytes per pixel = 64 bytes
  const subDest = new Uint8Array(subSize)

  await frame.copyTo(subDest, { rect: { x: 2, y: 2, width: 4, height: 4 } })

  // Verify first pixel of subregion matches expected data
  // Row 2, Col 2 of 8-wide RGBA = offset (2*8 + 2) * 4 = 72
  t.is(subDest[0], data[72])
  t.is(subDest[1], data[73])
  t.is(subDest[2], data[74])
  t.is(subDest[3], data[75])

  frame.close()
})

test('VideoFrame: allocationSize uses visible rect by default', (t) => {
  const data = new Uint8Array(64 * 64 * 4) // RGBA

  const frame = new VideoFrame(data, {
    format: 'RGBA',
    codedWidth: 64,
    codedHeight: 64,
    timestamp: 0,
    visibleRect: { x: 0, y: 0, width: 32, height: 32 },
  })

  // Default allocationSize should use visible rect dimensions
  const size = frame.allocationSize()
  t.is(size, 32 * 32 * 4) // 32x32 pixels * 4 bytes per pixel

  frame.close()
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

// ============================================================================
// High Bit-Depth Pixel Format Tests (WPT)
// These formats use 2 bytes per sample instead of 1
// ============================================================================

/**
 * Calculate buffer size for YUV formats with given bits per sample
 */
function calculateYuvSize(
  width: number,
  height: number,
  subsampleX: number, // 2 for 4:2:0/4:2:2, 1 for 4:4:4
  subsampleY: number, // 2 for 4:2:0, 1 for 4:2:2/4:4:4
  bytesPerSample: number,
  hasAlpha: boolean,
): number {
  const ySize = width * height * bytesPerSample
  const uvWidth = Math.ceil(width / subsampleX)
  const uvHeight = Math.ceil(height / subsampleY)
  const uvSize = uvWidth * uvHeight * bytesPerSample * 2
  const aSize = hasAlpha ? width * height * bytesPerSample : 0
  return ySize + uvSize + aSize
}

// ============================================================================
// High Bit-Depth Format Tests (10-bit and 12-bit)
// All high bit-depth formats are fully implemented and mapped to FFmpeg
// ============================================================================

// 10-bit formats (2 bytes per sample)

test('VideoFrame: construction from I420P10 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 2, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I420P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I420P10')
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)
  t.is(frame.timestamp, 1234)

  frame.close()
})

test('VideoFrame: construction from I422P10 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 1, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I422P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I422P10')
  frame.close()
})

test('VideoFrame: construction from I444P10 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 1, 1, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I444P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I444P10')
  frame.close()
})

// 12-bit formats (2 bytes per sample)

test('VideoFrame: construction from I420P12 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 2, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I420P12',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I420P12')
  frame.close()
})

test('VideoFrame: construction from I422P12 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 1, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I422P12',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I422P12')
  frame.close()
})

test('VideoFrame: construction from I444P12 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 1, 1, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I444P12',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I444P12')
  frame.close()
})

// 8-bit alpha variants

test('VideoFrame: construction from I422A buffer', (t) => {
  const width = 4
  const height = 2
  // I422A: Y (w*h) + U (w/2*h) + V (w/2*h) + A (w*h)
  const ySize = width * height
  const uvSize = (width / 2) * height * 2
  const aSize = width * height
  const data = new Uint8Array(ySize + uvSize + aSize)

  const frame = new VideoFrame(data, {
    format: 'I422A',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I422A')
  t.is(frame.numberOfPlanes, 4) // Y, U, V, A
  frame.close()
})

test('VideoFrame: construction from I444A buffer', (t) => {
  const width = 4
  const height = 2
  // I444A: Y (w*h) + U (w*h) + V (w*h) + A (w*h)
  const size = width * height * 4
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I444A',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I444A')
  t.is(frame.numberOfPlanes, 4) // Y, U, V, A
  frame.close()
})

// 10-bit alpha variants

test('VideoFrame: construction from I420AP10 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 2, 2, true)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I420AP10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I420AP10')
  t.is(frame.numberOfPlanes, 4) // Y, U, V, A
  frame.close()
})

test('VideoFrame: construction from I422AP10 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 1, 2, true)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I422AP10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I422AP10')
  t.is(frame.numberOfPlanes, 4)
  frame.close()
})

test('VideoFrame: construction from I444AP10 buffer', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 1, 1, 2, true)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I444AP10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 1234,
  })

  t.is(frame.format, 'I444AP10')
  t.is(frame.numberOfPlanes, 4)
  frame.close()
})

// ============================================================================
// High Bit-Depth copyTo Tests
// ============================================================================

test('VideoFrame: copyTo I420P10', async (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 2, 2, false)
  const sourceData = new Uint8Array(size)

  // Fill with test pattern
  for (let i = 0; i < sourceData.length; i++) {
    sourceData[i] = i % 256
  }

  const frame = new VideoFrame(sourceData, {
    format: 'I420P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  const allocSize = frame.allocationSize()
  t.true(allocSize > 0, 'allocation size should be positive')

  const dest = new Uint8Array(allocSize)
  const layout = await frame.copyTo(dest)

  t.true(layout.length >= 3, 'should have at least 3 planes (Y, U, V)')

  frame.close()
})

test('VideoFrame: copyTo I444P10', async (t) => {
  const width = 4
  const height = 2
  const bps = 2 // bytes per sample
  const sourceData = new Uint8Array(width * height * bps * 3) // Y, U, V planes

  for (let i = 0; i < sourceData.length; i++) {
    sourceData[i] = i % 256
  }

  const frame = new VideoFrame(sourceData, {
    format: 'I444P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  const dest = new Uint8Array(frame.allocationSize())
  const layout = await frame.copyTo(dest)

  // Verify we have 3 planes (Y, U, V)
  t.is(layout.length, 3, 'should have 3 planes')

  // Verify each plane has the expected data
  // FFmpeg may use stride padding, so we extract row by row
  const rowBytes = width * bps
  const planeSize = rowBytes * height

  for (let planeIdx = 0; planeIdx < 3; planeIdx++) {
    const planeLayout = layout[planeIdx]
    const srcPlaneOffset = planeIdx * planeSize
    for (let row = 0; row < height; row++) {
      const srcRowStart = srcPlaneOffset + row * rowBytes
      const dstRowStart = planeLayout.offset + row * planeLayout.stride
      const srcRow = sourceData.slice(srcRowStart, srcRowStart + rowBytes)
      const dstRow = dest.slice(dstRowStart, dstRowStart + rowBytes)
      t.deepEqual(Array.from(dstRow), Array.from(srcRow), `plane ${planeIdx} row ${row} data mismatch`)
    }
  }

  frame.close()
})

test('VideoFrame: copyTo I420AP10 with alpha', async (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 2, 2, true)
  const sourceData = new Uint8Array(size)

  for (let i = 0; i < sourceData.length; i++) {
    sourceData[i] = i % 256
  }

  const frame = new VideoFrame(sourceData, {
    format: 'I420AP10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  const dest = new Uint8Array(frame.allocationSize())
  const layout = await frame.copyTo(dest)

  t.is(layout.length, 4, 'should have 4 planes (Y, U, V, A)')

  frame.close()
})

// ============================================================================
// High Bit-Depth allocationSize Tests
// ============================================================================

test('VideoFrame: allocationSize I420P10', (t) => {
  const width = 4
  const height = 2
  const expectedSize = calculateYuvSize(width, height, 2, 2, 2, false)
  const data = new Uint8Array(expectedSize)

  const frame = new VideoFrame(data, {
    format: 'I420P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.allocationSize(), expectedSize)

  frame.close()
})

test('VideoFrame: allocationSize I444AP10', (t) => {
  const width = 4
  const height = 2
  const expectedSize = calculateYuvSize(width, height, 1, 1, 2, true)
  const data = new Uint8Array(expectedSize)

  const frame = new VideoFrame(data, {
    format: 'I444AP10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.allocationSize(), expectedSize)

  frame.close()
})

// ============================================================================
// High Bit-Depth ColorSpace Tests
// ============================================================================

test('VideoFrame: I420P10 default colorSpace', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 2, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I420P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  // Per WebCodecs spec, unspecified color space values remain null
  // Color space is only populated when explicitly provided or from encoded data
  t.is(frame.colorSpace.primaries, null)
  t.is(frame.colorSpace.transfer, null)
  t.is(frame.colorSpace.matrix, null)
  t.is(frame.colorSpace.fullRange, null)

  frame.close()
})

test('VideoFrame: I420P10 explicit BT.2020 colorSpace', (t) => {
  const width = 4
  const height = 2
  const size = calculateYuvSize(width, height, 2, 2, 2, false)
  const data = new Uint8Array(size)

  const frame = new VideoFrame(data, {
    format: 'I420P10',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
    colorSpace: {
      primaries: 'bt2020',
      transfer: 'pq',
      matrix: 'bt2020-ncl',
      fullRange: false,
    },
  })

  t.is(frame.colorSpace.primaries, 'bt2020')
  t.is(frame.colorSpace.transfer, 'pq')
  t.is(frame.colorSpace.matrix, 'bt2020-ncl')
  t.is(frame.colorSpace.fullRange, false)

  frame.close()
})
