/**
 * ImageDecoder API Conformance Tests
 *
 * Tests WebCodecs ImageDecoder specification compliance.
 * Tests frame_index support for animated images (GIF, WebP).
 */

import test from 'ava'
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

import { ImageDecoder, type ImageDecoderInit } from '../index.js'
import { bmp, gif, type GifFrame } from './helpers/image-files.js'

const __dirname = dirname(fileURLToPath(import.meta.url))

// ============================================================================
// Static Image Tests (PNG)
// ============================================================================

test('ImageDecoder decodes static PNG image', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  t.is(decoder.type, 'image/png')
  t.true(decoder.complete)

  const result = await decoder.decode()

  t.true(result.complete)
  t.truthy(result.image)
  t.is(result.image.codedWidth, 8)
  t.is(result.image.codedHeight, 8)
  t.is(result.image.duration, null)

  // Static images should have frameCount = 1
  const tracks = decoder.tracks
  t.is(tracks.length, 1)
  t.false(tracks.selectedTrack!.animated)
  t.is(tracks.selectedTrack!.frameCount, 1)

  result.image.close()
  decoder.close()
})

test('ImageDecoder PNG default frame_index is 0', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // Decode without options (default frame_index = 0)
  const result1 = await decoder.decode()
  t.true(result1.complete)

  // Decode with explicit frame_index = 0
  const result2 = await decoder.decode({ frameIndex: 0 })
  t.true(result2.complete)

  result1.image.close()
  result2.image.close()
  decoder.close()
})

test('ImageDecoder PNG throws for out-of-bounds frame_index', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // First decode to populate frame count
  const result = await decoder.decode()
  result.image.close()

  // Now try to access invalid frame index
  const error = await t.throwsAsync(() => decoder.decode({ frameIndex: 999 }), {
    message: /out of bounds/,
  })
  t.true(error instanceof RangeError)

  decoder.close()
})

test('ImageDecoder MIME mismatch rejects with EncodingError', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/jpeg' })
  const error = await t.throwsAsync(decoder.decode())

  t.true(error instanceof DOMException)
  t.is(error.name, 'EncodingError')
  decoder.close()
})

// ============================================================================
// Animated Image Tests (GIF)
// ============================================================================

test('ImageDecoder detects animated GIF format', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  t.is(decoder.type, 'image/gif')
  t.true(decoder.complete)

  // animated is set synchronously in constructor
  const tracksBefore = decoder.tracks
  t.true(tracksBefore.selectedTrack!.animated)
  // Note: frameCount may be 0 or populated depending on whether
  // background pre-parsing has completed. Use tracks.ready to
  // reliably get the frame count (see test below).

  decoder.close()
})

test('ImageDecoder GIF frame_count is populated after first decode', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Decode first frame
  const result = await decoder.decode()
  t.true(result.complete)
  result.image.close()

  // After decode, frameCount should be populated
  const tracks = decoder.tracks
  t.true(tracks.selectedTrack!.animated)
  t.true(tracks.selectedTrack!.frameCount > 1)

  const first = await decoder.decode({ frameIndex: 0 })
  const second = await decoder.decode({ frameIndex: 1 })
  t.is(first.image.timestamp, 0)
  t.is(second.image.timestamp, 1_000_000)
  t.is(first.image.duration, 1_000_000)
  t.is(second.image.duration, 1_000_000)
  first.image.close()
  second.image.close()

  decoder.close()
})

test('ImageDecoder parses finite GIF repetition count', async (t) => {
  const data = Buffer.from(readFileSync(join(__dirname, 'fixtures/animated.gif')))
  // NETSCAPE2.0 application extension loop-count bytes in this fixture.
  data[35] = 5
  data[36] = 0
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  await decoder.tracks.ready
  t.is(decoder.tracks.selectedTrack!.repetitionCount, 5)
  decoder.close()
})

test('ImageDecoder GIF can decode specific frames by index', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Decode first frame (index 0)
  const result0 = await decoder.decode({ frameIndex: 0 })
  t.true(result0.complete)
  t.truthy(result0.image)

  // Get frame count
  // Note: FFmpeg's single-packet image decoder may not extract all animation frames.
  // For full animation support, use a video decoder approach instead.
  const frameCount = decoder.tracks.selectedTrack!.frameCount
  t.true(frameCount >= 1, `Expected at least 1 frame, got ${frameCount}`)

  // Decode subsequent frames if available
  for (let i = 1; i < frameCount; i++) {
    const result = await decoder.decode({ frameIndex: i })
    t.true(result.complete)
    t.truthy(result.image)
    result.image.close()
  }

  result0.image.close()
  decoder.close()
})

test('ImageDecoder GIF throws for out-of-bounds frame_index', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // First decode to populate frame count
  const result = await decoder.decode()
  result.image.close()

  const frameCount = decoder.tracks.selectedTrack!.frameCount

  // Try to access one beyond the last frame
  await t.throwsAsync(() => decoder.decode({ frameIndex: frameCount }), {
    message: /out of bounds/,
  })

  decoder.close()
})

// ============================================================================
// Reset and Close Tests
// ============================================================================

test('ImageDecoder reset() clears cache and frame_count', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Decode to populate cache
  const result1 = await decoder.decode()
  result1.image.close()
  const frameCountAfterDecode = decoder.tracks.selectedTrack!.frameCount
  t.true(frameCountAfterDecode > 0)

  // Reset
  decoder.reset()

  // Frame count should be reset to 0 for animated formats
  t.is(decoder.tracks.selectedTrack!.frameCount, 0)

  // Should be able to decode again
  const result2 = await decoder.decode()
  t.true(result2.complete)
  result2.image.close()

  // Frame count should be repopulated
  t.is(decoder.tracks.selectedTrack!.frameCount, frameCountAfterDecode)

  decoder.close()
})

test('ImageDecoder close() prevents further decodes', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  decoder.close()

  await t.throwsAsync(() => decoder.decode(), {
    message: /closed/,
  })
})

test('ImageDecoder reset() after close() throws', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  decoder.close()

  try {
    decoder.reset()
    t.fail('reset should throw InvalidStateError')
  } catch (error) {
    t.true(error instanceof DOMException, 'reset error should be DOMException')
    t.is((error as DOMException).name, 'InvalidStateError')
  }
})

