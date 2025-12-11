/**
 * Frame comparison utilities
 *
 * Provides PSNR (Peak Signal-to-Noise Ratio) calculation for
 * comparing original and decoded video frames in lossy codec tests.
 */

import { VideoFrame } from '../../index.js'

/**
 * Frame comparison result
 */
export interface FrameComparisonResult {
  /** Peak Signal-to-Noise Ratio in dB (higher is better, Infinity for identical) */
  psnr: number
  /** Mean Squared Error (lower is better, 0 for identical) */
  mse: number
  /** Whether the frames are considered identical (pixel-perfect match) */
  identical: boolean
  /** Whether PSNR exceeds the acceptable threshold */
  acceptable: boolean
  /** Number of pixels compared */
  pixelCount: number
}

/**
 * PSNR quality thresholds
 *
 * These thresholds are based on typical video codec quality metrics:
 * - 30 dB: Acceptable quality for lossy compression
 * - 35 dB: Good quality
 * - 40 dB: High quality, hard to distinguish from original
 * - 50 dB+: Near-lossless
 */
export const PSNRThresholds = {
  /** Minimum acceptable quality for lossy codecs */
  acceptable: 30,
  /** Good quality threshold */
  good: 35,
  /** High quality, nearly indistinguishable from original */
  high: 40,
  /** Near-lossless quality */
  nearLossless: 50,
} as const

/**
 * Calculate Mean Squared Error between two buffers
 *
 * MSE = (1/n) * sum((original[i] - decoded[i])^2)
 */
export function calculateMSE(original: Uint8Array, decoded: Uint8Array): number {
  if (original.length !== decoded.length) {
    throw new Error(`Buffer size mismatch: original=${original.length}, decoded=${decoded.length}`)
  }

  if (original.length === 0) {
    return 0
  }

  let sumSquaredError = 0
  for (let i = 0; i < original.length; i++) {
    const diff = original[i] - decoded[i]
    sumSquaredError += diff * diff
  }

  return sumSquaredError / original.length
}

/**
 * Calculate Peak Signal-to-Noise Ratio
 *
 * PSNR = 10 * log10(MAX^2 / MSE)
 *
 * For 8-bit images, MAX = 255
 * Returns Infinity if MSE is 0 (identical images)
 */
export function calculatePSNR(original: Uint8Array, decoded: Uint8Array): number {
  const mse = calculateMSE(original, decoded)

  if (mse === 0) {
    return Infinity
  }

  const maxValue = 255
  return 10 * Math.log10((maxValue * maxValue) / mse)
}

/**
 * Compare two video frames and return quality metrics
 *
 * @param original - The original uncompressed frame
 * @param decoded - The frame after encode/decode roundtrip
 * @param threshold - PSNR threshold to consider acceptable (default: 30 dB)
 */
export async function compareFrames(
  original: VideoFrame,
  decoded: VideoFrame,
  threshold: number = PSNRThresholds.acceptable,
): Promise<FrameComparisonResult> {
  // Verify dimensions match
  if (original.codedWidth !== decoded.codedWidth || original.codedHeight !== decoded.codedHeight) {
    throw new Error(
      `Frame dimension mismatch: original=${original.codedWidth}x${original.codedHeight}, ` +
        `decoded=${decoded.codedWidth}x${decoded.codedHeight}`,
    )
  }

  // Extract pixel data
  const originalSize = original.allocationSize()
  const decodedSize = decoded.allocationSize()

  if (originalSize !== decodedSize) {
    throw new Error(`Frame allocation size mismatch: original=${originalSize}, decoded=${decodedSize}`)
  }

  const originalData = new Uint8Array(originalSize)
  const decodedData = new Uint8Array(decodedSize)

  await original.copyTo(originalData)
  await decoded.copyTo(decodedData)

  const mse = calculateMSE(originalData, decodedData)
  const psnr = mse === 0 ? Infinity : 10 * Math.log10((255 * 255) / mse)
  const identical = mse === 0
  const acceptable = psnr >= threshold || identical

  return {
    psnr,
    mse,
    identical,
    acceptable,
    pixelCount: originalData.length,
  }
}

/**
 * Compare raw buffers and return quality metrics
 *
 * @param original - Original pixel data
 * @param decoded - Decoded pixel data
 * @param threshold - PSNR threshold to consider acceptable
 */
