/**
 * Small PNG, BMP and GIF files built in memory, for decoder tests.
 */

import { deflateSync } from 'node:zlib'

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
export function png(
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

/** Big-endian bytes of 16-bit samples, for 16-bit PNG rows. */
export const be16 = (samples: number[]) => samples.flatMap((v) => [v >> 8, v & 0xff])

/** Little-endian bytes of 16-bit words, for 16-bit BMP rows. */
export const le16 = (words: number[]) => words.flatMap((v) => [v & 0xff, v >> 8])

export interface BmpOptions {
  /** DIB header size: 40, or 56 (V3), 108 (V4), 124 (V5) */
  header?: number
  /** 0 BI_RGB, 3 BI_BITFIELDS */
  compression?: number
  /** R, G, B (and A) masks: in a V3+ header, or after a 40-byte one */
  masks?: number[]
  palette?: number[][]
  colorsUsed?: number
  /** Pixel data as stored, in place of `rows` (RLE) */
  raw?: number[]
}

/** Bottom-up BMP. `rows` are top-down, already in file byte order. */
export function bmp(
  width: number,
  height: number,
  bpp: number,
  rows: number[][],
  { header = 40, compression = 0, masks = [], palette = [], colorsUsed = palette.length, raw }: BmpOptions = {},
): Uint8Array {
  const rowBytes = Math.ceil((width * bpp) / 32) * 4
  const extraMasks = header === 40 ? masks.length * 4 : 0
  const offset = 14 + header + extraMasks + palette.length * 4
  const dataLength = raw ? raw.length : rowBytes * height
  const buf = Buffer.alloc(offset + dataLength)
  buf.write('BM', 0, 'latin1')
  buf.writeUInt32LE(buf.length, 2)
  buf.writeUInt32LE(offset, 10)
  buf.writeUInt32LE(header, 14)
  buf.writeInt32LE(width, 18)
  buf.writeInt32LE(height, 22)
  buf.writeUInt16LE(1, 26)
  buf.writeUInt16LE(bpp, 28)
  buf.writeUInt32LE(compression, 30)
  buf.writeUInt32LE(dataLength, 34)
  buf.writeUInt32LE(colorsUsed, 46)
  // the masks follow the first 40 header bytes, inside a V3+ header
  masks.forEach((mask, i) => buf.writeUInt32LE(mask >>> 0, 54 + i * 4))
  // V4+: LCS_sRGB
  if (header >= 108) buf.writeUInt32LE(0x73524742, 70)
  palette.forEach(([r, g, b], i) => Buffer.from([b, g, r, 0]).copy(buf, 14 + header + extraMasks + i * 4))
  if (raw) Buffer.from(raw).copy(buf, offset)
  else rows.forEach((row, y) => Buffer.from(row).copy(buf, offset + (height - 1 - y) * rowBytes))
  return new Uint8Array(buf)
}

export interface GifFrame {
  x: number
  y: number
  width: number
  height: number
  /** Palette index of each pixel, row by row */
  indices: number[]
  /** Transparent palette index (graphic control extension) */
  transparent?: number
  /** Disposal method (graphic control extension): 1 keep, 2 restore background, 3 restore previous */
  dispose?: number
  /** Delay (graphic control extension), in hundredths of a second */
  delay?: number
  /** LZW codes to store in place of the codes of `indices`, for broken image data */
  codes?: number[]
}

/** LZW codes at a fixed code width: a clear code before the table would grow. */
function lzw(indices: number[], minCodeSize: number): number[] {
  const clear = 1 << minCodeSize
  const perClear = (1 << (minCodeSize + 1)) - (clear + 2)
  const codes: number[] = []
  for (let i = 0; i < indices.length; i += perClear) codes.push(clear, ...indices.slice(i, i + perClear))
  codes.push(clear + 1)
  return codes
}

/** Image data: the minimum code size, then the codes, one bit wider, in data sub-blocks. */
function imageData(codes: number[], minCodeSize: number): number[] {
  const width = minCodeSize + 1
  const out: number[] = []
  let acc = 0
  let bits = 0
  for (const code of codes) {
    acc |= code << bits
    bits += width
    for (; bits >= 8; bits -= 8, acc >>>= 8) out.push(acc & 0xff)
  }
  if (bits) out.push(acc & 0xff)
  const blocks = [minCodeSize]
  for (let i = 0; i < out.length; i += 255) blocks.push(Math.min(255, out.length - i), ...out.slice(i, i + 255))
  return [...blocks, 0]
}

/** GIF89a with a global palette of 2^n colors (`palette` is padded with black). */
export function gif(width: number, height: number, palette: number[][], frames: GifFrame[]): Uint8Array {
  const bits = Math.max(1, Math.ceil(Math.log2(palette.length)))
  const colors = Array.from({ length: 1 << bits }, (_, i) => palette[i] ?? [0, 0, 0])
  const u16 = (v: number) => [v & 0xff, v >> 8]
  const out = [...Buffer.from('GIF89a'), ...u16(width), ...u16(height), 0x80 | 0x70 | (bits - 1), 0, 0, ...colors.flat()]
  const minCodeSize = Math.max(2, bits)
  for (const f of frames) {
    if (f.transparent !== undefined || f.dispose !== undefined || f.delay !== undefined) {
      const flags = ((f.dispose ?? 0) << 2) | (f.transparent === undefined ? 0 : 1)
      out.push(0x21, 0xf9, 4, flags, ...u16(f.delay ?? 0), f.transparent ?? 0, 0)
    }
    out.push(0x2c, ...u16(f.x), ...u16(f.y), ...u16(f.width), ...u16(f.height), 0)
    out.push(...imageData(f.codes ?? lzw(f.indices, minCodeSize), minCodeSize))
  }
  out.push(0x3b)
  return Uint8Array.from(out)
}
