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
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import { EncodedVideoChunk, ImageDecoder, VideoDecoder, VideoFrame, type VideoPixelFormat } from '../index.js'

import { be16, bmp, gif, le16, png } from './helpers/image-files.js'

// ============================================================================
// Helpers
// ============================================================================

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
// ImageDecoder: PNG / GIF / BMP built in memory (Chrome 155 bytes)
// ============================================================================

const RGB3x2 = [
  [10, 20, 30, 40, 50, 60, 70, 80, 90],
  [1, 2, 3, 4, 5, 6, 7, 8, 9],
]
const RGBW = [255, 0, 0, 0, 255, 0, 0, 0, 255]
const GIF_PALETTE = [
  [255, 0, 0],
  [0, 255, 0],
  [0, 0, 255],
  [255, 255, 255],
]
const BGRA_MASKS = [0xff0000, 0xff00, 0xff, 0xff000000]

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
  // Chrome ignores a tRNS chunk the png crate rejects: empty, or too short for its color type
  [
    'palette PNG with an empty tRNS chunk (pal8)',
    png(
      3,
      1,
      3,
      8,
      [[0, 1, 2]],
      [
        ['PLTE', RGBW],
        ['tRNS', []],
      ],
    ),
    'image/png',
    'BGRX',
    [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255],
  ],
  [
    'gray PNG with a 1-byte tRNS chunk (gray8)',
    png(3, 1, 0, 8, [[0, 50, 100]], [['tRNS', [50]]]),
    'image/png',
    'BGRX',
    [0, 0, 0, 255, 50, 50, 50, 255, 100, 100, 100, 255],
  ],
  // FFmpeg drops the tRNS chunk of a 1, 2 or 4-bit gray PNG; Chrome makes the key transparent
  [
    '1-bit gray PNG with tRNS (monob)',
    png(9, 1, 0, 1, [[0b10101010, 0b10000000]], [['tRNS', [0, 1]]]),
    'image/png',
    'BGRA',
    [255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0].concat([
      0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255, 255, 255, 255, 0,
    ]),
  ],
  [
    '2-bit gray PNG with tRNS (gray8)',
    png(5, 1, 0, 2, [[0b00011011, 0b10000000]], [['tRNS', [0, 2]]]),
    'image/png',
    'BGRA',
    [0, 0, 0, 255, 85, 85, 85, 255, 170, 170, 170, 0, 255, 255, 255, 255, 170, 170, 170, 0],
  ],
  [
    '4-bit gray PNG with tRNS (gray8)',
    png(4, 1, 0, 4, [[0x0f, 0x7f]], [['tRNS', [0, 15]]]),
    'image/png',
    'BGRA',
    [0, 0, 0, 255, 255, 255, 255, 0, 119, 119, 119, 255, 255, 255, 255, 0],
  ],
  // RGBA is swapped to BGRA, not premultiplied: the alpha-0 pixel keeps its color
  ['RGBA PNG (rgba)', png(2, 1, 6, 8, [[10, 20, 30, 40, 50, 60, 70, 0]]), 'image/png', 'BGRA', [30, 20, 10, 40, 70, 60, 50, 0]],
  [
    'RGB PNG with tRNS (rgba)',
    png(3, 1, 2, 8, [[1, 2, 3, 10, 20, 30, 1, 2, 3]], [['tRNS', [0, 1, 0, 2, 0, 3]]]),
    'image/png',
    'BGRA',
    [3, 2, 1, 0, 30, 20, 10, 255, 3, 2, 1, 0],
  ],
  // Chrome keeps the high byte of each 16-bit sample (swscale rounds: 0x12ff would be 0x13)
  [
    '16-bit gray PNG (gray16be)',
    png(3, 1, 0, 16, [be16([0x12ff, 0x8080, 0xff00])]),
    'image/png',
    'BGRX',
    [18, 18, 18, 255, 128, 128, 128, 255, 255, 255, 255, 255],
  ],
  [
    '16-bit gray + alpha PNG (ya16be)',
    png(2, 1, 4, 16, [be16([0x40ff, 0x80ff, 0xc0c0, 0x00ff])]),
    'image/png',
    'BGRA',
    [64, 64, 64, 128, 192, 192, 192, 0],
  ],
  [
    '16-bit RGB PNG (rgb48be)',
    png(2, 1, 2, 16, [be16([0x10ff, 0x2080, 0x30c0, 0xfe80, 0x0180, 0x7fff])]),
    'image/png',
    'BGRX',
    [48, 32, 16, 255, 127, 1, 254, 255],
  ],
  [
    '16-bit RGBA PNG (rgba64be)',
    png(2, 1, 6, 16, [be16([0x10ff, 0x2080, 0x30c0, 0x40ff, 0xfe80, 0x0180, 0x7fff, 0x0080])]),
    'image/png',
    'BGRA',
    [48, 32, 16, 64, 127, 1, 254, 0],
  ],
  // GIF: BGRA when the frame has a transparent index, whose pixels are 0, 0, 0, 0
  [
    'GIF (bgra)',
    gif(3, 2, GIF_PALETTE, [{ x: 0, y: 0, width: 3, height: 2, indices: [0, 1, 2, 3, 2, 1] }]),
    'image/gif',
    'BGRX',
    [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255],
  ],
  [
    'GIF with a transparent index (bgra)',
    gif(3, 2, GIF_PALETTE, [{ x: 0, y: 0, width: 3, height: 2, indices: [0, 1, 2, 3, 2, 1], transparent: 3 }]),
    'image/gif',
    'BGRA',
    [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 255, 0, 255],
  ],
  // FFmpeg returns only the first frame of a GIF without graphic control extensions
  [
    'GIF with 2 frames and no graphic control extension (bgra)',
    gif(4, 2, GIF_PALETTE, [
      { x: 0, y: 0, width: 4, height: 2, indices: [0, 1, 2, 3, 3, 2, 1, 0] },
      { x: 1, y: 0, width: 2, height: 1, indices: [3, 3] },
    ]),
    'image/gif',
    'BGRX',
    [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
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
    bmp(3, 1, 8, [[2, 1, 0]], {
      palette: [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
      ],
    }),
    'image/bmp',
    'BGRX',
    [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
  ],
  // an index past colorsUsed is opaque black (FFmpeg's palette has alpha 0 there)
  [
    '4-bit palette BMP with indices past colorsUsed (pal8)',
    bmp(4, 1, 4, [[0x01, 0x23]], {
      palette: [
        [255, 0, 0],
        [0, 255, 0],
      ],
    }),
    'image/bmp',
    'BGRX',
    [0, 0, 255, 255, 0, 255, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255],
  ],
  // 32-bit: alpha only from a V3+ bitfield alpha mask, or a V4+ BI_RGB alpha mask
  ['32-bit BMP with alpha bytes (bgra)', bmp(2, 1, 32, [[30, 20, 10, 128, 60, 50, 40, 0]]), 'image/bmp', 'BGRX', [30, 20, 10, 255, 60, 50, 40, 255]],
  [
    'V3 BMP with BI_RGB and an alpha mask (bgra)',
    bmp(2, 1, 32, [[30, 20, 10, 128, 60, 50, 40, 0]], { header: 56, masks: BGRA_MASKS }),
    'image/bmp',
    'BGRX',
    [30, 20, 10, 255, 60, 50, 40, 255],
  ],
  [
    'V4 BMP with BI_RGB and an alpha mask (bgra)',
    bmp(2, 1, 32, [[30, 20, 10, 128, 60, 50, 40, 0]], { header: 108, masks: BGRA_MASKS }),
    'image/bmp',
    'BGRA',
    [30, 20, 10, 128, 60, 50, 40, 0],
  ],
  // FFmpeg calls an all-zero alpha bgr0; Chrome keeps it, so the image is transparent
  [
    'V3 BMP with an alpha mask and zero alpha (bgr0)',
    bmp(2, 1, 32, [[30, 20, 10, 0, 60, 50, 40, 0]], { header: 56, compression: 3, masks: BGRA_MASKS }),
    'image/bmp',
    'BGRA',
    [30, 20, 10, 0, 60, 50, 40, 0],
  ],
  [
    'V4 BMP with RGBA bitfields (rgba)',
    bmp(2, 1, 32, [[10, 20, 30, 128, 40, 50, 60, 0]], {
      header: 108,
      compression: 3,
      masks: [0xff, 0xff00, 0xff0000, 0xff000000],
    }),
    'image/bmp',
    'BGRA',
    [30, 20, 10, 128, 60, 50, 40, 0],
  ],
  // the alpha field of the header masks, even over a color field (FFmpeg reads the byte the color masks leave)
  [
    'V4 BMP with the alpha mask on blue (bgra)',
    bmp(2, 1, 32, [[30, 20, 10, 128, 60, 50, 40, 0]], {
      header: 108,
      compression: 3,
      masks: [0xff0000, 0xff00, 0xff, 0xff],
    }),
    'image/bmp',
    'BGRA',
    [30, 20, 10, 30, 60, 50, 40, 60],
  ],
  [
    'V5 BMP with ARGB bitfields and the alpha mask on red (argb)',
    bmp(2, 1, 32, [[128, 10, 20, 30, 0, 40, 50, 60]], {
      header: 124,
      compression: 3,
      masks: [0xff00, 0xff0000, 0xff000000, 0xff00],
    }),
    'image/bmp',
    'BGRA',
    [30, 20, 10, 10, 60, 50, 40, 40],
  ],
  [
    'V3 BMP with a 4-bit alpha field (bgra)',
    bmp(2, 1, 32, [[30, 20, 10, 0x9f, 60, 50, 40, 0x0f]], {
      header: 56,
      compression: 3,
      masks: [0xff0000, 0xff00, 0xff, 0xf0000000],
    }),
    'image/bmp',
    'BGRA',
    [30, 20, 10, 153, 60, 50, 40, 0],
  ],
  // 16-bit: the image crate's round(v * 255 / (2^n - 1)) (swscale replicates bits: 3 -> 24, not 25)
  [
    '16-bit BMP (rgb555le)',
    bmp(4, 1, 16, [le16([0x0c63, 0x7fff, 0x0001, 0x5294])]),
    'image/bmp',
    'BGRX',
    [25, 25, 25, 255, 255, 255, 255, 255, 8, 0, 0, 255, 165, 165, 165, 255],
  ],
  [
    '16-bit 5-6-5 BMP (rgb565le)',
    bmp(3, 1, 16, [le16([0x1963, 0xffff, 0x0841])], { compression: 3, masks: [0xf800, 0x7e0, 0x1f] }),
    'image/bmp',
    'BGRX',
    [25, 45, 25, 255, 255, 255, 255, 255, 8, 8, 8, 255],
  ],
  [
    '16-bit 4-4-4 BMP (rgb444le)',
    bmp(3, 1, 16, [le16([0x0123, 0x0fff, 0x0a5c])], { compression: 3, masks: [0xf00, 0xf0, 0xf] }),
    'image/bmp',
    'BGRX',
    [51, 34, 17, 255, 255, 255, 255, 255, 204, 85, 170, 255],
  ],
  // the alpha mask of a V3+ header, which FFmpeg ignores
  [
    'V3 BMP with 4-4-4-4 bitfields (rgb444le)',
    bmp(3, 1, 16, [le16([0x8123, 0xffff, 0x0a5c])], {
      header: 56,
      compression: 3,
      masks: [0xf00, 0xf0, 0xf, 0xf000],
    }),
    'image/bmp',
    'BGRA',
    [51, 34, 17, 136, 255, 255, 255, 255, 204, 85, 170, 0],
  ],
  [
    'V4 BMP with 1-5-5-5 bitfields (rgb555le)',
    bmp(2, 1, 16, [le16([0x8c63, 0x0c63])], {
      header: 108,
      compression: 3,
      masks: [0x7c00, 0x3e0, 0x1f, 0x8000],
    }),
    'image/bmp',
    'BGRA',
    [25, 25, 25, 255, 25, 25, 25, 0],
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
  // Lossy WebPs with a VP8X header and an ALPH chunk (yuva420p): Chrome decodes them with
  // libwebp, which upsamples the chroma ("fancy upsampling")
  [
    'lossy WebP 7x5 with alpha (yuva420p)',
    b64(
      'UklGRrIAAABXRUJQVlA4WAoAAAAQAAAABgAABAAAQUxQSCQAAAAAASpVf6oA/wAqVX8A//8AKlUBqtT/AP8Af6rU/wAAVX+q1AFWUDggaAAAADACAJ0BKgcABQACADQlsAJ0ugH4AAMfRtwsAP7mC/Q+JAp/LO9pf+MWBuPBxG6prxnsXx7+nbh9YM/26R+btc85vS1/1uwx2Z6gf9//0N/7geX/1H7c/oeeIO/3+/g3B//RS7RAAAAA',
    ),
    'image/webp',
    'BGRA',
    Array.from(
      b64(
        'LAcBAUcSJCpkDE9VbAlxf18MkKqcQ+0AcgHX/6yKZwBfNigqdT1dVXhBhX+DWroAdUbL/5lP//+xmDoAkoU6Kmp0TlVujYgBWYieqmSIxNSOj/z/ka0WAJK6Nv9oqksAa75/f3PJqapktLnUiMn6/1PPAgBQxxAAj/xwVY7vh3+q+bOqsf/i1IT19wE=',
      ),
    ),
  ],
  [
    'lossy WebP 7x5 with gradient-filtered raw alpha (yuva420p)',
    b64(
      'UklGRqoAAABXRUJQVlA4WAoAAAAQAAAABgAABAAAQUxQSCQAAAAMASkrKitW//8BAABW/wAAAACCqtUrANUBf6sAAAABVasAAAJWUDggYAAAAFACAJ0BKgcABQACADQlsAJ0ugH4AALSpJ1+AAD+7SGagBBN2zJ6YJ6ujDWXHYotO/hmfvm7F0fFfkpCjm7+/ucZUsz1A/7//g3/uB5f/a/x5/BuQR9/v4N0v/2KK2tAAA==',
    ),
    'image/webp',
    'BGRA',
    Array.from(
      b64(
        'PgAAAVQLGSplEEJVWQBbf2MElapyEM0AYgDf/3dIIgBtPSkqaTpLVYJRi39oM6AAfUDZ/4k///9/dhEAiIw3KmWCTFVki34BWnieqm95ztSZhP//e6wNAIvKOf9jt0cAbMN+f3zEtKqDudXUocX//2fnFQB9+TgAlf9qVY/sgH+w9L6qt/3v1I3w/wE=',
      ),
    ),
  ],
  [
    'lossy WebP 2x1 with alpha (yuva420p)',
    b64(
      'UklGRlwAAABXRUJQVlA4WAoAAAAQAAAAAQAAAAAAQUxQSAMAAAAA9dMAVlA4IDIAAAAwAgCdASoCAAEAAUAmJaACdLoB+AADIPeKAAD+68PC8bWSH/82BqizfQD/43ZCofZQAA==',
    ),
    'image/webp',
    'BGRA',
    Array.from(b64('hMb/9U2Py9M=')),
  ],
  [
    'lossy WebP 3x1 with alpha (yuva420p)',
    b64(
      'UklGRmwAAABXRUJQVlA4WAoAAAAQAAAAAgAAAAAAQUxQSAQAAAAABykLVlA4IEIAAACwAQCdASoDAAEAAUAmJagCdAEO+mBgAP78D76a7lEQ9/5cn/owL8//6m///Qd/9Mf9GBfn//U3/hq87bPAcCI28AA=',
    ),
    'image/webp',
    'BGRA',
    Array.from(b64('L2aAByozSClfDRwL')),
  ],
  [
    'lossy WebP 1x3 with alpha (yuva420p)',
    b64(
      'UklGRnYAAABXRUJQVlA4WAoAAAAQAAAAAAAAAgAAQUxQSAQAAAAAISnJVlA4IEwAAABwAgCdASoBAAMAAUAmJagCdLoBJgLf/A9EArXgAPqeo6dX/7A6yb8keTP8Fm/g3wN5M/wWb+Df7/eb3/1mFdp/+swrtP/2M39ZQAAA',
    ),
    'image/webp',
    'BGRA',
    Array.from(b64('tHwuIeDXeClAlxXJ')),
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

test('ImageDecoder: each GIF frame is BGRA or BGRX by the frames it is drawn over, like Chrome', async (t) => {
  const data = gif(4, 2, GIF_PALETTE, [
    { x: 0, y: 0, width: 4, height: 2, indices: [0, 1, 2, 3, 3, 2, 1, 0] },
    // transparent index over an opaque frame, then restored to the background (transparent)
    { x: 1, y: 0, width: 2, height: 1, indices: [0, 1], transparent: 0, dispose: 2 },
    { x: 0, y: 0, width: 4, height: 2, indices: [2, 2, 3, 3, 1, 1, 3, 3], transparent: 3 },
    // opaque, but drawn over a frame with a transparent pixel; restored to the frame before
    { x: 0, y: 0, width: 2, height: 2, indices: [0, 0, 0, 0], dispose: 3 },
    { x: 0, y: 1, width: 4, height: 1, indices: [1, 2, 1, 2] },
  ])
  const [R, G, B, W, T] = [
    [0, 0, 255, 255],
    [0, 255, 0, 255],
    [255, 0, 0, 255],
    [255, 255, 255, 255],
    [0, 0, 0, 0],
  ]
  const chrome: [VideoPixelFormat, number[][]][] = [
    ['BGRX', [R, G, B, W, W, B, G, R]],
    ['BGRX', [R, G, G, W, W, B, G, R]],
    ['BGRA', [B, B, T, W, G, G, G, R]],
    ['BGRA', [R, R, T, W, R, R, G, R]],
    ['BGRA', [B, B, T, W, G, B, G, B]],
  ]
  const decoder = new ImageDecoder({ data, type: 'image/gif' })
  for (const [frameIndex, [format, pixels]] of chrome.entries()) {
    const { image } = await decoder.decode({ frameIndex })
    t.is(image.format, format, `frame ${frameIndex}`)
    t.deepEqual(Array.from((await rawCopy(image)).bytes), pixels.flat(), `frame ${frameIndex}`)
    image.close()
  }
  decoder.close()
})

test('ImageDecoder: a BMP bitfield mask the image crate rejects fails the metadata, like Chrome', async (t) => {
  // the alpha mask 0x8001 is not contiguous (FFmpeg ignores it)
  const data = bmp(2, 1, 16, [le16([0x8c63, 0x0c63])], {
    header: 56,
    compression: 3,
    masks: [0x7c00, 0x3e0, 0x1f, 0x8001],
  })
  // A decode() made in the constructor's task is always still pending when the decoder closes
  // (Chrome closes it in a task of its own).
  for (let i = 0; i < 100; i++) {
    const decoder = new ImageDecoder({ data, type: 'image/bmp' })
    const pending = decoder.decode()
    await t.throwsAsync(decoder.tracks.ready, {
      instanceOf: DOMException,
      name: 'InvalidStateError',
      message: 'Failed to retrieve track metadata.',
    })
    await t.throwsAsync(pending, { instanceOf: DOMException, name: 'EncodingError', message: 'Failed to decode frame at index 0' })
    t.true(decoder.closed)
  }
})

// A lossy WebP 33x17 with a VP8X header and no alpha (yuv420p)
const VP8X_WEBP_33x17 =
  'UklGRngBAABXRUJQVlA4WAoAAAAAAAAAIAAAEAAAVlA4IFoBAADwCQCdASohABEAPpE8lkiloyIhKqwAsBIJbACdMoR6t7t5glZYTvaq9HP4g9f/PqvwA35nrAPEA90n/XPoK6RE0SgBr3IaMNsZX6n+IwMYdZNd6mtSqJ5QAP75VA5EyFbxb29DVj1Nv6GuoaqE8Z4l0g2vU33cZ2okfpnRwJJVQ3ZK4Q5ITUOrnzwtN/6h+HBJI/Boy9nzx8lDleOnz59ETc2nsh2m7MT1hY/xdlMWS6QCN+nFI/VNQX/u6KoGr3TVhXmmU5imHwPX84sU0t6KbpBOpIBiCTU0wcSTSUS1ZosuBAxydDCtnB4+eQPW1//vQ40FNf/70OM/YyxkghYEhKQSp0e7nFeenDKBx5SmYdccd7RGtZCIKj2qCHd+CJYm7srJytvh1a6kL6njOVkfa1Wzhr9ypESsdNRI/wK/Cti6v/BIASsoOEk1kP53Qu/XK/dr/d3TFAAA'

test('ImageDecoder: a lossy WebP with a VP8X header is BGRA or BGRX by its alpha flag alone, like Chrome', async (t) => {
  const opaque = b64(VP8X_WEBP_33x17)
  // the alpha flag without an ALPH chunk: alpha 255
  const flagged = opaque.slice()
  flagged[20] |= 0x10
  // an ALPH chunk (raw, all 0) without the alpha flag: not used
  const alph = new Uint8Array(8 + 1 + 33 * 17)
  alph.set(Buffer.from('ALPH'))
  new DataView(alph.buffer).setUint32(4, 1 + 33 * 17, true)
  const unflagged = new Uint8Array([...opaque.subarray(0, 30), ...alph, ...opaque.subarray(30)])
  new DataView(unflagged.buffer).setUint32(4, unflagged.length - 8, true)
  for (const [data, format] of [
    [opaque, 'BGRX'],
    [flagged, 'BGRA'],
    [unflagged, 'BGRX'],
  ] as const) {
    const image = await decodeImage(data, 'image/webp')
    t.is(image.format, format)
    const { bytes, layout } = await rawCopy(image)
    t.deepEqual(layout, [[0, 33 * 4]])
    // Chrome's bytes are the same for the three files: the fourth byte is 255
    t.is(sha256(bytes), '85d7c51efa963c202789b44aadb7868a3fcf823f933e64a06e22381b20c0c043')
    t.is(image.colorSpace.matrix, 'rgb')
    t.true(image.colorSpace.fullRange)
    image.close()
  }
})

test("ImageDecoder: a lossy WebP 3x255 with alpha decodes to BGRA with Chrome's bytes", async (t) => {
  const image = await decodeImage(
    b64(
      'UklGRmwCAABXRUJQVlA4WAoAAAAQAAAAAgAA/gAAQUxQSC4AAAABFyAQSELffI2ICAe1jSQ1eI7Mw37/BZNG9L8OBqmDqW9gwFbH0AZaZ6P+Na0DVlA4IBgCAADQDwCdASoDAP8APoE2lUelIyIhOfO9UKAQCWwAnTKqeL+g/kB7AlJ/oH4K4OUrn6Xde/Yf3QKUc/VYn86Wm70xB/NwsQsZgPRXHQJc1fP1Lvu7hImr1ogGAaeTsoIxTtSMWWv5yAj+vu4HDyYbCAoYTcFuQkVxrVscz1JFqfKXUIegoczJkAAA/vUsHhCUdm+/x9vwwu1KbyGWIzN6/u34elXTT3D4cF4NBYSZm/FSSYVngEnn9R74lnCRhu1PaPNG1CJnw5l+Iu/JAwltcnMOo7IcLDTM/69zAZ3/V43aMRJ5LRJKE99kG/8DhlZkhRXunDMFZUsJN0G9Z7FKHKGXOfCuXtx4i8I8nDYjkn7KJpUaobnm6b5qWf51SnqeVR+ap9xatPohlfWSUt5uTwViEm6Hrcz03jZoQ255IXmO3cKVqdxcdXrII5ofIREI1yNDbydvhv6p7KFyLNcnMZTR+NL3AxW1sY9ahlWqArvj+DBuOd1eVoGsoQAm/aujZolMO4Ird46u6kEfGpy7hqG+MOiRMQgt+tuxDm1Dvzxi4wwDetux3t04dOg746e6VIPRXDphPZvgu8lhMSSsZX7O8SzdTpCrcuE4P37LlrTZ2zxykwx+G/YkdDUV2CEZKvaa/Sr7WZyoZgZkaIBiSIcc9Ahnf2m9Z81SLPNDUhRdoS0grJt+mUXzFZIJtJtXNTgB4tDVbjcAAA==',
    ),
    'image/webp',
  )
  t.is(image.format, 'BGRA')
  const { bytes, layout } = await rawCopy(image)
  t.deepEqual(layout, [[0, 3 * 4]])
  t.is(sha256(bytes), '38fda1421cd0647de110b51369e125c43e5efafcd5106416fe51a9fa4a728ad6')
  image.close()
})

test('ImageDecoder: a simple lossy WebP (no VP8X header) keeps its I420 planes, like Chrome', async (t) => {
  const image = await decodeImage(
    b64(
      'UklGRmoAAABXRUJQVlA4IF4AAADQAgCdASoGAAQAAgA0JbACdLoA1AD9APQAA+kcQriAAP6J3+j9SBe0/ai2tqnxX1Sg/n+yB2IHbrXPPY58G/UVH076xg+J9fh//qO3/DPkbeJxwrgVUTf/YwEt6AAA',
    ),
    'image/webp',
  )
  t.is(image.format, 'I420')
  const { bytes, layout } = await rawCopy(image)
  t.deepEqual(layout, [
    [0, 6],
    [24, 3],
    [30, 3],
  ])
  t.deepEqual(Array.from(bytes), Array.from(b64('Eig7OU1iU1FcfXmMcouMoKOzlbSuxtDdm5eMiVw7davSL2aN')))
  image.close()
})

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

// ============================================================================
// ImageDecoder: JPEG (16x8, written by Pillow)
// ============================================================================

// [name, JPEG, Chrome format, Chrome layout, Chrome copyTo() bytes]
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
    t.deepEqual(copy.bytes, b64(chrome))
    t.true(image.colorSpace.fullRange)
    image.close()
  })
}

// FFmpeg writes 4:2:2 JPEGs with 2x2/1x2/1x2 sampling factors and 4:4:4 with 1x2/1x2/1x2
// (16x8, `ffmpeg -pix_fmt yuvj422p|yuvj444p`). Chrome decodes a JPEG to YUV only for 1x1
// chroma with 1x1, 2x1 or 2x2 luma: these are BGRX.
// [name, JPEG, Chrome copyTo() bytes]
const FFMPEG_JPEG_CASES: [string, string, string][] = [
  [
    '4:2:2 (2x2/1x2/1x2)',
    '/9j//gAPTGF2YzYzLjEuMTAyAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAFkAAQAAAAAAAAAAAAAAAAAAAAUBAQEBAAAAAAAAAAAAAAAAAAYCBBAAAgMBAQAAAAAAAAAAAAAAAAQHI6ExQREAAgIDAQAAAAAAAAAAAAAABgAzI0GBUUP/wAARCAAIABADASIAAhIAAxIA/9oADAMBAAIRAxEAPwA5OP8AlWCqcf8AKsHE/BVPwk3M5bO5TBv6bcwEWxWcynQPy0//2Q==',
    'wSkY/74qHv+2Kiv/rio3/6UrQ/+cKk//lypb/44pZ/+GKnP/fil+/3Ypi/9uKZj/ZSqk/1wpr/9VKrv/USvB/7xAF/+5QR3/sUEq/6pBNv+fQkH/mEJO/5BCWv+HQWb/gEJy/3dBff9xQIr/akCX/19Aof9YQa7/UEG6/01CwP+2Wxj/s1we/6tcKf+jXDb/mVxC/5FcT/+MXFv/g1tm/3tbc/9zW33/a1uK/2Nbl/9ZW6H/UVyu/0pcu/9GXcH/sXMZ/610H/+mdSv/nnU3/5R1Qv+MdU//hnVb/310Z/92dHP/bXN+/2Z0i/9edJj/VXOi/05zr/9Gc7z/Q3TC/6mNF/+mjh7/no4q/5eON/+OjkL/ho5P/3+OW/92jWf/b45z/2aNfv9gjIv/WIyY/06Nov9Gja//P427/zuOwf+mpRn/oqYf/5umLP+Tpjn/iaZD/4GmUP95pl3/caZn/2mmdP9gpn7/W6WL/1OlmP9IpaT/QaWx/zmmvP82p8L/n78a/5zAIP+UwSv/jcE4/4LBQ/97wU//dcFc/2zAaP9lwHT/XMB//1TAjP9NwJn/QsCj/zvAsP8zwL3/MMHD/5vXGf+X1x//kNgq/4fXNv9+2EL/dtlO/27ZW/9m2Gf/Xthz/1XXfv9Q14v/R9aX/z7Xov8216//Lti8/yvYwv8=',
  ],
  [
    '4:4:4 (1x2/1x2/1x2)',
    '/9j//gAPTGF2YzYzLjEuMTAyAP/bAEMACAQEBAQEBQUFBQUFBgYGBgYGBgYGBgYGBgcHBwgICAcHBwYGBwcICAgICQkJCAgICAkJCgoKDAwLCw4ODhERFP/EAFoAAQEAAAAAAAAAAAAAAAAAAAUGAQEBAQEAAAAAAAAAAAAAAAAGBQQHEAACAwEBAAAAAAAAAAAAAAAABAcjoTFBEQACAwEBAAAAAAAAAAAAAAAABQYjoTJC/8AAEQgACAAQAwESAAISAAMSAP/aAAwDAQACEQMRAD8AOTj/AJVhYp+FZ3M+rNDDv0HEMt5s0OIfIGnH/KsLFPwzO5n1ZoYd+jrCGW82aHEPk//Z',
    'xSoV/74qHv+2KSz/rik5/6cqRP+eKVD/lSld/5ApZv+EKXT/fil9/3Ypi/9uKZj/Zyqk/14pr/9VKbz/UCnF/75BFP+5QR3/sEEr/6hBOP+hQEL/mEFP/5BBXP+JQWX/fkFz/3lBfP9vQYr/aEGX/2FAof9YQa7/UEG7/0lBxP+6WxX/s1we/6tcK/+jXDf/m1tD/5NcT/+KW13/hVtm/3lbdP9zW33/a1uK/2Nbl/9bWqL/U1uu/0pbvP9FW8X/tHQW/610H/+mdCz/nnQ5/5V0Q/+OdFD/hHRd/390Zv90c3X/bXN+/2Z0i/9edJj/VXOi/05zr/9EdLz/P3TF/62NFv+ojR//no0s/5eNOf+OjUP/ho1Q/3+NXf94jWb/bY11/2iMfv9ejYv/V42Y/06Nov9Gja//P428/ziNxf+nphb/oqYf/5mlLf+Rpjn/iaVF/4GlUf95pV7/cqVn/2eldf9ipX7/WaWN/1GmmP9IpaT/QaWx/zmlvf8ypcb/o8AX/5zAIP+UwC3/i8A6/4TARP98wFH/c8Bf/26/aP9jv3b/XMB//1TAjP9LwJn/RL+j/zzAsP8zv77/Lr/H/5zYFv+X1x//jtgs/4XXOP9+2EP/dthQ/27XXv9n12f/XNd1/1fXfv9O2Iv/RdeX/z7Xov8216//Lte9/yfXxv8=',
  ],
]

for (const [name, jpeg, chrome] of FFMPEG_JPEG_CASES) {
  test(`ImageDecoder: ${name} JPEG written by FFmpeg decodes to BGRX like Chrome`, async (t) => {
    const image = await decodeImage(b64(jpeg), 'image/jpeg')
    t.is(image.format, 'BGRX')
    t.is(image.allocationSize(), 16 * 8 * 4)
    const copy = await rawCopy(image)
    t.deepEqual(copy.layout, [[0, 64]])
    t.deepEqual(copy.bytes, b64(chrome))
    image.close()
  })
}

// ============================================================================
// ImageDecoder: desiredWidth/desiredHeight (Chrome uses them only for JPEG)
// ============================================================================

test('ImageDecoder: desiredWidth/desiredHeight do not scale PNG, GIF, WebP, AVIF or BMP, like Chrome', async (t) => {
  for (const [data, type] of [
    [wptFixture('four-colors.png'), 'image/png'],
    [wptFixture('four-colors.gif'), 'image/gif'],
    [wptFixture('four-colors.webp'), 'image/webp'],
    [wptFixture('four-colors.avif'), 'image/avif'],
    [png(4, 2, 0, 8, [Array(4).fill(77), Array(4).fill(77)]), 'image/png'],
    [bmp(2, 1, 24, [[30, 20, 10, 60, 50, 40]]), 'image/bmp'],
  ] as const) {
    const full = await decodeImage(data, type)
    const fullBytes = (await rawCopy(full)).bytes
    for (const [desiredWidth, desiredHeight] of [
      [1, 1],
      [2, 1],
      [640, 480],
    ]) {
      const image = await decodeImage(data, type, { desiredWidth, desiredHeight })
      t.is(image.format, full.format, type)
      t.deepEqual([image.codedWidth, image.codedHeight], [full.codedWidth, full.codedHeight], type)
      t.deepEqual((await rawCopy(image)).bytes, fullBytes, type)
      image.close()
    }
    full.close()
  }
})

// [JPEG_CASES name, desired size, Chrome's size, Chrome's copyTo() bytes (BGRX)]. Chrome
// decodes at M/8 of the size, rounded up, with M = floor(sqrt(desired area / area) * 8),
// and to BGRX when M < 8. libjpeg-turbo scales in its IDCT.
const SCALED_JPEG_CASES: [string, [number, number], [number, number], string][] = [
  [
    '4:2:0',
    [8, 4],
    [8, 4],
    'wxAJ/7YRJv+jD0v/kg9q/4YQh/91Eaf/Yw/M/1QP6P/GUAn/ulEm/6VPSf+WT2r/iVGG/3hRp/9kT8r/V1Dn/7+RCP+xkiX/n5FJ/42Qaf+BkoX/cZKm/1yRyv9Rkuf/xtAI/7vRJf+mz0j/l89q/4nRhf960Kb/ZtDK/1nQ5v8=',
  ],
  [
    '4:2:2',
    [15, 7],
    [14, 7],
    'wAIH/70CEv+0Aif/rAE5/58DSv+YBFz/jAJu/4gDgv99AZP/dQKl/2oEt/9iA8n/WQLe/1YC6v/DJAj/wiUV/7glKf+wJDv/oyZN/5wnXv+RJXH/jCaF/4Ellv95Jqf/bCe4/2Qny/9bJuH/WSbs/8BMBf+/TRH/tUwl/61MOP+fTUj/mE5a/45Nbf+ITYD/fkyU/3dNpf9rTrX/Yk3H/1lM3f9YTen/wm8G/75wE/+3byf/rW85/6FwSv+acVv/j3Bv/4pwgv+AbpX/eG+n/2pxtv9jccn/WXDe/1hw6v/Ckwn/wJMU/7eTKP+vkzz/opRM/5qVXv+Pk3D/ipSE/3+Sl/94k6j/a5W4/2OVyv9alOD/WJTt/766BP+9uhH/tLon/6y5Of+gukj/mLta/466bf+IuoD/frmT/3e6pf9pu7b/YbrI/1i63P9Wu+j/w90H/8HeE/+33Cj/sNw7/6LeSv+b31z/kd5v/4vegv+B3Zb/ed6n/23euP9j3sr/XN7e/1nf6/8=',
  ],
  ['4:4:4', [4, 2], [4, 2], 'vDAZ/5swV/97MJj/XDDX/72wGP+csFf/fLCZ/1yx2f8='],
  [
    'grayscale',
    [8, 4],
    [8, 4],
    'IiIi/yoqKv8yMjL/OTk5/0FBQf9JSUn/UVFR/1hYWP9ISEj/UFBQ/1dXV/9fX1//Z2dn/29vb/92dnb/fn5+/21tbf91dXX/fX19/4SEhP+MjIz/lJSU/5ycnP+kpKT/k5OT/5ubm/+ioqL/qqqq/7Kysv+6urr/wsLC/8nJyf8=',
  ],
]

for (const [name, [desiredWidth, desiredHeight], [width, height], chrome] of SCALED_JPEG_CASES) {
  test(`ImageDecoder: a ${name} JPEG at desiredWidth/desiredHeight ${desiredWidth}x${desiredHeight} decodes to BGRX ${width}x${height}, like Chrome`, async (t) => {
    const jpeg = b64(JPEG_CASES.find((c) => c[0] === name)![1])
    const image = await decodeImage(jpeg, 'image/jpeg', { desiredWidth, desiredHeight })
    t.is(image.format, 'BGRX')
    t.deepEqual([image.codedWidth, image.codedHeight], [width, height])
    const copy = await rawCopy(image)
    t.deepEqual(copy.layout, [[0, width * 4]])
    t.deepEqual(copy.bytes, b64(chrome))
    image.close()
  })
}

test('ImageDecoder: a JPEG with a desired area at least its own decodes at full size, like Chrome', async (t) => {
  const jpeg = b64(JPEG_CASES[0][1])
  for (const [desiredWidth, desiredHeight] of [
    [16, 8],
    [128, 1],
    [1000, 1000],
  ]) {
    const image = await decodeImage(jpeg, 'image/jpeg', { desiredWidth, desiredHeight })
    t.is(image.format, 'I420')
    t.deepEqual([image.codedWidth, image.codedHeight], [16, 8])
    image.close()
  }
})

test("ImageDecoder: four-colors.jpg at desired sizes has Chrome's size and bytes, with Chrome's 32-bit math", async (t) => {
  const data = wptFixture('four-colors.jpg')
  // sha256 of Chrome's copyTo() bytes (BGRX)
  const full = 'da76d824e943b611eba6c53b51254d65789dc0e2d5722ea3f10469666fce7d8a'
  const half = '89a8e98e9d42e432c5cae4986e8b1736ad69b15545a79ff8885ecc943351894f'
  const sevenEighths = '51ec1ecf901c289bcc7ca83158b35faf6ff79bc6363fd17842508d2c1de40d8b'
  const eighth = '5bf78c35f1ddfe376b1305fa9d43eb6fb97b6944ed682868e5567fe01a3bd6dd'
  for (const [desiredWidth, desiredHeight, size, hash] of [
    [160, 120, [160, 120], half],
    // only the area counts
    [1280, 15, [160, 120], half],
    // ToNumber, then truncation
    ['160', '120', [160, 120], half],
    [160.9, 120.9, [160, 120], half],
    [319, 239, [280, 210], sevenEighths],
    [40, 30, [40, 30], eighth],
    // 4 bytes per desired pixel wrap at 2^32 (to 262144 and 18532 bytes)
    [65536, 16385, [280, 210], sevenEighths],
    [46341, 46341, [40, 30], eighth],
    [640, 480, [320, 240], full],
    // 0 (also null), or a side of 2^31 or more (a negative int): no desired size
    [0, 120, [320, 240], full],
    [null, 120, [320, 240], full],
    [2 ** 31, 1, [320, 240], full],
    [2 ** 32 - 1, 1, [320, 240], full],
  ] as const) {
    const image = await decodeImage(data, 'image/jpeg', { desiredWidth, desiredHeight })
    const label = `${desiredWidth}x${desiredHeight}`
    t.is(image.format, 'BGRX', label)
    t.deepEqual([image.codedWidth, image.codedHeight], size, label)
    t.is(sha256((await rawCopy(image)).bytes), hash, label)
    image.close()
  }
  // the budget wraps to 0: under 1/64 of the image
  const decoder = new ImageDecoder({ data, type: 'image/jpeg', desiredWidth: 65536, desiredHeight: 16384 })
  await t.throwsAsync(decoder.tracks.ready, { name: 'InvalidStateError', message: 'Failed to retrieve track metadata.' })
})

// ============================================================================
// ImageDecoder: AVIF with the identity matrix (gbrp)
// ============================================================================

const wptFixture = (name: string) =>
  new Uint8Array(readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'fixtures/wpt', name)))
