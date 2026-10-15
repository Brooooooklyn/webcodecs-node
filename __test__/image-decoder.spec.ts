/**
 * ImageDecoder API Conformance Tests
 *
 * Tests WebCodecs ImageDecoder specification compliance.
 * Tests frame_index support for animated images (GIF, WebP).
 */

import test from 'ava'
import { readFileSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

import { ImageDecoder, type ImageDecoderInit } from '../index.js'

import { bmp } from './helpers/image-files.js'

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
    // M = 0: fails after the FFmpeg decode
    [
      'JPEG desired 39x30',
      {
        type: 'image/jpeg',
        data: readFileSync(join(__dirname, 'fixtures/wpt/four-colors.jpg')),
        desiredWidth: 39,
        desiredHeight: 30,
      },
    ],
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
