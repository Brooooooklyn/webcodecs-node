/**
 * WPT Frame Utilities
 *
 * Helper functions for creating and validating video frames in WPT-style tests.
 * These are Node.js equivalents of the browser-based WPT utilities that use OffscreenCanvas.
 */

import type { ExecutionContext } from 'ava'

import { VideoEncoder, VideoFrame } from '../../index.js'
import type { VideoEncoderConfig, VideoFrameBufferInit } from '../../standard.js'

const DOT_SIZE = 20
const DOT_STEP = DOT_SIZE * 2

/**
 * Four colors used in test frames (SMPTE-like pattern)
 * Top-left: Yellow, Top-right: Red, Bottom-left: Blue, Bottom-right: Green
 */
const FOUR_COLORS = {
  yellow: { r: 255, g: 255, b: 0 },
  red: { r: 255, g: 0, b: 0 },
  blue: { r: 0, g: 0, b: 255 },
  green: { r: 0, g: 255, b: 0 },
} as const

/**
 * Creates a four-color RGBA frame buffer
 */
function createFourColorsBuffer(width: number, height: number): Uint8Array {
  const buffer = new Uint8Array(width * height * 4)

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const offset = (y * width + x) * 4
      let color: { r: number; g: number; b: number }

      if (y < height / 2) {
        color = x < width / 2 ? FOUR_COLORS.yellow : FOUR_COLORS.red
      } else {
        color = x < width / 2 ? FOUR_COLORS.blue : FOUR_COLORS.green
      }

      buffer[offset] = color.r
      buffer[offset + 1] = color.g
      buffer[offset + 2] = color.b
      buffer[offset + 3] = 255 // alpha
    }
  }

  return buffer
}

/**
 * Puts black dots on the buffer at predefined positions.
 * This creates an analog of a basic barcode for validation.
 */
function putBlackDots(buffer: Uint8Array, width: number, height: number, count: number): void {
  for (let i = 1; i <= count; i++) {
    let x = i * DOT_STEP
    let y = DOT_STEP * (Math.floor(x / width) + 1)
    x = x % width

    // Draw a DOT_SIZE x DOT_SIZE black square
    for (let dy = 0; dy < DOT_SIZE && y + dy < height; dy++) {
      for (let dx = 0; dx < DOT_SIZE && x + dx < width; dx++) {
        const offset = ((y + dy) * width + (x + dx)) * 4
        buffer[offset] = 0 // R
        buffer[offset + 1] = 0 // G
        buffer[offset + 2] = 0 // B
        // Keep alpha at 255
      }
    }
  }
}

/**
 * Get the position of dot i for validation
 */
function getDotPosition(i: number, width: number): { x: number; y: number } {
  let x = i * DOT_STEP + DOT_SIZE / 2
  let y = DOT_STEP * (Math.floor(x / width) + 1) + DOT_SIZE / 2
  x = x % width

  // Adjust for sampling (center of dot)
  if (x > 0) x = x - 1
  if (y > 0) y = y - 1

  return { x: Math.floor(x), y: Math.floor(y) }
}

/**
 * Creates a frame with colored quadrants and N black dots for visual verification.
 * This is the Node.js equivalent of the WPT createDottedFrame function.
 *
 * @param width - Frame width
 * @param height - Frame height
 * @param dots - Number of dots to place (used for validation)
 * @param timestamp - Frame timestamp (defaults to dots value)
 */
export function createDottedFrame(width: number, height: number, dots: number, timestamp?: number): VideoFrame {
  const ts = timestamp ?? dots
  const duration = 33333 // ~30fps

  // Create RGBA buffer with four-color pattern
  const buffer = createFourColorsBuffer(width, height)

  // Add black dots
  putBlackDots(buffer, width, height, dots)

  const init: VideoFrameBufferInit = {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: ts,
    duration,
    // Set explicit colorSpace like browser canvas would provide
    colorSpace: {
      primaries: 'bt709',
      transfer: 'srgb',
      matrix: 'rgb',
      fullRange: true,
    },
  }

  return new VideoFrame(buffer, init)
}

/**
 * Creates a simple four-color frame without dots.
 *
 * @param width - Frame width
 * @param height - Frame height
 * @param timestamp - Frame timestamp
 */
export function createFrame(width: number, height: number, timestamp: number = 0): VideoFrame {
  const duration = 33333 // ~30fps
  const buffer = createFourColorsBuffer(width, height)

  const init: VideoFrameBufferInit = {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp,
    duration,
    // Set explicit colorSpace like browser canvas would provide
    colorSpace: {
      primaries: 'bt709',
      transfer: 'srgb',
      matrix: 'rgb',
      fullRange: true,
    },
  }

  return new VideoFrame(buffer, init)
}

