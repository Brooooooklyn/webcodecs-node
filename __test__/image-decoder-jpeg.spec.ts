/**
 * ImageDecoder JPEG tests: Chrome 155's results through the JS API. The digests are
 * sha256 of Chrome's copyTo() bytes (default layout). The inputs give the same bytes on
 * every CPU (with and without libjpeg-turbo's SIMD code).
 */

import test from 'ava'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { ImageDecoder, type VideoFrame, type VideoPixelFormat } from '../index.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const fixture = (name: string) => new Uint8Array(readFileSync(join(__dirname, 'fixtures/jpeg', name)))
const sha256 = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex')
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

async function digests(image: VideoFrame) {
  const bytes = new Uint8Array(image.allocationSize())
  await image.copyTo(bytes)
  return { all: sha256(bytes), y: sha256(bytes.subarray(0, image.codedWidth * image.codedHeight)) }
}

async function decodeBuffer(data: Uint8Array, init: Record<string, unknown> = {}) {
  const decoder = new ImageDecoder({ data, type: 'image/jpeg', ...init })
  const { image } = await decoder.decode()
  decoder.close()
  return image
}

// The first half of `data` at once, the rest 30 ms later, the close 30 ms after that
function twoPartStream(data: Uint8Array) {
  const half = data.length >> 1
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(data.slice(0, half))
      void (async () => {
        try {
          await sleep(30)
          controller.enqueue(data.slice(half))
          await sleep(30)
          controller.close()
        } catch {
          // the decoder failed and canceled the stream
        }
      })()
    },
  })
}

// All of `data` at once, the close 60 ms later
function openStream(data: Uint8Array) {
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(data)
      setTimeout(() => controller.close(), 60)
    },
  })
}

// The first half at once, the rest and the close together 30 ms later
function lastChunkClosesStream(data: Uint8Array) {
  const half = data.length >> 1
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(data.slice(0, half))
      setTimeout(() => {
        controller.enqueue(data.slice(half))
        controller.close()
      }, 30)
    },
  })
}

// All of `data` and the close in start(), or in pull()
const closedStreams = {
  start: (data: Uint8Array) =>
    new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(data)
        controller.close()
      },
    }),
  pull: (data: Uint8Array) =>
    new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.enqueue(data)
        controller.close()
      },
    }),
}

// [fixture, Chrome format, size, sha256 of the bytes]. Chrome decodes 4:2:0, 4:2:2 and 4:4:4
// YCbCr to YUV planes, everything else to BGRX.
const BUFFER_CASES: [string, VideoPixelFormat, [number, number], string][] = [
  ['444-progressive-33x17.jpg', 'I444', [33, 17], 'faead195d1830cd7209a604c8916ee95276b8d5972bf8ab4c06eaf1d2a7f761d'],
  ['scans-99.jpg', 'I444', [61, 47], 'd6e2162b6e0cf4284fba367e8066d8ffcdd4cec34e06babc3249607d0b8c1fbf'],
  // corrupt AC data in a progressive scan: no error
  [
    'progressive-junk-in-scan.jpg',
    'I420',
    [64, 48],
    'd6add19cb2298231cfcd4f3edd0c9b5f42b36b672d8f62489f331a630ef4eec0',
  ],
  ['440-37x9.jpg', 'BGRX', [37, 9], '63e19493c9ee853f2862eafeb1d9b79faf5972d2f68704b94e035446d085ec94'],
  ['411-127x8.jpg', 'BGRX', [127, 8], 'f2518308fa4137e744c9f9c9ff8709843dcb8635ad2fb2f0e4c556e6ca813d85'],
  ['cmyk-33x17.jpg', 'BGRX', [33, 17], '4de98e3464eb05a4056670b236b6fec8ccae3b73696a16f58f3e664a6268a91f'],
  ['ycck-33x17.jpg', 'BGRX', [33, 17], 'c0f20c6e0b50d6f2771c7194f783f2f89476ce79fd3ae2b8a509d7fea6856a49'],
  // only the first image (4:4:0) is read
  ['concatenated-440-420.jpg', 'BGRX', [64, 48], 'e2b941269bd8c218d10ab825d44a1d51fbc46ad57c3c633e7892fd74d0387388'],
]