const sha256 = (bytes: Uint8Array) => createHash('sha256').update(bytes).digest('hex')

test('ImageDecoder: a still AVIF with the identity matrix (gbrp) decodes to I444 with the G, B, R planes, like Chrome', async (t) => {
  const image = await decodeImage(wptFixture('four-colors.avif'), 'image/avif')
  t.is(image.format, 'I444')
  t.is(image.allocationSize(), 320 * 240 * 3)
  const { bytes, layout } = await rawCopy(image)
  t.deepEqual(layout, [
    [0, 320],
    [76800, 320],
    [153600, 320],
  ])
  // Chrome 155's bytes, and its G, B, R samples in the four quadrants
  t.is(sha256(bytes), 'd2d1b9fbfa388b547bd24d674e26667e2ce0c45a5353e2cf590a95e8d144fcaf')
  const gbr = (x: number, y: number) => [0, 1, 2].map((p) => bytes[p * 76800 + y * 320 + x])
  t.deepEqual(
    [gbr(80, 60), gbr(240, 60), gbr(80, 180), gbr(240, 180)],
    [
      [255, 0, 255],
      [0, 0, 255],
      [0, 255, 0],
      [255, 0, 0],
    ],
  )
  // copyTo RGBA reads the planes as RGB: yellow, red, blue, green
  const rgba = new Uint8Array(image.allocationSize({ format: 'RGBA' }))
  await image.copyTo(rgba, { format: 'RGBA' })
  const px = (x: number, y: number) => Array.from(rgba.subarray((y * 320 + x) * 4, (y * 320 + x) * 4 + 4))
  t.deepEqual(
    [px(80, 60), px(240, 60), px(80, 180), px(240, 180)],
    [
      [255, 255, 0, 255],
      [255, 0, 0, 255],
      [0, 0, 255, 255],
      [0, 255, 0, 255],
    ],
  )
  t.true(image.colorSpace.fullRange)
  image.close()
})

