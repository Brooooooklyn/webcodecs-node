/**
 * ImageDecoder for PNG, APNG and WebP, checked against Chrome 155.
 *
 * fixtures/png-webp-chrome155.json holds what Chrome for Testing 155.0.8059.79 gives for the
 * files it lists (its `about` says how each part was taken); the files are in
 * fixtures/png-webp/, with a note on each in the JSON. The tests give the same bytes the same way
 * and compare. Where our error message is worded differently from Chrome's (a frame index out of
 * range, a closed decoder), only the error's name is compared.
 *
 * No timer races a result: a test awaits the promise it checks, and one that never settles fails
 * through ava's timeout. A timer only shows that something did not happen yet (a promise still
 * pending, a track not changed) while one byte is missing.
 */

import test from 'ava'
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { crc32, deflateSync } from 'node:zlib'

import { ImageDecoder, type VideoFrame } from '../index.js'

const fixtures = join(dirname(fileURLToPath(import.meta.url)), 'fixtures')

type Track = [animated: boolean, frameCount: number, repetitionCount: string] | null
interface ChromeResults {
  files: Record<string, { path: string; type: string; note: string }>
  whole: Record<string, { ready: string; completed: string; track: Track; frames: string[] }>
  sweeps: Record<string, [at: number, ready: string, track: Track][]>
  edges: [name: string, frameIndex: number, at: number | null, result: string | null][]
  ends: [name: string, cut: number, ready: string, completed: string, track: Track, decodes: [number, string][]][]
  afterReady: Record<string, { buffer: string[]; stream: string[] }>
}
const chrome: ChromeResults = JSON.parse(readFileSync(join(fixtures, 'png-webp-chrome155.json'), 'utf8'))

function file(name: string) {
  const { path, type } = chrome.files[name]
  return { data: new Uint8Array(readFileSync(join(fixtures, path))), type }
}

const errorText = (error: unknown) => `${(error as Error).name}: ${(error as Error).message}`

// What a promise has given so far: "pending", "fulfilled" or "<name>: <message>"
function watch(promise: Promise<unknown>) {
  const seen = { value: 'pending' }
  promise.then(
    () => (seen.value = 'fulfilled'),
    (error) => (seen.value = errorText(error)),
  )
  return seen
}

async function settled(promise: Promise<unknown>) {
  try {
    await promise
    return 'fulfilled'
  } catch (error) {
    return errorText(error)
  }
}

function trackOf(decoder: ImageDecoder): Track {
  const track = decoder.tracks.selectedTrack
  return track ? [track.animated, track.frameCount, String(track.repetitionCount)] : null
}

// "<format> <width>x<height> <timestamp> <duration> <first 16 hex digits of the SHA-256 of copyTo()>"
async function describe(image: VideoFrame) {
  const bytes = new Uint8Array(image.allocationSize())
  await image.copyTo(bytes)
  const sha = createHash('sha256').update(bytes).digest('hex').slice(0, 16)
  return `${image.format} ${image.codedWidth}x${image.codedHeight} ${image.timestamp} ${image.duration} ${sha}`
}

async function result(call: Promise<{ image: VideoFrame }>) {
  try {
    const { image } = await call
    try {
      return await describe(image)
    } finally {
      image.close()
    }
  } catch (error) {
    return errorText(error)
  }
}

// `actual` in the form of `expected`: the format alone for "ok <format>", the name alone for an
// error whose message is worded differently here (an expected name with no message)
function like(actual: string, expected: string) {
  if (expected.startsWith('ok ')) return actual.includes(': ') ? actual : `ok ${actual.split(' ')[0]}`
  return expected.includes(' ') ? actual : actual.split(':')[0]
}

// A stream that gives what `send()` gets, and stays open until `close()`
function openStream() {
  let controller!: ReadableStreamDefaultController<Uint8Array>
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      controller = c
    },
  })
  return {
    stream,
    send: (bytes: Uint8Array) => {
      if (bytes.length) controller.enqueue(bytes.slice())
    },
    close: () => {
      try {
        controller.close()
      } catch {}
    },
  }
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 1))

async function until(check: () => boolean) {
  while (!check()) await tick()
}

// Whether `check` holds all through `ms`
async function holds(check: () => boolean, ms: number) {
  const end = performance.now() + ms
  while (performance.now() < end) {
    if (!check()) return false
    await tick()
  }
  return check()
}

