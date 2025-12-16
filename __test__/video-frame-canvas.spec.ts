/**
 * VideoFrame Canvas API Conformance Tests
 *
 * Tests VideoFrame creation from @napi-rs/canvas Canvas objects.
 *
 * Skipped on aarch64-windows where @napi-rs/canvas is not available.
 */

import test from 'ava'

import { VideoFrame, VideoEncoder, EncodedVideoChunk } from '../index.js'

// Skip all canvas tests on aarch64-windows (no @napi-rs/canvas support)
const isAarch64Windows = process.platform === 'win32' && process.arch === 'arm64'

const runTest = isAarch64Windows ? test.skip : test

// Dynamic import to avoid loading canvas on unsupported platforms
let createCanvas: typeof import('@napi-rs/canvas').createCanvas
if (!isAarch64Windows) {
  const canvas = await import('@napi-rs/canvas')
  createCanvas = canvas.createCanvas
}

// ============================================================================
// Constructor Tests - Canvas Source
// ============================================================================

runTest('VideoFrame: constructor from Canvas with valid timestamp', (t) => {
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  // Fill with red
  ctx.fillStyle = '#FF0000'
  ctx.fillRect(0, 0, 320, 240)

  const frame = new VideoFrame(canvas, { timestamp: 0 })

  t.is(frame.format, 'RGBA')
  t.is(frame.codedWidth, 320)
  t.is(frame.codedHeight, 240)
  t.is(frame.displayWidth, 320)
  t.is(frame.displayHeight, 240)
  t.is(frame.timestamp, 0)

  frame.close()
})

runTest('VideoFrame: Canvas requires timestamp in init', (t) => {
  const canvas = createCanvas(100, 100)

  // Missing init entirely
  t.throws(
    () => {
      new VideoFrame(canvas, undefined as never)
    },
    { message: /timestamp is required/ },
  )

  // Init without timestamp
  t.throws(
    () => {
      new VideoFrame(canvas, {} as never)
    },
    { message: /timestamp is required/ },
  )
})

// Note: @napi-rs/canvas clamps zero dimensions to default values (e.g., 0 -> 350),
// so we use mock canvas objects to test our validation code.

runTest('VideoFrame: Canvas with zero width throws', (t) => {
  // Mock canvas object with zero width
  const mockCanvas = {
    width: 0,
    height: 100,
    data: () => Buffer.alloc(0),
  }

  t.throws(
    () => {
      new VideoFrame(mockCanvas as never, { timestamp: 0 })
    },
    { message: /Canvas width must be greater than 0/ },
  )
})

runTest('VideoFrame: Canvas with zero height throws', (t) => {
  // Mock canvas object with zero height
  const mockCanvas = {
    width: 100,
    height: 0,
    data: () => Buffer.alloc(0),
  }

  t.throws(
    () => {
      new VideoFrame(mockCanvas as never, { timestamp: 0 })
    },
    { message: /Canvas height must be greater than 0/ },
  )
})

runTest('VideoFrame: Canvas with zero width and height throws', (t) => {
  // Mock canvas object with both zero dimensions
  const mockCanvas = {
    width: 0,
    height: 0,
    data: () => Buffer.alloc(0),
  }

  // Should throw for width first (checked before height)
  t.throws(
    () => {
      new VideoFrame(mockCanvas as never, { timestamp: 0 })
    },
    { message: /Canvas width must be greater than 0/ },
  )
})

runTest('VideoFrame: Canvas with all init options', (t) => {
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  // Draw something
  ctx.fillStyle = '#0000FF'
  ctx.fillRect(0, 0, 320, 240)

  const frame = new VideoFrame(canvas, {
    timestamp: 1000,
    duration: 33333,
    displayWidth: 640,
    displayHeight: 480,
  })

  t.is(frame.format, 'RGBA')
  t.is(frame.codedWidth, 320)
  t.is(frame.codedHeight, 240)
  t.is(frame.timestamp, 1000)
  t.is(frame.duration, 33333)
  t.is(frame.displayWidth, 640)
  t.is(frame.displayHeight, 480)

  frame.close()
})

runTest('VideoFrame: Canvas with visibleRect', (t) => {
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  ctx.fillStyle = '#00FF00'
  ctx.fillRect(0, 0, 320, 240)

  const frame = new VideoFrame(canvas, {
    timestamp: 0,
    visibleRect: { x: 10, y: 10, width: 300, height: 220 },
  })

  t.is(frame.codedWidth, 320)
  t.is(frame.codedHeight, 240)

  const visibleRect = frame.visibleRect
  t.is(visibleRect.x, 10)
  t.is(visibleRect.y, 10)
  t.is(visibleRect.width, 300)
  t.is(visibleRect.height, 220)

  frame.close()
})