// ============================================================================
// MIME Type Support Tests
// ============================================================================

test('ImageDecoder.isTypeSupported returns true for supported types', async (t) => {
  t.true(await ImageDecoder.isTypeSupported('image/png'))
  t.true(await ImageDecoder.isTypeSupported('image/jpeg'))
  t.true(await ImageDecoder.isTypeSupported('image/gif'))
  t.true(await ImageDecoder.isTypeSupported('image/webp'))
  t.true(await ImageDecoder.isTypeSupported('image/bmp'))
  t.true(await ImageDecoder.isTypeSupported('image/jxl'))
})

test('ImageDecoder decodes JPEG XL image data', async (t) => {
  // libjxl/testdata jxl/spline_on_first_frame.jxl at commit 73695d3.
  // The tiny upstream conformance sample is BSD-3-Clause licensed.
  const data = Buffer.from(
    '/wpHQCTYYyAAAHQAAuAoKipGxmAQgF8AAEToz4Qk4ox2AughlkBwAADYY1gAAHAAAuAgKipGxkgAfgEAEKFfEhIJRFhA4YklEBxgCA==',
    'base64',
  )
  const decoder = new ImageDecoder({ data, type: 'image/jxl' })
  const result = await decoder.decode()

  t.is(result.image.codedWidth, 32)
  t.is(result.image.codedHeight, 32)
  // libjxl decodes this sample to rgb24; Chrome 155 reports it as BGRX
  t.is(result.image.format, 'BGRX')
  t.is(result.image.timestamp, 0)
  t.true(result.complete)

  result.image.close()
  decoder.close()
})

test('ImageDecoder.isTypeSupported returns false for unsupported types', async (t) => {
  t.false(await ImageDecoder.isTypeSupported('image/unknown'))
  t.false(await ImageDecoder.isTypeSupported('video/mp4'))
  t.false(await ImageDecoder.isTypeSupported(''))
})

// ============================================================================
// Frame Caching Tests
// ============================================================================

test('ImageDecoder caches frames after first decode', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // First decode creates cache
  const result1 = await decoder.decode()
  t.true(result1.complete)

  // Second decode should use cache (same result)
  const result2 = await decoder.decode()
  t.true(result2.complete)

  // Both should have valid dimensions
  t.is(result1.image.codedWidth, result2.image.codedWidth)
  t.is(result1.image.codedHeight, result2.image.codedHeight)

  result1.image.close()
  result2.image.close()
  decoder.close()
})

test('ImageDecoder completed getter resolves immediately for buffered data', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // completed should resolve immediately since data is buffered
  await t.notThrowsAsync(async () => {
    await decoder.completed
  })

  decoder.close()
  // like Chrome, also once the decoder is closed
  await t.notThrowsAsync(decoder.completed)
})

test('ImageDecoder stream becomes ready before EOF once a frame is decodable', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  let closeStream!: () => void
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(data)
      closeStream = () => controller.close()
    },
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/png' })

  await decoder.tracks.ready
  const result = await decoder.decode()
  t.false(decoder.complete)
  t.is(result.image.codedWidth, 8)

  closeStream()
  await decoder.completed
  t.true(decoder.complete)
  result.image.close()
  decoder.close()
})

test('ImageDecoder streamed animation waits for a requested future frame', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(streamController) {
      controller = streamController
      // This prefix contains exactly the first of the fixture's three frames.
      controller.enqueue(data.subarray(0, 900))
    },
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/gif' })

  await decoder.tracks.ready
  t.false(decoder.complete)
  t.is(decoder.tracks.selectedTrack!.frameCount, 1)

  let settled = 0
  const secondFramePromises = [
    decoder.decode({ frameIndex: 1 }).finally(() => {
      settled += 1
    }),
    decoder.decode({ frameIndex: 1 }).finally(() => {
      settled += 1
    }),
  ]
  await new Promise((resolve) => setTimeout(resolve, 25))
  t.is(settled, 0, 'decodes should wait while the requested frame may still arrive')

  controller.enqueue(data.subarray(900))
  controller.close()
  const secondFrames = await Promise.all(secondFramePromises)
  t.is(secondFrames[0].image.timestamp, 1_000_000)
  t.is(secondFrames[1].image.timestamp, 1_000_000)
  t.true(secondFrames[0].complete)
  t.true(secondFrames[1].complete)
  t.is(decoder.tracks.selectedTrack!.frameCount, 3)

  secondFrames[0].image.close()
  secondFrames[1].image.close()
  decoder.close()
})

test('ImageDecoder pending streamed decode rejects after a stream error', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(streamController) {
      controller = streamController
      controller.enqueue(data.subarray(0, 900))
    },
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/gif' })

  await decoder.tracks.ready
  t.false(decoder.complete)
  t.is(decoder.tracks.selectedTrack!.frameCount, 1)

  const completedOutcome = decoder.completed.then(
    () => ({ status: 'resolved' as const }),
    (error: unknown) => ({ status: 'rejected' as const, error }),
  )
  const decodeOutcomePromise = decoder.decode({ frameIndex: 1 }).then(
    (result) => {
      result.image.close()
      return { status: 'resolved' as const }
    },
    (error: unknown) => ({ status: 'rejected' as const, error }),
  )

  controller.error(new Error('intentional stream failure'))

  const decodeOutcome = await Promise.race([
    decodeOutcomePromise,
    new Promise<{ status: 'timeout' }>((resolve) =>
      setTimeout(() => resolve({ status: 'timeout' }), 500),
    ),
  ])
  const completed = await completedOutcome

  t.true(decoder.complete)
  t.is(completed.status, 'rejected')
  if (completed.status === 'rejected') {
    t.is((completed.error as Error).message, 'intentional stream failure')
  }
  t.is(decodeOutcome.status, 'rejected', 'decode must not remain pending after a stream error')
  if (decodeOutcome.status === 'rejected') {
    t.true(decodeOutcome.error instanceof RangeError)
  }

  decoder.close()
})

test('ImageDecoder tracks.ready resolves after metadata is parsed', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Wait for tracks.ready (metadata parsing)
  await decoder.tracks.ready

  // After ready, frameCount should be populated for animated GIF
  const track = decoder.tracks.selectedTrack!
  t.true(track.animated)
  t.true(track.frameCount > 0, `Expected frameCount > 0 after ready, got ${track.frameCount}`)

  decoder.close()
})