test('ImageDecoder: PNG, APNG and WebP from a buffer give the track, frames and errors of Chrome', async (t) => {
  for (const [name, expected] of Object.entries(chrome.whole)) {
    const { data, type } = file(name)
    const decoder = new ImageDecoder({ data, type })
    t.is(await settled(decoder.tracks.ready), expected.ready, name)
    t.is(await settled(decoder.completed), expected.completed, name)
    // Chrome has no track once the metadata failed; here a placeholder stays (all formats)
    if (expected.track) t.deepEqual(trackOf(decoder), expected.track, name)
    const frames: string[] = []
    for (const [frameIndex, want] of expected.frames.entries()) {
      frames.push(like(await result(decoder.decode({ frameIndex })), want))
    }
    t.deepEqual(frames, expected.frames, name)
    // the decodes do not change the track (Chrome drops it when the decoder fails)
    if (expected.track && !decoder.closed) t.deepEqual(trackOf(decoder), expected.track, name)
    decoder.close()
  }
})

test('ImageDecoder: tracks.ready and the PNG or WebP track change at the byte where Chrome changes them, with the stream open', async (t) => {
  for (const [name, steps] of Object.entries(chrome.sweeps)) {
    const { data, type } = file(name)
    const source = openStream()
    const decoder = new ImageDecoder({ data: source.stream, type })
    t.teardown(() => {
      decoder.close()
      source.close()
    })
    const ready = watch(decoder.tracks.ready)
    let sent = 0
    for (let i = 1; i < steps.length; i++) {
      const [, readyBefore, trackBefore] = steps[i - 1]
      const [at, readyAt, trackAt] = steps[i]
      const label = `${name} at ${at} bytes`
      const unchanged = () =>
        ready.value === readyBefore &&
        (trackBefore === null || JSON.stringify(trackOf(decoder)) === JSON.stringify(trackBefore))
      // one byte short
      source.send(data.slice(sent, at - 1))
      sent = at - 1
      t.true(await holds(unchanged, 50), `${label}: nothing changes one byte before`)
      source.send(data.slice(sent, at))
      sent = at
      await until(
        () =>
          ready.value === readyAt && (trackAt === null || JSON.stringify(trackOf(decoder)) === JSON.stringify(trackAt)),
      )
      t.is(ready.value, readyAt, label)
      if (trackAt) t.deepEqual(trackOf(decoder), trackAt, label)
    }
  }
})

test('ImageDecoder: a PNG or WebP decode() settles at the byte where Chrome settles it, with the stream open', async (t) => {
  for (const [name, frameIndex, at, expected] of chrome.edges) {
    const { data, type } = file(name)
    const label = `${name} frame ${frameIndex}`
    const source = openStream()
    const decoder = new ImageDecoder({ data: source.stream, type })
    t.teardown(() => {
      decoder.close()
      source.close()
    })
    if (at === null || expected === null) {
      // Chrome does not settle it before the end of the stream, even with every byte
      source.send(data)
      const call = watch(decoder.decode({ frameIndex }))
      t.true(await holds(() => call.value === 'pending', 100), `${label}: pending with every byte`)
      continue
    }
    source.send(data.slice(0, at - 1))
    const call = decoder.decode({ frameIndex })
    const seen = watch(call)
    t.true(await holds(() => seen.value === 'pending', 50), `${label}: pending one byte before ${at}`)
    source.send(data.slice(at - 1, at))
    // a frame as "<format> <sha>"
    const got = await result(call)
    const frame = !got.includes(': ')
    t.is(frame ? `${got.split(' ')[0]} ${got.split(' ')[4]}` : got, expected, label)
  }
})

test('ImageDecoder: a PNG or WebP stream cut short ends like Chrome', async (t) => {
  for (const [name, cut, ready, completed, track, decodes] of chrome.ends) {
    const { data, type } = file(name)
    const label = `${name} cut at ${cut}`
    const source = openStream()
    source.send(data.slice(0, cut))
    source.close()
    const decoder = new ImageDecoder({ data: source.stream, type })
    t.is(await settled(decoder.tracks.ready), ready, label)
    t.is(await settled(decoder.completed), completed, label)
    // Chrome has no track once the metadata failed; here a placeholder stays (all formats)
    if (track) t.deepEqual(trackOf(decoder), track, label)
    const got: [number, string][] = []
    for (const [frameIndex, want] of decodes) {
      got.push([frameIndex, like(await result(decoder.decode({ frameIndex })), want)])
    }
    t.deepEqual(got, decodes, label)
    decoder.close()
  }
})