test('ImageDecoder: a 10-bit still AVIF with the identity matrix (gbrp10le) decodes to I444P10, like Chrome', async (t) => {
  // avifenc -d 10 -y 444 --cicp 1/13/0 -r full, 17x5
  const image = await decodeImage(
    b64(
      'AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUEAAADrbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAAAAAAAOcGl0bQAAAAAAAQAAAB5pbG9jAAAAAEQAAAEAAQAAAAEAAAETAAABFQAAAChpaW5mAAAAAAABAAAAGmluZmUCAAAAAAEAAGF2MDFDb2xvcgAAAABqaXBycAAAAEtpcGNvAAAAFGlzcGUAAAAAAAAAEQAAAAUAAAAQcGl4aQAAAAADCgoKAAAADGF2MUOBIEAAAAAAE2NvbHJuY2x4AAEADQAAgAAAABdpcG1hAAAAAAAAAAEAAQQBAoMEAAABHW1kYXQSAAoIOBChGMBDQAgyhgIRgAIIIIQA/7OADBdAzEPUfUID6UhLJmq+heFQMBP0ZA9HieUgsVfkYrARkrPP///0+sHOlNaDBnL1SQCGjpZg7NsI7K2dpPFglqm2Yn//64ZKdKkTxDDqmezHo7wdooBeI3zbX8PiLa/7Ab85qKfeC1h3LWiXFzJEBA8PCt1gkW+wHJ/+LFAK3+jPRR5I/bHBoh9+t1O1kLGZ7YlRDVQY4bv0uysEPy/vuPHsl70t5K/WPYa/0KD/alWyhqBxwFI+20QZ+M4ajEHnvsgvzk1/Ixn/Tnj9UCsSjyUHToW7FC1lvjSJ7VS4CAWhVqDc7hbf/84dHUElbJX+/HhzQwMDJ0QTDT/3',
    ),
    'image/avif',
  )
  t.is(image.format, 'I444P10')
  const { bytes, layout } = await rawCopy(image)
  t.deepEqual(layout, [
    [0, 34],
    [170, 34],
    [340, 34],
  ])
  t.deepEqual(
    Array.from(bytes),
    Array.from(
      b64(
        'kAGhAbgByQHgAfEBBwIbAjICQwJZAm0CgwKTAqgCvALSAq8CwALXAugC/wIQAyYDOgNRA2IDeAOMA6IDsgPHA9sD8QPKA98D8wMAABcAKAA+AFIAaQB6AJAApAC6AMoA3wDzAAkB5wD3AA8BIAE3AUgBXgFyAYkBmgGwAcQB2gHqAf8BEwIpAgMCFAIsAj0CUgJoAngCjgKiArcCyALeAvICBwMYAy4DQQMEADIAbACdANAABgE6AW8BoAHaAQYCPgJ1AqUC2wISA0EDeAA+ABwA5gCbAHMBTQEWAdgBmgF7Ak4CAgLQArECfwM2A+oA2gCCAHUAOADrAdIBhwFJAT8B4gLZApkCRwI1AukDqgNeAWoBNAHBAY8BWgBgACwA/ACKAFMDYgMxA/4DiQNVAh4C0AHnAbgBSwEAAdMA6QC8AHAAAgDbA/EDpAN2AwoD3AKTAh0AsABGAdgBbQIAA5UDKQC7AE8B4gF5AgwDpQM6ANEAXQFIANsAcQEDApoCMAPBA1sA6gB9AQ4CpAI4A80DYQD3AIgBdwAJAZ8BMgLGAlkD9QOAABMBqAE8AtMCaQP8A48AHwG2AaIANAHKAV0C8wKIAxwAsgBCAdUBaAL+ApcDIwC6AEwB4QHNAGAB9gGIAh4DtANFANsAbwEBApcCKgO+A1IA5AB4AQ8C',
      ),
    ),
  )
  image.close()
})