test('ImageDecoder tracks.ready resolves for static images', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // Wait for tracks.ready
  await decoder.tracks.ready

  // Static images should have frameCount = 1
  const track = decoder.tracks.selectedTrack!
  t.false(track.animated)
  t.is(track.frameCount, 1)

  decoder.close()
})

// ============================================================================
// GIF and BMP: timing, errors and streams (Chrome 155 values)
// ============================================================================

const PALETTE = [
  [255, 0, 0],
  [0, 255, 0],
  [0, 0, 255],
  [255, 255, 255],
]
const fullFrame = (frame: Partial<GifFrame> = {}): GifFrame => ({
  x: 0,
  y: 0,
  width: 4,
  height: 2,
  indices: [0, 1, 2, 3, 3, 2, 1, 0],
  ...frame,
})
const partFrame = (frame: Partial<GifFrame> = {}): GifFrame => ({ x: 1, y: 0, width: 2, height: 1, indices: [3, 3], ...frame })
// a clear code, then a code past the table
const BAD_CODES = [4, 7]
const TWO_FRAMES = gif(4, 2, PALETTE, [fullFrame({ delay: 10 }), partFrame({ delay: 10, indices: [1, 2] })])
const ONE_FRAME = gif(4, 2, PALETTE, [fullFrame({ delay: 10 })])
const BMP_3x2 = bmp(3, 2, 24, [
  [30, 20, 10, 60, 50, 40, 90, 80, 70],
  [3, 2, 1, 6, 5, 4, 9, 8, 7],
])
const unexpectedEnd = (frameIndex: number) => ({
  instanceOf: RangeError,
  message: `Unexpected end of image. Request for frame index ${frameIndex} can't be satisfied.`,
})
const failedFrame = (frameIndex: number) => ({
  instanceOf: DOMException,
  name: 'EncodingError',
  message: `Failed to decode frame at index ${frameIndex}`,
})

test('ImageDecoder: GIF frames last their delay and start when the frames before them end, like Chrome', async (t) => {
  const data = gif(4, 2, PALETTE, [fullFrame({ delay: 0 }), partFrame({ delay: 5 }), partFrame({ delay: 100, indices: [0, 0] })])
  const decoder = new ImageDecoder({ data, type: 'image/gif' })
  for (const [frameIndex, [timestamp, duration]] of [
    [0, 0],
    [0, 50_000],
    [50_000, 1_000_000],
  ].entries()) {
    const { image } = await decoder.decode({ frameIndex })
    t.is(image.timestamp, timestamp, `frame ${frameIndex}`)
    t.is(image.duration, duration, `frame ${frameIndex}`)
    image.close()
  }
  decoder.close()

  // a GIF of one frame has a duration too
  for (const [frame, duration] of [
    [fullFrame({ delay: 50 }), 500_000],
    [fullFrame(), 0],
  ] as const) {
    const single = new ImageDecoder({ data: gif(4, 2, PALETTE, [frame]), type: 'image/gif' })
    const { image } = await single.decode()
    t.is(image.duration, duration)
    image.close()
    single.close()
  }
})

test('ImageDecoder: a GIF frame with broken data, and the frames drawn on it, reject with RangeError, like Chrome', async (t) => {
  const data = gif(4, 2, PALETTE, [
    fullFrame({ delay: 10 }),
    partFrame({ delay: 10, codes: BAD_CODES }),
    // drawn on frame 1
    partFrame({ delay: 10, x: 0, indices: [1, 1] }),
  ])
  const decoder = new ImageDecoder({ data, type: 'image/gif' })
  await t.throwsAsync(decoder.decode({ frameIndex: 1 }), unexpectedEnd(1))
  await t.throwsAsync(decoder.decode({ frameIndex: 2 }), unexpectedEnd(2))
  const { image } = await decoder.decode({ frameIndex: 0 })
  t.is(image.format, 'BGRX')
  image.close()
  await t.throwsAsync(decoder.decode({ frameIndex: 1 }), unexpectedEnd(1))
  t.false(decoder.closed)
  decoder.close()
})

test('ImageDecoder: once every GIF frame failed, decode() rejects with EncodingError and the decoder closes, like Chrome', async (t) => {
  const data = gif(4, 2, PALETTE, [fullFrame({ delay: 10, codes: BAD_CODES }), partFrame({ delay: 10 })])
  const decoder = new ImageDecoder({ data, type: 'image/gif' })
  await t.throwsAsync(decoder.decode({ frameIndex: 0 }), unexpectedEnd(0))
  // frame 1 is drawn on frame 0
  await t.throwsAsync(decoder.decode({ frameIndex: 1 }), failedFrame(1))
  t.true(decoder.closed)
  await t.throwsAsync(decoder.decode({ frameIndex: 0 }), { instanceOf: DOMException, name: 'InvalidStateError' })
  // the data was complete
  await t.notThrowsAsync(decoder.completed)

  // a decode() made before the frame failed rejects the same way
  const single = new ImageDecoder({ data: gif(4, 2, PALETTE, [fullFrame({ codes: BAD_CODES })]), type: 'image/gif' })
  await t.throwsAsync(single.decode(), failedFrame(0))
  t.true(single.closed)
})

test('ImageDecoder: decode() calls are handled in call order, like Chrome', async (t) => {
  // every frame fails: what a call finds depends on the frames that the calls before it failed
  const data = gif(4, 2, PALETTE, [0, 1, 2].map(() => fullFrame({ delay: 10, codes: BAD_CODES })))
  for (let run = 0; run < 20; run++) {
    const decoder = new ImageDecoder({ data, type: 'image/gif' })
    await decoder.tracks.ready
    const settled: number[] = []
    const results = await Promise.allSettled(
      [5, 0, 2, 1, 5].map((frameIndex, call) => decoder.decode({ frameIndex }).finally(() => settled.push(call))),
    )
    const errors = results.map((result) =>
      result.status === 'rejected' ? `${(result.reason as Error).name}: ${(result.reason as Error).message}` : 'ok',
    )
    // frame 5 is out of range
    t.regex(errors[0], /^RangeError: /)
    t.deepEqual(errors.slice(1), [
      "RangeError: Unexpected end of image. Request for frame index 0 can't be satisfied.",
      "RangeError: Unexpected end of image. Request for frame index 2 can't be satisfied.",
      'EncodingError: Failed to decode frame at index 1',
      'EncodingError: Failed to decode frame at index 5',
    ])
    t.deepEqual(settled, [0, 1, 2, 3, 4])
    t.true(decoder.closed)
  }
})