test('ImageDecoder: a PNG or WebP decode() made right after tracks.ready gives the frames of Chrome, from a buffer and from one chunk on an open stream', async (t) => {
  // The first decode() is made where `await tracks.ready` resumes, each next one once the last
  // settled. On the open stream, no decode() past the last frame: Chrome waits for more data.
  const frames = async (decoder: ImageDecoder, expected: string[]) => {
    await decoder.tracks.ready
    const got: string[] = []
    for (const [frameIndex, want] of expected.entries()) {
      got.push(like(await result(decoder.decode({ frameIndex })), want))
    }
    return got
  }
  for (const [name, expected] of Object.entries(chrome.afterReady)) {
    const { data, type } = file(name)
    const buffered = new ImageDecoder({ data, type })
    t.deepEqual(await frames(buffered, expected.buffer), expected.buffer, `${name} buffer`)
    buffered.close()

    const source = openStream()
    t.teardown(source.close)
    source.send(data)
    const streamed = new ImageDecoder({ data: source.stream, type })
    t.deepEqual(await frames(streamed, expected.stream), expected.stream, `${name} stream`)
    streamed.close()
  }
})

test('ImageDecoder: reset() keeps the PNG and WebP track, and preferAnimation changes nothing, like Chrome', async (t) => {
  const cases: [string, Track][] = [
    ['anim_lossy_opaque.webp', [true, 3, 'Infinity']],
    ['apng-3f-inf.png', [true, 3, 'Infinity']],
    ['four-colors.webp', [false, 1, '0']],
  ]
  for (const [name, track] of cases) {
    const { data, type } = file(name)
    const source = openStream()
    source.send(data)
    source.close()
    const decoder = new ImageDecoder({ data: source.stream, type })
    await decoder.tracks.ready
    await decoder.completed
    t.deepEqual(trackOf(decoder), track, name)
    decoder.reset()
    t.deepEqual(trackOf(decoder), track, `${name} after reset()`)
    const { image } = await decoder.decode({ frameIndex: 0 })
    t.is(image.format, 'BGRX', name)
    image.close()
    t.deepEqual(trackOf(decoder), track, `${name} after the next decode()`)
    decoder.close()

    if (!track![0]) continue
    for (const preferAnimation of [false, true, undefined]) {
      const decoder = new ImageDecoder({ data, type, ...(preferAnimation === undefined ? {} : { preferAnimation }) })
      await decoder.tracks.ready
      t.is(decoder.tracks.length, 1)
      t.deepEqual(trackOf(decoder), track, `${name} preferAnimation ${preferAnimation}`)
      const { image } = await decoder.decode({ frameIndex: 1 })
      t.deepEqual([image.format, image.timestamp], ['BGRX', 100_000], `${name} preferAnimation ${preferAnimation}`)
      image.close()
      decoder.close()
    }
  }
})

test('ImageDecoder: a WebP frame that fails closes the decoder at once, and rejects completed on an open stream, like Chrome', async (t) => {
  // frame 4's VP8L data is broken; frames 0 to 3 are whole at 650 bytes
  const { data, type } = file('anim-mux-bad4body.webp')
  const source = openStream()
  t.teardown(source.close)
  const decoder = new ImageDecoder({ data: source.stream, type })
  source.send(data.slice(0, 650))
  const first = await decoder.decode({ frameIndex: 0 })
  const fourth = await decoder.decode({ frameIndex: 3 })
  t.deepEqual(
    [first.image.format, first.image.timestamp, fourth.image.format, fourth.image.timestamp],
    ['BGRA', 0, 'BGRA', 200_000],
  )
  first.image.close()
  fourth.image.close()
  t.deepEqual(trackOf(decoder), [true, 4, '2'])
  const completed = decoder.completed
  source.send(data.slice(650))
  t.is(await result(decoder.decode({ frameIndex: 4 })), 'EncodingError: Failed to decode frame at index 4')
  t.is(await settled(completed), 'AbortError: Aborted by close.')
  t.is(like(await result(decoder.decode({ frameIndex: 0 })), 'InvalidStateError'), 'InvalidStateError')
})

test('ImageDecoder: after a WebP error found past the header, decode() waits for the end of the stream and then fails, like Chrome', async (t) => {
  // frame 1's VP8L signature is broken: the demuxer fails once it reaches frame 1
  const { data, type } = file('anim-mux-bad1.webp')
  const source = openStream()
  t.teardown(source.close)
  const decoder = new ImageDecoder({ data: source.stream, type })
  source.send(data.slice(0, 128))
  const { image } = await decoder.decode({ frameIndex: 0 })
  t.deepEqual([image.format, image.timestamp], ['BGRA', 0])
  image.close()
  t.deepEqual(trackOf(decoder), [true, 1, '2'])
  source.send(data.slice(128))
  const next = decoder.decode({ frameIndex: 1 })
  const seen = watch(next)
  t.true(await holds(() => seen.value === 'pending', 100), 'frame 1 waits with every byte')
  t.deepEqual(trackOf(decoder), [true, 1, '2'])
  const completed = decoder.completed
  source.close()
  t.is(await result(next), 'EncodingError: Failed to decode frame at index 1')
  t.is(await settled(completed), 'fulfilled')
  t.is(like(await result(decoder.decode({ frameIndex: 0 })), 'InvalidStateError'), 'InvalidStateError')
})

