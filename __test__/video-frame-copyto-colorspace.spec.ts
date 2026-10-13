/**
 * VideoFrame.copyTo({ format: 'RGBA' | 'RGBX' | 'BGRA' | 'BGRX', colorSpace }) colour conversion.
 *
 * fixtures/copyto-chrome155.json holds real Chrome 155 output for solid 8x8 frames (one RGBA pixel
 * per source and target colour space) and for random HDR pixels.
 */

import test from 'ava'
import { readFileSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

import {
  ImageDecoder,
  Mp4Demuxer,
  VideoDecoder,
  VideoFrame,
  resetHardwareFallbackState,
  type EncodedVideoChunk,
  type PredefinedColorSpace,
  type VideoColorSpaceInit,
  type VideoFrameCopyToOptions,
  type VideoPixelFormat,
} from '../index.js'
import { i420ToRgba, LIBYUV, libyuvTableFor, yuvToRgba, type Rect } from './helpers/libyuv.js'

const __dirname = dirname(fileURLToPath(import.meta.url))

interface ChromeFixture {
  chrome: string
  frame: { width: number; height: number }
  tags: Record<string, VideoColorSpaceInit>
  /** "SRCFMT|tag|fill|s or p|R,G,B,A": tag 'null' = colorSpace omitted, s = srgb, p = display-p3 */
  rows: string[]
  /** [Y, U, V, R, G, B] per 2x2 block */
  hdrRandom: Array<{
    format: VideoPixelFormat
    colorSpace: VideoColorSpaceInit
    target: PredefinedColorSpace
    px: number[][]
  }>
}

const fixture: ChromeFixture = JSON.parse(readFileSync(join(__dirname, 'fixtures/copyto-chrome155.json'), 'utf8'))

const RGB_FORMATS = ['RGBA', 'BGRA', 'RGBX', 'BGRX'] as const

test.beforeEach(() => {
  resetHardwareFallbackState()
})

const isRgb = (format: string) => /^(RGB|BGR)/.test(format)
const le16 = (samples: ArrayLike<number>) => new Uint8Array(Uint16Array.from(samples).buffer)

async function copy(frame: VideoFrame, options: VideoFrameCopyToOptions): Promise<Uint8Array> {
  const out = new Uint8Array(frame.allocationSize(options))
  await frame.copyTo(out, options)
  return out
}

/** Index of the first differing byte, or -1. */
function firstMismatch(got: Uint8Array, want: Uint8Array): number {
  if (got.length !== want.length) return Math.min(got.length, want.length)
  return got.findIndex((v, i) => v !== want[i])
}

/** Solid frame data: RGB bytes as stored, or Y, U, V (, A) samples. */
function solidFrameData(format: string, fill: number[], width: number, height: number): Uint8Array {
  const luma = width * height
  if (isRgb(format)) {
    const data = new Uint8Array(luma * 4)
    for (let i = 0; i < luma; i++) data.set(fill, 4 * i)
    return data
  }
  const [y, u, v, a] = fill
  const quarter = (width / 2) * (height / 2)
  const plane = (value: number, n: number) => Array.from({ length: n }, () => value)
  switch (format) {
    case 'NV12':
      return Uint8Array.from([...plane(y, luma), ...plane(0, quarter).flatMap(() => [u, v])])
    case 'I420P10':
      return le16([...plane(y, luma), ...plane(u, quarter), ...plane(v, quarter)])
    case 'I420A':
      return Uint8Array.from([...plane(y, luma), ...plane(u, quarter), ...plane(v, quarter), ...plane(a, luma)])
    default: {
      const chroma = format === 'I444' ? luma : format === 'I422' ? luma / 2 : quarter
      return Uint8Array.from([...plane(y, luma), ...plane(u, chroma), ...plane(v, chroma)])
    }
  }
}

// ============================================================================
// Chrome 155 oracle
// ============================================================================

const groups = new Map<string, string[]>()
for (const row of fixture.rows) {
  const [format, tag] = row.split('|')
  const key = `${format}/${tag}`
  groups.set(key, [...(groups.get(key) ?? []), row])
}

for (const [group, rows] of groups) {
  test(`copyTo matches Chrome ${fixture.chrome}: ${group}`, async (t) => {
    const { width, height } = fixture.frame
    const failures: string[] = []
    for (const row of rows) {
      const [format, tag, fill, target, pixel] = row.split('|')
      const px = pixel.split(',').map(Number)
      const tolerance = tag === 'pq' || tag === 'hlg' ? 2 : 1
      const frame = new VideoFrame(solidFrameData(format, fill.split(',').map(Number), width, height), {
        format: format as VideoPixelFormat,
        codedWidth: width,
        codedHeight: height,
        timestamp: 0,
        ...(tag === 'null' ? {} : { colorSpace: fixture.tags[tag] }),
      })
      const targets: Array<PredefinedColorSpace | undefined> = target === 'p' ? ['display-p3'] : [undefined, 'srgb']
      try {
        for (const dst of RGB_FORMATS) {
          const want = dst.startsWith('BGR') ? [px[2], px[1], px[0], px[3]] : px
          for (const colorSpace of targets) {
            const label = `${row} -> ${dst} ${colorSpace ?? '(colorSpace omitted)'}`
            let out: Uint8Array
            try {
              out = await copy(frame, { format: dst, ...(colorSpace ? { colorSpace } : {}) })
            } catch (e) {
              failures.push(`${label}: threw ${(e as Error).message}`)
              continue
            }
            let worst = 0
            for (let i = 0; i < out.length; i++) {
              const c = i % 4
              worst = Math.max(worst, c === 3 ? (out[i] === want[3] ? 0 : Infinity) : Math.abs(out[i] - want[c]))
            }
            if (worst > tolerance)
              failures.push(`${label}: got ${out.subarray(0, 4).join(',')}, want ${want.join(',')}`)
          }
        }
      } finally {
        frame.close()
      }
    }
    t.deepEqual(failures, [])
  })
}

test(`copyTo matches Chrome ${fixture.chrome} on random HDR pixels`, async (t) => {
  const failures: string[] = []
  for (const c of fixture.hdrRandom) {
    // pixel i of the table is the 2x2 block at x = 2i, so every block has its own chroma sample
    const n = c.px.length
    const width = 2 * n
    const y = Array.from({ length: width * 2 }, () => 0)
    c.px.forEach(([Y], i) => {
      y[2 * i] = y[2 * i + 1] = y[width + 2 * i] = y[width + 2 * i + 1] = Y
    })
    const u = c.px.map((p) => p[1])
    const v = c.px.map((p) => p[2])
    const data = c.format === 'I420P10' ? le16([...y, ...u, ...v]) : Uint8Array.from([...y, ...u, ...v])
    const frame = new VideoFrame(data, {
      format: c.format,
      codedWidth: width,
      codedHeight: 2,
      timestamp: 0,
      colorSpace: c.colorSpace,
    })
    const out = await copy(frame, { format: 'RGBA', colorSpace: c.target })
    frame.close()
    c.px.forEach(([, , , r, g, b], i) => {
      for (const o of [8 * i, 8 * i + 4, 4 * width + 8 * i, 4 * width + 8 * i + 4]) {
        const diff = Math.max(Math.abs(out[o] - r), Math.abs(out[o + 1] - g), Math.abs(out[o + 2] - b))
        if (diff > 2 || out[o + 3] !== 255) {
          failures.push(`${c.format} ${JSON.stringify(c.colorSpace)} -> ${c.target} block ${i}: diff ${diff}`)
          break
        }
      }
    })
  }
  t.deepEqual(failures, [])
})

// ============================================================================
// YUV -> R'G'B' stage on non-solid frames (libyuv integer math, nearest chroma)
// ============================================================================

/** primaries bt709 + an sRGB-curve transfer -> no colour change after the YUV stage */
const LIBYUV_TAGS: Record<string, VideoColorSpaceInit> = {
  H709: { primaries: 'bt709', transfer: 'bt709', matrix: 'bt709', fullRange: false },
  F709: { primaries: 'bt709', transfer: 'bt709', matrix: 'bt709', fullRange: true },
  I601: { primaries: 'bt709', transfer: 'smpte170m', matrix: 'smpte170m', fullRange: false },
  JPEG: { primaries: 'bt709', transfer: 'smpte170m', matrix: 'bt470bg', fullRange: true },
  B2020: { primaries: 'bt709', transfer: 'bt709', matrix: 'bt2020-ncl', fullRange: false },
  V2020: { primaries: 'bt709', transfer: 'bt709', matrix: 'bt2020-ncl', fullRange: true },
}

const STAGE1_FORMATS = {
  I420: { hs: 1, vs: 1, bits: 8 },
  I422: { hs: 1, vs: 0, bits: 8 },
  I444: { hs: 0, vs: 0, bits: 8 },
  NV12: { hs: 1, vs: 1, bits: 8 },
  I420A: { hs: 1, vs: 1, bits: 8 },
  I420P10: { hs: 1, vs: 1, bits: 10 },
} as const

const W = 64
const H = 32
const ODD_RECT: Rect = { x: 6, y: 2, width: 13, height: 9 }

/** Seeded random 64x32 frame data plus the planes the reference reads. */
function randomYuv(format: keyof typeof STAGE1_FORMATS, seed: number) {
  const { hs, vs, bits } = STAGE1_FORMATS[format]
  const random = (max: number) => () => {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0
    return (seed >>> 16) % (max + 1)
  }
  const sample = random((1 << bits) - 1)
  const chroma = (W >> hs) * (H >> vs)
  const y = Array.from({ length: W * H }, sample)
  const u = Array.from({ length: chroma }, sample)
  const v = Array.from({ length: chroma }, sample)
  const a = format === 'I420A' ? Array.from({ length: W * H }, random(255)) : undefined
  let data: Uint8Array
  if (format === 'NV12') data = Uint8Array.from([...y, ...u.flatMap((s, i) => [s, v[i]])])
  else if (bits === 10) data = le16([...y, ...u, ...v])
  else data = Uint8Array.from([...y, ...u, ...v, ...(a ?? [])])
  return { data, planes: { width: W, hs, vs, bits, y, u, v, a } }
}

for (const format of Object.keys(STAGE1_FORMATS) as Array<keyof typeof STAGE1_FORMATS>) {
  test(`copyTo RGBA from random ${format} is libyuv-exact for every matrix`, async (t) => {
    const { data, planes } = randomYuv(format, 1)
    const failures: string[] = []
    for (const [name, colorSpace] of Object.entries(LIBYUV_TAGS)) {
      const frame = new VideoFrame(data, { format, codedWidth: W, codedHeight: H, timestamp: 0, colorSpace })
      for (const rect of [{ x: 0, y: 0, width: W, height: H }, ODD_RECT]) {
        const got = await copy(frame, { format: 'RGBA', rect })
        const at = firstMismatch(got, yuvToRgba(planes, libyuvTableFor(colorSpace), rect))
        if (at >= 0) failures.push(`${name} ${JSON.stringify(rect)}: first difference at byte ${at}`)
      }
      frame.close()
    }
    t.deepEqual(failures, [])
  })
}

test('copyTo RGBA writes rows at the layout offset and stride', async (t) => {
  const { data, planes } = randomYuv('I420', 2)
  const colorSpace = LIBYUV_TAGS.H709
  const frame = new VideoFrame(data, { format: 'I420', codedWidth: W, codedHeight: H, timestamp: 0, colorSpace })
  const layout = [{ offset: 8, stride: ODD_RECT.width * 4 + 12 }]
  const out = new Uint8Array(frame.allocationSize({ format: 'RGBA', rect: ODD_RECT, layout }))
  t.deepEqual(await frame.copyTo(out, { format: 'RGBA', rect: ODD_RECT, layout }), layout)
  frame.close()
  const want = yuvToRgba(planes, libyuvTableFor(colorSpace), ODD_RECT)
  const row = ODD_RECT.width * 4
  for (let r = 0; r < ODD_RECT.height; r++) {
    const start = layout[0].offset + r * layout[0].stride
    t.is(firstMismatch(out.subarray(start, start + row), want.subarray(r * row, (r + 1) * row)), -1, `row ${r}`)
  }
})

// ============================================================================
// Regressions: frames the first version of the colorSpace path rejected or got wrong
// ============================================================================

const RGB_TAGS: Record<string, VideoColorSpaceInit> = {
  empty: {},
  partial: { primaries: 'bt709' },
  // WPT's RGBX frames: matrix and range are ignored for RGB sources
  wptBt709Limited: { primaries: 'bt709', transfer: 'bt709', matrix: 'bt709', fullRange: false },
}

test('copyTo between RGB formats is an exact copy or swizzle for sRGB-like tags', async (t) => {
  const source = Uint8Array.from({ length: 4 * 4 * 4 }, (_, i) => (i * 37 + 11) & 255)
  // the same pixels with R and B swapped
  const swizzled = source.map((v, i) => (i % 4 === 0 ? source[i + 2] : i % 4 === 2 ? source[i - 2] : v))
  for (const [name, colorSpace] of Object.entries(RGB_TAGS)) {
    for (const format of RGB_FORMATS) {
      const frame = new VideoFrame(source, { format, codedWidth: 4, codedHeight: 4, timestamp: 0, colorSpace })
      for (const dst of RGB_FORMATS) {
        const sameOrder = dst.startsWith('BGR') === format.startsWith('BGR')
        const want = sameOrder ? source : swizzled
        t.is(firstMismatch(await copy(frame, { format: dst }), want), -1, `${name}: ${format} -> ${dst}`)
      }
      frame.close()
    }
  }
})

test('copyTo of a GIF frame to its own format equals the plain copy', async (t) => {
  const decoder = new ImageDecoder({ data: readFileSync(join(__dirname, 'fixtures/animated.gif')), type: 'image/gif' })
  const { image } = await decoder.decode()
  try {
    const plain = await copy(image, {})
    t.is(firstMismatch(await copy(image, { format: image.format! }), plain), -1, `format ${image.format}`)
  } finally {
    image.close()
    decoder.close()
  }
})

test('copyTo RGBA of a grayscale PNG keeps the gray level', async (t) => {
  const decoder = new ImageDecoder({ data: readFileSync(join(__dirname, 'fixtures/gray.png')), type: 'image/png' })
  const { image } = await decoder.decode()
  try {
    const out = await copy(image, { format: 'RGBA' })
    t.deepEqual(Array.from(out), Array.from({ length: 16 }, () => [64, 64, 64, 255]).flat())
  } finally {
    image.close()
    decoder.close()
  }
})

test('copyTo RGBA of a PQ frame is tone mapped instead of rejected', async (t) => {
  const frame = new VideoFrame(new Uint8Array(24).fill(128), {
    format: 'I420',
    codedWidth: 4,
    codedHeight: 4,
    timestamp: 0,
    colorSpace: { primaries: 'bt2020', transfer: 'pq', matrix: 'bt2020-ncl', fullRange: false },
  })
  const out = await copy(frame, { format: 'RGBA' })
  frame.close()
  t.deepEqual(Array.from(out), Array.from({ length: 16 }, () => [137, 137, 137, 255]).flat())
})

test('copyTo RGBA saturates 10-bit luma and alpha samples above 1023', async (t) => {
  // a JS buffer can hold any 16-bit sample; out-of-range values must clamp, not wrap
  for (const format of ['I420P10', 'I420AP10'] as const) {
    for (const value of [1024, 65535]) {
      const y = [value, value, value, value]
      const frame = new VideoFrame(le16([...y, 512, 512, ...(format === 'I420AP10' ? y : [])]), {
        format,
        codedWidth: 2,
        codedHeight: 2,
        timestamp: 0,
        colorSpace: LIBYUV_TAGS.H709,
      })
      const out = await copy(frame, { format: 'RGBA' })
      frame.close()
      t.deepEqual(Array.from(out), Array.from({ length: 4 }, () => [255, 255, 255, 255]).flat(), `${format} ${value}`)
    }
  }
})

/** First frame of an MP4 fixture, decoded in software. */
async function decodeFirstFrame(file: string): Promise<VideoFrame> {
  const errors: Error[] = []
  const chunks: EncodedVideoChunk[] = []
  const demuxer = new Mp4Demuxer({ videoOutput: (chunk) => chunks.push(chunk), error: (e) => errors.push(e) })
  await demuxer.load(join(__dirname, 'fixtures', file))
  const config = demuxer.videoDecoderConfig!
  await demuxer.demuxAsync()
  demuxer.close()
  const frames: VideoFrame[] = []
  const decoder = new VideoDecoder({ output: (f) => frames.push(f), error: (e) => errors.push(e) })
  decoder.configure({ ...config, hardwareAcceleration: 'prefer-software' })
  for (const chunk of chunks) decoder.decode(chunk)
  await decoder.flush()
  decoder.close()
  for (const f of frames.slice(1)) f.close()
  if (errors.length > 0 || frames.length === 0) {
    frames[0]?.close()
    throw errors[0] ?? new Error(`no frame decoded from ${file}`)
  }
  return frames[0]
}

test('copyTo RGBA of an interlaced H.264 frame matches libyuv I601', async (t) => {
  const frame = await decodeFirstFrame('wpt/h264_interlaced.mp4')
  try {
    const { codedWidth: width, codedHeight: height } = frame
    const full = { x: 0, y: 0, width, height }
    t.is(libyuvTableFor(frame.colorSpace), LIBYUV.I601)
    const planes = await copy(frame, { rect: full })
    for (const rect of [full, { x: 4, y: 2, width: 13, height: 7 }]) {
      const got = await copy(frame, { format: 'RGBA', rect })
      t.is(firstMismatch(got, i420ToRgba(planes, width, height, LIBYUV.I601, rect)), -1, JSON.stringify(rect))
    }
  } finally {
    frame.close()
  }
})

test('copyTo honors the Display P3 tag of decoded video', async (t) => {
  const frame = await decodeFirstFrame('h264_p3.mp4')
  try {
    const p3 = await copy(frame, { format: 'RGBA', colorSpace: 'display-p3' })
    const srgb = await copy(frame, { format: 'RGBA' })
    // the P3 copy is the YUV stage only; converting it to sRGB must give the sRGB copy exactly
    const p3Frame = new VideoFrame(p3, {
      format: 'RGBA',
      codedWidth: frame.visibleRect.width,
      codedHeight: frame.visibleRect.height,
      timestamp: 0,
      colorSpace: { primaries: 'smpte432', transfer: 'iec61966-2-1', matrix: 'rgb', fullRange: true },
    })
    const converted = await copy(p3Frame, { format: 'RGBA' })
    p3Frame.close()
    t.is(firstMismatch(converted, srgb), -1)
    t.not(firstMismatch(p3, srgb), -1, 'display-p3 and srgb copies should differ')
  } finally {
    frame.close()
  }
})

test('concurrent copyTo calls give the same bytes as sequential ones', async (t) => {
  const w = 320
  const h = 180
  const i420 = (colorSpace: VideoColorSpaceInit) =>
    new VideoFrame(
      Uint8Array.from({ length: (w * h * 3) / 2 }, (_, i) => 16 + ((i * 7) % 220)),
      { format: 'I420', codedWidth: w, codedHeight: h, timestamp: 0, colorSpace },
    )
  const p10 = (colorSpace: VideoColorSpaceInit) =>
    new VideoFrame(le16(Array.from({ length: (w * h * 3) / 2 }, (_, i) => 64 + ((i * 13) % 876))), {
      format: 'I420P10',
      codedWidth: w,
      codedHeight: h,
      timestamp: 0,
      colorSpace,
    })
  const rgba = (colorSpace?: VideoColorSpaceInit) =>
    new VideoFrame(
      Uint8Array.from({ length: w * h * 4 }, (_, i) => (i * 31) & 255),
      {
        format: 'RGBA',
        codedWidth: w,
        codedHeight: h,
        timestamp: 0,
        ...(colorSpace ? { colorSpace } : {}),
      },
    )
  const frames = [
    i420({ primaries: 'bt709', transfer: 'bt709', matrix: 'bt709', fullRange: false }),
    i420({ primaries: 'bt2020', transfer: 'bt709', matrix: 'bt2020-ncl', fullRange: false }),
    p10({ primaries: 'bt2020', transfer: 'pq', matrix: 'bt2020-ncl', fullRange: false }),
    p10({ primaries: 'bt2020', transfer: 'hlg', matrix: 'bt2020-ncl', fullRange: false }),
    rgba({ primaries: 'smpte432', transfer: 'iec61966-2-1', matrix: 'rgb', fullRange: true }),
    rgba(),
  ]
  const targets = ['srgb', 'display-p3', undefined] as const
  const calls = Array.from({ length: 96 }, (_, k) => {
    const x = (k * 34) % 160
    const y = (k * 18) % 90
    const colorSpace = targets[k % 3]
    const options: VideoFrameCopyToOptions = {
      format: RGB_FORMATS[k % 4],
      ...(colorSpace ? { colorSpace } : {}),
      rect: { x, y, width: 1 + ((k * 23) % 159), height: 1 + ((k * 13) % 89) },
    }
    return [frames[k % frames.length], options] as const
  })
  const sequential: Uint8Array[] = []
  for (const [frame, options] of calls) sequential.push(await copy(frame, options))
  const concurrent = await Promise.all(calls.map(([frame, options]) => copy(frame, options)))
  for (const frame of frames) frame.close()
  const mismatched = calls.flatMap((_, i) => (firstMismatch(concurrent[i], sequential[i]) === -1 ? [] : [i]))
  t.deepEqual(mismatched, [])
})

// ============================================================================
// Cost guard: only the copy rect is converted
// ============================================================================

// QEMU-emulated armv7 timings are not meaningful (same skip as demuxer.spec.ts)
const isLinuxArmv7 = process.platform === 'linux' && process.arch === 'arm'
const testTiming = isLinuxArmv7 ? test.skip : test.serial

testTiming('copyTo RGBA of a 2x2 rect costs far less than the full 4K frame', async (t) => {
  const w = 3840
  const h = 2160
  const data = new Uint8Array((w * h * 3) / 2)
  for (let i = 0; i < data.length; i++) data[i] = 16 + ((i * 7) % 220)
  const frame = new VideoFrame(data, {
    format: 'I420',
    codedWidth: w,
    codedHeight: h,
    timestamp: 0,
    colorSpace: { primaries: 'bt709', transfer: 'bt709', matrix: 'bt709', fullRange: false },
  })
  const full: VideoFrameCopyToOptions = { format: 'RGBA' }
  const rect: VideoFrameCopyToOptions = { format: 'RGBA', rect: { x: 0, y: 0, width: 2, height: 2 } }
  const fullOut = new Uint8Array(frame.allocationSize(full))
  const rectOut = new Uint8Array(frame.allocationSize(rect))
  const time = async (options: VideoFrameCopyToOptions, out: Uint8Array) => {
    const start = process.hrtime.bigint()
    await frame.copyTo(out, options)
    return Number(process.hrtime.bigint() - start) / 1e6
  }
  await time(full, fullOut)
  await time(rect, rectOut)
  // Interleaved samples, so both see the same machine load. Load (other test files, Rosetta for
  // the x64 macOS job) only adds time, mostly to the tiny rect copy, so compare the fastest runs.
  const fullMs: number[] = []
  const rectMs: number[] = []
  for (let i = 0; i < 9; i++) {
    fullMs.push(await time(full, fullOut))
    rectMs.push(await time(rect, rectOut))
  }
  frame.close()
  const fullBest = Math.min(...fullMs)
  const rectBest = Math.min(...rectMs)
  t.log(`best of 9: full 4K ${fullBest.toFixed(2)} ms, 2x2 rect ${rectBest.toFixed(3)} ms`)
  t.true(rectBest < fullBest / 20, 'a 2x2 rect copy must not convert the whole frame')
})
