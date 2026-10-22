/**
 * ImageDecoder JPEG tests: Chrome 155's results through the JS API. The digests are
 * sha256 of Chrome's copyTo() bytes (default layout). The inputs give the same bytes on
 * every CPU (with and without libjpeg-turbo's SIMD code).
 */

import test from 'ava'
import { execFileSync, spawnSync } from 'node:child_process'
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

// Chrome's BGRX frames of JPEGs decoded while the stream is open
const OPEN_STREAM_BGRX = [
  ['444-progressive-33x17.jpg', '9aa93940af52ce3d3c37026abbc1dfe33ee2d25115367b5a6cc803ca85afa88b'],
  ['420-37x9.jpg', '634235a6bc72fe6d9a6dc91352b1124bf100d1c0b44cb912207f20af461e51b2'],
  ['422-15x15.jpg', '065ee0bdba7f0be384ab06ad9d89fabd6cba63f8981ee9812bb3812688917e48'],
] as const

test('ImageDecoder: a JPEG decode() made while the stream is open is BGRX, like Chrome', async (t) => {
  const rest = (source: Source) => source.rest()
  const close = (source: Source) => source.close()
  const restAndClose = (source: Source) => {
    source.rest()
    source.close()
  }
  for (const [name, hash] of OPEN_STREAM_BGRX) {
    const data = fixture(name)
    const half = data.length >> 1
    // With half the data (decode() at once, the rest 30 ms later, the close 30 ms after that), with
    // all of it and the close 60 ms after the data (decode() at once, or at 30 ms), or with half
    // and the rest and the close in one task 30 ms later: Chrome decodes the rest before it reads
    // the close. The test gives each part itself, after the decode(): a stream timer made before
    // the test's own wait could fire first on a slow runner. The waits stay: with the rest and the
    // close about 1 ms after the decode(), Chrome decodes the 4:2:0 and 4:2:2 JPEGs to YUV.
    for (const [cut, wait, parts] of [
      [half, 0, [[30, rest], [30, close]]],
      [data.length, 0, [[60, close]]],
      [data.length, 30, [[30, close]]],
      [half, 0, [[30, restAndClose]]],
    ] as const) {
      const source = cutStream(data, cut)
      const decoder = new ImageDecoder({ data: source.stream, type: 'image/jpeg' })
      await sleep(wait)
      const pending = decoder.decode()
      for (const [ms, part] of parts) {
        await sleep(ms)
        part(source)
      }
      const { image } = await pending
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

// All the data is read, and the close comes in the task of the decode(): Chrome decodes the data
// before it reads the close, however long the decode takes. No timer: the test sets the order.
test('ImageDecoder: a JPEG decode() made just before the stream closes is BGRX, like Chrome', async (t) => {
  for (const [name, hash] of OPEN_STREAM_BGRX) {
    const data = fixture(name)
    const { stream, close } = cutStream(data, data.length)
    const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
    await decoder.tracks.ready
    const pending = decoder.decode()
    close()
    const { image } = await pending
    t.is(image.format, 'BGRX', name)
    t.is((await digests(image)).all, hash, name)
    image.close()
    decoder.close()
  }
})

// Fake timers on the main context replace globals that the decoder used for its JPEG stream work:
// @sinonjs/fake-timers queueMicrotask, setImmediate and setTimeout, Vitest's vi.useFakeTimers()
// setImmediate and setTimeout, node:test's mock.timers setImmediate. (Jest's fake timers live in
// the vm context of its tests, which the addon does not read.) The decoder does not schedule that
// work with them, as Chrome's does not: a decode() on an open stream resolves while they never
// run a callback. In a child process that stubs all three for its whole run. The stream gets `first` bytes in start(), the rest (if any) in
// a later task, through the real setImmediate, as from I/O; it stays open. The child prints the
// frame's format and the decoder's `complete` (false: the stream is still open), and does not
// answer (the stream never closes) if the decode() waits for the stubs.
function decodeWithStubbedTimers(data: Uint8Array, first: number, timerFirst = false, forceFallback = false) {
  const script = `
    const fs = require('node:fs');
    if (${forceFallback}) process.isBun = true;
    const { ImageDecoder } = require('./index.js');
    const data = Buffer.from('${Buffer.from(data).toString('base64')}', 'base64');
    const later = setImmediate;
    for (const name of ['queueMicrotask', 'setImmediate', 'setTimeout']) globalThis[name] = () => {};
    let controller;
    const stream = new ReadableStream({
      start(c) {
        controller = c;
        c.enqueue(new Uint8Array(data.subarray(0, ${first})));
      },
    });
    if (${timerFirst} && ${first} < data.length) later(() => controller.enqueue(new Uint8Array(data.subarray(${first}))));
    const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' });
    decoder.decode().then(
      ({ image }) => fs.writeSync(1, 'RESULT:' + image.format + ' ' + decoder.complete + '\\n'),
      (error) => fs.writeSync(1, 'RESULT:' + error.name + '\\n'),
    ).then(() => process.exit(0));
    if (!${timerFirst} && ${first} < data.length) later(() => controller.enqueue(new Uint8Array(data.subarray(${first}))));
  `
  const output = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    timeout: 30_000,
    encoding: 'utf8',
  })
  return /RESULT:(.*)/.exec(output)?.[1]
}

// All the data is in the stream before the decode(): the try is posted from a promise reaction.
test('ImageDecoder: a JPEG decode() on an open stream does not wait for page timers', (t) => {
  const data = fixture('420-37x9.jpg')
  t.is(decodeWithStubbedTimers(data, data.length), 'BGRX false')
})

// The data up to the SOS marker before the decode(), the rest in a later task: the host scheduler
// marks the stream idle, so the rest is decoded as it comes, like Chrome. This remains true when
// the real immediate was queued before the native scheduler.
test('ImageDecoder: a JPEG chunk that comes to an idle stream does not wait for page timers', (t) => {
  const data = fixture('420-37x9.jpg')
  const sos = data.findIndex((byte, i) => byte === 0xff && data[i + 1] === 0xda)
  t.is(decodeWithStubbedTimers(data, sos), 'BGRX false')
  t.is(decodeWithStubbedTimers(data, sos, true), 'BGRX false')
})

// Force the portable queue in a Node child too, so its shutdown and callback paths do not depend
// on a Bun-only CI job.
test('ImageDecoder: the fallback JPEG scheduler is host-driven and ordered before immediates', (t) => {
  const data = fixture('420-37x9.jpg')
  const sos = data.findIndex((byte, i) => byte === 0xff && data[i + 1] === 0xda)
  t.is(decodeWithStubbedTimers(data, sos, false, true), 'BGRX false')
  t.is(decodeWithStubbedTimers(data, sos, true, true), 'BGRX false')
})

// A stream that trickles in while a decode() waits, each chunk to an idle stream. Chrome's decoder
// reads each byte once. Here a try decodes the data from its first byte, so a try for each chunk
// would make the work grow with the square of the chunk count: only a try whose new bytes can
// change its result runs (markers, not the data of a scan). Two junk bytes before the SOS marker
// make libjpeg warn once in each header read that gets that far (the ffmpeg log), which counts
// them: 3 per try, 1 per wake of the decode(). In a child process, which owns its log. The try at
// the last chunk makes the frame and wakes the decode(). The close, a moment later, makes the
// decoder decode all the data once more (3 more reads), on another thread: those lines can come
// before the marker, after it (before the exit), or not at all. So the format comes from the
// marker's line alone, and the reads count in the whole output.
test('ImageDecoder: a JPEG stream that trickles in is not decoded again for each chunk', (t) => {
  const data = fixture('cmyk-33x17.jpg')
  const sos = data.findIndex((byte, i) => byte === 0xff && data[i + 1] === 0xda)
  const junk = new Uint8Array(data.length + 2)
  junk.set(data.subarray(0, sos))
  junk.set(data.subarray(sos), sos + 2)
  const script = `
    const fs = require('node:fs');
    const { ImageDecoder } = require('./index.js');
    const data = Buffer.from('${Buffer.from(junk).toString('base64')}', 'base64');
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const stream = new ReadableStream({
      start(controller) {
        (async () => {
          for (let i = 0; i < data.length; i += 8) {
            controller.enqueue(new Uint8Array(data.subarray(i, i + 8)));
            await sleep(1);
          }
          controller.close();
        })();
      },
    });
    const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' });
    decoder.decode().then(
      ({ image }) => fs.writeSync(1, 'MARKER:' + image.format + '\\n'),
      (error) => fs.writeSync(1, 'MARKER:' + error.name + '\\n'),
    ).then(() => process.exit(0));
  `
  const output = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    env: { ...process.env, WEBCODECS_LOG: 'ffmpeg=warn' },
    timeout: 30_000,
    encoding: 'utf8',
  })
  const format = /MARKER:(\w+)/.exec(output)?.[1]
  const reads = output.split('extraneous bytes before marker').length - 1
  t.is(format, 'BGRX')
  // 229 chunks, 186 of them in the scan: 934 reads with a try for each one
  t.true(reads > 0 && reads < 60, `${reads} header reads`)
})