test('ImageDecoder: an APNG error past the header ends the frame list, and frames after it fail at the end of the stream, like Chrome', async (t) => {
  // the fcTL of frame 3 has a bad CRC: Chrome counts 3 frames
  const { data, type } = file('apng-blend-badfctlcrc3.png')
  const source = openStream()
  t.teardown(source.close)
  const decoder = new ImageDecoder({ data: source.stream, type })
  source.send(data)
  const { image } = await decoder.decode({ frameIndex: 2 })
  t.deepEqual([image.format, image.timestamp], ['BGRA', 130_000])
  image.close()
  const later = [3, 4, 5].map((frameIndex) => decoder.decode({ frameIndex }))
  const calls = later.map(watch)
  t.true(await holds(() => calls.every(({ value }) => value === 'pending'), 100), 'they wait with every byte')
  t.deepEqual(trackOf(decoder), [true, 3, 'Infinity'])
  source.close()
  for (const call of later) t.is(like(await result(call), 'RangeError'), 'RangeError')
  t.deepEqual(trackOf(decoder), [true, 3, 'Infinity'])

  // frame 1 does not inflate: its decode() waits for the end, then fails alone
  const broken = file('apng-blend-corrupt1.png')
  const stream = openStream()
  t.teardown(stream.close)
  const other = new ImageDecoder({ data: stream.stream, type: broken.type })
  stream.send(broken.data.slice(0, 2175))
  const call = other.decode({ frameIndex: 1 })
  const seen = watch(call)
  t.true(await holds(() => seen.value === 'pending', 100), 'frame 1 waits at 2175 bytes')
  stream.send(broken.data.slice(2175))
  t.true(await holds(() => seen.value === 'pending', 100), 'frame 1 waits with every byte')
  stream.close()
  t.is(await result(call), "RangeError: Unexpected end of image. Request for frame index 1 can't be satisfied.")
  other.close()
})

// Chrome decodes YUV planes only once all data is here, and not after an RGB decode of part of the
// data (image_decoder_core.cc, have_completed_rgb_decode_; reset() keeps it)
test('ImageDecoder: a simple lossy WebP decodes to I420 from whole data, and to BGRX for good once a decode() ran on part of it, like Chrome', async (t) => {
  const { data, type } = file('webp-valid.webp')
  const frame = async (decoder: ImageDecoder) => {
    const { image } = await decoder.decode()
    const format = image.format
    image.close()
    return format
  }

  // the whole file, then the end: I420, also after reset()
  const whole = openStream()
  whole.send(data)
  whole.close()
  const decoder = new ImageDecoder({ data: whole.stream, type })
  await decoder.completed
  t.is(await frame(decoder), 'I420')
  decoder.reset()
  t.is(await frame(decoder), 'I420')
  decoder.close()

  // a decode() made at 30 bytes (the header): BGRX, and BGRX after the end and after reset()
  const part = openStream()
  t.teardown(part.close)
  const early = new ImageDecoder({ data: part.stream, type })
  part.send(data.slice(0, 30))
  await early.tracks.ready
  const first = early.decode()
  part.send(data.slice(30))
  const { image } = await first
  t.is(image.format, 'BGRX')
  image.close()
  part.close()
  await early.completed
  t.is(await frame(early), 'BGRX')
  early.reset()
  t.is(await frame(early), 'BGRX')
  early.close()
})

// Two decode() calls of frame 0 on an open PNG or WebP stream, among these steps: "decode" (a
// call), "all" (the whole file in one chunk), "head" and "rest" (the file in two chunks, cut in its
// middle), "idle" (a later task, from setImmediate: the stream is idle). Steps with no "idle"
// between them run in one task. The stream stays open until both calls settle.
async function twoCalls(name: string, steps: string[]) {
  const { data, type } = file(name)
  const cut = data.length >> 1
  const source = openStream()
  const decoder = new ImageDecoder({ data: source.stream, type })
  const calls: Promise<string>[] = []
  for (const step of steps) {
    if (step === 'decode') calls.push(result(decoder.decode()))
    if (step === 'all') source.send(data)
    if (step === 'head') source.send(data.subarray(0, cut))
    if (step === 'rest') source.send(data.subarray(cut))
    if (step === 'idle') await new Promise((resolve) => setImmediate(resolve))
  }
  const got = await Promise.all(calls)
  source.close()
  decoder.close()
  return got
}