for (const [name, format, size, hash] of BUFFER_CASES) {
  test(`ImageDecoder: ${name} decodes to ${format} with Chrome's bytes`, async (t) => {
    const image = await decodeBuffer(fixture(name))
    t.is(image.format, format)
    t.deepEqual([image.codedWidth, image.codedHeight], size)
    t.is((await digests(image)).all, hash)
    t.deepEqual(image.colorSpace.toJSON(), {
      primaries: 'bt709',
      transfer: 'iec61966-2-1',
      matrix: format === 'BGRX' ? 'rgb' : 'smpte170m',
      fullRange: true,
    })
    image.close()
  })
}

// 4:2:0 and 4:2:2 at odd sizes: the format, and the Y plane's bytes
test("ImageDecoder: odd-size 4:2:0 and 4:2:2 JPEGs decode to I420 and I422 with Chrome's Y plane", async (t) => {
  for (const [name, format, size, y] of [
    ['420-37x9.jpg', 'I420', [37, 9], '61bdc52f5e08e685dd6afd1e58040901ca57b1b978736d4e8ba32200676108ce'],
    ['422-15x15.jpg', 'I422', [15, 15], 'f6ba813556bdffe3894d2d6587d535a50199fa4f68d964537159884c17dfb355'],
  ] as const) {
    const image = await decodeBuffer(fixture(name))
    t.is(image.format, format, name)
    t.deepEqual([image.codedWidth, image.codedHeight], size, name)
    t.is((await digests(image)).y, y, name)
    image.close()
  }
})

// Chrome decodes at M/8 of the size (rounded up), to BGRX when M < 8. Here the desired size is
// the M/8 size. [fixture, size, [M = 1..7 digests]]
const SCALED_CASES: [string, [number, number], string[]][] = [
  [
    '444-progressive-33x17.jpg',
    [33, 17],
    [
      '53a6b1bac81922a1eb2351892437f659f23eda206bba8fe099f3b5973946def5',
      '923a26a0369bf73af681da04fb59f9732e0dd1eb6cad8ae1c1754967626309eb',
      'd18d9dedd8470deb9aee44d9c5f8da7a7df325943936116103535b3bbbbc1147',
      '6f1ed259613c0eb344c49513e27a306f133d2eeb12aec1e644266bafb1fec59b',
      'f87c69370c972cc0f858e971508829876a007b4a2c608a46a6771ffc3d7cab6d',
      '77ef15fb9270db8df14a2b60bd086d4310297037f5558db6ab8cce93fd37ebf0',
      '642e865a04a250adcf2774a63e6059648008c301b376d8a58c9a9b4a0e280d11',
    ],
  ],
  [
    '420-37x9.jpg',
    [37, 9],
    [
      '5ad43d7526442dc2bc0bdd1c201df15e794d374084e3fb0e055f75310ba37cb1',
      '1343b057306693f06848e883d7f519e9c7a3f3a061b4d44377fba83cef749e17',
      'ce61a5cac847b67c0f7b297678d3898c349f166fab922568fbb6d92c56e94fd7',
      '6b3c2b587168fd48c3a6c51b95ca0122a35ddba3c2555e56bd56aa4f161ad086',
      'ac1afec48a889ac8cf1508078866c04e2a69782308549d38042be943e426cd57',
      'b5dcb357ce0d7f3343fcde302fb414b2b05ec737c87dcdfb7a0300089aa72a2a',
      'bfba99642a08299e954c1c18e891a0f7040e73d9e4415e74990a22e761b9ff9a',
    ],
  ],
]

for (const [name, [width, height], hashes] of SCALED_CASES) {
  test(`ImageDecoder: ${name} at M/8 for M = 1..7 has Chrome's size and bytes`, async (t) => {
    for (const [i, hash] of hashes.entries()) {
      const m = i + 1
      const size = [Math.ceil((width * m) / 8), Math.ceil((height * m) / 8)]
      const image = await decodeBuffer(fixture(name), { desiredWidth: size[0], desiredHeight: size[1] })
      t.is(image.format, 'BGRX', `M = ${m}`)
      t.deepEqual([image.codedWidth, image.codedHeight], size, `M = ${m}`)
      t.is((await digests(image)).all, hash, `M = ${m}`)
      image.close()
    }
  })
}