// The decoder reads a JPEG stream on the JS thread, and looks `then` up on the promise of each
// read() of its reader. An own `then` on each promise sees the reactions it is handed.
function watchReads(stream: ReadableStream<Uint8Array>, seen: (onValue: unknown, onError: unknown) => void) {
  const getReader = stream.getReader.bind(stream)
  Object.defineProperty(stream, 'getReader', {
    value: () => {
      const reader = getReader()
      const read = reader.read.bind(reader)
      Object.defineProperty(reader, 'read', {
        value: () => {
          const promise = read()
          const then = promise.then.bind(promise)
          // oxlint-disable-next-line unicorn/no-thenable -- the promise is a thenable already
          void Object.defineProperty(promise, 'then', {
            value: (onValue: () => void, onError: () => void) => {
              seen(onValue, onError)
              return then(onValue, onError)
            },
          })
          return promise
        },
      })
      return reader
    },
  })
  return stream
}

// A source that gives its chunks at once is read in one task, one read per chunk. Each read gets
// the same two reactions, so the reads leave no functions behind (before, each read made two new
// ones, which the GC freed only after that task).
test('ImageDecoder: the reads of a JPEG stream share one pair of reactions', async (t) => {
  const data = fixture('420-37x9.jpg')
  const reactions = new Set<unknown>()
  let reads = 0
  let sent = 0
  const stream = new ReadableStream<Uint8Array>(
    {
      pull(controller) {
        if (sent < data.length) {
          controller.enqueue(data.slice(sent, ++sent))
        } else {
          controller.close()
        }
      },
    },
    { highWaterMark: 0 },
  )
  watchReads(stream, (onValue, onError) => {
    reads++
    reactions.add(onValue)
    reactions.add(onError)
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
  await decoder.completed
  const { image } = await decoder.decode()
  t.is(image.format, 'I420')
  t.deepEqual([image.codedWidth, image.codedHeight], [37, 9])
  image.close()
  decoder.close()
  // one read for each byte, and one for the end
  t.is(reads, data.length + 1)
  t.is(reactions.size, 2)
})

// The two reactions hold the stream, and the decoder holds them while it reads: it lets go of them
// when nothing reads on. In a child process with --expose-gc, which drops the decoder and the
// stream and then checks with WeakRefs that the reactions of every read are collected: after the
// end, after an error, and after a header that fails, which stops the reading while the stream
// stays open (the last read waits forever). The last case also stops the reading while the JS
// thread is between a read() and its then(): the first pull() runs inside that read() and waits
// until the decoder has closed (the reading stops right after that). Reactions made after that
// stop would hold the stream forever.
test('ImageDecoder: a JPEG stream lets go of its reactions when nothing reads on', (t) => {
  const script = `
    const fs = require('node:fs');
    const { ImageDecoder } = require('./index.js');
    const data = Buffer.from('${Buffer.from(fixture('420-37x9.jpg')).toString('base64')}', 'base64');
    function watchReads(stream, seen) {
      const getReader = stream.getReader.bind(stream);
      Object.defineProperty(stream, 'getReader', {
        value: () => {
          const reader = getReader();
          const read = reader.read.bind(reader);
          Object.defineProperty(reader, 'read', {
            value: () => {
              const promise = read();
              const then = promise.then;
              Object.defineProperty(promise, 'then', {
                value(onValue, onError) {
                  seen(onValue, onError);
                  return then.call(this, onValue, onError);
                },
              });
              return promise;
            },
          });
          return reader;
        },
      });
      return stream;
    }
    const turn = () => new Promise((resolve) => setImmediate(resolve));
    async function collected(refs) {
      for (let i = 0; i < 50; i++) {
        gc();
        await turn();
        if (refs.every((ref) => ref.deref() === undefined)) return true;
      }
      return false;
    }
    function watched(stream, refs) {
      return watchReads(stream, (onValue, onError) => refs.push(new WeakRef(onValue), new WeakRef(onError)));
    }
    async function oneCase(kind) {
      const refs = [];
      let controller;
      let stream = watched(new ReadableStream({ start: (c) => void (controller = c) }), refs);
      let decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' });
      const completed = decoder.completed.catch(() => {});
      decoder.tracks.ready.catch(() => {});
      if (kind === 'header fails') {
        controller.enqueue(new Uint8Array([0, ...data]));
      } else {
        controller.enqueue(data.slice(0, 623));
        await decoder.tracks.ready;
        if (kind === 'end') {
          controller.enqueue(data.slice(623));
          controller.close();
        } else {
          controller.error(new Error('stream error'));
        }
      }
      await completed;
      stream = decoder = controller = null;
      return collected(refs);
    }
    async function stopInRead() {
      // 1 MiB: the reading stops a moment after the chunk is read
      const junk = new Uint8Array(1 << 20);
      junk.set(data, 1);
      const refs = [];
      let controller;
      let decoder;
      let open = false;
      let stream = watched(new ReadableStream({
        start: (c) => void (controller = c),
        pull() {
          // The first pull() runs in the read made in the reaction to the chunk (start() had not
          // finished at the first read, made in the constructor).
          if (!decoder || open) return;
          open = !decoder.closed;
          const limit = performance.now() + 10000;
          while (!decoder.closed && performance.now() < limit) {}
          const end = performance.now() + 50;
          while (performance.now() < end) {}
        },
      }, { highWaterMark: 0 }), refs);
      decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' });
      const completed = decoder.completed.catch(() => {});
      decoder.tracks.ready.catch(() => {});
      controller.enqueue(junk);
      await completed;
      const reached = open && refs.length === 4;
      stream = decoder = controller = null;
      return { reached, collected: await collected(refs) };
    }
    (async () => {
      const cases = {};
      for (const kind of ['end', 'error', 'header fails']) cases[kind] = await oneCase(kind);
      const race = { reached: 0, leaked: 0 };
      for (let i = 0; i < 20 && race.reached < 3; i++) {
        const { reached, collected } = await stopInRead();
        if (reached) race.reached++;
        if (!collected) race.leaked++;
      }
      fs.writeSync(1, 'RESULT:' + JSON.stringify({ cases, race }) + '\\n');
      process.exit(0);
    })();
  `
  const output = execFileSync(process.execPath, ['--expose-gc', '-e', script], {
    cwd: process.cwd(),
    timeout: 60_000,
    encoding: 'utf8',
  })
  const result = JSON.parse(/RESULT:(.*)/.exec(output)?.[1] ?? 'null')
  t.deepEqual(result?.cases, { end: true, error: true, 'header fails': true })
  // The window was hit: the reading stopped while the read() of the first pull() ran
  t.true(result?.race.reached > 0, JSON.stringify(result?.race))
  t.is(result?.race.leaked, 0)
})

// Each load of the addon gets its own env, so one thread can hold several. The JPEG streams still
// open when an env is torn down are let go by a cleanup hook of that env, which runs before napi's
// own hooks of that env. In a child process, workers load the addon three times, open a JPEG stream
// in the first two loads, and are terminated. With one hook for the whole thread (added at the
// first load's stream), the second load's streams were let go after napi's hooks of that load had
// run. napi lets go of a reference made under a load that is not the last one through the
// thread-safe function of that load, which was closing then, so it panicked (twice in each worker;
// the panics were caught).
test('ImageDecoder: a worker ends cleanly with JPEG streams open in more than one load of the addon', (t) => {
  const script = `
    const { Worker } = require('node:worker_threads');
    require('./index.js');
    const file = Object.keys(require.cache).find((name) => name.endsWith('.node'));
    const data = '${Buffer.from(fixture('420-37x9.jpg')).toString('base64')}';
    const worker = \`
      const { parentPort, workerData } = require('node:worker_threads');
      const data = Buffer.from(workerData.data, 'base64');
      const sos = data.findIndex((byte, i) => byte === 0xff && data[i + 1] === 0xda);
      const loads = [];
      const load = () => {
        const m = { exports: {} };
        process.dlopen(m, workerData.file);
        loads.push(m.exports);
      };
      // the header only, and the stream left open: decode() waits
      const open = (m) => {
        const stream = new ReadableStream({ start: (c) => c.enqueue(data.subarray(0, sos)) });
        const decoder = new m.ImageDecoder({ data: stream, type: 'image/jpeg' });
        decoder.decode().catch(() => {});
        return decoder;
      };
      load();
      const first = open(loads[0]);
      load();
      const second = open(loads[1]);
      load();
      globalThis.keep = [first, second];
      setTimeout(() => parentPort.postMessage(new Set(loads.map((m) => m.ImageDecoder)).size), 20);
    \`;
    (async () => {
      const results = [];
      for (let i = 0; i < 5; i++) {
        const w = new Worker(worker, { eval: true, workerData: { file, data } });
        w.on('message', () => w.terminate());
        const loads = new Promise((resolve) => w.once('message', resolve));
        const code = await new Promise((resolve) => w.once('exit', resolve));
        results.push({ loads: await loads, code });
      }
      require('node:fs').writeSync(1, 'RESULT:' + JSON.stringify(results) + '\\n');
      process.exit(0);
    })();
  `
  const child = spawnSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    timeout: 60_000,
    encoding: 'utf8',
  })
  t.is(child.status, 0, child.stderr)
  // three envs in each worker, each terminated (exit code 1)
  t.deepEqual(
    JSON.parse(/RESULT:(.*)/.exec(child.stdout)?.[1] ?? 'null'),
    Array.from({ length: 5 }, () => ({ loads: 3, code: 1 })),
  )
  t.notRegex(child.stderr, /panicked/)
})