// A decode() made before the data, or with it, on an open stream resolves once the read of the
// chunk that holds its frame is done, also while another call waits: both calls get the frame
// that Chrome 155 gives with every byte and the stream open. Among them: a decode() on the empty
// stream, then in a later task a second one and the file, in one chunk (in either order) or in two
// chunks (the call after them or between them), and two calls with the whole file between them, in
// one task. A call that never settles fails through ava's timeout.
test('ImageDecoder: two PNG or WebP decode() calls made before or with the data on an open stream both resolve once it is read', async (t) => {
  const cases: [string, string[]][] = [
    ['A; idle; B, all', ['decode', 'idle', 'decode', 'all']],
    ['A; idle; all, B', ['decode', 'idle', 'all', 'decode']],
    ['A, all, B', ['decode', 'all', 'decode']],
    ['idle; A, all, B', ['idle', 'decode', 'all', 'decode']],
    ['A; idle; head, rest, B', ['decode', 'idle', 'head', 'rest', 'decode']],
    ['A; idle; head, B, rest', ['decode', 'idle', 'head', 'decode', 'rest']],
  ]
  for (const name of ['four-colors.png', 'apng-3f-inf.png', 'four-colors.webp', 'webp-valid.webp', 'anim_lossy_opaque.webp']) {
    const [frame] = chrome.afterReady[name].stream
    for (const [label, steps] of cases) {
      t.deepEqual(await twoCalls(name, steps), [frame, frame], `${name} ${label}`)
    }
  }
})

// A VP8X still WebP cut at its image chunk header, or a few bytes after it, before the frame header
// is whole: libwebp's demuxer has the frame there with frame number 0, so Blink finds no frame 1
// (WebPDemuxGetFrame). Chrome fails a decode() made then, and the decoder with it; with no decode()
// then, the frame decodes once the rest comes. The results of Chrome 155 (3 runs) are the frame of
// the whole file, and for the decode() at the cut, the error below.
test('ImageDecoder: a VP8X still WebP split at its image chunk header decodes once the rest comes, and fails a decode() made at the cut, like Chrome', async (t) => {
  const cases: [string, number[]][] = [
    // VP8X, then a VP8 chunk at 30: its frame header is whole at 48 bytes
    ['craft_vp8x_opaque_33x17.webp', [38, 39, 42, 47]],
    // VP8X, then a VP8L chunk at 30: its header is whole at 43 bytes, its frame at 47
    ['vp8x-vp8l.webp', [38, 39, 42]],
  ]
  for (const [name, cuts] of cases) {
    const { data, type } = file(name)
    const [frame] = chrome.whole[name].frames
    for (const cut of cuts) {
      const label = `${name} cut at ${cut}`
      // the first chunk is read alone: tracks.ready resolves before the rest is sent
      const later = openStream()
      t.teardown(later.close)
      const decoder = new ImageDecoder({ data: later.stream, type })
      later.send(data.slice(0, cut))
      await decoder.tracks.ready
      later.send(data.slice(cut))
      later.close()
      t.is(await settled(decoder.completed), 'fulfilled', label)
      t.is(await result(decoder.decode()), frame, label)
      decoder.close()

      const early = openStream()
      t.teardown(early.close)
      const failing = new ImageDecoder({ data: early.stream, type })
      early.send(data.slice(0, cut))
      await failing.tracks.ready
      t.is(await result(failing.decode()), 'EncodingError: Failed to decode frame at index 0', label)
      early.send(data.slice(cut))
      early.close()
      t.is(await settled(failing.completed), 'AbortError: Aborted by close.', label)
      t.is(like(await result(failing.decode()), 'InvalidStateError'), 'InvalidStateError', label)
    }
  }
})

// A chunk with its 4-byte tag, its size, and its pad byte
function webpChunk(tag: string, payload: Uint8Array) {
  const out = new Uint8Array(8 + payload.length + (payload.length & 1))
  out.set(Buffer.from(tag, 'latin1'))
  new DataView(out.buffer).setUint32(4, payload.length, true)
  out.set(payload, 8)
  return out
}

const concat = (...parts: Uint8Array[]) => Uint8Array.from(parts.flatMap((part) => [...part]))

