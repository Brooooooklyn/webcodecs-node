/**
 * Test frame generation utilities
 *
 * Provides functions to generate test VideoFrames with various patterns
 * for encoding/decoding validation.
 */

import { VideoFrame, type VideoFrameBufferInit } from '../../index.js'

/** RGB color representation */
export interface RGBColor {
  r: number // 0-255
  g: number // 0-255
  b: number // 0-255
}

/** Common test colors */
export const TestColors = {
  red: { r: 255, g: 0, b: 0 },
  green: { r: 0, g: 255, b: 0 },
  blue: { r: 0, g: 0, b: 255 },
  white: { r: 255, g: 255, b: 255 },
  black: { r: 0, g: 0, b: 0 },
  gray: { r: 128, g: 128, b: 128 },
  yellow: { r: 255, g: 255, b: 0 },
  cyan: { r: 0, g: 255, b: 255 },
  magenta: { r: 255, g: 0, b: 255 },
} as const

/**
 * Convert RGB to YUV (BT.601)
 *
 * Y  =  0.299 * R + 0.587 * G + 0.114 * B
 * U  = -0.169 * R - 0.331 * G + 0.500 * B + 128
 * V  =  0.500 * R - 0.419 * G - 0.081 * B + 128
 */
export function rgbToYuv(color: RGBColor): { y: number; u: number; v: number } {
  const y = Math.round(0.299 * color.r + 0.587 * color.g + 0.114 * color.b)
  const u = Math.round(-0.169 * color.r - 0.331 * color.g + 0.5 * color.b + 128)
  const v = Math.round(0.5 * color.r - 0.419 * color.g - 0.081 * color.b + 128)
  return {
    y: Math.max(0, Math.min(255, y)),
    u: Math.max(0, Math.min(255, u)),
    v: Math.max(0, Math.min(255, v)),
  }
}

/**
 * Calculate I420 buffer size
 *
 * I420 layout: Y plane (w*h) + U plane (w/2 * h/2) + V plane (w/2 * h/2)
 */
export function calculateI420Size(width: number, height: number): number {
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  return ySize + uvSize * 2
}

/**
 * Calculate RGBA buffer size
 */
export function calculateRGBASize(width: number, height: number): number {
  return width * height * 4
}

/**
 * Generate a solid color I420 frame
 *
 * Creates a VideoFrame filled with a single color, useful for basic encode/decode testing.
 */
export function generateSolidColorI420Frame(
  width: number,
  height: number,
  color: RGBColor,
  timestamp: number,
  duration?: number,
): VideoFrame {
  const yuv = rgbToYuv(color)
  const bufferSize = calculateI420Size(width, height)
  const buffer = Buffer.alloc(bufferSize)

  // Y plane
  const ySize = width * height
  buffer.fill(yuv.y, 0, ySize)

  // U plane (half resolution)
  const uvWidth = width / 2
  const uvHeight = height / 2
  const uSize = uvWidth * uvHeight
  buffer.fill(yuv.u, ySize, ySize + uSize)

  // V plane
  buffer.fill(yuv.v, ySize + uSize, ySize + uSize * 2)

  const init: VideoFrameBufferInit = {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp,
    duration,
  }

  return new VideoFrame(new Uint8Array(buffer), init)
}

/**
 * Generate a solid color RGBA frame
 */
export function generateSolidColorRGBAFrame(
  width: number,
  height: number,
  color: RGBColor,
  timestamp: number,
  alpha: number = 255,
  duration?: number,
): VideoFrame {
  const bufferSize = calculateRGBASize(width, height)
  const buffer = Buffer.alloc(bufferSize)

  for (let i = 0; i < width * height; i++) {
    const offset = i * 4
    buffer[offset] = color.r
    buffer[offset + 1] = color.g
    buffer[offset + 2] = color.b
    buffer[offset + 3] = alpha
  }

  const init: VideoFrameBufferInit = {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp,
    duration,
  }

  return new VideoFrame(new Uint8Array(buffer), init)
}

/**
 * Generate a horizontal gradient I420 frame
 *
 * Creates a gradient from black to white horizontally, useful for visual testing.
 */
export function generateGradientI420Frame(width: number, height: number, timestamp: number, duration?: number): VideoFrame {
  const bufferSize = calculateI420Size(width, height)
  const buffer = Buffer.alloc(bufferSize)

  // Y plane - gradient from 16 (black) to 235 (white) in video range
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const luma = Math.round(16 + (219 * x) / (width - 1))
      buffer[y * width + x] = luma
    }
  }

  // U and V planes - neutral (128)
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  buffer.fill(128, ySize, ySize + uvSize * 2)

  const init: VideoFrameBufferInit = {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp,
    duration,
  }

  return new VideoFrame(new Uint8Array(buffer), init)
}

/**
 * Generate a checkerboard I420 frame
 *
 * Creates an alternating pattern, useful for detecting encoding artifacts.
 */