runTest('VideoFrame: Canvas pixel data is RGBA', async (t) => {
  const canvas = createCanvas(2, 2)
  const ctx = canvas.getContext('2d')

  // Fill with red (RGBA: 255, 0, 0, 255)
  ctx.fillStyle = '#FF0000'
  ctx.fillRect(0, 0, 2, 2)

  const frame = new VideoFrame(canvas, { timestamp: 0 })

  // Copy the data to verify format
  const buffer = new Uint8Array(2 * 2 * 4) // RGBA = 4 bytes per pixel
  await frame.copyTo(buffer)

  // Check first pixel is red (RGBA)
  t.is(buffer[0], 255, 'R should be 255')
  t.is(buffer[1], 0, 'G should be 0')
  t.is(buffer[2], 0, 'B should be 0')
  t.is(buffer[3], 255, 'A should be 255')

  frame.close()
})

runTest('VideoFrame: Canvas with drawn graphics', async (t) => {
  const canvas = createCanvas(100, 100)
  const ctx = canvas.getContext('2d')

  // Fill with blue background
  ctx.fillStyle = '#0000FF'
  ctx.fillRect(0, 0, 100, 100)

  // Draw a white line
  ctx.strokeStyle = '#FFFFFF'
  ctx.lineWidth = 2
  ctx.beginPath()
  ctx.moveTo(0, 0)
  ctx.lineTo(100, 100)
  ctx.stroke()

  const frame = new VideoFrame(canvas, { timestamp: 5000 })

  t.is(frame.format, 'RGBA')
  t.is(frame.codedWidth, 100)
  t.is(frame.codedHeight, 100)
  t.is(frame.timestamp, 5000)

  frame.close()
})

runTest('VideoFrame: Canvas various sizes', (t) => {
  const sizes = [
    [1, 1],
    [16, 16],
    [640, 480],
    [1920, 1080],
  ] as const

  for (const [width, height] of sizes) {
    const canvas = createCanvas(width, height)

    const frame = new VideoFrame(canvas, { timestamp: 0 })

    t.is(frame.codedWidth, width, `Width should be ${width}`)
    t.is(frame.codedHeight, height, `Height should be ${height}`)
    t.is(frame.format, 'RGBA')

    frame.close()
  }
})

runTest('VideoFrame: Canvas sRGB color space default', (t) => {
  const canvas = createCanvas(100, 100)

  const frame = new VideoFrame(canvas, { timestamp: 0 })

  // Per W3C spec, RGBA defaults to sRGB color space
  const colorSpace = frame.colorSpace
  t.is(colorSpace.primaries, 'bt709')
  t.is(colorSpace.transfer, 'iec61966-2-1') // sRGB
  t.is(colorSpace.fullRange, true)

  frame.close()
})

runTest('VideoFrame: Canvas clone preserves data', async (t) => {
  const canvas = createCanvas(10, 10)
  const ctx = canvas.getContext('2d')

  // Fill with green
  ctx.fillStyle = '#00FF00'
  ctx.fillRect(0, 0, 10, 10)

  const frame1 = new VideoFrame(canvas, { timestamp: 100 })
  const frame2 = new VideoFrame(frame1, { timestamp: 200 })

  t.is(frame1.timestamp, 100)
  t.is(frame2.timestamp, 200)
  t.is(frame1.format, frame2.format)
  t.is(frame1.codedWidth, frame2.codedWidth)
  t.is(frame1.codedHeight, frame2.codedHeight)

  // Verify pixel data is the same
  const buffer1 = new Uint8Array(10 * 10 * 4)
  const buffer2 = new Uint8Array(10 * 10 * 4)
  await frame1.copyTo(buffer1)
  await frame2.copyTo(buffer2)

  t.deepEqual(buffer1, buffer2, 'Cloned frame should have same pixel data')

  frame1.close()
  frame2.close()
})

// ============================================================================
// Integration Tests
// ============================================================================

runTest('VideoFrame: Canvas can be encoded (smoke test)', async (t) => {
  // This is a basic smoke test to verify Canvas frames work with encoders
  // Full encoder tests are in other test files
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  ctx.fillStyle = '#FF0000'
  ctx.fillRect(0, 0, 320, 240)

  const chunks: InstanceType<typeof EncodedVideoChunk>[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Create frame from canvas and encode
  const frame = new VideoFrame(canvas, { timestamp: 0 })
  encoder.encode(frame)
  frame.close()

  await encoder.flush()
  encoder.close()

  t.true(chunks.length > 0, 'Should have encoded at least one chunk')
  t.is(chunks[0]!.type, 'key', 'First chunk should be keyframe')
})