// libwebp reads data that starts with a VP8, VP8L or ALPH chunk header, with no RIFF header, as a
// still image (WebPDemux in its raw mode, ParseHeadersInternal), and Chrome decodes it. Only a
// RIFF file is a simple lossy WebP for Blink: a VP8 chunk alone decodes to BGRX. Data that starts
// with a VP8X chunk has no header for Chrome. Of two ALPH chunks, the last one is the alpha
// (ParseOptionalChunks). The results of Chrome 155, 3 runs, from a buffer and from a stream with
// every byte, then its end.
test('ImageDecoder: a WebP with no RIFF header decodes from its first image chunk, like Chrome', async (t) => {
  const fourColorsWebp = new Uint8Array(readFileSync(join(fixtures, 'wpt/four-colors.webp')))
  const valid = file('webp-valid.webp').data
  const alpha = file('lossy_alpha_33x17.webp').data
  const junk = new Uint8Array(16).fill(0xab)
  // lossy_alpha_33x17.webp's ALPH chunk (at 30) and VP8 chunk (at 360), and ALPH chunks of no
  // filter and no compression with every alpha 0x80, or 0
  const [realAlpha, vp8] = [alpha.subarray(30, 360), alpha.subarray(360)]
  const flatAlpha = (value: number) => webpChunk('ALPH', concat(Uint8Array.of(0), new Uint8Array(33 * 17).fill(value)))
  const cases: [string, Uint8Array, string, string[]][] = [
    // the VP8L chunk
    ['four-colors.webp from byte 12', fourColorsWebp.subarray(12), 'fulfilled', ['BGRX 320x240 0 null da76d824e943b611', 'RangeError']],
    // the VP8 chunk, also with bytes after it
    ['webp-valid.webp from byte 12', valid.subarray(12), 'fulfilled', ['BGRX 64x48 0 null 747f27e26639aadf', 'RangeError']],
    [
      'webp-valid.webp from byte 12, and 16 bytes',
      Uint8Array.from([...valid.subarray(12), ...junk]),
      'fulfilled',
      ['BGRX 64x48 0 null 747f27e26639aadf', 'RangeError'],
    ],
    // the ALPH chunk, then the VP8 chunk
    ['lossy_alpha_33x17.webp from byte 30', alpha.subarray(30), 'fulfilled', ['BGRA 33x17 0 null c107f3facba67155', 'RangeError']],
    // two ALPH chunks, then the VP8 chunk: the second one's alpha
    ['ALPH, ALPH of 0x80, VP8', concat(realAlpha, flatAlpha(0x80), vp8), 'fulfilled', ['BGRA 33x17 0 null 51903e04b07b1d20']],
    ['ALPH of 0x80, ALPH, VP8', concat(flatAlpha(0x80), realAlpha, vp8), 'fulfilled', ['BGRA 33x17 0 null c107f3facba67155']],
    ['ALPH of 0, ALPH of 0x80, VP8', concat(flatAlpha(0), flatAlpha(0x80), vp8), 'fulfilled', ['BGRA 33x17 0 null 51903e04b07b1d20']],
    // the VP8 chunk, 100 bytes short
    [
      'webp-valid.webp from byte 12, 100 bytes short',
      valid.subarray(12, valid.length - 100),
      'fulfilled',
      ['EncodingError: Failed to decode frame at index 0'],
    ],
    // a VP8X chunk first
    [
      'lossy_alpha_33x17.webp from byte 12',
      alpha.subarray(12),
      'InvalidStateError: Failed to retrieve track metadata.',
      ['InvalidStateError'],
    ],
  ]
  for (const [name, data, ready, frames] of cases) {
    for (const streamed of [false, true]) {
      const label = `${name}${streamed ? ', streamed' : ''}`
      const source = openStream()
      source.send(data)
      source.close()
      const decoder = new ImageDecoder({ data: streamed ? source.stream : data, type: 'image/webp' })
      t.is(await settled(decoder.tracks.ready), ready, label)
      t.is(await settled(decoder.completed), 'fulfilled', label)
      if (ready === 'fulfilled') t.deepEqual(trackOf(decoder), [false, 1, '0'], label)
      const got: string[] = []
      for (const [frameIndex, want] of frames.entries()) {
        got.push(like(await result(decoder.decode({ frameIndex })), want))
      }
      t.deepEqual(got, frames, label)
      decoder.close()
    }
  }
})

