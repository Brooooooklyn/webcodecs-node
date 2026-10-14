/**
 * libyuv's integer YUV -> RGB conversion (row_common.cc YuvConstants, LIBYUV_UNLIMITED_* off).
 *
 * Chrome runs this as the first stage of VideoFrame.copyTo({ format: 'RGBA' | ... }), and so does
 * this addon. Used as the exact reference for copyTo on non-solid frames.
 */

import type { VideoColorSpaceInit } from '../../index.js'

/** [YG, YB, UB, UG, VG, VR] */
export type LibyuvTable = readonly [number, number, number, number, number, number]

export const LIBYUV = {
  I601: [18997, -1160, 128, 25, 52, 102],
  JPEG: [16320, 32, 113, 22, 46, 90],
  H709: [18997, -1160, 128, 14, 34, 115],
  F709: [16320, 32, 119, 12, 30, 101],
  B2020: [19003, -1160, 128, 12, 42, 107],
  V2020: [16320, 32, 120, 11, 37, 94],
} as const satisfies Record<string, LibyuvTable>

export interface Rect {
  x: number
  y: number
  width: number
  height: number
}

const clamp255 = (v: number) => (v < 0 ? 0 : v > 255 ? 255 : v)

/** One pixel from 16-bit expanded luma and 8-bit chroma. */
function yuvPixel(y16: number, u: number, v: number, t: LibyuvTable): [number, number, number] {
  const y1 = Math.floor((y16 * t[0]) / 65536)
  return [
    clamp255((y1 + (v - 128) * t[5] + t[1]) >> 6),
    clamp255((y1 - ((u - 128) * t[3] + (v - 128) * t[4]) + t[1]) >> 6),
    clamp255((y1 + (u - 128) * t[2] + t[1]) >> 6),
  ]
}

/** 8-bit Y, U, V -> [R, G, B]. */
export function yuvPixel8(y: number, u: number, v: number, t: LibyuvTable): [number, number, number] {
  return yuvPixel(y * 0x0101, u, v, t)
}

/** 10-bit Y, U, V -> [R, G, B]. libyuv cuts chroma to 8 bits first. */
export function yuvPixel10(y: number, u: number, v: number, t: LibyuvTable): [number, number, number] {
  return yuvPixel((y << 6) | (y >> 4), Math.min(u >> 2, 255), Math.min(v >> 2, 255), t)
}

/** Planar YUV samples; `u`/`v` rows are `ceil(width / 2^hs)` samples wide. */
export interface YuvPlanes {
  width: number
  /** log2 chroma subsampling: I420 1/1, I422 1/0, I444 0/0 */
  hs: number
  vs: number
  bits: 8 | 10
  y: ArrayLike<number>
  u: ArrayLike<number>
  v: ArrayLike<number>
  a?: ArrayLike<number>
}

/** Converts `rect` to tightly packed RGBA with nearest chroma, like copyTo({ format: 'RGBA' }). */
export function yuvToRgba(src: YuvPlanes, t: LibyuvTable, rect: Rect): Uint8Array {
  const cw = (src.width + (1 << src.hs) - 1) >> src.hs
  const pixel = src.bits === 10 ? yuvPixel10 : yuvPixel8
  const out = new Uint8Array(rect.width * rect.height * 4)
  for (let r = 0; r < rect.height; r++) {
    for (let c = 0; c < rect.width; c++) {
      const x = rect.x + c
      const y = rect.y + r
      const ci = (y >> src.vs) * cw + (x >> src.hs)
      const rgb = pixel(src.y[y * src.width + x], src.u[ci], src.v[ci], t)
      out.set([...rgb, src.a ? src.a[y * src.width + x] : 255], 4 * (r * rect.width + c))
    }
  }
  return out
}

const f32 = Math.fround
const INV255 = f32(1 / 255)

/** Round half to even, like Skia's float to int store. */
const rint = (v: number) => {
  const r = Math.round(v)
  return r - v === 0.5 && r % 2 !== 0 ? r - 1 : r
}

/**
 * Chrome's alpha handling when a copy changes the format but not the colour: an 8-bit premultiply
 * ('rgb': SkSwizzler (c * a + 127) / 255, 'yuv': libyuv ARGBAttenuate (c * a + 255) >> 8, 'none' for
 * RGBX/BGRX sources), then Skia's float32 unpremultiply. Alpha 0 gives 0.
 */
export function chromeAlphaRoundTrip(c: number, a: number, premultiply: 'rgb' | 'yuv' | 'none'): number {
  const p = premultiply === 'rgb' ? Math.floor((c * a + 127) / 255) : premultiply === 'yuv' ? (c * a + 255) >> 8 : c
  const scale = a === 0 ? 0 : f32(1 / f32(a * INV255))
  return rint(f32(Math.min(f32(f32(p * INV255) * scale), 1) * 255))
}

/** Tightly packed 8-bit I420 (as copyTo() writes it) -> RGBA for `rect`. */
export function i420ToRgba(planes: Uint8Array, width: number, height: number, t: LibyuvTable, rect: Rect) {
  const ySize = width * height
  const cSize = Math.ceil(width / 2) * Math.ceil(height / 2)
  return yuvToRgba(
    {
      width,
      hs: 1,
      vs: 1,
      bits: 8,
      y: planes.subarray(0, ySize),
      u: planes.subarray(ySize, ySize + cSize),
      v: planes.subarray(ySize + cSize, ySize + 2 * cSize),
    },
    t,
    rect,
  )
}

/** The table Chrome picks from the matrix and range members only (missing matrix -> I601). */
export function libyuvTableFor(cs: VideoColorSpaceInit | null | undefined): LibyuvTable {
  const full = cs?.fullRange === true
  switch (cs?.matrix) {
    case 'bt709':
      return full ? LIBYUV.F709 : LIBYUV.H709
    case 'bt470bg':
    case 'smpte170m':
      return full ? LIBYUV.JPEG : LIBYUV.I601
    case 'bt2020-ncl':
      return full ? LIBYUV.V2020 : LIBYUV.B2020
    default:
      return LIBYUV.I601
  }
}