test('ImageDecoder: an animated AVIF with the identity matrix decodes to BGRX, like Chrome', async (t) => {
  const image = await decodeImage(wptFixture('four-colors-flip.avif'), 'image/avif')
  t.is(image.format, 'BGRX')
  t.is(sha256((await rawCopy(image)).bytes), 'da76d824e943b611eba6c53b51254d65789dc0e2d5722ea3f10469666fce7d8a')
  image.close()
})

test("ImageDecoder: a streamed 'avis' AVIF resolves nothing before its 'moov' box, then is BGRX, like Chrome", async (t) => {
  // avifenc -l --fps 1, two 8x4 frames (R = 32x, G = 64y, B = 200, then R = 200, G = 32x,
  // B = 64y), with the 'moov' box moved after 'mdat' and the iloc and stco offsets moved
  // with it. Bytes 0..429 (ftyp, meta, mdat) hold every frame but not the track, which the
  // 'avis' brand needs.
  const data = b64(
    'AAAALGZ0eXBhdmlzAAAAAGF2aWZhdmlzbXNmMWlzbzhtaWYxbWlhZk1BMUEAAADrbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAAAAAAAOcGl0bQAAAAAAAQAAAB5pbG9jAAAAAEQAAAEAAQAAAAEAAAEfAAAASwAAAChpaW5mAAAAAAABAAAAGmluZmUCAAAAAAEAAGF2MDFDb2xvcgAAAABqaXBycAAAAEtpcGNvAAAAFGlzcGUAAAAAAAAACAAAAAQAAAAQcGl4aQAAAAADCAgIAAAADGF2MUOBIAAAAAAAE2NvbHJuY2x4AAEADQAAgAAAABdpcG1hAAAAAAAAAAEAAQQBAoMEAAAAlm1kYXQSAAoMIAAAAQ/Nr5ICGgBAMjkQAIAAhsGyOHh+ofhKRcGopR8MBJib3oMJ/fEcKQB5nPtRbwL1Pk8Q6rWvgLj/w2uZPl//QEo2JyASADI/MAOAgP34egAEAAACrct9MQt6HZPdOYifvHj/XwybUkEN39rTRHllNN7RPTSXGiHKk9yXR6d3MDftgBedoycgAAACxm1vb3YAAAB4bXZoZAEAAAAAAAAA5vVOggAAAADm9U6CAAAAAf//////////AAEAAAEAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAEAAAJGdHJhawAAAGh0a2hkAQAAAQAAAADm9U6CAAAAAOb1ToIAAAABAAAAAP//////////AAAAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAQAAAAAAIAAAABAAAAAAALGVkdHMAAAAkZWxzdAEAAAEAAAABAAAAAAAAAAIAAAAAAAAAAAABAAAAAAGqbWRpYQAAACxtZGhkAQAAAAAAAADm9U6CAAAAAOb1ToIAAAABAAAAAAAAAAJVxAAAAAAAIWhkbHIAAAAAAAAAAHBpY3QAAAAAAAAAAAAAAAAAAAABVW1pbmYAAAAUdm1oZAAAAAEAAAAAAAAAAAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAARVzdGJsAAAAlXN0c2QAAAAAAAAAAQAAAIVhdjAxAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAAAgABABIAAAASAAAAAAAAAABCkFPTSBDb2RpbmcAAAAAAAAAAAAAAAAAAAAAAAAAAAAAGP//AAAADGF2MUOBIAAAAAAAE2NvbHJuY2x4AAEADQAAgAAAABBjY3N0AAAAAHwAAAAAAAAYc3R0cwAAAAAAAAABAAAAAgAAAAEAAAAcc3RzYwAAAAAAAAABAAAAAQAAAAIAAAABAAAAHHN0c3oAAAAAAAAAAAAAAAIAAABLAAAAQwAAABRzdGNvAAAAAAAAAAEAAAEfAAAAFHN0c3MAAAAAAAAAAQAAAAE=',
  )
  let release!: () => void
  const gate = new Promise<void>((resolve) => (release = resolve))
  const chunks = [data.slice(0, 429), data.slice(429)]
  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      // Hold the 'moov' box until the checks below are done.
      if (chunks.length === 1) await gate
      const chunk = chunks.shift()
      if (chunk) controller.enqueue(chunk)
      else controller.close()
    },
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/avif' })
  const pending = (promise: Promise<unknown>) =>
    Promise.race([promise.then(() => false), new Promise((resolve) => setTimeout(resolve, 300, true))])
  const decoding = decoder.decode({ frameIndex: 0 })
  // Chrome 155 waits for the track: neither tracks.ready nor decode() resolves
  t.true(await pending(decoder.tracks.ready))
  t.true(await pending(decoding))
  release()
  const early = (await decoding).image
  await decoder.completed
  const late = (await decoder.decode({ frameIndex: 0 })).image
  t.is(decoder.tracks.selectedTrack?.frameCount, 2)
  decoder.close()
  const whole = await decodeImage(data, 'image/avif')

  // Chrome 155 returns these BGRX bytes, streamed or not
  const chrome = Array.from({ length: 32 }, (_, i) => [200, 64 * Math.floor(i / 8), 32 * (i % 8), 255]).flat()
  for (const image of [early, late, whole]) {
    t.is(image.format, 'BGRX')
    t.deepEqual(Array.from((await rawCopy(image)).bytes), chrome)
    image.close()
  }
})

