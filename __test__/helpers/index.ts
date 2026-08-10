/**
 * Test helpers index
 *
 * Re-exports all test utilities for convenient importing.
 */

import { getPreferredHardwareAccelerator, VideoEncoder } from '../../index.js'

/**
 * Check if we're running in a CI environment.
 */
export function isCI(): boolean {
  return Boolean(process.env.CI || process.env.GITHUB_ACTIONS)
}

/**
 * Check if hardware acceleration is available and usable on this system.
 * Returns true only if there's a working hardware accelerator AND we're not in CI.
 *
 * Note: In CI environments (GitHub Actions, etc.), hardware accelerators like VideoToolbox
 * may be detected as "available" but are not actually usable because of VM limitations.
 */
export function hasHardwareAcceleration(): boolean {
  // In CI environments, hardware is detected but not usable
  if (isCI()) {
    return false
  }
  return getPreferredHardwareAccelerator() !== null
}

/**
 * Cache for HEVC alpha support check - only need to check once per process.
 */
let hevcAlphaSupportChecked = false
let hevcAlphaSupported = false

/**
 * Check if HEVC alpha encoding is supported (requires libx265 built with -DENABLE_ALPHA=ON).
 * This checks synchronously if possible, or returns cached result.
 */
export async function hasHevcAlphaSupport(): Promise<boolean> {
  if (hevcAlphaSupportChecked) {
    return hevcAlphaSupported
  }

  return new Promise((resolve) => {
    let errorReceived = false
    const encoder = new VideoEncoder({
      output: () => {},
      error: (e) => {
        // Check if the error is specifically about alpha not being supported
        if (e.message.includes('does not support alpha')) {
          errorReceived = true
        }
      },
    })

    encoder.configure({
      codec: 'hev1.1.6.L93.B0',
      width: 64,
      height: 64,
      alpha: 'keep',
      hardwareAcceleration: 'prefer-software',
    })

    // The "unsupported" verdict arrives asynchronously on the error callback. A
    // negative can therefore be detected as soon as it lands, while a positive can
    // only be concluded by waiting. Poll for the negative and fall back to a
    // generous deadline, rather than betting the verdict on a fixed 50ms budget a
    // loaded runner can overshoot -- a misdetection here silently poisons every
    // test that depends on it. The result is cached for the rest of the process.
    const deadline = Date.now() + 2_000
    const settle = () => {
      hevcAlphaSupportChecked = true
      hevcAlphaSupported = encoder.state === 'configured' && !errorReceived
      if (encoder.state !== 'closed') {
        encoder.close()
      }
      resolve(hevcAlphaSupported)
    }
    const poll = () => {
      if (errorReceived || encoder.state !== 'configured' || Date.now() >= deadline) {
        settle()
        return
      }
      setTimeout(poll, 10)
    }
    setTimeout(poll, 10)
  })
}

export * from './frame-generator.js'
export * from './frame-comparator.js'
export * from './codec-matrix.js'
export * from './audio-generator.js'
export * from './wpt-utils.js'

// Re-export types from the native module
export type { EncodedVideoChunk } from '../../index.js'
export type { EncodedVideoChunkMetadata } from '../../index.js'
export type { EncodedAudioChunk } from '../../index.js'
export type { EncodedAudioChunkMetadata } from '../../index.js'

// Note: Encoder callbacks now receive EncodedVideoChunk/EncodedAudioChunk class instances
// directly per W3C WebCodecs spec.
