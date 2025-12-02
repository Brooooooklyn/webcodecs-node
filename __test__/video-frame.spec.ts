/**
 * VideoFrame API Conformance Tests
 *
 * Tests WebCodecs VideoFrame specification compliance.
 */

import test from 'ava'

import { VideoFrame } from '../index.js'
import {
  generateSolidColorI420Frame,
  generateSolidColorRGBAFrame,
  generateGradientI420Frame,
  TestColors,
  calculateI420Size,
  calculateRGBASize,
  TestResolutions,
} from './helpers/index.js'

// ============================================================================
// Constructor Tests
// ============================================================================

test('VideoFrame: constructor with I420 data', (t) => {
  const width = 320
  const height = 240
  const timestamp = 1000

  const frame = generateSolidColorI420Frame(width, height, TestColors.red, timestamp)

  t.is(frame.format, 'I420')
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)
  t.is(frame.displayWidth, width)
  t.is(frame.displayHeight, height)
  t.is(frame.timestamp, timestamp)

  frame.close()
})

test('VideoFrame: constructor with RGBA data', (t) => {
  const width = 320
  const height = 240
  const timestamp = 2000

  const frame = generateSolidColorRGBAFrame(width, height, TestColors.blue, timestamp)

  t.is(frame.format, 'RGBA')
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)
  t.is(frame.timestamp, timestamp)

  frame.close()
})

test('VideoFrame: constructor with duration', (t) => {
  const width = 320
  const height = 240
  const timestamp = 0
  const duration = 33333 // ~30fps

  const frame = generateSolidColorI420Frame(width, height, TestColors.green, timestamp, duration)

  t.is(frame.timestamp, timestamp)
  t.is(frame.duration, duration)

  frame.close()
})

test('VideoFrame: constructor with displayWidth/displayHeight', (t) => {
  const width = 320
  const height = 240
  const displayWidth = 640
  const displayHeight = 480

  const size = calculateI420Size(width, height)
  const buffer = new Uint8Array(size)

  const frame = new VideoFrame(buffer, {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
    displayWidth,
    displayHeight,
  })

  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)
  t.is(frame.displayWidth, displayWidth)
  t.is(frame.displayHeight, displayHeight)

  frame.close()
})

// ============================================================================
// Property Tests
// ============================================================================

test('VideoFrame: format property returns correct pixel format', (t) => {
  const i420Frame = generateSolidColorI420Frame(128, 96, TestColors.white, 0)
  t.is(i420Frame.format, 'I420')
  i420Frame.close()

  const rgbaFrame = generateSolidColorRGBAFrame(128, 96, TestColors.white, 0)
  t.is(rgbaFrame.format, 'RGBA')
  rgbaFrame.close()
})

test('VideoFrame: codedWidth and codedHeight are correct', (t) => {
  for (const [_, res] of Object.entries(TestResolutions)) {
    if (res.width > 1280) continue // Skip large resolutions for speed

    const frame = generateSolidColorI420Frame(res.width, res.height, TestColors.gray, 0)
    t.is(frame.codedWidth, res.width, `Width mismatch for ${res.width}x${res.height}`)
    t.is(frame.codedHeight, res.height, `Height mismatch for ${res.width}x${res.height}`)
    frame.close()
  }
})

test('VideoFrame: timestamp property', (t) => {
  const timestamps = [0, 1000, 33333, 1000000, 9007199254740991] // Including max safe integer

  for (const ts of timestamps) {
    const frame = generateSolidColorI420Frame(128, 96, TestColors.black, ts)
    t.is(frame.timestamp, ts, `Timestamp ${ts} not preserved`)
    frame.close()
  }
})

test('VideoFrame: duration property (optional)', (t) => {
  // Without duration
  const frame1 = generateSolidColorI420Frame(128, 96, TestColors.red, 0)
  t.is(frame1.duration, null)
  frame1.close()

  // With duration
  const frame2 = generateSolidColorI420Frame(128, 96, TestColors.red, 0, 33333)
  t.is(frame2.duration, 33333)
  frame2.close()
})

test('VideoFrame: colorSpace property exists', (t) => {
  const frame = generateSolidColorI420Frame(128, 96, TestColors.blue, 0)

  // colorSpace should be an object (may have undefined properties)
  t.is(typeof frame.colorSpace, 'object')
  t.truthy(frame.colorSpace)

  frame.close()
})

// ============================================================================
// Method Tests
// ============================================================================

test('VideoFrame: allocationSize() returns correct size for I420', (t) => {
  const testCases = [
    { width: 128, height: 96 },
    { width: 320, height: 240 },
    { width: 640, height: 480 },
  ]

  for (const { width, height } of testCases) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.red, 0)
    const expectedSize = calculateI420Size(width, height)
    t.is(frame.allocationSize(), expectedSize, `allocationSize mismatch for ${width}x${height}`)
    frame.close()
  }
})

