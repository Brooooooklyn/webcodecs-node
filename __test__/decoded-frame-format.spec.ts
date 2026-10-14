/**
 * Decoded frames carry a WebCodecs format that describes their bytes.
 *
 * FFmpeg decoders also output formats WebCodecs has no name for (rgb24, pal8, gray, monob,
 * ya8, rgb48be, argb, yuvj*, gbrp, ...). Such frames used to be labelled RGBX/BGRX/RGBA/I420,
 * so copyTo() read past the planes (issue #127) or returned wrong bytes, and
 * VideoEncoder.encode() aborted the process (issue #126 A1-A3).
 *
 * Expected formats and bytes are what Chrome 155 returns for the same input
 * (ImageDecoder / VideoDecoder, copyTo() with no options).
 */

import { execFileSync } from 'node:child_process'
import { deflateSync } from 'node:zlib'

import test from 'ava'

import { EncodedVideoChunk, ImageDecoder, VideoDecoder, VideoFrame, type VideoPixelFormat } from '../index.js'

// ============================================================================
// Helpers
// ============================================================================

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
  return c >>> 0
})

function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff] ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

function pngChunk(type: string, data: ArrayLike<number>): Buffer {
  const body = Buffer.concat([Buffer.from(type, 'latin1'), Buffer.from(Uint8Array.from(data))])
  const out = Buffer.alloc(body.length + 8)
  out.writeUInt32BE(body.length - 4, 0)
  body.copy(out, 4)
  out.writeUInt32BE(crc32(body), body.length + 4)
  return out
}

/** PNG with unfiltered rows; `extra` chunks go before IDAT. */
function png(
  width: number,
  height: number,
  colorType: number,
  bitDepth: number,
  rows: number[][],
  extra: [string, number[]][] = [],
): Uint8Array {
  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(width, 0)
  ihdr.writeUInt32BE(height, 4)
  ihdr[8] = bitDepth
  ihdr[9] = colorType
  const raw = Buffer.concat(rows.map((row) => Buffer.from([0, ...row])))
  return new Uint8Array(
    Buffer.concat([
      Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
      pngChunk('IHDR', ihdr),
      ...extra.map(([type, data]) => pngChunk(type, data)),
      pngChunk('IDAT', deflateSync(Buffer.concat([raw]))),
      pngChunk('IEND', []),
    ]),
  )
}

/** Bottom-up BI_RGB BMP. `rows` are top-down, already in file byte order. */
function bmp(width: number, height: number, bpp: number, rows: number[][], palette: number[][] = []): Uint8Array {
  const rowBytes = Math.ceil((width * bpp) / 32) * 4
  const offset = 54 + palette.length * 4
  const buf = Buffer.alloc(offset + rowBytes * height)
  buf.write('BM', 0, 'latin1')
  buf.writeUInt32LE(buf.length, 2)
  buf.writeUInt32LE(offset, 10)
  buf.writeUInt32LE(40, 14)
  buf.writeInt32LE(width, 18)
  buf.writeInt32LE(height, 22)
  buf.writeUInt16LE(1, 26)
  buf.writeUInt16LE(bpp, 28)
  buf.writeUInt32LE(rowBytes * height, 34)
  buf.writeUInt32LE(palette.length, 46)
  palette.forEach(([r, g, b], i) => Buffer.from([b, g, r, 0]).copy(buf, 54 + i * 4))
  rows.forEach((row, y) => Buffer.from(row).copy(buf, offset + (height - 1 - y) * rowBytes))
  return new Uint8Array(buf)
}

const b64 = (s: string) => new Uint8Array(Buffer.from(s, 'base64'))

async function decodeImage(data: Uint8Array, type: string, init: Record<string, unknown> = {}) {
  const decoder = new ImageDecoder({ data, type, ...init })
  const { image } = await decoder.decode()
  decoder.close()
  return image
}

async function rawCopy(frame: VideoFrame) {
  const bytes = new Uint8Array(frame.allocationSize())
  const layout = await frame.copyTo(bytes)
  return { bytes, layout: layout.map((p) => [p.offset, p.stride]) }
}

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

function maxDiff(a: Uint8Array, b: Uint8Array): number {
  if (a.length !== b.length) return Infinity
  let max = 0
  for (let i = 0; i < a.length; i++) max = Math.max(max, Math.abs(a[i] - b[i]))
  return max
}