/**
 * Validates that a frame has the expected number of black dots.
 * Works by copying frame data and checking pixel values at dot locations.
 *
 * @param frame - The VideoFrame to validate
 * @param expectedDots - Expected number of black dots
 * @returns true if validation passes
 */
export async function validateBlackDots(frame: VideoFrame, expectedDots: number): Promise<boolean> {
  const width = frame.displayWidth
  const height = frame.displayHeight

  // Copy frame data to RGBA buffer
  // Note: Frame may be in different format after decode, but we check based on luma
  const allocSize = frame.allocationSize()
  const buffer = new Uint8Array(allocSize)
  await frame.copyTo(buffer)

  // Get frame format to interpret the buffer correctly
  const format = frame.format

  // For validation, we need to check if specific positions are dark
  const tolerance = 60

  for (let i = 1; i <= expectedDots; i++) {
    const pos = getDotPosition(i, width)

    // Check if position is within bounds
    if (pos.x >= width || pos.y >= height) {
      continue
    }

    // Get pixel value based on format
    let isDark: boolean

    if (format === 'RGBA' || format === 'RGBX' || format === 'BGRA' || format === 'BGRX') {
      // RGBA/BGRA: 4 bytes per pixel
      const offset = (pos.y * width + pos.x) * 4
      const r = format === 'BGRA' || format === 'BGRX' ? buffer[offset + 2] : buffer[offset]
      const g = buffer[offset + 1]
      const b = format === 'BGRA' || format === 'BGRX' ? buffer[offset] : buffer[offset + 2]
      isDark = r <= tolerance && g <= tolerance && b <= tolerance
    } else if (format === 'I420' || format === 'I420A' || format === 'NV12') {
      // YUV formats: Check Y (luma) plane
      // Y value for black should be close to 16 (video range) or 0 (full range)
      const yOffset = pos.y * width + pos.x
      const y = buffer[yOffset]
      // Black in video range Y is around 16, full range is 0
      isDark = y <= tolerance
    } else {
      // For other formats, check first bytes as approximation
      const offset = pos.y * width + pos.x
      isDark = buffer[offset] <= tolerance
    }

    if (!isDark) {
      // The pixel at dot position is too bright - validation fails
      return false
    }
  }

  return true
}

/**
 * Check if encoder supports the given config, skip test if not.
 * This is the Node.js equivalent of WPT's checkEncoderSupport.
 *
 * @param t - AVA execution context
 * @param config - Video encoder configuration to check
 */
export async function checkEncoderSupport(t: ExecutionContext, config: VideoEncoderConfig): Promise<void> {
  try {
    const support = await VideoEncoder.isConfigSupported(config)
    if (!support.supported) {
      t.log(`Skipping: Unsupported config: ${JSON.stringify(config)}`)
      return t.pass()
    }
  } catch {
    t.log(`Skipping: Config check failed: ${JSON.stringify(config)}`)
    return t.pass()
  }
}

/**
 * Encoder configurations for different codec variants
 */
export const ENCODER_CONFIGS = {
  av1: {
    codec: 'av01.0.04M.08',
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
  av1_444_high: {
    codec: 'av01.1.04M.08.0.000',
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
  vp8: {
    codec: 'vp8',
    hasEmbeddedColorSpace: false,
    hardwareAcceleration: 'prefer-software' as const,
  },
  vp9_p0: {
    codec: 'vp09.00.10.08',
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
  vp9_p2: {
    codec: 'vp09.02.10.10',
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
  h264_avc: {
    codec: 'avc1.42001E',
    avc: { format: 'avc' as const },
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
  h264_annexb: {
    codec: 'avc1.42001E',
    avc: { format: 'annexb' as const },
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
  h265_hevc: {
    codec: 'hvc1.1.6.L123.00',
    hevc: { format: 'hevc' as const },
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
  h265_annexb: {
    codec: 'hvc1.1.6.L123.00',
    hevc: { format: 'annexb' as const },
    hasEmbeddedColorSpace: true,
    hardwareAcceleration: 'prefer-software' as const,
  },
} as const

/**
 * Create full encoder config with common defaults
 */
export function createEncoderConfig(
  variant: keyof typeof ENCODER_CONFIGS,
  width: number = 320,
  height: number = 200,
): VideoEncoderConfig {
  const base = ENCODER_CONFIGS[variant]
  return {
    ...base,
    width,
    height,
    bitrate: 1_000_000,
    bitrateMode: 'constant',
    framerate: 30,
  } as VideoEncoderConfig
}

/**
 * Common test resolutions for WPT tests
 */
export const WPT_RESOLUTIONS = {
  small: { width: 320, height: 200 },
  medium: { width: 640, height: 480 },
  large: { width: 800, height: 600 },
} as const