// Chrome decodes to YUV only when all the data is here before the first decode() of the image.
// A decode() made before that decodes the image to RGB rows as the data arrives.
test('ImageDecoder: a streamed JPEG closed before decode() is YUV, like a buffer', async (t) => {
  for (const [name, format, y] of [
    ['444-progressive-33x17.jpg', 'I444', '84b069c456a420975549ec97d0d5e35561a5cec5feead9fcb0fe127f2cfdf5a9'],
    ['420-37x9.jpg', 'I420', '61bdc52f5e08e685dd6afd1e58040901ca57b1b978736d4e8ba32200676108ce'],
  ] as const) {
    const decoder = new ImageDecoder({ data: twoPartStream(fixture(name)), type: 'image/jpeg' })
    await decoder.completed
    const { image } = await decoder.decode()
    t.is(image.format, format, name)
    t.is((await digests(image)).y, y, name)
    image.close()
    decoder.close()
  }
})

// A stream whose data and close come in one task (here before the reader reads it): Chrome reads
// both before it decodes, also for a decode() made at once.
test('ImageDecoder: a JPEG stream whose data and close come together is YUV, like Chrome', async (t) => {
  for (const [name, format, y] of [
    ['444-progressive-33x17.jpg', 'I444', '84b069c456a420975549ec97d0d5e35561a5cec5feead9fcb0fe127f2cfdf5a9'],
    ['420-37x9.jpg', 'I420', '61bdc52f5e08e685dd6afd1e58040901ca57b1b978736d4e8ba32200676108ce'],
    ['422-15x15.jpg', 'I422', 'f6ba813556bdffe3894d2d6587d535a50199fa4f68d964537159884c17dfb355'],
  ] as const) {
    for (const [source, stream] of Object.entries(closedStreams)) {
      const decoder = new ImageDecoder({ data: stream(fixture(name)), type: 'image/jpeg' })
      const { image, complete } = await decoder.decode()
      t.is(image.format, format, `${name} ${source}`)
      t.true(complete, `${name} ${source}`)
      t.is((await digests(image)).y, y, `${name} ${source}`)
      image.close()
      const again = await decoder.decode()
      t.is(again.image.format, format, `${name} ${source}`)
      again.image.close()
      decoder.close()
    }
  }
})

test('ImageDecoder: a JPEG decode() made while the stream is open is BGRX, like Chrome', async (t) => {
  for (const [name, hash] of [
    ['444-progressive-33x17.jpg', '9aa93940af52ce3d3c37026abbc1dfe33ee2d25115367b5a6cc803ca85afa88b'],
    ['420-37x9.jpg', '634235a6bc72fe6d9a6dc91352b1124bf100d1c0b44cb912207f20af461e51b2'],
    ['422-15x15.jpg', '065ee0bdba7f0be384ab06ad9d89fabd6cba63f8981ee9812bb3812688917e48'],
  ]) {
    // With half the data (decode() at once), with all of it and the close 60 ms later (decode()
    // at once, or at 30 ms), or with the rest and the close in one task: Chrome decodes the rest
    // before it reads the close.
    for (const [stream, wait] of [
      [twoPartStream, 0],
      [openStream, 0],
      [openStream, 30],
      [lastChunkClosesStream, 0],
    ] as const) {
      const decoder = new ImageDecoder({ data: stream(fixture(name)), type: 'image/jpeg' })
      await sleep(wait)
      const { image } = await decoder.decode()
      t.is(image.format, 'BGRX', name)
      t.is((await digests(image)).all, hash, name)
      image.close()
      // a later decode() gives the same frame
      await decoder.completed
      const again = await decoder.decode()
      t.is(again.image.format, 'BGRX', name)
      again.image.close()
      decoder.close()
    }
  }
})

// reset() aborts a waiting decode() in Chrome, before it decodes, but keeps the choice of RGB that a
// decode of part of the data made.
test("ImageDecoder: reset() keeps a JPEG's choice of RGB or YUV, like Chrome", async (t) => {
  for (const [name, resetAt, format] of [
    // reset() in the task of the decode(): nothing was decoded
    ['444-progressive-33x17.jpg', 0, 'I444'],
    ['420-37x9.jpg', 0, 'I420'],
    // reset() at 20 ms, before the rest comes at 30 ms: half of the data was decoded, with rows
    // of the progressive JPEG (its first scan) and none of the 4:2:0 one (its header ends after
    // the half)
    ['444-progressive-33x17.jpg', 20, 'BGRX'],
    ['420-37x9.jpg', 20, 'I420'],
  ] as const) {
    const decoder = new ImageDecoder({ data: twoPartStream(fixture(name)), type: 'image/jpeg' })
    const pending = decoder.decode().then(
      ({ image }) => image.close(),
      () => {},
    )
    if (resetAt) await sleep(resetAt)
    decoder.reset()
    await decoder.completed
    const { image } = await decoder.decode()
    t.is(image.format, format, `${name} reset at ${resetAt} ms`)
    image.close()
    await pending
    decoder.close()
  }
})