// reset() aborts a waiting decode() in Chrome, before it decodes, but keeps the choice of RGB that a
// decode of part of the data made. The first half of the data is there at the decode(). The test
// gives the rest (30 ms after the reset()) and the close (30 ms after that) itself, after the
// reset(): a stream timer made before the test's own wait could fire first on a slow runner.
test("ImageDecoder: reset() keeps a JPEG's choice of RGB or YUV, like Chrome", async (t) => {
  for (const [name, ready, resetAt, format] of [
    // reset() in the task of the decode(): nothing was decoded
    ['444-progressive-33x17.jpg', false, 0, 'I444'],
    ['420-37x9.jpg', false, 0, 'I420'],
    // reset() 300 ms after tracks.ready: the header is in the first half, and the decode() tries
    // that half right after the header is read. The try decodes rows (the first scan). JS cannot
    // see the try itself; the 300 ms leave room for a slow, loaded runner.
    ['444-progressive-33x17.jpg', true, 300, 'BGRX'],
    // reset() 300 ms after the decode(): the header ends after the half (tracks.ready waits for
    // the rest), so nothing was decoded
    ['420-37x9.jpg', false, 300, 'I420'],
  ] as const) {
    const data = fixture(name)
    const source = cutStream(data, data.length >> 1)
    const decoder = new ImageDecoder({ data: source.stream, type: 'image/jpeg' })
    const pending = decoder.decode().then(
      ({ image }) => image.close(),
      () => {},
    )
    if (ready) await decoder.tracks.ready
    if (resetAt) await sleep(resetAt)
    decoder.reset()
    await sleep(30)
    source.rest()
    await sleep(30)
    source.close()
    await decoder.completed
    const { image } = await decoder.decode()
    t.is(image.format, format, `${name} reset at ${resetAt} ms${ready ? ' after tracks.ready' : ''}`)
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
  return {
    stream,
    rest: () => controller.enqueue(data.slice(cut)),
    enqueue: (chunk: Uint8Array) => controller.enqueue(chunk),
    close: () => controller.close(),
  }
}
type Source = ReturnType<typeof cutStream>

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

// @napi-rs/canvas has no aarch64 Windows build
const canvasTest = process.platform === 'win32' && process.arch === 'arm64' ? test.skip : test

// A reset() while a decode()'s frame work runs: like Chrome, the reset() drops what the work finds.
// The work fails here, but the decode() rejects with AbortError and the decoder stays open. Chrome's
// decoder stays failed, so the next decode() fails and closes it. A reset() before the work starts
// gives the same results.
canvasTest('ImageDecoder: a JPEG frame that fails after reset() closes the decoder at the next decode(), like Chrome', async (t) => {
  const { createCanvas } = await import('@napi-rs/canvas')
  // 4000x3000 of noise (about 10 MB): libjpeg-turbo takes tens of ms, and the reset() below comes
  // while it runs
  const canvas = createCanvas(4000, 3000)
  const ctx = canvas.getContext('2d')
  const noise = ctx.createImageData(4000, 3000)
  const pixels = noise.data
  let seed = 1
  for (let i = 0; i < pixels.length; i += 4) {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0
    pixels[i] = seed >>> 24
    pixels[i + 1] = (seed >>> 16) & 255
    pixels[i + 2] = (seed >>> 8) & 255
    pixels[i + 3] = 255
  }
  ctx.putImageData(noise, 0, 0)
  const jpeg = new Uint8Array(canvas.encodeSync('jpeg', 90))
  const sos = jpeg.findIndex((byte, i) => byte === 0xff && jpeg[i + 1] === 0xda)
  // the data ends inside the scan
  const data = jpeg.subarray(0, sos + Math.floor((jpeg.length - sos) * 0.9))
  for (let attempt = 1; ; attempt++) {
    const decoder = new ImageDecoder({ data, type: 'image/jpeg' })
    await decoder.tracks.ready
    const stale = decoder.decode()
    stale.catch(() => {})
    await sleep(0)
    // On a busy machine the work can fail and close the decoder before reset(): try again
    if (decoder.closed && attempt < 5) {
      await t.throwsAsync(stale, FAILED)
      continue
    }
    decoder.reset()
    await t.throwsAsync(stale, ABORTED)
    t.false(decoder.closed)
    await t.throwsAsync(decoder.decode(), FAILED)
    t.true(decoder.closed)
    break
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

// The same with all the data read and the close in the task of the decode() (no timer): a bad
// image fails before Chrome reads the close, one that ends too early only after it.
test('ImageDecoder: a JPEG decode() made just before the stream closes fails where Chrome fails it', async (t) => {
  for (const name of FRAME_ERRORS) {
    const data = fixture(name)
    const { stream, close } = cutStream(data, data.length)
    const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
    const completed = decoder.completed.then(
      () => 'resolved',
      (error: DOMException) => `${error.name}: ${error.message}`,
    )
    await decoder.tracks.ready
    const pending = decoder.decode()
    close()
    await t.throwsAsync(pending, FAILED, name)
    const early = !['truncated.jpg', 'no-eoi.jpg'].includes(name)
    t.is(await completed, early ? 'AbortError: Aborted by close.' : 'resolved', name)
    t.is(decoder.complete, !early, name)
    t.true(decoder.closed, name)
  }
})

// The stream ends just before decode(), in the same task: the close alone (all the data is read),
// or the rest in one or two chunks and the close. Chrome reads them inside enqueue() and close(),
// before the decode(), so it decodes all the data: YUV planes, or a failure found after the end
// (`completed` resolves). No timer: the test sets the order. The reader waits for data when the
// stream ends (its idle check has run), so the rest comes to an idle stream.
test('ImageDecoder: a JPEG decode() made just after the stream ends decodes all the data, like Chrome', async (t) => {
  type End = (rest: Uint8Array, enqueue: (chunk: Uint8Array) => void, close: () => void) => void
  const ends: Record<string, End> = {
    'close()': (_rest, _enqueue, close) => close(),
    'the rest and close()': (rest, enqueue, close) => {
      enqueue(rest)
      close()
    },
    'the rest in two chunks and close()': (rest, enqueue, close) => {
      enqueue(rest.slice(0, rest.length >> 1))
      enqueue(rest.slice(rest.length >> 1))
      close()
    },
  }
  // [fixture, the bytes read before, Chrome's format (none: the frame fails), Y plane]
  const cases = [
    ['444-progressive-33x17.jpg', 711, 'I444', '84b069c456a420975549ec97d0d5e35561a5cec5feead9fcb0fe127f2cfdf5a9'],
    ['420-37x9.jpg', 760, 'I420', '61bdc52f5e08e685dd6afd1e58040901ca57b1b978736d4e8ba32200676108ce'],
    ['12-bit.jpg', 2355, null, null],
    ['arithmetic.jpg', 1033, null, null],
    ['scans-100.jpg', 3392, null, null],
  ] as const
  for (const [order, end] of Object.entries(ends)) {
    for (const [name, half, format, y] of cases) {
      const label = `${name}: ${order}, decode()`
      const data = fixture(name)
      const cut = order === 'close()' ? data.length : half
      const { stream, enqueue, close } = cutStream(data, cut)
      const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
      const completed = decoder.completed.then(
        () => 'resolved',
        (error: DOMException) => `${error.name}: ${error.message}`,
      )
      await decoder.tracks.ready
      await new Promise((resolve) => setImmediate(resolve))
      end(data.slice(cut), enqueue, close)
      const pending = decoder.decode()
      if (format) {
        const { image } = await pending
        t.is(image.format, format, label)
        t.is((await digests(image)).y, y, label)
        image.close()
        t.false(decoder.closed, label)
      } else {
        await t.throwsAsync(pending, FAILED, label)
        t.true(decoder.closed, label)
      }
      t.is(await completed, 'resolved', label)
      t.true(decoder.complete, label)
      decoder.close()
    }
  }
})

// All the data is read and the stream stays open; decode(), then reset() in the same task or 3
// microtasks later. Chrome applies a failed decode in a task of its own, so the reset() drops the
// failure: reset() does not throw, the decode() rejects with AbortError, and the next decode()
// fails and closes the decoder. In the same task, the test waits until the try has surely failed.
test('ImageDecoder: a reset() right after a JPEG decode() on an open stream drops the frame failure, like Chrome', async (t) => {
  for (const name of ['12-bit.jpg', 'arithmetic.jpg', 'scans-100.jpg']) {
    for (const gap of ['same task', 'microtasks'] as const) {
      const label = `${name}, reset() in the ${gap}`
      const data = fixture(name)
      const { stream, close } = cutStream(data, data.length)
      const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
      await decoder.tracks.ready
      const pending = decoder.decode()
      pending.catch(() => {})
      if (gap === 'same task') {
        const end = performance.now() + 20
        while (!decoder.closed && performance.now() < end) {}
      } else {
        for (let i = 0; i < 3; i++) await Promise.resolve()
      }
      t.false(decoder.closed, label)
      t.notThrows(() => decoder.reset(), label)
      await t.throwsAsync(pending, ABORTED, label)
      t.false(decoder.closed, label)
      await t.throwsAsync(decoder.decode(), FAILED, label)
      t.true(decoder.closed, label)
      close()
    }
  }
})

// The same, but the stream ends in the task of the decode(), and the reset() comes after 3
// microtasks (the end is read by then) or after 20 ms more of that task. Chrome answers in the
// order it was asked: the failed decode, which the reset() drops, then the end, which resolves
// `completed`. So reset() does not throw, the decode() rejects with AbortError, `completed`
// resolves, and the next decode() fails and closes the decoder.
test('ImageDecoder: a reset() after a JPEG decode() and the end of the stream drops the frame failure, like Chrome', async (t) => {
  for (const name of ['12-bit.jpg', 'arithmetic.jpg', 'scans-100.jpg']) {
    for (const wait of ['microtasks', 'same task'] as const) {
      const label = `${name}, reset() after the ${wait}`
      const data = fixture(name)
      const { stream, close } = cutStream(data, data.length)
      const decoder = new ImageDecoder({ data: stream, type: 'image/jpeg' })
      const completed = decoder.completed.then(
        () => 'resolved',
        (error: DOMException) => `${error.name}: ${error.message}`,
      )
      await decoder.tracks.ready
      const pending = decoder.decode()
      pending.catch(() => {})
      close()
      for (let i = 0; i < 3; i++) await Promise.resolve()
      if (wait === 'same task') {
        const end = performance.now() + 20
        while (!decoder.closed && performance.now() < end) {}
      }
      t.false(decoder.closed, label)
      t.notThrows(() => decoder.reset(), label)
      await t.throwsAsync(pending, ABORTED, label)
      t.false(decoder.closed, label)
      await t.throwsAsync(decoder.decode(), FAILED, label)
      t.true(decoder.closed, label)
      t.is(await completed, 'resolved', label)
      t.true(decoder.complete, label)
    }
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