export function compareBuffers(
  original: Uint8Array,
  decoded: Uint8Array,
  threshold: number = PSNRThresholds.acceptable,
): FrameComparisonResult {
  const mse = calculateMSE(original, decoded)
  const psnr = mse === 0 ? Infinity : 10 * Math.log10((255 * 255) / mse)
  const identical = mse === 0
  const acceptable = psnr >= threshold || identical

  return {
    psnr,
    mse,
    identical,
    acceptable,
    pixelCount: original.length,
  }
}

/**
 * Compare Y channel only (luminance) for I420 frames
 *
 * Luminance is typically more sensitive to quality issues than chroma.
 */
export async function compareI420LuminanceOnly(
  original: VideoFrame,
  decoded: VideoFrame,
  threshold: number = PSNRThresholds.acceptable,
): Promise<FrameComparisonResult> {
  const width = original.codedWidth
  const height = original.codedHeight
  const ySize = width * height

  // Extract Y plane only
  const originalData = new Uint8Array(original.allocationSize())
  const decodedData = new Uint8Array(decoded.allocationSize())

  await original.copyTo(originalData)
  await decoded.copyTo(decodedData)

  const originalY = originalData.subarray(0, ySize)
  const decodedY = decodedData.subarray(0, ySize)

  return compareBuffers(originalY, decodedY, threshold)
}

/**
 * Batch compare multiple frame pairs
 *
 * Returns aggregate statistics across all frames.
 */
export interface BatchComparisonResult {
  /** Number of frames compared */
  frameCount: number
  /** Minimum PSNR across all frames */
  minPSNR: number
  /** Maximum PSNR across all frames */
  maxPSNR: number
  /** Average PSNR across all frames */
  avgPSNR: number
  /** Number of frames meeting the threshold */
  acceptableCount: number
  /** Number of identical frames */
  identicalCount: number
  /** Whether all frames meet the threshold */
  allAcceptable: boolean
  /** Individual frame results */
  results: FrameComparisonResult[]
}

export function compareFrameBatch(
  originals: VideoFrame[],
  decoded: VideoFrame[],
  threshold: number = PSNRThresholds.acceptable,
): BatchComparisonResult {
  if (originals.length !== decoded.length) {
    throw new Error(`Frame count mismatch: original=${originals.length}, decoded=${decoded.length}`)
  }

  if (originals.length === 0) {
    return {
      frameCount: 0,
      minPSNR: Infinity,
      maxPSNR: -Infinity,
      avgPSNR: 0,
      acceptableCount: 0,
      identicalCount: 0,
      allAcceptable: true,
      results: [],
    }
  }

  const results: FrameComparisonResult[] = []
  let totalPSNR = 0
  let minPSNR = Infinity
  let maxPSNR = -Infinity
  let acceptableCount = 0
  let identicalCount = 0

  for (let i = 0; i < originals.length; i++) {
    const result = compareFrames(originals[i], decoded[i], threshold)
    results.push(result)

    if (result.psnr !== Infinity) {
      totalPSNR += result.psnr
      minPSNR = Math.min(minPSNR, result.psnr)
      maxPSNR = Math.max(maxPSNR, result.psnr)
    }

    if (result.acceptable) acceptableCount++
    if (result.identical) identicalCount++
  }

  // Handle case where all frames are identical
  const finiteResults = results.filter((r) => r.psnr !== Infinity)
  const avgPSNR = finiteResults.length > 0 ? totalPSNR / finiteResults.length : Infinity

  return {
    frameCount: originals.length,
    minPSNR: minPSNR === Infinity ? Infinity : minPSNR,
    maxPSNR: maxPSNR === -Infinity ? Infinity : maxPSNR,
    avgPSNR,
    acceptableCount,
    identicalCount,
    allAcceptable: acceptableCount === originals.length,
    results,
  }
}

/**
 * Format PSNR value for display
 */
export function formatPSNR(psnr: number): string {
  if (psnr === Infinity) {
    return 'Inf dB (identical)'
  }
  return `${psnr.toFixed(2)} dB`
}

/**
 * Get quality description based on PSNR value
 */
export function getQualityDescription(psnr: number): string {
  if (psnr === Infinity) return 'identical'
  if (psnr >= PSNRThresholds.nearLossless) return 'near-lossless'
  if (psnr >= PSNRThresholds.high) return 'high'
  if (psnr >= PSNRThresholds.good) return 'good'
  if (psnr >= PSNRThresholds.acceptable) return 'acceptable'
  return 'poor'
}