const FAILED = { instanceOf: DOMException, name: 'EncodingError', message: 'Failed to decode frame at index 0' }
const METADATA = { instanceOf: DOMException, name: 'InvalidStateError', message: 'Failed to retrieve track metadata.' }
const ABORTED = { instanceOf: DOMException, name: 'AbortError', message: 'Aborted by reset.' }

// The first `cut` bytes at once; the stream stays open until `close()`
function cutStream(data: Uint8Array, cut: number) {
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c
      c.enqueue(data.slice(0, cut))
    },
  })
  return { stream, rest: () => controller.enqueue(data.slice(cut)), close: () => controller.close() }
}

// Whether a promise is still pending after `ms`
async function pendingAfter(promise: Promise<unknown>, ms: number) {
  let pending = true
  promise.then(
    () => (pending = false),
    () => (pending = false),
  )
  await sleep(ms)
  return pending
}

// The data stops inside the scan, so decode() waits for the rest. Chrome rejects it at reset(). The
// stream then ends there: the decoder is still open, and the next decode() fails, as in Chrome.
test('ImageDecoder: reset() rejects a JPEG decode() that waits for data with AbortError, like Chrome', async (t) => {
  for (const [name, cut] of [
    ['444-progressive-33x17.jpg', 711],
    ['420-37x9.jpg', 760],
  ] as const) {
    const { stream, close } = cutStream(fixture(name), cut)
    const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
    await decoder.tracks.ready
    const waiting = decoder.decode()
    t.true(await pendingAfter(waiting, 50), `${name}: decode() waits`)
    decoder.reset()
    await t.throwsAsync(waiting, ABORTED, name)
    t.false(decoder.closed, name)
    close()
    await decoder.completed
    t.false(decoder.closed, name)
    await t.throwsAsync(decoder.decode(), FAILED, name)
    t.true(decoder.closed, name)
    await t.throwsAsync(decoder.decode(), { instanceOf: DOMException, name: 'InvalidStateError' }, name)
  }
})

// A decode() made before the header is here waits for the tracks. reset() does not resolve
// tracks.ready (Chrome keeps it pending too); the decode() rejects once the tracks are ready.
test('ImageDecoder: reset() before the JPEG header leaves tracks.ready pending', async (t) => {
  for (const [name, format] of [
    ['444-progressive-33x17.jpg', 'I444'],
    ['420-37x9.jpg', 'I420'],
  ] as const) {
    const { stream, rest, close } = cutStream(fixture(name), 100)
    const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
    const ready = decoder.tracks.ready
    const waiting = decoder.decode()
    waiting.catch(() => {})
    await sleep(20)
    decoder.reset()
    t.true(await pendingAfter(ready, 50), `${name}: tracks.ready`)
    rest()
    close()
    await t.notThrowsAsync(ready, name)
    await t.throwsAsync(waiting, ABORTED, name)
    // nothing decoded part of the data: YUV
    const { image } = await decoder.decode()
    t.is(image.format, format, name)
    image.close()
    decoder.close()
  }
})

// 12-bit, arithmetic coding, 100 scans (Blink's limit), data that ends before the image, no EOI
const FRAME_ERRORS = ['12-bit.jpg', 'arithmetic.jpg', 'scans-100.jpg', 'truncated.jpg', 'no-eoi.jpg']

