/**
 * Odd-size frames keep their last chroma column and row, like Chrome 155.
 *
 * A plane has ceil(width / sample width) columns and ceil(height / sample height) rows
 * (Chromium's VideoFrameLayout), so an I420 3x3 frame has 2x2 U and V planes.
 *
 * fixtures/odd-size-chrome155.json holds what Chrome 155 returns for every case below:
 * allocationSize(), the copyTo() layout and bytes. Buffer frames hold `pattern(format, size)`.
 * The decoded inputs are one 7x5 keyframe per codec (FFmpeg testsrc, libvpx / libvpx-vp9
 * lossless / SVT-AV1) and 7x5 images (avifenc, cwebp, cjpeg).
 */

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import {
  EncodedVideoChunk,
  ImageDecoder,
  VideoDecoder,
  VideoFrame,
  type PlaneLayout,
  type VideoFrameCopyToOptions,
  type VideoPixelFormat,
} from '../index.js'

type Layout = number[][]

interface Decoded {
  name: string
  format: VideoPixelFormat
  allocationSize: number
  layout: Layout
  data: string
}

interface Fixture {
  chrome: string
  frame: { width: number; height: number; sizes: Record<string, number> }
  buffers: Array<{ format: VideoPixelFormat; width: number; height: number; allocationSize: number; layout: Layout }>
  floorStrides: Array<{ format: VideoPixelFormat; width: number; height: number; layout: Layout }>
  paddedLayouts: Array<{
    format: VideoPixelFormat
    width: number
    height: number
    layout: Layout
    input: string
    packed: string
    allocationSize: number
    copied: string
  }>
  /** `error` is Chrome's rejection of a rect that is not sample-aligned */
  rects: Array<
    { format: VideoPixelFormat; rect: number[] } & (
      | { error: string }
      | { error?: undefined; allocationSize: number; layout: Layout; data: string }
    )
  >
  rgb: Array<{ format: VideoPixelFormat; options: VideoFrameCopyToOptions; data: string }>
  discard: Array<{
    format: VideoPixelFormat
    sourceSize: number
    result: VideoPixelFormat
    allocationSize: number
    layout: Layout
  }>
  decoded: Array<Decoded & { codec: string; chunk: string }>
  images: Array<Decoded & { type: string; image: string }>
}

const fixture: Fixture = JSON.parse(
  readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'fixtures/odd-size-chrome155.json'), 'utf8'),
)
const { width: W, height: H } = fixture.frame

const b64 = (s: string) => new Uint8Array(Buffer.from(s, 'base64'))
const toLayout = (layout: Layout): PlaneLayout[] => layout.map(([offset, stride]) => ({ offset, stride }))
const rectOf = ([x, y, width, height]: number[]) => ({ x, y, width, height })

/** 8-bit: (i * 7 + 3) & 255. 10/12-bit: little-endian samples (i * 37 + 5) & max. */
function pattern(format: string, size: number): Uint8Array {
  const out = new Uint8Array(size)
  const bits = format.includes('P12') ? 12 : format.includes('P10') ? 10 : 8
  if (bits === 8) {
    for (let i = 0; i < size; i++) out[i] = (i * 7 + 3) & 255
  } else {
    for (let i = 0; i + 1 < size; i += 2) {
      const v = ((i >> 1) * 37 + 5) & ((1 << bits) - 1)
      out[i] = v & 255
      out[i + 1] = v >> 8
    }
  }
  return out
}

function bufferFrame(format: VideoPixelFormat, width: number, height: number, data: Uint8Array, init = {}) {
  return new VideoFrame(data, { format, codedWidth: width, codedHeight: height, timestamp: 0, ...init })
}

async function copy(frame: VideoFrame, options?: VideoFrameCopyToOptions) {
  const bytes = new Uint8Array(frame.allocationSize(options))
  const layout = await frame.copyTo(bytes, options)
  return { bytes, layout: layout.map((p) => [p.offset, p.stride]) }
}

function maxDiff(a: Uint8Array, b: Uint8Array): number {
  if (a.length !== b.length) return Infinity
  let max = 0
  for (let i = 0; i < a.length; i++) max = Math.max(max, Math.abs(a[i] - b[i]))
  return max
}

// The error class becomes TypeError with the VideoFrame error work; check the message only.
const STRIDE_ERROR = { message: /stride/ }
const ALIGNMENT_ERROR = { message: /aligned/ }

// ============================================================================
// Buffer frames
// ============================================================================

for (const { format, width, height, allocationSize, layout } of fixture.buffers) {
  test(`${format} ${width}x${height}: buffer size, layout and bytes match Chrome`, async (t) => {
    t.throws(() => bufferFrame(format, width, height, new Uint8Array(allocationSize - 1)), {
      instanceOf: TypeError,
    })
    const data = pattern(format, allocationSize)
    const frame = bufferFrame(format, width, height, data)
    t.is(frame.allocationSize(), allocationSize)
    const out = await copy(frame)
    t.deepEqual(out.layout, layout)
    t.deepEqual(out.bytes, data)
    frame.close()
  })
}

// ============================================================================
// Custom layouts
// ============================================================================

for (const { format, width, height, layout } of fixture.floorStrides) {
  test(`${format} ${width}x${height}: a chroma stride rounded down is rejected, like Chrome`, async (t) => {
    const planes = toLayout(layout)
    t.throws(() => bufferFrame(format, width, height, new Uint8Array(4096), { layout: planes }), STRIDE_ERROR)
    const frame = bufferFrame(format, width, height, new Uint8Array(4096))
    t.throws(() => frame.allocationSize({ layout: planes }), STRIDE_ERROR)
    await t.throwsAsync(frame.copyTo(new Uint8Array(4096), { layout: planes }), STRIDE_ERROR)
    frame.close()
  })
}