test("ImageDecoder: an AVIF with the identity matrix and an alpha plane decodes to BGRA with Chrome's bytes", async (t) => {
  // avifenc -l, 17x5 with alpha
  const image = await decodeImage(
    b64(
      'AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUEAAAGGbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAAAAAAAOcGl0bQAAAAAAAQAAACxpbG9jAAAAAEQAAAIAAQAAAAEAAAHmAAABQwACAAAAAQAAAa4AAAA4AAAAQmlpbmYAAAAAAAIAAAAaaW5mZQIAAAAAAQAAYXYwMUNvbG9yAAAAABppbmZlAgAAAAACAABhdjAxQWxwaGEAAAAAGmlyZWYAAAAAAAAADmF1eGwAAgABAAEAAADDaXBycAAAAJ1pcGNvAAAAFGlzcGUAAAAAAAAAEQAAAAUAAAAQcGl4aQAAAAADCAgIAAAADGF2MUOBIAAAAAAAE2NvbHJuY2x4AAEADQAAgAAAAA5waXhpAAAAAAEIAAAADGF2MUOBABwAAAAAOGF1eEMAAAAAdXJuOm1wZWc6bXBlZ0I6Y2ljcDpzeXN0ZW1zOmF1eGlsaWFyeTphbHBoYQAAAAAeaXBtYQAAAAAAAAACAAEEAQKDBAACBAEFhgcAAAGDbWRhdBIACgUYEKEYVDItEACjWjEPaehqtCmzusF203x/ZVTsmKwyCl3sqTyCraasU5t45zx4fWNQIuVEEgAKCDgQoRhAQ0AIMrQCEAAAqjGf///9oHeMXcgzYCXuxYyyXuu8we3yYCYCXu3hq7M1ENxFlTlT9Tno6w77Idc60OUxsOfV8akRy73d+C4NN9oWgUH1IDH89vF57WB70P8o5weAWa7/5lYtNpm+Wiz////hWb8QGdYER1k822tL27l5XLE8HEiH/D4NKZGxg4nVqzlb9xBqcdEn6E5l4y672f+y29iCJYvT8f33BtxbFRF3x98NLd/00yvxlzCbI+gDD2x6EXUKGB1perOpMl44YbLBZZcZIztB80ogITg0GUnIp1n//zdjwkCKr3/AAIf//4fObSXMtt1t1dTCd5LIRgRttoRgRtt1toRo4eqoPOx/mibc7fw1fJOYZDmfsGIiEiPyUDERCRCRGDERGDIDEy/Ez4aMdaDU+QkvrMHLnyA=',
    ),
    'image/avif',
  )
  t.is(image.format, 'BGRA')
  t.deepEqual(
    (await rawCopy(image)).bytes,
    b64(
      'AGQHAA1pLDIablFkJ3N2ljR4m8hBfcD6ToLlLFuHCl5ojC+QdZFUwoKWefSPm54mnKDDWKml6Iq2qg28w68y7tC0VyAdqxIAELA3Mge1XGQ6uoGWKb+myFzEy/pTyfAsRs4VXnXTOpBo2F/Cn92E9JLiqSaB585YtOzziqvxGLze9j3uzftiIDryHQA390IyIPxnZB0BjJYOBrHIewvW+nQQ+yxhFSBeUhpFkE8fasK4JI/0tSm0JqYu2ViTM/6KjDgjvPk9SO7qQm0gVzkoAFo+TTJNQ3JkcEiXlmNNvMgWUuH6GVcGLAxcK14/YVCQImZ1wtVrmvTYcL8my3XkWP56CYrhfy68lIRT7oeJeCB0gDMAeYVYMm6KfWRTj6KWQJTHyDWZ7Po6nhEsL6M2XhyoW5ABrYDC9rKl9Pu3yibovO9Y3cEUisLGOby3y17upNCDIA==',
    ),
  )
  image.close()
})