test('ImageDecoder: a JPEG Chrome fails to decode rejects decode() and closes the decoder', async (t) => {
  // a buffer, or a stream whose data and close come together: Chrome resolves `completed`
  // before it decodes the frame with all the data
  const sources = { buffer: (data: Uint8Array) => data, ...closedStreams }
  for (const name of FRAME_ERRORS) {
    for (const [source, data] of Object.entries(sources)) {
      const label = `${name} ${source}`
      const decoder = new ImageDecoder({ data: data(fixture(name)), type: 'image/jpeg' })
      const completed = decoder.completed
      await t.throwsAsync(decoder.decode(), FAILED, label)
      // the header is fine
      await t.notThrowsAsync(decoder.tracks.ready, label)
      await t.notThrowsAsync(completed, label)
      t.true(decoder.complete, label)
      t.true(decoder.closed, label)
      await t.throwsAsync(decoder.decode(), { instanceOf: DOMException, name: 'InvalidStateError' }, label)
    }
  }
})

test('ImageDecoder: frameIndex 1 of a JPEG is a RangeError before any decode, like Chrome', async (t) => {
  for (const name of FRAME_ERRORS) {
    for (const data of [fixture(name), twoPartStream(fixture(name))]) {
      const decoder = new ImageDecoder({ data, type: 'image/jpeg' })
      await t.throwsAsync(decoder.decode({ frameIndex: 1 }), { instanceOf: RangeError }, name)
      // frame 0 was not decoded: the decoder is still open, and decode() fails now
      t.false(decoder.closed, name)
      await t.throwsAsync(decoder.decode(), FAILED, name)
    }
  }
})

test('ImageDecoder: a streamed JPEG fails when Chrome fails it, which rejects completed only before the end', async (t) => {
  for (const name of FRAME_ERRORS) {
    // half the data, decode() at once, the rest 30 ms later, then the close
    const decoder = new ImageDecoder({ data: twoPartStream(fixture(name)), type: 'image/jpeg' })
    const completed = decoder.completed.then(
      () => 'resolved',
      (error: DOMException) => `${error.name}: ${error.message}`,
    )
    await t.throwsAsync(decoder.decode(), FAILED, name)
    // A bad image fails as soon as it is decoded. One that ends too early fails only once the
    // stream is closed: `completed` resolves first.
    const early = !['truncated.jpg', 'no-eoi.jpg'].includes(name)
    t.is(await completed, early ? 'AbortError: Aborted by close.' : 'resolved', name)
    t.is(decoder.complete, !early, name)
    t.true(decoder.closed, name)
  }
})

// Bad headers, made from 420-37x9.jpg: Chrome rejects the metadata.
function badHeaders() {
  const data = fixture('420-37x9.jpg')
  const sof = data.findIndex((byte, i) => byte === 0xff && data[i + 1] === 0xc0)
  const sos = data.findIndex((byte, i) => byte === 0xff && data[i + 1] === 0xda)
  const heightZero = data.slice()
  heightZero[sof + 5] = 0
  heightZero[sof + 6] = 0
  return {
    // the data ends before the header does: an error only once all the data is here
    'cut before SOS': { data: data.slice(0, sos), early: false },
    'junk before SOI': { data: new Uint8Array([0, ...data]), early: true },
    'height 0': { data: heightZero, early: true },
  }
}

test('ImageDecoder: a JPEG with a bad header rejects tracks.ready and closes the decoder', async (t) => {
  for (const [name, { data }] of Object.entries(badHeaders())) {
    const decoder = new ImageDecoder({ data, type: 'image/jpeg' })
    const completed = decoder.completed
    const pending = decoder.decode()
    await t.throwsAsync(decoder.tracks.ready, METADATA, name)
    await t.throwsAsync(pending, FAILED, name)
    await t.notThrowsAsync(completed, name)
    t.true(decoder.closed, name)
    await t.throwsAsync(decoder.decode(), { instanceOf: DOMException, name: 'InvalidStateError' }, name)
  }
})

test('ImageDecoder: a streamed JPEG with a bad header rejects completed only before the end', async (t) => {
  for (const [name, { data, early }] of Object.entries(badHeaders())) {
    const decoder = new ImageDecoder({ data: twoPartStream(data), type: 'image/jpeg' })
    const completed = decoder.completed.then(
      () => 'resolved',
      (error: DOMException) => `${error.name}: ${error.message}`,
    )
    const pending = decoder.decode()
    await t.throwsAsync(decoder.tracks.ready, METADATA, name)
    await t.throwsAsync(pending, FAILED, name)
    t.is(await completed, early ? 'AbortError: Aborted by close.' : 'resolved', name)
    t.true(decoder.closed, name)
  }
})