// Runs `script` in a child Node process so a native crash or abort fails the test
// instead of killing the ava worker. The script loads ./index.js and prints JSON.
function runChild(script: string, extraEnv: NodeJS.ProcessEnv = {}): unknown {
  const env = { ...process.env, ...extraEnv }
  delete env.WEBCODECS_LOG
  const out = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    env,
    timeout: 120_000,
    encoding: 'utf8',
    maxBuffer: 1 << 20,
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  const line = out.split('\n').find((l) => l.startsWith('RESULT '))
  return JSON.parse(line!.slice('RESULT '.length))
}

// Same PNG writer as above, as source text for child processes.
const CHILD_PNG = `
const zlib = require('node:zlib');
const crcTable = Array.from({ length: 256 }, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0 });
const crc32 = (b) => { let c = 0xffffffff; for (const x of b) c = crcTable[(c ^ x) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0 };
const chunk = (type, data) => { const body = Buffer.concat([Buffer.from(type), Buffer.from(data)]); const out = Buffer.alloc(body.length + 8); out.writeUInt32BE(body.length - 4, 0); body.copy(out, 4); out.writeUInt32BE(crc32(body), body.length + 4); return out };
const png = (w, h, colorType, bitDepth, row, plte) => {
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = bitDepth; ihdr[9] = colorType;
  const raw = Buffer.concat(Array.from({ length: h }, () => Buffer.concat([Buffer.from([0]), Buffer.from(row)])));
  return new Uint8Array(Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), ...(plte ? [chunk('PLTE', plte)] : []), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', [])]));
};
`

// ============================================================================
// ImageDecoder: PNG / BMP built in memory (Chrome 155 bytes)
// ============================================================================

const RGB3x2 = [
  [10, 20, 30, 40, 50, 60, 70, 80, 90],
  [1, 2, 3, 4, 5, 6, 7, 8, 9],
]
const RGBW = [255, 0, 0, 0, 255, 0, 0, 0, 255]