test('ImageDecoder: a streamed decode() waits for the decode() calls before it, like Chrome', async (t) => {
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(streamController) {
      controller = streamController
      // frame 0, and the rect of frame 1
      controller.enqueue(TWO_FRAMES.slice(0, 68))
    },
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/gif' })
  await decoder.tracks.ready
  const settled: number[] = []
  const decodes = [1, 0].map((frameIndex) => decoder.decode({ frameIndex }).finally(() => settled.push(frameIndex)))
  await new Promise((resolve) => setTimeout(resolve, 25))
  // frame 0 is in, but its decode() waits for the one of frame 1
  t.deepEqual(settled, [])

  controller.enqueue(TWO_FRAMES.slice(68))
  controller.close()
  const results = await Promise.all(decodes)
  t.deepEqual(settled, [1, 0])
  t.deepEqual(
    results.map(({ image }) => image.timestamp),
    [100_000, 0],
  )
  results.forEach(({ image }) => image.close())
  decoder.close()
})

// Chrome's close() rejects the decode() calls that wait at once, so a decode() made after it
// settles last.
test('ImageDecoder: a decode() made after close() settles after the calls made before it, like Chrome', async (t) => {
  const jpeg = new Uint8Array(readFileSync(join(__dirname, 'fixtures/jpeg/420-37x9.jpg')))
  const animated = new Uint8Array(readFileSync(join(__dirname, 'fixtures/animated.gif')))
  // the JPEG data is cut inside the scan, and the GIF has no frame 50: the calls wait
  for (const [data, type, cut, frameIndex] of [
    [jpeg, 'image/jpeg', Math.floor(jpeg.length * 0.8), 0],
    [animated, 'image/gif', animated.length, 50],
  ] as const) {
    for (let run = 0; run < 10; run++) {
      let controller!: ReadableStreamDefaultController<Uint8Array>
      const stream = new ReadableStream<Uint8Array>({
        start(streamController) {
          controller = streamController
          controller.enqueue(data.slice(0, cut))
        },
      })
      const decoder = new ImageDecoder({ data: stream, type })
      // the open stream would keep the worker alive
      t.teardown(() => {
        try {
          controller.close()
        } catch {}
      })
      await decoder.tracks.ready
      const settled: number[] = []
      const decodes = [0, 1, 2].map((call) => decoder.decode({ frameIndex }).finally(() => settled.push(call)))
      await new Promise((resolve) => setTimeout(resolve, 10))
      decoder.close()
      decodes.push(decoder.decode({ frameIndex }).finally(() => settled.push(3)))
      const results = await Promise.allSettled(decodes)
      t.deepEqual(settled, [0, 1, 2, 3], type)
      t.true(
        results.every((result) => result.status === 'rejected'),
        type,
      )
      t.is(((results[3] as PromiseRejectedResult).reason as DOMException).name, 'InvalidStateError', type)
    }
  }
})

// Chrome rejects the decode() calls that wait at reset(). So the decode() made after it does not
// wait for them, though the calls are handled in call order.
test('ImageDecoder: reset() rejects a waiting GIF decode() with AbortError, and the next decode() goes on, like Chrome', async (t) => {
  // the fixture's frames are bytes 0..901, 902..1765 and 1766..2629
  const data = new Uint8Array(readFileSync(join(__dirname, 'fixtures/animated.gif')))
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(streamController) {
      controller = streamController
      controller.enqueue(data.slice(0, 902))
    },
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/gif' })
  // also when a check fails: a pending call or the open stream would keep the worker alive
  t.teardown(() => {
    decoder.close()
    try {
      controller.close()
    } catch {}
  })
  await decoder.tracks.ready
  const settled: number[] = []
  const waiting = decoder.decode({ frameIndex: 2 }).finally(() => settled.push(2))
  waiting.catch(() => {})
  await new Promise((resolve) => setTimeout(resolve, 50))
  t.deepEqual(settled, [], 'decode(2) waits for frame 2')

  decoder.reset()
  const next = decoder.decode({ frameIndex: 0 }).finally(() => settled.push(0))
  next.catch(() => {})
  // more data, but frame 2 is still not whole
  controller.enqueue(data.slice(902, 1810))
  // fails at once if a call stays pending (the stream stays open)
  const within = <T>(promise: Promise<T>) => {
    let timer: ReturnType<typeof setTimeout> | undefined
    const late = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error('still pending after 5 s')), 5000)
    })
    return Promise.race([promise, late]).finally(() => clearTimeout(timer))
  }
  await t.throwsAsync(within(waiting), { instanceOf: DOMException, name: 'AbortError', message: 'Aborted by reset.' })
  const { image } = await within(next)
  t.is(image.timestamp, 0)
  t.is(image.duration, 1_000_000)
  t.deepEqual(settled, [2, 0])
  t.false(decoder.closed)
  image.close()
})

test('ImageDecoder: a stream error ends the GIF or BMP data, like Chrome', async (t) => {
  const streamed = (data: Uint8Array, cut: number, type: string) => {
    let controller!: ReadableStreamDefaultController<Uint8Array>
    const stream = new ReadableStream<Uint8Array>({
      start(streamController) {
        controller = streamController
        controller.enqueue(data.slice(0, cut))
      },
    })
    return { decoder: new ImageDecoder({ data: stream, type }), controller }
  }
  // the data of the only frame ends early: it fails, so the decoder fails
  for (const [data, type] of [
    [ONE_FRAME, 'image/gif'],
    [BMP_3x2, 'image/bmp'],
  ] as const) {
    const { decoder, controller } = streamed(data, data.length - 3, type)
    await decoder.tracks.ready
    const pending = decoder.decode()
    await new Promise((resolve) => setTimeout(resolve, 25))
    controller.error(new Error('stream broke'))
    await t.throwsAsync(pending, failedFrame(0))
    t.true(decoder.closed, type)
  }
  // the header ends early: the metadata fails
  for (const [data, cut, type] of [
    [TWO_FRAMES, 15, 'image/gif'],
    [BMP_3x2, 30, 'image/bmp'],
  ] as const) {
    const { decoder, controller } = streamed(data, cut, type)
    const pending = decoder.decode()
    await new Promise((resolve) => setTimeout(resolve, 25))
    controller.error(new Error('stream broke'))
    await t.throwsAsync(decoder.tracks.ready, {
      instanceOf: DOMException,
      name: 'InvalidStateError',
      message: 'Failed to retrieve track metadata.',
    })
    await t.throwsAsync(pending, failedFrame(0))
    t.true(decoder.closed, type)
  }
})