test('VideoFrame: allocationSize() returns correct size for RGBA', (t) => {
  const testCases = [
    { width: 128, height: 96 },
    { width: 320, height: 240 },
  ]

  for (const { width, height } of testCases) {
    const frame = generateSolidColorRGBAFrame(width, height, TestColors.red, 0)
    const expectedSize = calculateRGBASize(width, height)
    t.is(frame.allocationSize(), expectedSize, `allocationSize mismatch for ${width}x${height}`)
    frame.close()
  }
})

test('VideoFrame: copyTo() extracts frame data', async (t) => {
  const width = 128
  const height = 96
  const frame = generateSolidColorI420Frame(width, height, TestColors.green, 0)

  const size = frame.allocationSize()
  const buffer = new Uint8Array(size)

  await frame.copyTo(buffer)

  // Buffer should be filled (not all zeros for a colored frame)
  let hasNonZero = false
  for (let i = 0; i < buffer.length; i++) {
    if (buffer[i] !== 0) {
      hasNonZero = true
      break
    }
  }
  t.true(hasNonZero, 'copyTo should extract non-zero data')

  frame.close()
})

test('VideoFrame: copyTo() preserves data round-trip', async (t) => {
  const width = 128
  const height = 96

  // Create source data
  const sourceSize = calculateI420Size(width, height)
  const sourceData = new Uint8Array(sourceSize)

  // Fill with a pattern
  for (let i = 0; i < sourceSize; i++) {
    sourceData[i] = i % 256
  }

  const frame = new VideoFrame(sourceData, {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  // Extract and compare
  const extractedData = new Uint8Array(sourceSize)
  await frame.copyTo(extractedData)

  for (let i = 0; i < sourceSize; i++) {
    t.is(extractedData[i], sourceData[i], `Data mismatch at index ${i}`)
  }

  frame.close()
})

test('VideoFrame: clone() creates independent copy', async (t) => {
  const frame = generateSolidColorI420Frame(128, 96, TestColors.yellow, 12345, 33333)

  const cloned = frame.clone()

  // Properties should match
  t.is(cloned.format, frame.format)
  t.is(cloned.codedWidth, frame.codedWidth)
  t.is(cloned.codedHeight, frame.codedHeight)
  t.is(cloned.timestamp, frame.timestamp)
  t.is(cloned.duration, frame.duration)

  // Close original - clone should still work
  frame.close()

  // Clone should still be accessible
  t.is(cloned.codedWidth, 128)
  t.is(cloned.codedHeight, 96)

  const size = cloned.allocationSize()
  const buffer = new Uint8Array(size)
  await t.notThrowsAsync(async () => cloned.copyTo(buffer))

  cloned.close()
})

test('VideoFrame: close() releases resources', (t) => {
  const frame = generateSolidColorI420Frame(128, 96, TestColors.red, 0)

  // Should not throw
  t.notThrows(() => frame.close())

  // Idempotent - calling close again should not throw
  t.notThrows(() => frame.close())
})

// ============================================================================
// Edge Case Tests
// ============================================================================

test('VideoFrame: minimum dimensions (2x2 for I420)', (t) => {
  const width = 2
  const height = 2
  const size = calculateI420Size(width, height) // 6 bytes
  const buffer = new Uint8Array(size)

  const frame = new VideoFrame(buffer, {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })

  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)

  frame.close()
})

test('VideoFrame: timestamp of 0 is valid', (t) => {
  const frame = generateSolidColorI420Frame(128, 96, TestColors.black, 0)
  t.is(frame.timestamp, 0)
  frame.close()
})

test('VideoFrame: large timestamp values', (t) => {
  // 1 hour in microseconds
  const oneHourUs = 3600 * 1000000
  const frame = generateSolidColorI420Frame(128, 96, TestColors.white, oneHourUs)
  t.is(frame.timestamp, oneHourUs)
  frame.close()
})

test('VideoFrame: gradient pattern creates varied data', async (t) => {
  const frame = generateGradientI420Frame(320, 240, 0)

  const size = frame.allocationSize()
  const buffer = new Uint8Array(size)
  await frame.copyTo(buffer)

  // Check that Y values vary (gradient)
  const uniqueValues = new Set<number>()
  for (let i = 0; i < 320 * 240; i++) {
    uniqueValues.add(buffer[i])
  }

  // Gradient should have many unique luma values
  t.true(uniqueValues.size > 100, 'Gradient should have varied Y values')

  frame.close()
})