// Data with no RIFF header whose stream stops inside its image chunk: Chrome's decoder waits for
// the chunk's payload (libwebp's incremental decoder), and the frame decodes once it is here. A
// decode() made at the cut waits; one made after the rest came decodes. The results of Chrome 155,
// 3 runs. Chrome's VP8 decoder can end a byte before the chunk does (at 1123 of webp-valid's 1124
// bytes, at 719 of 720 below); here a VP8 frame waits for its whole payload, as in a RIFF file.
test('ImageDecoder: a WebP with no RIFF header split inside its image chunk decodes once the chunk is here, like Chrome', async (t) => {
  const fourColors = new Uint8Array(readFileSync(join(fixtures, 'wpt/four-colors.webp'))).subarray(12)
  const valid = file('webp-valid.webp').data.subarray(12)
  const alpha = file('lossy_alpha_33x17.webp').data.subarray(30)
  // [name, data, frame, cuts where the chunk's payload is not all here, cut where it is]
  const cases: [string, Uint8Array, string, number[], number][] = [
    // a VP8L chunk of 57 bytes and its pad byte
    ['four-colors.webp from byte 12', fourColors, 'BGRX 320x240 0 null da76d824e943b611', [20, 30, 64], 65],
    // a VP8 chunk of 1116 bytes
    ['webp-valid.webp from byte 12', valid, 'BGRX 64x48 0 null 747f27e26639aadf', [30, 500], 1124],
    // an ALPH chunk of 322 bytes, then a VP8 chunk of 382 bytes at 330
    ['lossy_alpha_33x17.webp from byte 30', alpha, 'BGRA 33x17 0 null c107f3facba67155', [400], 720],
  ]
  for (const [name, data, frame, short, whole] of cases) {
    for (const cut of [...short, whole]) {
      for (const atCut of [false, true]) {
        const label = `${name} cut at ${cut}${atCut ? ', decode() at the cut' : ''}`
        const source = openStream()
        t.teardown(source.close)
        const decoder = new ImageDecoder({ data: source.stream, type: 'image/webp' })
        source.send(data.subarray(0, cut))
        await decoder.tracks.ready
        let early: Promise<string> | null = null
        if (atCut && cut === whole) {
          t.is(await result(decoder.decode()), frame, label)
        } else if (atCut) {
          const call = decoder.decode()
          const seen = watch(call)
          early = result(call)
          t.true(await holds(() => seen.value === 'pending', 50), label)
        }
        source.send(data.subarray(cut))
        source.close()
        t.is(await settled(decoder.completed), 'fulfilled', label)
        if (early) t.is(await early, frame, label)
        t.is(await result(decoder.decode()), frame, label)
        decoder.close()
      }
    }
  }
})