test('ImageDecoder: GIF repetitionCount comes from the last loop extension, like Chrome', async (t) => {
  const netscape = (count: number) => [0x21, 0xff, 0x0b, ...Buffer.from('NETSCAPE2.0'), 3, 1, count & 0xff, count >> 8, 0]
  // after the header and the global palette
  const withExtensions = (data: Uint8Array, extensions: number[]) =>
    Uint8Array.from([...data.subarray(0, 25), ...extensions, ...data.subarray(25)])
  for (const [data, repetitionCount] of [
    [TWO_FRAMES, 0],
    // a still GIF loops forever
    [ONE_FRAME, Infinity],
    [withExtensions(TWO_FRAMES, [...netscape(3), ...netscape(7)]), 7],
    [withExtensions(TWO_FRAMES, [...netscape(0), ...netscape(3)]), 3],
    // inside a comment
    [withExtensions(TWO_FRAMES, [0x21, 0xfe, 19, ...netscape(5), 0]), 0],
  ] as const) {
    const decoder = new ImageDecoder({ data, type: 'image/gif' })
    await decoder.tracks.ready
    t.is(decoder.tracks.selectedTrack!.repetitionCount, repetitionCount)
    decoder.close()
  }

  // an empty sub-block after the NETSCAPE2.0 id does not end the extension
  const padded = withExtensions(ONE_FRAME, [0x21, 0xff, 0x0b, ...Buffer.from('NETSCAPE2.0'), 0, 0])
  const decoder = new ImageDecoder({ data: padded, type: 'image/gif' })
  const { image } = await decoder.decode()
  t.is(decoder.tracks.selectedTrack!.frameCount, 1)
  image.close()
  decoder.close()
})

test('ImageDecoder: the truncated last frame of a GIF rejects with RangeError, like Chrome', async (t) => {
  const decoder = new ImageDecoder({ data: TWO_FRAMES.subarray(0, TWO_FRAMES.length - 4), type: 'image/gif' })
  await decoder.tracks.ready
  t.is(decoder.tracks.selectedTrack!.frameCount, 2)
  const { image } = await decoder.decode({ frameIndex: 0 })
  image.close()
  await t.throwsAsync(decoder.decode({ frameIndex: 1 }), unexpectedEnd(1))
  t.false(decoder.closed)
  decoder.close()
})

test('ImageDecoder: a GIF or BMP that Chrome cannot open rejects tracks.ready and the pending decodes', async (t) => {
  const badMagic = Uint8Array.from([...Buffer.from('GIF88a'), ...TWO_FRAMES.subarray(6)])
  for (const [data, type] of [
    [badMagic, 'image/gif'],
    [BMP_3x2.subarray(0, 20), 'image/bmp'],
  ] as const) {
    const decoder = new ImageDecoder({ data, type })
    const pending = decoder.decode()
    await t.throwsAsync(decoder.tracks.ready, {
      instanceOf: DOMException,
      name: 'InvalidStateError',
      message: 'Failed to retrieve track metadata.',
    })
    await t.throwsAsync(pending, failedFrame(0))
    t.true(decoder.closed, type)
  }
})

test('ImageDecoder: a BMP with missing pixel data rejects with EncodingError and closes, like Chrome', async (t) => {
  const decoder = new ImageDecoder({ data: BMP_3x2.subarray(0, BMP_3x2.length - 3), type: 'image/bmp' })
  await decoder.tracks.ready
  t.is(decoder.tracks.selectedTrack!.frameCount, 1)
  await t.throwsAsync(decoder.decode(), failedFrame(0))
  t.true(decoder.closed)
})

/**
 * Decodes each file from a buffer and from a stream in a child process, for its peak RSS: one
 * "<source>: ready <result> | decode() <result>" line per decode.
 */
function decodeInChild(files: [Uint8Array, string][]): { results: string[]; maxRSS: number } {
  const script = `
const fs = require('node:fs');
const { ImageDecoder } = require('./index.js');
const fmt = (e) => e.name + ': ' + e.message;
(async () => {
  const results = [];
  for (const arg of process.argv.slice(1)) {
    const [type, b64] = arg.split(',');
    for (const source of ['buffer', 'stream']) {
      const bytes = new Uint8Array(Buffer.from(b64, 'base64'));
      const data = source === 'stream' ? new ReadableStream({ start(c) { c.enqueue(bytes); c.close(); } }) : bytes;
      const decoder = new ImageDecoder({ data, type });
      const ready = await decoder.tracks.ready.then(() => 'resolves', fmt);
      const decoded = await decoder.decode().then(() => 'resolves', fmt);
      results.push(source + ': ready ' + ready + ' | decode() ' + decoded);
    }
  }
  // on a line of its own: with WEBCODECS_LOG set, FFmpeg's log lines go to stdout too
  fs.writeSync(1, '\\nRESULT ' + JSON.stringify({ results, maxRSS: process.resourceUsage().maxRSS * 1024 }) + '\\n');
  process.exit(0);
})();`
  const args = files.map(([data, type]) => `${type},${Buffer.from(data).toString('base64')}`)
  const stdout = execFileSync(process.execPath, ['-e', script, ...args], {
    cwd: process.cwd(),
    timeout: 60_000,
    encoding: 'utf8',
  })
  const line = stdout.split('\n').find((l) => l.startsWith('RESULT '))!
  return JSON.parse(line.slice('RESULT '.length)) as { results: string[]; maxRSS: number }
}