// ============================================================================
// ImageDecoder: AVIF read like CrabbyAvif (alpha, animation, Chrome's RGB bytes)
// ============================================================================

const avifFixture = (name: string) =>
  new Uint8Array(readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'fixtures/avif', name)))

// [fixture (avifenc 1.4.2, 64x48), ImageDecoderInit, then Chrome 155's format, size, frame count
// and the sha256 of each frame's copyTo() bytes]. Chrome keeps the YUV planes of one image
// without alpha, unless it is 4:0:0 or YCgCo-Re/Ro. Any other image is BGRA when it has an alpha
// image (even one whose samples are all 255), else BGRX.
const AVIF_CASES: [string, Record<string, unknown>, VideoPixelFormat, [number, number], number, string[]][] = [
  ['s8_420.avif', {}, 'I420', [64, 48], 1, ['9a5911d577ee9153154a4fcc263064f28e2c883d03e9b4dc6dce913354d71316']],
  // an alpha item, at every chroma layout and depth (CrabbyAvif converts each its own way)
  ['a8_400.avif', {}, 'BGRA', [64, 48], 1, ['b1bf7a6337e5f1c30fe35809535b68aff071bd62b8536d5b2667bccf0d267a08']],
  ['a10_400.avif', {}, 'BGRA', [64, 48], 1, ['415c28f7359a5120024a00966d6614dc04d17f252e52bb9ce4e02b0fca63be92']],
  ['a12_400.avif', {}, 'BGRA', [64, 48], 1, ['91aa8cf2328a7efbb8ba448218c4845fb7c93c948db3b620306f5f35a230b4ee']],
  ['a8_420.avif', {}, 'BGRA', [64, 48], 1, ['e9ea7a5738019e824d3a4048bfebf4a1b2d5f66a497b6b444df1667b8f6fcc66']],
  ['a10_420.avif', {}, 'BGRA', [64, 48], 1, ['7c73eefce1a87abea84b2f7105366ec1bcae093a4898c4b119985f6aebc54184']],
  ['a12_420.avif', {}, 'BGRA', [64, 48], 1, ['f9983d8ef81cab75979d377cbf8b407d21e50af3cefee88d42b45b1059bf553f']],
  ['a8_422.avif', {}, 'BGRA', [64, 48], 1, ['0b420ebeee7621cd23595f78e1214fd9e1e94e4ca094631770ee556719ae04a8']],
  ['a10_422.avif', {}, 'BGRA', [64, 48], 1, ['951f167e5fdf45e0cad5a0a6c1ecb737a0b56d2f0ed560211de1e0508aad914e']],
  ['a12_422.avif', {}, 'BGRA', [64, 48], 1, ['69cfd5f1c878fd2dac2f00bf81206123c50f15245dbdec91a53fdad1fb6e623a']],
  ['a8_444.avif', {}, 'BGRA', [64, 48], 1, ['9f2e4f4d804611452d4abcc0791734d631654128bc2d3b3e3de7f0529d177648']],
  ['a10_444.avif', {}, 'BGRA', [64, 48], 1, ['fc87ebc28de71c853cd91edfdf9bb80daab189fdb1bf7628f8118eab55005923']],
  ['a12_444.avif', {}, 'BGRA', [64, 48], 1, ['684973d16ebc97c1b45713fe931b102023d6209db87b372fbb026fe59a855fea']],
  // MP4Box: alpha samples all 255
  ['aopitem8_420.avif', {}, 'BGRA', [64, 48], 1, ['54dc136e01cbbe386b47b3302c3d299c6f53c9b45564aefda10b05d06f854596']],
  // 'prem': Chrome un-premultiplies
  ['p8_420.avif', {}, 'BGRA', [64, 48], 1, ['49150539371492afc2b9dbace99e38cde0fd10d58caad6384ec7a84a976fb3fa']],
  // limited range
  ['l8_420a.avif', {}, 'BGRA', [64, 48], 1, ['3a64cefff2fdd769a1d464a342b07ca19a53d7ce576af5bfd93714c5962b9eee']],
  ['odd8_420a.avif', {}, 'BGRA', [33, 17], 1, ['9a95cd90cce1fcd67ad61aabb46140c9e8f54627faf56f38a1a3e9b249bf5fb6']],
  ['odd8_444a.avif', {}, 'BGRA', [33, 17], 1, ['bf1ab6699fda669efb8ebdd8fd981f5c1586115e8c2c0eda09a6dcdab25eff57']],
  // a8_420 with the nclx matrix patched to BT.709 (the AV1 sequence header says BT.601): the
  // item's nclx wins
  ['cpatch_m1_420a.avif', {}, 'BGRA', [64, 48], 1, ['25fb011f3b3e84b281eef8b8f255aeeea9ccfd4e7a68015786c0c661c5768696']],
  // a8_420 with iloc v2 and the colour item in 'idat', as 2 extents in reverse order
  ['idat8_420a.avif', {}, 'BGRA', [64, 48], 1, ['e9ea7a5738019e824d3a4048bfebf4a1b2d5f66a497b6b444df1667b8f6fcc66']],
  // avifenc --progressive: an item in 2 extents, which FFmpeg's mov demuxer cannot read
  ['prog8_420.avif', {}, 'I420', [64, 48], 1, ['381aafb402b14a233d14a58489c086c48ec0de61f4bcc00313a8b8c11906803f']],
  // no alpha, but 4:0:0 or YCgCo-Re
  ['s10_400.avif', {}, 'BGRX', [64, 48], 1, ['94364978ed150d6a3974f676af44f5204964fef827681bc558658f4495591e29']],
  ['nm16_444_10.avif', {}, 'BGRX', [64, 48], 1, ['188a34848b8092799c3e054c8d828cc9c1066ae7aedbc511353e23276f5810f8']],
  // 'avis' tracks: one frame, three frames, three frames with an alpha track and 'prem', and
  // that one with co64, 2 chunks and runs in stsc and stts
  ['avis1_8_420.avif', {}, 'I420', [64, 48], 1, ['4d5585cebe94f474eb6bff24b274df89d86ea60092c90ba89c68f57647a76b07']],
  [
    'anim8_420.avif',
    {},
    'BGRX',
    [64, 48],
    3,
    [
      'bd5ed34f941a8b630305b2087591e0f53883c0c96fe69b13f8c6974ee179b495',
      'a50824648ea584a86fcc8cedc1788d8d5c3974fe07f667daba06afd2e56fbfae',
      '5fe876a40c42ccfadcbab00565b9a5e623310cfdf4463b97fae6258ec4ded879',
    ],
  ],
  [
    'panim8_420a.avif',
    {},
    'BGRA',
    [64, 48],
    3,
    [
      '72529243c81a8e8d969a3b7b7fd284a89644f4a806313e4430b8b945bfb6b3d6',
      '3b76fd899774397f8028ec4931ac4c40489717effe5b520030311766abedd8cf',
      '9434f3960c2b64338295e492250ca587ffc5f4ee323adc7ed3fa4fd603421119',
    ],
  ],
  [
    'tables_panim8_420a.avif',
    {},
    'BGRA',
    [64, 48],
    3,
    [
      '72529243c81a8e8d969a3b7b7fd284a89644f4a806313e4430b8b945bfb6b3d6',
      '3b76fd899774397f8028ec4931ac4c40489717effe5b520030311766abedd8cf',
      '9434f3960c2b64338295e492250ca587ffc5f4ee323adc7ed3fa4fd603421119',
    ],
  ],
  // anim8_420 with the sample entry's colr patched to BT.709: Chrome ignores it
  [
    'cpatch_m1_anim.avif',
    {},
    'BGRX',
    [64, 48],
    3,
    [
      'bd5ed34f941a8b630305b2087591e0f53883c0c96fe69b13f8c6974ee179b495',
      'a50824648ea584a86fcc8cedc1788d8d5c3974fe07f667daba06afd2e56fbfae',
      '5fe876a40c42ccfadcbab00565b9a5e623310cfdf4463b97fae6258ec4ded879',
    ],
  ],
  // preferAnimation: false decodes the primary item of an 'avis' file
  [
    'anim8_420.avif',
    { preferAnimation: false },
    'I420',
    [64, 48],
    1,
    ['4d5585cebe94f474eb6bff24b274df89d86ea60092c90ba89c68f57647a76b07'],
  ],
  [
    'panim8_420a.avif',
    { preferAnimation: false },
    'BGRA',
    [64, 48],
    1,
    ['72529243c81a8e8d969a3b7b7fd284a89644f4a806313e4430b8b945bfb6b3d6'],
  ],
]