for (const { format, width, height, layout, input, packed, allocationSize, copied } of fixture.paddedLayouts) {
  test(`${format} ${width}x${height}: padded layouts read and write like Chrome`, async (t) => {
    const planes = toLayout(layout)
    const frame = bufferFrame(format, width, height, b64(input), { layout: planes })
    t.deepEqual((await copy(frame)).bytes, b64(packed))
    t.is(frame.allocationSize({ layout: planes }), allocationSize)
    t.deepEqual((await copy(frame, { layout: planes })).bytes, b64(copied))
    frame.close()
  })
}

// ============================================================================
// Rects at the right and bottom edges and at odd offsets (7x5 frames)
// ============================================================================

for (const expected of fixture.rects) {
  const { format, rect } = expected
  const what = expected.error === undefined ? 'copies' : 'is rejected'
  test(`${format} ${W}x${H}: rect ${rect.join(',')} ${what} like Chrome`, async (t) => {
    const frame = bufferFrame(format, W, H, pattern(format, fixture.frame.sizes[format]))
    const visibleRect = rectOf(rect)
    if (expected.error !== undefined) {
      t.throws(() => frame.allocationSize({ rect: visibleRect }), ALIGNMENT_ERROR)
      t.throws(() => new VideoFrame(frame, { visibleRect }), ALIGNMENT_ERROR)
    } else {
      t.is(frame.allocationSize({ rect: visibleRect }), expected.allocationSize)
      const out = await copy(frame, { rect: visibleRect })
      t.deepEqual(out.layout, expected.layout)
      t.deepEqual(out.bytes, b64(expected.data))
      // A frame whose visibleRect is the rect copies the same planes
      const view = new VideoFrame(frame, { visibleRect })
      t.deepEqual(await copy(view), out)
      view.close()
    }
    frame.close()
  })
}

// ============================================================================
// RGB copies read the last chroma column and row (BT.709 tags, 7x5 frames)
// ============================================================================

for (const { format, options, data } of fixture.rgb) {
  test(`${format} ${W}x${H}: copyTo(${JSON.stringify(options)}) matches Chrome`, async (t) => {
    const colorSpace = { primaries: 'bt709', transfer: 'bt709', matrix: 'bt709', fullRange: false } as const
    const frame = bufferFrame(format, W, H, pattern(format, fixture.frame.sizes[format]), { colorSpace })
    t.deepEqual((await copy(frame, options)).bytes, b64(data))
    frame.close()
  })
}

// ============================================================================
// alpha: 'discard'
// ============================================================================

for (const { format, sourceSize, result, allocationSize, layout } of fixture.discard) {
  test(`${format} ${W}x${H}: alpha 'discard' keeps the chroma planes, like Chrome`, async (t) => {
    const source = pattern(format, sourceSize)
    const frame = bufferFrame(format, W, H, source)
    const opaque = new VideoFrame(frame, { alpha: 'discard' })
    t.is(opaque.format, result)
    t.is(opaque.allocationSize(), allocationSize)
    const out = await copy(opaque)
    t.deepEqual(out.layout, layout)
    // the source without its alpha plane, which comes last
    t.deepEqual(out.bytes, source.subarray(0, allocationSize))
    opaque.close()
    frame.close()
  })
}

// ============================================================================
// Decoded frames (7x5)
// ============================================================================

async function decodeVideo(codec: string, data: Uint8Array): Promise<VideoFrame> {
  const frames: VideoFrame[] = []
  let error: Error | undefined
  const decoder = new VideoDecoder({
    output: (frame) => frames.push(frame),
    error: (e) => {
      error = e
    },
  })
  decoder.configure({ codec, hardwareAcceleration: 'prefer-software' })
  decoder.decode(new EncodedVideoChunk({ type: 'key', timestamp: 0, data }))
  await decoder.flush()
  decoder.close()
  if (error) throw error
  frames.slice(1).forEach((f) => f.close())
  return frames[0]
}

async function decodeImage(data: Uint8Array, type: string): Promise<VideoFrame> {
  const decoder = new ImageDecoder({ data, type })
  const { image } = await decoder.decode()
  decoder.close()
  return image
}

for (const { name, codec, chunk, format, allocationSize, layout, data } of fixture.decoded) {
  test(`VideoDecoder: ${name} ${W}x${H} copies every chroma sample, like Chrome`, async (t) => {
    const frame = await decodeVideo(codec, b64(chunk))
    t.is(frame.format, format)
    t.is(frame.allocationSize(), allocationSize)
    const out = await copy(frame)
    t.deepEqual(out.layout, layout)
    t.deepEqual(out.bytes, b64(data))
    frame.close()
  })
}

for (const { name, type, image, format, allocationSize, layout, data } of fixture.images) {
  test(`ImageDecoder: ${name} ${W}x${H} copies every chroma sample, like Chrome`, async (t) => {
    const frame = await decodeImage(b64(image), type)
    t.is(frame.format, format)
    t.is(frame.allocationSize(), allocationSize)
    const out = await copy(frame)
    t.deepEqual(out.layout, layout)
    // FFmpeg's JPEG IDCT and libjpeg-turbo's differ by 1 on some samples
    t.true(maxDiff(out.bytes, b64(data)) <= (type === 'image/jpeg' ? 1 : 0))
    frame.close()
  })
}