test('ImageDecoder: an RLE BMP whose data ends early fails before its canvas is allocated', (t) => {
  // RLE8, 65535 x 8191 (4 * w * h just under 2^31, Blink's limit) with no pixel data, 32 moves of
  // 255 rows down, or 8190 ends of row, then the end of the data. Like Chrome 155, tracks.ready
  // resolves and decode() rejects. Each one made 2 GiB of output before its data ran out.
  const rle8 = (raw: number[]) =>
    bmp(65535, 8191, 8, [], { compression: 1, palette: [[0, 0, 0], [255, 255, 255]], raw })
  const files = [
    rle8([]),
    rle8(Array.from({ length: 32 }, () => [0, 2, 0, 255]).flat()),
    rle8(Array.from({ length: 8190 }, () => [0, 0]).flat()),
  ]
  const { results, maxRSS } = decodeInChild(files.map((data) => [data, 'image/bmp']))
  const failed = 'ready resolves | decode() EncodingError: Failed to decode frame at index 0'
  t.deepEqual(results, files.flatMap(() => [`buffer: ${failed}`, `stream: ${failed}`]))
  // about 50 MiB on macOS arm64, 2 GiB before
  t.true(maxRSS < 512 * 2 ** 20, `peak RSS ${Math.round(maxRSS / 2 ** 20)} MiB`)
})

test('ImageDecoder: a GIF or BMP too large for a frame fails before its canvas is allocated', (t) => {
  // 65535 x 8191 passes Blink's limit (4 * w * h < 2^31), and FFmpeg cannot make the frame:
  // av_image_check_size needs (8 * w + 1024) * (h + 128) < 2^31 - 1. An RLE8 BMP that ends at
  // once is a black image, and the GIF has one 1x1 frame. tracks.ready resolves, and decode()
  // rejects like Chrome 155. Each one made 2 GiB of canvas before its frame failed.
  const black = bmp(65535, 8191, 8, [], { compression: 1, palette: [[0, 0, 0], [255, 255, 255]], raw: [0, 1] })
  const files: [Uint8Array, string][] = [
    [black, 'image/bmp'],
    [gif(65535, 8191, PALETTE, [{ x: 0, y: 0, width: 1, height: 1, indices: [0] }]), 'image/gif'],
  ]
  const { results, maxRSS } = decodeInChild(files)
  const failed = 'ready resolves | decode() EncodingError: Failed to decode frame at index 0'
  t.deepEqual(results, files.flatMap(() => [`buffer: ${failed}`, `stream: ${failed}`]))
  // about 50 MiB on macOS arm64, 4 GiB before
  t.true(maxRSS < 512 * 2 ** 20, `peak RSS ${Math.round(maxRSS / 2 ** 20)} MiB`)
})

test('ImageDecoder: a streamed GIF or BMP counts a frame once its header arrives, and decode() waits for its data, like Chrome', async (t) => {
  for (const [data, type, header, frameIndex] of [
    // frame 0, and the rect of frame 1 (its Image Descriptor up to the flags byte)
    [TWO_FRAMES, 'image/gif', 68, 1],
    // the file and info headers
    [BMP_3x2, 'image/bmp', 54, 0],
  ] as const) {
    let controller!: ReadableStreamDefaultController<Uint8Array>
    const stream = new ReadableStream<Uint8Array>({
      start(streamController) {
        controller = streamController
        controller.enqueue(data.slice(0, header))
      },
    })
    const decoder = new ImageDecoder({ data: stream, type })
    await decoder.tracks.ready
    t.is(decoder.tracks.selectedTrack!.frameCount, frameIndex + 1, type)

    let settled = false
    const decoded = decoder.decode({ frameIndex }).finally(() => {
      settled = true
    })
    await new Promise((resolve) => setTimeout(resolve, 25))
    t.false(settled, type)

    controller.enqueue(data.slice(header))
    controller.close()
    const { image, complete } = await decoded
    t.true(complete)
    t.is(image.format, 'BGRX')
    image.close()
    decoder.close()
  }
})

// ============================================================================
// ImageDecoder Options Tests (W3C spec)
// ============================================================================

test('ImageDecoder desiredWidth/desiredHeight are [EnforceRange] unsigned long, like Chrome', (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const range = "Value is outside the 'unsigned long' value range."
  const notNumber = "Value is not of type 'unsigned long'."
  const infinite = "Value is infinite and not of type 'unsigned long'."
  const cases: [Record<string, unknown>, string, string][] = [
    [{ desiredWidth: -1, desiredHeight: 24 }, 'desiredWidth', range],
    [{ desiredWidth: 2 ** 32, desiredHeight: 24 }, 'desiredWidth', range],
    [{ desiredWidth: NaN, desiredHeight: 24 }, 'desiredWidth', notNumber],
    [{ desiredWidth: 'abc', desiredHeight: 24 }, 'desiredWidth', notNumber],
    [{ desiredWidth: Infinity, desiredHeight: 24 }, 'desiredWidth', infinite],
    // converted even when the other one is missing
    [{ desiredHeight: -Infinity }, 'desiredHeight', infinite],
    // dictionary members are read in name order
    [{ desiredWidth: NaN, desiredHeight: -1 }, 'desiredHeight', range],
  ]
  for (const [init, member, problem] of cases) {
    t.throws(() => new ImageDecoder({ data, type: 'image/png', ...init }), {
      instanceOf: TypeError,
      message: `Failed to construct 'ImageDecoder': Failed to read the '${member}' property from 'ImageDecoderInit': ${problem}`,
    })
  }
})

test('ImageDecoder ignores desiredWidth or desiredHeight alone, like Chrome', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  // null converts to 0, which Chrome treats as no desired size
  for (const init of [{ desiredWidth: 4 }, { desiredHeight: 4 }, { desiredWidth: null, desiredHeight: 4 }]) {
    const decoder = new ImageDecoder({ data, type: 'image/png', ...init } as ImageDecoderInit)
    const { image } = await decoder.decode()
    t.deepEqual([image.codedWidth, image.codedHeight], [8, 8])
    image.close()
    decoder.close()
  }
})