const IMAGE_CASES: [string, Uint8Array, string, VideoPixelFormat, number[]][] = [
  // [name, data, type, Chrome format, Chrome copyTo() bytes]
  [
    'RGB PNG 2x1 (rgb24)',
    png(2, 1, 2, 8, [[255, 0, 0, 255, 0, 0]]),
    'image/png',
    'BGRX',
    [0, 0, 255, 255, 0, 0, 255, 255],
  ],
  [
    'RGB PNG 3x2, odd width (rgb24)',
    png(3, 2, 2, 8, RGB3x2),
    'image/png',
    'BGRX',
    [30, 20, 10, 255, 60, 50, 40, 255, 90, 80, 70, 255, 3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255],
  ],
  [
    'palette PNG (pal8)',
    png(3, 1, 3, 8, [[0, 1, 2]], [['PLTE', RGBW]]),
    'image/png',
    'BGRX',
    [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255],
  ],
  [
    'palette PNG with tRNS (pal8)',
    png(
      3,
      1,
      3,
      8,
      [[0, 1, 2]],
      [
        ['PLTE', RGBW],
        ['tRNS', [255, 128, 0]],
      ],
    ),
    'image/png',
    'BGRA',
    [0, 0, 255, 255, 0, 255, 0, 128, 255, 0, 0, 0],
  ],
  // Chrome labels by the tRNS chunk, not by the alpha values in it
  [
    'palette PNG with a tRNS chunk of 255s (pal8)',
    png(
      3,
      1,
      3,
      8,
      [[0, 1, 2]],
      [
        ['PLTE', RGBW],
        ['tRNS', [255, 255, 255]],
      ],
    ),
    'image/png',
    'BGRA',
    [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255],
  ],
  [
    'palette PNG with a short tRNS chunk of 255s (pal8)',
    png(
      3,
      1,
      3,
      8,
      [[0, 1, 2]],
      [
        ['PLTE', RGBW],
        ['tRNS', [255]],
      ],
    ),
    'image/png',
    'BGRA',
    [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255],
  ],
  [
    '1-bit palette PNG with a tRNS chunk of 255s (pal8)',
    png(
      9,
      1,
      3,
      1,
      [[0b10101010, 0b10000000]],
      [
        ['PLTE', [10, 20, 30, 200, 210, 220]],
        ['tRNS', [255, 255]],
      ],
    ),
    'image/png',
    'BGRA',
    [220, 210, 200, 255, 30, 20, 10, 255, 220, 210, 200, 255, 30, 20, 10, 255, 220, 210, 200, 255].concat([
      30, 20, 10, 255, 220, 210, 200, 255, 30, 20, 10, 255, 220, 210, 200, 255,
    ]),
  ],
  [
    'gray PNG (gray8)',
    png(3, 1, 0, 8, [[0, 100, 255]]),
    'image/png',
    'BGRX',
    [0, 0, 0, 255, 100, 100, 100, 255, 255, 255, 255, 255],
  ],
  [
    '2-bit gray PNG (gray8)',
    png(5, 1, 0, 2, [[0b00011011, 0]]),
    'image/png',
    'BGRX',
    [0, 0, 0, 255, 85, 85, 85, 255, 170, 170, 170, 255, 255, 255, 255, 255, 0, 0, 0, 255],
  ],
  [
    '1-bit gray PNG (monob)',
    png(9, 1, 0, 1, [[0b10101010, 0b10000000]]),
    'image/png',
    'BGRX',
    [255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255].concat([
      0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
    ]),
  ],
  [
    'gray + alpha PNG (ya8)',
    png(2, 1, 4, 8, [[10, 255, 200, 100]]),
    'image/png',
    'BGRA',
    [10, 10, 10, 255, 200, 200, 200, 100],
  ],
  [
    'gray PNG with tRNS (ya8)',
    png(3, 1, 0, 8, [[0, 50, 100]], [['tRNS', [0, 50]]]),
    'image/png',
    'BGRA',
    [0, 0, 0, 255, 50, 50, 50, 0, 100, 100, 100, 255],
  ],
  // 16-bit samples whose low byte is below 0x80: Chrome keeps the high byte and
  // swscale rounds, which agree for these values.
  [
    '16-bit gray PNG (gray16be)',
    png(2, 1, 0, 16, [[0, 0, 0x80, 0x40]]),
    'image/png',
    'BGRX',
    [0, 0, 0, 255, 128, 128, 128, 255],
  ],
  [
    '16-bit RGB PNG (rgb48be)',
    png(1, 1, 2, 16, [[0x11, 0x11, 0x22, 0x22, 0xff, 0xff]]),
    'image/png',
    'BGRX',
    [255, 34, 17, 255],
  ],
  [
    '24-bit BMP (bgr24)',
    bmp(3, 2, 24, [
      [30, 20, 10, 60, 50, 40, 90, 80, 70],
      [3, 2, 1, 6, 5, 4, 9, 8, 7],
    ]),
    'image/bmp',
    'BGRX',
    [30, 20, 10, 255, 60, 50, 40, 255, 90, 80, 70, 255, 3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255],
  ],
  [
    '32-bit BMP with zero alpha (bgr0)',
    bmp(2, 1, 32, [[30, 20, 10, 0, 60, 50, 40, 0]]),
    'image/bmp',
    'BGRX',
    [30, 20, 10, 255, 60, 50, 40, 255],
  ],
  [
    '8-bit palette BMP (pal8)',
    bmp(
      3,
      1,
      8,
      [[2, 1, 0]],
      [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
      ],
    ),
    'image/bmp',
    'BGRX',
    [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
  ],
  // 6x4 lossless WebPs (argb): the header's alpha hint picks BGRA or BGRX
  [
    'lossless WebP with alpha (argb)',
    b64('UklGRi4AAABXRUJQVlA4TCIAAAAvBcAAELmM6H/sIqL/YSDIthnY/NmO8A3KGzomME6iJDUA'),
    'image/webp',
    'BGRA',
    Array.from(
      b64(
        'AAAA/w0AKP8aAFD/JwB4gDQAoIBBAMiAHTwA/yo8KP83PFD/RDx4gFE8oIBePMiAOngA/0d4KP9UeFD/YXh4gG54oIB7eMiAV7QA/2S0KP9xtFD/frR4gIu0oICYtMiA',
      ),
    ),
  ],
  [
    'lossless WebP without alpha (argb)',
    b64('UklGRjIAAABXRUJQVlA4TCUAAAAvBcAAALmM6H/sIqL/AYVsI0AHdv5iP8KgCARSnMAIxwQATOkDAA=='),
    'image/webp',
    'BGRX',
    Array.from(
      b64(
        'AAAA/w0AKP8aAFD/JwB4/zQAoP9BAMj/HTwA/yo8KP83PFD/RDx4/1E8oP9ePMj/OngA/0d4KP9UeFD/YXh4/254oP97eMj/V7QA/2S0KP9xtFD/frR4/4u0oP+YtMj/',
      ),
    ),
  ],
]

for (const [name, data, type, format, expected] of IMAGE_CASES) {
  test(`ImageDecoder: ${name} decodes to ${format} with Chrome's bytes`, async (t) => {
    const image = await decodeImage(data, type)
    t.is(image.format, format)
    t.is(image.allocationSize(), expected.length)
    const { bytes, layout } = await rawCopy(image)
    t.deepEqual(Array.from(bytes), expected)
    t.deepEqual(layout, [[0, image.codedWidth * 4]])
    // The frame is RGB now: copyTo RGBA swaps the channels of each opaque pixel
    // (pixels with alpha below 255 take Chrome's premultiplied path).
    const rgba = new Uint8Array(image.allocationSize({ format: 'RGBA' }))
    await image.copyTo(rgba, { format: 'RGBA' })
    for (let i = 0; i < rgba.length; i += 4) {
      if (format === 'BGRA' && expected[i + 3] !== 255) continue
      t.deepEqual([rgba[i], rgba[i + 1], rgba[i + 2]], [expected[i + 2], expected[i + 1], expected[i]])
    }
    t.is(image.colorSpace.matrix, 'rgb')
    t.true(image.colorSpace.fullRange)
    image.close()
  })
}

test('ImageDecoder: a decoded RGB frame keeps its format and bytes through new VideoFrame() and clone()', async (t) => {
  const image = await decodeImage(png(3, 2, 2, 8, RGB3x2), 'image/png')
  const copy = new VideoFrame(image, { timestamp: 1 })
  const clone = image.clone()
  const expected = (await rawCopy(image)).bytes
  for (const frame of [copy, clone]) {
    t.is(frame.format, 'BGRX')
    t.deepEqual((await rawCopy(frame)).bytes, expected)
    frame.close()
  }
  image.close()
})

test('ImageDecoder: desiredWidth/desiredHeight scale a gray PNG as BGRX', async (t) => {
  const image = await decodeImage(
    png(4, 2, 0, 8, [
      [77, 77, 77, 77],
      [77, 77, 77, 77],
    ]),
    'image/png',
    { desiredWidth: 2, desiredHeight: 1 },
  )
  t.is(image.format, 'BGRX')
  t.is(image.codedWidth, 2)
  t.deepEqual(Array.from((await rawCopy(image)).bytes), [77, 77, 77, 255, 77, 77, 77, 255])
  image.close()
})

// ============================================================================
// ImageDecoder: JPEG (16x8, written by Pillow)
// ============================================================================

// [name, JPEG, Chrome format, Chrome layout, Chrome copyTo() bytes]. The two IDCTs
// (FFmpeg, libjpeg-turbo) differ by 1 on some samples.
const JPEG_CASES: [string, string, VideoPixelFormat, number[][], string][] = [
  [
    '4:2:0',
    '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAAIABADASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAf/xAAZEAABBQAAAAAAAAAAAAAAAAAABggkMqL/xAAVAQEBAAAAAAAAAAAAAAAAAAAEBv/EABsRAAEEAwAAAAAAAAAAAAAAAAYABQcjIpHw/9oADAMBAAIRAxEAPwCdJxqlYeSppxqlYeQAx6TuedijowMXiu3tr//Z',
    'I420',
    [
      [0, 16],
      [128, 8],
      [160, 8],
    ],
    'GBofIyYqLzI3Oj5CRkpOUSotMTU5PUFESUxQVFhcYGM9QEVJTFBVV11fZGhrb3R3T1JXW15iZmlvcXZ6fYGGiWNmam5ydnp9goWJjZGVmZx1eHyAhIiMj5SXm5+jp6uuiIuQlJebn6Koqq+ztrq/wpueoqaqrrK1ur3BxcnN0dTbz8Cyp5mKfse8rJ+ThXZqrqKThXpsXFGdkoJ1aVxMQW59kqOyw9jnU2J2iJaovMs4R1tte42hsB0sQFJgcoaV',
  ],
  [
    '4:2:2',
    '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAAIABADASEAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAAB//EABkQAAEFAAAAAAAAAAAAAAAAAAAGCCQyov/EABUBAQEAAAAAAAAAAAAAAAAAAAMG/8QAGhEAAgIDAAAAAAAAAAAAAAAAAAYHIjIzUf/aAAwDAQACEQMRAD8AOk41SsPIppxqlYeSifXDOwsYPuu/D//Z',
    'I422',
    [
      [0, 16],
      [128, 8],
      [192, 8],
    ],
    'GBofIyYqLzI3Oj5CRkpOUSotMTU5PUFESUxQVFhcYGM9QEVJTFBVV11fZGhrb3R3T1JXW15iZmlvcXZ6fYGGiWNmam5ydnp9goWJjZGVmZx1eHyAhIiMj5SXm5+jp6uuiIuQlJebn6Koqq+ztrq/wpueoqaqrrK1ur3BxcnN0dTf1MS2q52OgtfLvK6jlYV6y7+wopeJem7AtaWXjH5vY7arm46CdWVaq6CQg3dqWk+flIR3a15OQ5eMfG9jVkY7coaXprrK2u5ne4ubr77P41drfIufr7/TSl5ufZKhscY+UmNyhpamujFFVWR5iJitITVFVWl4iZ0WKjpJXm19kg==',
  ],
  [
    '4:4:4',
    '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/2wBDAQMEBAUEBQkFBQkUDQsNFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBT/wAARCAAIABADAREAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAAB//EABkQAAEFAAAAAAAAAAAAAAAAAAAGCCQyov/EABUBAQEAAAAAAAAAAAAAAAAAAAcI/8QAGREAAwADAAAAAAAAAAAAAAAAAAYiBTEy/9oADAMBAAIRAxEAPwA6TjVKw8jTn3DdBOsPvNimnGqVh5JwzzhuiuVh95s//9k=',
    'I444',
    [
      [0, 16],
      [128, 16],
      [256, 16],
    ],
    'GBofIyYqLzI3Oj5CRkpOUSotMTU5PUFESUxQVFhcYGM9QEVJTFBVV11fZGhrb3R3T1JXW15iZmlvcXZ6fYGGiWNmam5ydnp9goWJjZGVmZx1eHyAhIiMj5SXm5+jp6uuiIuQlJebn6Koqq+ztrq/wpueoqaqrrK1ur3BxcnN0dTi3dbPyMG5tKynoJmSi4SA2tXOxsC4sayjn5iRioN8d87Jwru0raWgmJOMhX53cGvDvrewqaKalY2IgXpzbGVgubStpp+YkYyDfndwamNcV66qopuUjYaBeHNsZV9YUUyjnpaPiIF6dWxnYFlTTEVAmpWOh4B5cm1kX1hRS0Q8OHB3gYqRmaSrtbzGz9bf6fBkbHZ+ho6YoKqxu8TL1N7lVVxmb3Z+iZCaoau0u8TO1UdOWWFocXuCjJSep662wMg8Q01WXWVwd4GIkpuiq7W8LjVASE9YYmlze4WNlZ2nrx4mMDhASFJaZGt1foWOmJ8TGiUtND1HTlhganJ6goyU',
  ],
  [
    'grayscale',
    '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAMCAgMCAgMDAwMEAwMEBQgFBQQEBQoHBwYIDAoMDAsKCwsNDhIQDQ4RDgsLEBYQERMUFRUVDA8XGBYUGBIUFRT/wAALCAAIABABAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAB//EABkQAAEFAAAAAAAAAAAAAAAAAAAGCCQyov/aAAgBAQAAPwA6TjVKw8imnGqVh5P/2Q==',
    'BGRX',
    [[0, 64]],
    'GBgY/xoaGv8fHx//IyMj/yYmJv8qKir/Ly8v/zIyMv83Nzf/Ojo6/z4+Pv9CQkL/RkZG/0pKSv9OTk7/UVFR/yoqKv8tLS3/MTEx/zU1Nf85OTn/PT09/0FBQf9ERET/SUlJ/0xMTP9QUFD/VFRU/1hYWP9cXFz/YGBg/2NjY/89PT3/QEBA/0VFRf9JSUn/TExM/1BQUP9VVVX/V1dX/11dXf9fX1//ZGRk/2hoaP9ra2v/b29v/3R0dP93d3f/T09P/1JSUv9XV1f/W1tb/15eXv9iYmL/ZmZm/2lpaf9vb2//cXFx/3Z2dv96enr/fX19/4GBgf+Ghob/iYmJ/2NjY/9mZmb/ampq/25ubv9ycnL/dnZ2/3p6ev99fX3/goKC/4WFhf+JiYn/jY2N/5GRkf+VlZX/mZmZ/5ycnP91dXX/eHh4/3x8fP+AgID/hISE/4iIiP+MjIz/j4+P/5SUlP+Xl5f/m5ub/5+fn/+jo6P/p6en/6urq/+urq7/iIiI/4uLi/+QkJD/lJSU/5eXl/+bm5v/n5+f/6Kiov+oqKj/qqqq/6+vr/+zs7P/tra2/7q6uv+/v7//wsLC/5ubm/+enp7/oqKi/6ampv+qqqr/rq6u/7Kysv+1tbX/urq6/729vf/BwcH/xcXF/8nJyf/Nzc3/0dHR/9TU1P8=',
  ],
]

for (const [name, jpeg, format, layout, chrome] of JPEG_CASES) {
  test(`ImageDecoder: ${name} JPEG decodes to ${format} like Chrome`, async (t) => {
    const image = await decodeImage(b64(jpeg), 'image/jpeg')
    t.is(image.format, format)
    const copy = await rawCopy(image)
    t.deepEqual(copy.layout, layout)
    t.true(maxDiff(copy.bytes, b64(chrome)) <= 1)
    t.true(image.colorSpace.fullRange)
    image.close()
  })
}

// ============================================================================
// VideoDecoder: one lossless 16x16 keyframe each, so the decoded samples are known
// ============================================================================

// H.264 High 4:4:4 lossless RGB (libx264rgb): R = 16x, G = 16y, B = 255 - 16x
const H264_GBRP =
  'AAAAAWf0AAqR1kem4EBACAAAAwAIAAADAZB4kTJAAAAAAWjrwGvLAAABZYiEr9fjwAAxWJE8OjNw34w8QaDfnEQpWcnfc2RxU40GF/fcAjv8FQetyEAWQ9VlQndvahyUOyzMnj6KHMgOq4kfBkRrL5tpnwlrqU3Oq3mwdzanrJ1WEhoW1CQg8YZZQ5GZw2gxLTB3esNUu48Hj9VXo1qJb+RPkVewgnwcZH+llAxeeYcpHinfTvMg5ysS2rZV9O8yDnKxLasblnFXsIJ8HP7I7hbvXNhryh7NnfvMOUkjmw15PnvjRzlYltgYQmAhltgYQys4q7MGneZBzlYltf7Fc6qfF55hykec1SxB1ebWORb2zeF7mkWj4TKXJmbLxBPwvXQUrdAPaTcl2/NynjQmLpjNK/zsZXuFY9TmtiaELOTuDYDLrgXTeozKxFoeihRUN6jMrEWh6KE79EV7hWPU5sb8rDtv7d8oa8qIp/hsBl11u+UNeLhuliLQ9FDmQEOR9FDmQFoivb6O9RmViLQ9FDjAmxL1O4NgMuv1QQ=='
// H.264 lossless full-range 4:2:0 (yuvj420p): Y = 16x + y, U = 64 + 8cx, V = 192 - 8cy
const H264_YUVJ420P =
  'AAAAAWf0AAqusj02QAAAAwBAAAAMg8SJkgAAAAFo68BrywAAAWWIhK/YX2pQpgYkTMQ44xzXOBlgTbNgjYwi5kFaw0EdBHGG1NABfhTRrr9V6cbXuD7tvqNagAAcYAAB25lZ52mbF1q6DX7JCvUZlYi0OmAAAAMAAAMAAAMAAAMAAAMAAAMAAAMAAAMAAAMAAAMAAAMAAAMAAFfoqRUPuY5v5L3xaY4u3vXhJYYfmrVnSS5FDNR3tcTBQlihgAPS5Q6sCUmYKrQsCwoN39Q37fqJgHtEZDLysatic2bFXlfroYsvaxLfgYayk42kwlHW3fgCV6KhRXDqoadgEEZtgvYrKxXXCjChVDs5PksPb37CXvJhjtROy5QPkNVo/0+JEaX/uEakJ4JKOqhYJ23E6Q2eECgVlQp1IYXYxNPQEN3KEU9lZUKdSGF2MUJx'
// HEVC RExt lossless 4:0:0 (gray): Y = 16x + 3y
const HEVC_GRAY =
  'AAAAAUABDAH//wQIAAADAJ/IAAADAAD/lZgJAAAAAUIBAQQIAAADAJ/IAAADAAD/wiEWWVmq8rmwIAAAAwAgAAADAyEAAAABRAHBcakSAAABKAGuC3Ao9BUQf//8F/hP437fsfC+d7fxPy3v/GfM/H/OfPgFuAV85+R+G7hQ0SJCZtton3tRP+i+AAADAwTb0AVklFMBgpqwgAAHmkfRR7QtTf4KcOs6ujclwAAFNRBl5jR7+zrNmTAkjXRq6Z6u2IAAAwkECzmzndubq0FIbNghf/xA+8sIvOgqKAAcuV7sAA9IsYAAGGRoAy/MAoD8tFrh6AABNraAZl+rgm++fcwAAke3dMAAAFNKAK045AEN8yxJF+A='
// VP9 profile 1 lossless 4:4:0 (yuv440p): Y = 7x + 11y, U = 40 + 10x, V = 200 - 10x
const VP9_YUV440P =
  'okmDQgQAHgAewAcEg4MAAAAEAAB55fH///6+Lj3ZSp//9NL/82v//hv7+71/82v/pvDZu/XV/////wOg4EMr//8/f///tpnAhlf//n7////+uveOyD///Nt///7cdvpZX//5nrX//w39/d6/+bX//w39/d6/+Xef/04rfKx//8/f///5QX15OX//5+////UKFNHr//+bb///9tElrNY//+aogp/6bw2bv2Cf/TeGzd+unOFTz///5Wz7ni6i/+h7/ry73aen9ANi7/7BN0x/tF5TAA=='

const plane = (fn: (x: number, y: number) => number, w = 16, h = 16) =>
  Array.from({ length: w * h }, (_, i) => fn(i % w, Math.floor(i / w)) & 255)

test('VideoDecoder: RGB H.264 (gbrp) decodes to I444 holding the G, B, R planes, like Chrome', async (t) => {
  const frame = await decodeVideo('avc1.f4000a', b64(H264_GBRP))
  t.is(frame.format, 'I444')
  const { bytes, layout } = await rawCopy(frame)
  t.deepEqual(layout, [
    [0, 16],
    [256, 16],
    [512, 16],
  ])
  t.deepEqual(Array.from(bytes), [...plane((_, y) => 16 * y), ...plane((x) => 255 - 16 * x), ...plane((x) => 16 * x)])
  // The planes are RGB, so the colour space says so and RGBA is exact.
  t.is(frame.colorSpace.matrix, 'rgb')
  const rgba = new Uint8Array(frame.allocationSize({ format: 'RGBA' }))
  await frame.copyTo(rgba, { format: 'RGBA' })
  t.deepEqual(Array.from(rgba.subarray(0, 8)), [0, 0, 255, 255, 16, 0, 239, 255])
  frame.close()
})

test('VideoDecoder: full-range H.264 (yuvj420p) decodes to I420 with fullRange, like Chrome', async (t) => {
  const frame = await decodeVideo('avc1.f4000a', b64(H264_YUVJ420P))
  t.is(frame.format, 'I420')
  t.true(frame.colorSpace.fullRange)
  const { bytes } = await rawCopy(frame)
  t.deepEqual(Array.from(bytes), [
    ...plane((x, y) => 16 * x + y),
    ...plane((x) => 64 + 8 * x, 8, 8),
    ...plane((_, y) => 192 - 8 * y, 8, 8),
  ])
  frame.close()
})

test('VideoDecoder: monochrome HEVC (gray) decodes to I420 with neutral chroma', async (t) => {
  // Chrome 155 has no software HEVC decoder; I420 with U = V = 128 is what Chrome's AV1
  // decoder gives for monochrome video.
  const frame = await decodeVideo('hev1.4.10.L30.B0', b64(HEVC_GRAY))
  t.is(frame.format, 'I420')
  const { bytes } = await rawCopy(frame)
  t.deepEqual(Array.from(bytes), [...plane((x, y) => 16 * x + 3 * y), ...Array.from({ length: 128 }, () => 128)])
  frame.close()
})

test('VideoDecoder: 4:4:0 VP9 (yuv440p) decodes to I444', async (t) => {
  // Chrome 155 fails to decode this stream; I444 keeps every chroma sample.
  const frame = await decodeVideo('vp09.01.10.08', b64(VP9_YUV440P))
  t.is(frame.format, 'I444')
  const { bytes } = await rawCopy(frame)
  t.deepEqual(Array.from(bytes), [
    ...plane((x, y) => 7 * x + 11 * y),
    ...plane((x) => 40 + 10 * x),
    ...plane((x) => 200 - 10 * x),
  ])
  frame.close()
})

// ============================================================================
// Crash cases (child processes)
// ============================================================================

test('ImageDecoder: copyTo() of a wide RGB PNG stays inside the plane (no SIGSEGV)', (t) => {
  // 12000x100 crashed with SIGSEGV when the rgb24 frame was labelled RGBX (#127).
  const result = runChild(`
    ${CHILD_PNG}
    const { ImageDecoder } = require('./index.js');
    (async () => {
      const W = 12000, H = 100;
      const { image } = await new ImageDecoder({ data: png(W, H, 2, 8, new Uint8Array(W * 3).map((_, i) => [255, 0, 0][i % 3])), type: 'image/png' }).decode();
      const b = new Uint8Array(image.allocationSize());
      await image.copyTo(b);
      let bad = 0;
      for (let i = 0; i < b.length; i += 4) if (b[i] !== 0 || b[i + 1] !== 0 || b[i + 2] !== 255) bad++;
      console.log('RESULT ' + JSON.stringify({ format: image.format, size: b.length, bad }));
    })();
  `) as { format: string; size: number; bad: number }
  t.deepEqual(result, { format: 'BGRX', size: 12000 * 100 * 4, bad: 0 })
})

test('VideoEncoder: encode() of decoded gray PNG, JPEG and RGB H.264 frames produces chunks (no abort)', (t) => {
  // Each of these aborted the process in swscale (exit 134) before (#126 A2).
  const result = runChild(`
    ${CHILD_PNG}
    const { ImageDecoder, VideoDecoder, VideoEncoder, EncodedVideoChunk } = require('./index.js');
    const b64 = (s) => new Uint8Array(Buffer.from(s, 'base64'));
    const encode = async (frame) => {
      const chunks = []; let error;
      const encoder = new VideoEncoder({ output: (c) => chunks.push(c.byteLength), error: (e) => { error = e } });
      encoder.configure({ codec: 'avc1.42001f', width: frame.codedWidth, height: frame.codedHeight, hardwareAcceleration: 'prefer-software' });
      encoder.encode(frame, { keyFrame: true });
      await encoder.flush().catch((e) => { error = error ?? e });
      return error ? 'error: ' + error.message : frame.format + ' ' + (chunks.length > 0);
    };
    const image = async (data, type) => (await new ImageDecoder({ data, type }).decode()).image;
    (async () => {
      const out = [];
      out.push(await encode(await image(png(16, 16, 0, 8, new Uint8Array(16).fill(90)), 'image/png')));
      out.push(await encode(await image(b64(${JSON.stringify(JPEG_CASES[0][1])}), 'image/jpeg')));
      const frames = [];
      const decoder = new VideoDecoder({ output: (f) => frames.push(f), error: () => {} });
      decoder.configure({ codec: 'avc1.f4000a', hardwareAcceleration: 'prefer-software' });
      decoder.decode(new EncodedVideoChunk({ type: 'key', timestamp: 0, data: b64(${JSON.stringify(H264_GBRP)}) }));
      await decoder.flush();
      out.push(await encode(frames[0]));
      console.log('RESULT ' + JSON.stringify(out));
    })();
  `)
  t.deepEqual(result, ['BGRX true', 'I420 true', 'I444 true'])
})

test('ImageDecoder: desiredWidth/desiredHeight on a gray PNG does not abort', (t) => {
  // The scaler was created with an unknown pixel format and swscale aborted (exit 134).
  const result = runChild(`
    ${CHILD_PNG}
    const { ImageDecoder } = require('./index.js');
    (async () => {
      const { image } = await new ImageDecoder({ data: png(64, 64, 0, 8, new Uint8Array(64).fill(77)), type: 'image/png', desiredWidth: 32, desiredHeight: 32 }).decode();
      const b = new Uint8Array(image.allocationSize());
      await image.copyTo(b);
      console.log('RESULT ' + JSON.stringify({ format: image.format, width: image.codedWidth, first: Array.from(b.subarray(0, 4)) }));
    })();
  `)
  t.deepEqual(result, { format: 'BGRX', width: 32, first: [77, 77, 77, 255] })
})

const darwinTest = process.platform === 'darwin' ? test : test.skip

darwinTest(
  'ImageDecoder: copyTo() of rgb24, monob and pal8 PNGs reads no byte outside the frame (Guard Malloc)',
  (t) => {
    // Guard Malloc puts every allocation at the end of a page, so an over-read faults.
    // 64x64 rgb24 faulted here when it was labelled RGBX (#127).
    const result = runChild(
      `
    ${CHILD_PNG}
    const { ImageDecoder } = require('./index.js');
    (async () => {
      const W = 64, out = [];
      const cases = [
        png(W, W, 2, 8, new Uint8Array(W * 3).fill(200)),
        png(W, W, 0, 1, new Uint8Array(W / 8).fill(0xaa)),
        png(W, W, 3, 8, new Uint8Array(W).map((_, i) => i & 1), [255, 0, 0, 0, 0, 255]),
      ];
      for (const data of cases) {
        const { image } = await new ImageDecoder({ data, type: 'image/png' }).decode();
        const b = new Uint8Array(image.allocationSize());
        await image.copyTo(b);
        out.push(image.format + ' ' + b.length);
      }
      console.log('RESULT ' + JSON.stringify(out));
    })();
  `,
      { DYLD_INSERT_LIBRARIES: '/usr/lib/libgmalloc.dylib' },
    )
    t.deepEqual(result, ['BGRX 16384', 'BGRX 16384', 'BGRX 16384'])
  },
)