for (const [name, init, format, size, frameCount, hashes] of AVIF_CASES) {
  const options = Object.keys(init).length ? ` with ${JSON.stringify(init)}` : ''
  test(`ImageDecoder: ${name}${options} decodes to ${frameCount} ${format} frame(s), like Chrome`, async (t) => {
    const decoder = new ImageDecoder({ data: avifFixture(name), type: 'image/avif', ...init })
    await decoder.tracks.ready
    t.is(decoder.tracks.selectedTrack?.frameCount, frameCount)
    for (const [i, hash] of hashes.entries()) {
      const { image } = await decoder.decode({ frameIndex: i })
      t.is(image.format, format)
      t.deepEqual([image.codedWidth, image.codedHeight], size)
      t.is(sha256((await rawCopy(image)).bytes), hash, `frame ${i}`)
      image.close()
    }
    decoder.close()
  })
}

test('ImageDecoder: AVIF track frames take their timestamps and durations from the track', async (t) => {
  // [timestamp, duration] of each frame. Chrome 155 truncates each duration to microseconds
  // and sums those (66666 for 2/30 s, also as a duration); here they are rounded.
  for (const [name, timing] of [
    ['anim8_420.avif', [0, 33333, 33333, 33333, 66667, 33333]],
    // stts (1, 1), (2, 2) at timescale 30
    ['tables_panim8_420a.avif', [0, 33333, 33333, 66667, 100000, 66667]],
    ['avis1_8_420.avif', [0, null]],
    ['a8_420.avif', [0, null]],
  ] as const) {
    const decoder = new ImageDecoder({ data: avifFixture(name), type: 'image/avif' })
    const frames = []
    for (let i = 0; i < timing.length / 2; i++) {
      const { image } = await decoder.decode({ frameIndex: i })
      frames.push(image.timestamp, image.duration)
      image.close()
    }
    t.deepEqual(frames, [...timing], name)
    decoder.close()
  }
})