// The png crate 0.18.1 (StreamingDecoder::parse_cicp, parse_bkgd) fails the decoder with an io
// error, not with the format error of an ancillary chunk that it skips, for the first cICP before
// PLTE and IDAT whose full-range flag is over 1 or whose matrix coefficients are not 0, and for the
// first bKGD before IDAT in an indexed image with no PLTE yet. Chrome then rejects the metadata.
// Other bad ancillary chunks are skipped, and the image decodes as it does without them. The files
// are test.png (8x8 RGB) and an 8x8 indexed image, with chunks added; the results of Chrome 155,
// 3 runs, from a buffer and from a stream with every byte, then its end.
test('ImageDecoder: a PNG with a bad cICP, or an indexed PNG with bKGD before PLTE, fails its metadata like Chrome, and other bad ancillary chunks are skipped', async (t) => {
  const chunk = (type: string, payload: number[]) => {
    const body = Buffer.concat([Buffer.from(type, 'latin1'), Buffer.from(payload)])
    const out = Buffer.alloc(body.length + 8)
    out.writeUInt32BE(payload.length, 0)
    body.copy(out, 4)
    out.writeUInt32BE(crc32(body), body.length + 4)
    return out
  }
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])
  const testPng = Buffer.from(readFileSync(join(fixtures, 'test.png')))
  // test.png: IHDR (8 + 25 bytes), IDAT (18 bytes of data), IEND
  const [ihdr, idat, iend] = [testPng.subarray(8, 33), testPng.subarray(33, 63), testPng.subarray(63)]
  const rgb = (...extra: Buffer[]) => new Uint8Array(Buffer.concat([signature, ihdr, ...extra, idat, iend]))
  // 8x8, 8 bits, indexed: two colors in a checkerboard
  const indexedHeader = Buffer.from([0, 0, 0, 8, 0, 0, 0, 8, 8, 3, 0, 0, 0])
  const rows = Buffer.alloc(8 * 9)
  for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) rows[y * 9 + 1 + x] = (x + y) & 1
  const plte = chunk('PLTE', [255, 0, 0, 0, 0, 255])
  const indexedData = Buffer.concat([chunk('IDAT', [...deflateSync(rows)])])
  const indexed = (...extra: Buffer[]) =>
    new Uint8Array(Buffer.concat([signature, chunk('IHDR', [...indexedHeader]), ...extra, indexedData, chunk('IEND', [])]))
  const badCicp = chunk('cICP', [1, 13, 1, 2])
  const failing: [string, Uint8Array][] = [
    ['cICP with a full-range flag of 2', rgb(chunk('cICP', [1, 13, 0, 2]))],
    ['cICP with matrix coefficients 1', rgb(chunk('cICP', [1, 13, 1, 1]))],
    ['indexed, bKGD before PLTE', indexed(chunk('bKGD', [1]), plte)],
    ['indexed, bKGD of 2 bytes before PLTE', indexed(chunk('bKGD', [0, 1]), plte)],
    ['indexed, a bad cICP before PLTE', indexed(badCicp, plte)],
  ]
  const crcBroken = Buffer.from(badCicp)
  crcBroken[crcBroken.length - 1] ^= 1
  const skipped: [string, Uint8Array, Uint8Array][] = [
    ['cICP 1 13 0 1', rgb(chunk('cICP', [1, 13, 0, 1])), rgb()],
    ['cICP 9 16 0 1', rgb(chunk('cICP', [9, 16, 0, 1])), rgb()],
    ['cICP of 5 bytes', rgb(chunk('cICP', [1, 13, 0, 1, 0])), rgb()],
    ['cICP of 3 bytes', rgb(chunk('cICP', [1, 13, 0])), rgb()],
    ['a bad cICP after a good one', rgb(chunk('cICP', [1, 13, 0, 1]), badCicp), rgb()],
    ['a bad cICP after IDAT', new Uint8Array(Buffer.concat([signature, ihdr, idat, badCicp, iend])), rgb()],
    ['a bad cICP with a bad CRC', rgb(crcBroken), rgb()],
    ['bKGD of 2 bytes in an RGB image', rgb(chunk('bKGD', [0, 1])), rgb()],
    ['sRGB 7', rgb(chunk('sRGB', [7])), rgb()],
    ['gAMA 0', rgb(chunk('gAMA', [0, 0, 0, 0])), rgb()],
    ['sBIT 0', rgb(chunk('sBIT', [0, 8, 8])), rgb()],
    ['pHYs with unit 9', rgb(chunk('pHYs', [0, 0, 0, 1, 0, 0, 0, 1, 9])), rgb()],
    ['tRNS of 2 bytes in an RGB image', rgb(chunk('tRNS', [0, 1])), rgb()],
    ['tEXt with no keyword', rgb(chunk('tEXt', [0, 65])), rgb()],
    ['a bad iCCP', rgb(chunk('iCCP', [65, 0, 9, 1, 2])), rgb()],
    ['indexed, bKGD after PLTE', indexed(plte, chunk('bKGD', [1])), indexed(plte)],
    ['indexed, a bad cICP after PLTE', indexed(plte, badCicp), indexed(plte)],
  ]
  const open = (data: Uint8Array, streamed: boolean) => {
    const source = openStream()
    source.send(data)
    source.close()
    return new ImageDecoder({ data: streamed ? source.stream : data, type: 'image/png' })
  }
  for (const streamed of [false, true]) {
    for (const [name, data] of failing) {
      const label = `${name}${streamed ? ', streamed' : ''}`
      const decoder = open(data, streamed)
      t.is(await settled(decoder.tracks.ready), 'InvalidStateError: Failed to retrieve track metadata.', label)
      t.is(await settled(decoder.completed), streamed ? 'AbortError: Aborted by close.' : 'fulfilled', label)
      t.is(like(await result(decoder.decode()), 'InvalidStateError'), 'InvalidStateError', label)
    }
    for (const [name, data, without] of skipped) {
      const label = `${name}${streamed ? ', streamed' : ''}`
      const plain = new ImageDecoder({ data: without, type: 'image/png' })
      const expected = await result(plain.decode())
      plain.close()
      const decoder = open(data, streamed)
      t.is(await settled(decoder.tracks.ready), 'fulfilled', label)
      t.is(await settled(decoder.completed), 'fulfilled', label)
      t.is(await result(decoder.decode()), expected, label)
      t.is(like(await result(decoder.decode({ frameIndex: 1 })), 'RangeError'), 'RangeError', label)
      decoder.close()
    }
  }
  // the frames Chrome gives for the two images
  for (const [data, frame] of [
    [rgb(), 'BGRX 8x8 0 null c262b5c549abc0f3'],
    [indexed(plte), 'BGRX 8x8 0 null b558a9daad5f43d0'],
  ] as const) {
    const decoder = new ImageDecoder({ data, type: 'image/png' })
    t.is(await result(decoder.decode()), frame)
    decoder.close()
  }
})