export function generateCheckerboardI420Frame(
  width: number,
  height: number,
  timestamp: number,
  blockSize: number = 16,
  duration?: number,
): VideoFrame {
  const bufferSize = calculateI420Size(width, height)
  const buffer = Buffer.alloc(bufferSize)

  // Y plane - checkerboard pattern
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const blockX = Math.floor(x / blockSize)
      const blockY = Math.floor(y / blockSize)
      const isWhite = (blockX + blockY) % 2 === 0
      buffer[y * width + x] = isWhite ? 235 : 16
    }
  }

  // U and V planes - neutral
  const ySize = width * height
  const uvSize = (width / 2) * (height / 2)
  buffer.fill(128, ySize, ySize + uvSize * 2)

  const init: VideoFrameBufferInit = {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp,
    duration,
  }

  return new VideoFrame(new Uint8Array(buffer), init)
}

/**
 * Generate a color bars I420 frame (SMPTE-like pattern)
 *
 * Creates vertical color bars useful for codec color fidelity testing.
 */
export function generateColorBarsI420Frame(width: number, height: number, timestamp: number, duration?: number): VideoFrame {
  const colors = [
    TestColors.white,
    TestColors.yellow,
    TestColors.cyan,
    TestColors.green,
    TestColors.magenta,
    TestColors.red,
    TestColors.blue,
    TestColors.black,
  ]

  const bufferSize = calculateI420Size(width, height)
  const buffer = Buffer.alloc(bufferSize)

  const barWidth = Math.floor(width / colors.length)
  const ySize = width * height
  const uvWidth = width / 2
  const uvHeight = height / 2

  // Pre-calculate YUV values for each color
  const yuvColors = colors.map((c) => rgbToYuv(c))

  // Y plane
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const colorIndex = Math.min(Math.floor(x / barWidth), colors.length - 1)
      buffer[y * width + x] = yuvColors[colorIndex].y
    }
  }

  // U plane
  for (let y = 0; y < uvHeight; y++) {
    for (let x = 0; x < uvWidth; x++) {
      const colorIndex = Math.min(Math.floor((x * 2) / barWidth), colors.length - 1)
      buffer[ySize + y * uvWidth + x] = yuvColors[colorIndex].u
    }
  }

  // V plane
  const uSize = uvWidth * uvHeight
  for (let y = 0; y < uvHeight; y++) {
    for (let x = 0; x < uvWidth; x++) {
      const colorIndex = Math.min(Math.floor((x * 2) / barWidth), colors.length - 1)
      buffer[ySize + uSize + y * uvWidth + x] = yuvColors[colorIndex].v
    }
  }

  const init: VideoFrameBufferInit = {
    format: 'I420',
    codedWidth: width,
    codedHeight: height,
    timestamp,
    duration,
  }

  return new VideoFrame(new Uint8Array(buffer), init)
}

/**
 * Generate a sequence of frames with incrementing timestamps
 *
 * Useful for testing multi-frame encoding scenarios.
 */
export function generateFrameSequence(
  width: number,
  height: number,
  count: number,
  frameDurationUs: number = 33333, // ~30fps
  generator: 'solid' | 'gradient' | 'checkerboard' | 'colorbars' = 'gradient',
): VideoFrame[] {
  const frames: VideoFrame[] = []

  for (let i = 0; i < count; i++) {
    const timestamp = i * frameDurationUs
    let frame: VideoFrame

    switch (generator) {
      case 'solid':
        // Cycle through colors for variety
        const colorKeys = Object.keys(TestColors) as (keyof typeof TestColors)[]
        const color = TestColors[colorKeys[i % colorKeys.length]]
        frame = generateSolidColorI420Frame(width, height, color, timestamp, frameDurationUs)
        break
      case 'gradient':
        frame = generateGradientI420Frame(width, height, timestamp, frameDurationUs)
        break
      case 'checkerboard':
        frame = generateCheckerboardI420Frame(width, height, timestamp, 16, frameDurationUs)
        break
      case 'colorbars':
        frame = generateColorBarsI420Frame(width, height, timestamp, frameDurationUs)
        break
    }

    frames.push(frame)
  }

  return frames
}

/**
 * Extract raw I420 data from a VideoFrame
 *
 * Note: VideoFrame.copyTo() is async per W3C spec, so this function is async.
 */
export async function extractI420Data(frame: VideoFrame): Promise<Uint8Array> {
  const size = frame.allocationSize()
  const buffer = new Uint8Array(size)
  await frame.copyTo(buffer)
  return buffer
}

/**
 * Common test resolutions
 */
export const TestResolutions = {
  qvga: { width: 320, height: 240 },
  vga: { width: 640, height: 480 },
  hd720: { width: 1280, height: 720 },
  hd1080: { width: 1920, height: 1080 },
  uhd4k: { width: 3840, height: 2160 },
  // WebCodecs requires dimensions divisible by 2 for I420
  small: { width: 128, height: 96 },
  medium: { width: 640, height: 360 },
} as const