test('ImageDecoder: a JPEG desired size below 1/64 of its area fails the metadata and closes the decoder, like Chrome', async (t) => {
  // 320x240: 39x30 needs a scale below 1/8
  const data = readFileSync(join(__dirname, 'fixtures/wpt/four-colors.jpg'))
  const decoder = new ImageDecoder({ data, type: 'image/jpeg', desiredWidth: 39, desiredHeight: 30 })
  // Read and never awaited: like Chrome, its rejection is not reported as unhandled.
  t.true(decoder.tracks.ready instanceof Promise)
  const completed = decoder.completed
  const pending = decoder.decode()
  const metadataError = { instanceOf: DOMException, name: 'InvalidStateError', message: 'Failed to retrieve track metadata.' }
  await t.throwsAsync(decoder.tracks.ready, metadataError)
  await t.throwsAsync(pending, { instanceOf: DOMException, name: 'EncodingError', message: 'Failed to decode frame at index 0' })
  // The data was complete before the failure.
  await t.notThrowsAsync(completed)
  t.true(decoder.closed)
  await t.throwsAsync(decoder.decode(), { instanceOf: DOMException, name: 'InvalidStateError' })
  await t.throwsAsync(decoder.tracks.ready, metadataError)
})

test('ImageDecoder: a streamed JPEG with a desired size below 1/64 of its area rejects completed, like Chrome', async (t) => {
  const data = new Uint8Array(readFileSync(join(__dirname, 'fixtures/wpt/four-colors.jpg')))
  // With a COM segment bigger than the scan, cut before SOS, the metadata fails only once the last chunk is in.
  const com = [0xff, 0xfe, 1202 >> 8, 1202 & 0xff, ...new Uint8Array(1200)]
  const withCom = new Uint8Array([...data.subarray(0, 2), ...com, ...data.subarray(2)])
  const sos = withCom.findIndex((byte, i) => byte === 0xff && withCom[i + 1] === 0xda)
  for (const chunks of [
    [data],
    [data.subarray(0, 300), data.subarray(300)],
    [withCom.subarray(0, sos), withCom.subarray(sos)],
  ]) {
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(chunks[0])
        setTimeout(() => {
          chunks.slice(1).forEach((chunk) => controller.enqueue(chunk))
          controller.close()
        }, 50)
      },
    })
    const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg', desiredWidth: 39, desiredHeight: 30 })
    // Read now, awaited after it rejects: like Chrome, the rejection is not reported as unhandled.
    const completed = decoder.completed
    const pending = decoder.decode()
    await t.throwsAsync(decoder.tracks.ready, { name: 'InvalidStateError', message: 'Failed to retrieve track metadata.' })
    await t.throwsAsync(pending, { name: 'EncodingError', message: 'Failed to decode frame at index 0' })
    await t.throwsAsync(completed, { instanceOf: DOMException, name: 'AbortError', message: 'Aborted by close.' })
    t.false(decoder.complete)
    t.true(decoder.closed)
  }
  // `completed` never read: its rejection is not reported as unhandled either.
  const stream = new ReadableStream<Uint8Array>({ start: (controller) => controller.enqueue(data) })
  const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg', desiredWidth: 39, desiredHeight: 30 })
  await t.throwsAsync(decoder.tracks.ready, { name: 'InvalidStateError' })
  await new Promise((resolve) => setTimeout(resolve, 50))
})

test('ImageDecoder: a close() in the decode() task or a microtask wins over a failed metadata, every time', async (t) => {
  // The worker thread can find the failed metadata before the JS task reaches close(). The close()
  // still wins, as on main: decode() rejects with "ImageDecoder is closed", ready and completed
  // resolve. (Chrome 155 rejects decode() with AbortError "Aborted by failure." here.)
  const badJpeg = new Uint8Array(readFileSync(join(__dirname, 'fixtures/wpt/four-colors.jpg')))
  const sof = badJpeg.findIndex((byte, i) => byte === 0xff && badJpeg[i + 1] === 0xc2)
  badJpeg.fill(0, sof + 7, sof + 9)
  const cases: [string, ImageDecoderInit][] = [
    // 32-bit BI_BITFIELDS, red mask 0x00f0f000 (not contiguous): fails before any FFmpeg work
    [
      'bad BMP mask',
      {
        type: 'image/bmp',
        data: bmp(
          4,
          4,
          32,
          Array.from({ length: 4 }, (_, y) => Array.from({ length: 16 }, (_, x) => (((3 - y) * 16 + x) * 37) & 255)),
          { compression: 3, masks: [0x00f0f000, 0x0000ff00, 0x000000ff] },
        ),
      },
    ],
    // the GIF and BMP header errors (GifDecoder::new, BmpDecoder::new)
    [
      'GIF signature GIF8Xa',
      { type: 'image/gif', data: Uint8Array.from([...Buffer.from('GIF8Xa'), ...TWO_FRAMES.subarray(6)]) },
    ],
    ['GIF cut in the logical screen descriptor', { type: 'image/gif', data: TWO_FRAMES.slice(0, 9) }],
    [
      'BMP header size 7',
      {
        type: 'image/bmp',
        data: (() => {
          const data = BMP_3x2.slice()
          new DataView(data.buffer).setUint32(14, 7, true)
          return data
        })(),
      },
    ],
    [
      'BMP V5 header with its ICC profile past the end',
      {
        type: 'image/bmp',
        data: (() => {
          const data = bmp(1, 1, 24, [[0, 0, 0]], { header: 124 })
          const view = new DataView(data.buffer)
          // PROFILE_EMBEDDED, profile at 0x1000 (from the header), 16 bytes
          view.setUint32(70, 0x4d424544, true)
          view.setUint32(126, 0x1000, true)
          view.setUint32(130, 16, true)
          return data
        })(),
      },
    ],
    // M = 0: fails once the header is read, before any decode
    [
      'JPEG desired 39x30',
      {
        type: 'image/jpeg',
        data: readFileSync(join(__dirname, 'fixtures/wpt/four-colors.jpg')),
        desiredWidth: 39,
        desiredHeight: 30,
      },
    ],
    // SOF width 0: libjpeg-turbo fails the header
    ['JPEG with a bad header', { type: 'image/jpeg', data: badJpeg }],
  ]
  const busy = (ms: number) => {
    const end = performance.now() + ms
    while (performance.now() < end);
  }
  const closeIn: [string, (close: () => void) => void][] = [
    ['the same task', (close) => close()],
    // gives the worker time to finish first
    [
      'the same task, after 5 ms of sync work',
      (close) => {
        busy(5)
        close()
      },
    ],
    ['a microtask', (close) => queueMicrotask(close)],
  ]
  const outcome = (promise: Promise<unknown>) =>
    promise.then(
      () => 'resolves',
      (e: Error) => `${e instanceof DOMException ? 'DOMException ' : ''}${e.name}: ${e.message}`,
    )
  for (const [name, init] of cases) {
    // without a close(), the metadata fails
    const failing = new ImageDecoder(init)
    t.is(
      await outcome(failing.tracks.ready),
      'DOMException InvalidStateError: Failed to retrieve track metadata.',
      `${name}, no close()`,
    )
    for (const [when, schedule] of closeIn) {
      const seen: Record<string, number> = {}
      for (let i = 0; i < 50; i++) {
        const decoder = new ImageDecoder(init)
        const ready = outcome(decoder.tracks.ready)
        const completed = outcome(decoder.completed)
        const decoded = outcome(decoder.decode().then(({ image }) => image.close()))
        schedule(() => decoder.close())
        const key = `decode() ${await decoded} | ready ${await ready} | completed ${await completed}`
        seen[key] = (seen[key] ?? 0) + 1
      }
      t.deepEqual(
        seen,
        { 'decode() DOMException InvalidStateError: ImageDecoder is closed | ready resolves | completed resolves': 50 },
        `${name}, close() in ${when}`,
      )
    }
  }
})