test('ImageDecoder: a streamed AVIF with an alpha track is BGRA from its first frame, like Chrome', async (t) => {
  const data = avifFixture('panim8_420a.avif')
  let offset = 0
  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      await new Promise((resolve) => setTimeout(resolve, 5))
      if (offset >= data.length) return controller.close()
      controller.enqueue(data.slice(offset, offset + 256))
      offset += 256
    },
  })
  const decoder = new ImageDecoder({ data: stream, type: 'image/avif' })
  const { image } = await decoder.decode({ frameIndex: 0 })
  t.false(decoder.complete)
  t.is(decoder.tracks.selectedTrack?.frameCount, 3)
  t.is(image.format, 'BGRA')
  t.is(sha256((await rawCopy(image)).bytes), AVIF_CASES.find((c) => c[0] === 'panim8_420a.avif')![5][0])
  image.close()
  await decoder.completed
  decoder.close()
})

test('ImageDecoder: an AVIF that CrabbyAvif cannot convert to RGB fails decode() with EncodingError, like Chrome', async (t) => {
  for (const name of [
    // 4:0:0 with the ICtCp matrix (14)
    'n400m14.avif',
    // SMPTE 240M 4:2:0 with alpha and an av1C chroma_sample_position of 1 (not centred)
    'x_csp1_m240_420a.avif',
  ]) {
    const decoder = new ImageDecoder({ data: avifFixture(name), type: 'image/avif' })
    await decoder.tracks.ready
    t.is(decoder.tracks.selectedTrack?.frameCount, 1)
    await t.throwsAsync(decoder.decode(), { name: 'EncodingError', message: 'Failed to decode frame at index 0' }, name)
    decoder.close()
  }
})

test('ImageDecoder: an AVIF whose track samples end past its data fails the metadata, like Chrome', async (t) => {
  // avis1_8_420 with 3 samples in its only chunk (stsc): 2 of them past the end of the file
  const data = avifFixture('avis1_8_420.avif')
  new DataView(data.buffer).setUint32(Buffer.from(data).indexOf('stsc') + 16, 3)
  const decoder = new ImageDecoder({ data, type: 'image/avif' })
  await t.throwsAsync(decoder.tracks.ready, { name: 'InvalidStateError', message: 'Failed to retrieve track metadata.' })
})

test('ImageDecoder: a grid AVIF still decodes with FFmpeg, to its first tile', async (t) => {
  // Chrome 155 decodes all 4 tiles (I420 128x128); grids are not read like CrabbyAvif yet
  const image = await decodeImage(avifFixture('grid8_420.avif'), 'image/avif')
  t.is(image.format, 'I420')
  t.deepEqual([image.codedWidth, image.codedHeight], [64, 64])
  image.close()
})

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
  // The scaler was created with an unknown pixel format and swscale aborted (exit 134). Like
  // Chrome, a PNG is now decoded at full size.
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
  t.deepEqual(result, { format: 'BGRX', width: 64, first: [77, 77, 77, 255] })
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