test('ImageDecoder: a close() in the decode() task or a microtask leaves the GIF track as built, every time', async (t) => {
  // The worker thread can fill the track (frameCount, repetitionCount) before the JS task reaches
  // close(). As for a failed metadata, a close() before the decoder signals tracks.ready inside
  // wins: the track stays as the decoder built it. With buffer data, that signal comes in a later
  // task, so a close() in the task that made the decoder or in a microtask wins every time, as
  // here. A close() in a later task can come after the signal, and then keeps what the worker
  // found. Before, the track kept what the worker found whenever the worker finished first: almost
  // always after sync work (as after 5 ms here), now and then in the same task or a microtask, also
  // on main. Chrome 155 has no track then.
  const files: [string, Uint8Array][] = [
    ['two frames', TWO_FRAMES],
    ['one frame', ONE_FRAME],
    ['animated.gif', readFileSync(join(__dirname, 'fixtures/animated.gif'))],
  ]
  const busy = (ms: number) => {
    const end = performance.now() + ms
    while (performance.now() < end);
  }
  const closeIn: [string, (close: () => void) => void][] = [
    ['the same task', (close) => close()],
    [
      'the same task, after 5 ms of sync work',
      (close) => {
        busy(5)
        close()
      },
    ],
    ['a microtask', (close) => queueMicrotask(close)],
  ]
  const outcome = (promise: Promise<unknown>) =>
    promise.then(
      () => 'resolves',
      (e: Error) => `${e.name}: ${e.message}`,
    )
  const track = (decoder: ImageDecoder) => {
    const { frameCount, repetitionCount, animated } = decoder.tracks.selectedTrack!
    return `frameCount ${frameCount}, repetitionCount ${repetitionCount}, animated ${animated}`
  }
  // the track of a new GIF decoder
  const built = 'frameCount 0, repetitionCount Infinity, animated true'
  const seen: Record<string, Record<string, number>> = {}
  const expected: Record<string, Record<string, number>> = {}
  for (const [name, data] of files) {
    // without a close(), the track has the metadata, and a close() after tracks.ready keeps it
    const open = new ImageDecoder({ data, type: 'image/gif' })
    await open.tracks.ready
    const metadata = track(open)
    t.not(metadata, built, name)
    open.close()
    t.is(track(open), metadata, `${name}, close() after tracks.ready`)
    for (const [when, schedule] of closeIn) {
      const counts: Record<string, number> = {}
      for (let i = 0; i < 50; i++) {
        const decoder = new ImageDecoder({ data, type: 'image/gif' })
        const decoded = outcome(decoder.decode().then(({ image }) => image.close()))
        schedule(() => decoder.close())
        const ready = await outcome(decoder.tracks.ready)
        const key = `decode() ${await decoded} | ready ${ready} | ${track(decoder)}`
        counts[key] = (counts[key] ?? 0) + 1
      }
      seen[`${name}, close() in ${when}`] = counts
      expected[`${name}, close() in ${when}`] = {
        [`decode() InvalidStateError: ImageDecoder is closed | ready resolves | ${built}`]: 50,
      }
    }
  }
  t.deepEqual(seen, expected)
})

test('ImageDecoder preferAnimation false returns single frame for animated GIF', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/gif',
    preferAnimation: false,
  })

  // Decode first (and only) frame
  const result = await decoder.decode()
  t.true(result.complete)
  t.truthy(result.image)
  result.image.close()

  // With preferAnimation: false, should only have 1 frame
  const track = decoder.tracks.selectedTrack!
  t.is(track.frameCount, 1)
  t.false(track.animated)

  decoder.close()
})

test('ImageDecoder preferAnimation true decodes all frames', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/gif',
    preferAnimation: true,
  })

  // Wait for pre-parsing to complete
  await decoder.tracks.ready

  // With preferAnimation: true (or default), should decode all frames
  const track = decoder.tracks.selectedTrack!
  t.true(track.animated)
  t.true(track.frameCount >= 1)

  decoder.close()
})

test('ImageDecoder colorSpaceConversion default extracts color space metadata', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/png',
    colorSpaceConversion: 'default',
  })

  const result = await decoder.decode()
  t.true(result.complete)

  // With "default", color space may be populated from image metadata (if present)
  // The colorSpace object should exist regardless
  t.truthy(result.image.colorSpace)

  result.image.close()
  decoder.close()
})

test('ImageDecoder colorSpaceConversion none returns empty color space', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/png',
    colorSpaceConversion: 'none',
  })

  const result = await decoder.decode()
  t.true(result.complete)

  // With "none", color space metadata should be ignored (all null per Chromium behavior)
  const cs = result.image.colorSpace.toJSON() as {
    primaries: string | null
    transfer: string | null
    matrix: string | null
    fullRange: boolean | null
  }
  t.is(cs.primaries, null)
  t.is(cs.transfer, null)
  t.is(cs.matrix, null)
  t.is(cs.fullRange, null)

  result.image.close()
  decoder.close()
})

test('ImageDecoder invalid colorSpaceConversion throws TypeError', (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))

  t.throws(
    () =>
      new ImageDecoder({
        data,
        type: 'image/png',
        colorSpaceConversion: 'invalid' as unknown as 'none' | 'default',
      }),
    { message: /Invalid colorSpaceConversion value/ },
  )
})
