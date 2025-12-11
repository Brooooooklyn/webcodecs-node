/**
 * WPT Test Utilities
 *
 * Helper functions adapted from W3C Web Platform Tests (WPT) for WebCodecs.
 * These utilities provide consistent test patterns matching the WPT test harness.
 */

import type { ExecutionContext } from 'ava'

import type {
  AudioData,
  AudioDecoder,
  AudioEncoder,
  EncodedAudioChunk,
  EncodedVideoChunk,
  VideoDecoder,
  VideoEncoder,
  VideoFrame,
} from '../../index.js'

/**
 * Gives a chance to pending output and error callbacks to complete before resolving.
 * This is useful for ensuring all async callbacks have fired.
 */
export function endAfterEventLoopTurn(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

/**
 * Returns a codec initialization with callbacks that fail the test if called.
 * Use this when you don't expect any output or error callbacks.
 */
export function getDefaultCodecInit(t: ExecutionContext) {
  return {
    output: () => {
      t.fail('unexpected output callback')
    },
    error: () => {
      t.fail('unexpected error callback')
    },
  }
}

/**
 * Create a codec init that collects output and errors into arrays.
 */
export function createCollectingCodecInit<TOutput>() {
  const outputs: TOutput[] = []
  const errors: Error[] = []
  return {
    init: {
      output: (chunk: TOutput) => {
        outputs.push(chunk)
      },
      error: (e: Error) => {
        errors.push(e)
      },
    },
    outputs,
    errors,
  }
}

/**
 * Create a codec init that tracks error callbacks with a promise.
 */
export function createErrorTrackingCodecInit<TOutput>() {
  const outputs: TOutput[] = []
  let errorCount = 0
  let resolveError: (e: Error) => void
  const gotError = new Promise<Error>((resolve) => {
    resolveError = resolve
  })

  return {
    init: {
      output: (chunk: TOutput) => {
        outputs.push(chunk)
      },
      error: (e: Error) => {
        errorCount++
        resolveError(e)
      },
    },
    outputs,
    getErrorCount: () => errorCount,
    gotError,
  }
}

/**
 * Makes sure that we cannot close, configure, reset, flush, decode or encode a
 * closed codec. Tests the behavior of a codec in closed state.
 */
export async function testClosedCodec(
  t: ExecutionContext,
  codec: VideoEncoder | VideoDecoder | AudioEncoder | AudioDecoder,
  validConfig: object,
  codecInput: VideoFrame | AudioData | EncodedVideoChunk | EncodedAudioChunk,
) {
  t.is(codec.state, 'unconfigured')

  codec.close()
  t.is(codec.state, 'closed')

  // Configure should throw on closed codec
  t.throws(
    () => {
      ;(codec as VideoEncoder).configure(validConfig as unknown as Parameters<VideoEncoder['configure']>[0])
    },
    { name: 'InvalidStateError' },
    'configure should throw InvalidStateError',
  )

  // Reset should throw on closed codec
  t.throws(
    () => {
      codec.reset()
    },
    { name: 'InvalidStateError' },
    'reset should throw InvalidStateError',
  )

  // Close should throw on already closed codec (per W3C spec)
  t.throws(
    () => {
      codec.close()
    },
    { name: 'InvalidStateError' },
    'close should throw InvalidStateError',
  )

  // Encode/decode should throw on closed codec
  if ('encode' in codec) {
    t.throws(
      () => {
        ;(codec as VideoEncoder).encode(codecInput as VideoFrame)
      },
      { name: 'InvalidStateError' },
      'encode should throw InvalidStateError',
    )
  } else if ('decode' in codec) {
    t.throws(
      () => {
        ;(codec as VideoDecoder).decode(codecInput as EncodedVideoChunk)
      },
      { name: 'InvalidStateError' },
      'decode should throw InvalidStateError',
    )
  }

  // Flush should reject on closed codec
  await t.throwsAsync(codec.flush(), { name: 'InvalidStateError' }, 'flush should reject with InvalidStateError')
}

/**
 * Makes sure we cannot flush, encode or decode with an unconfigured codec,
 * and that reset is a valid no-op.
 */
export async function testUnconfiguredCodec(
  t: ExecutionContext,
  codec: VideoEncoder | VideoDecoder | AudioEncoder | AudioDecoder,
  codecInput: VideoFrame | AudioData | EncodedVideoChunk | EncodedAudioChunk,
) {
  t.is(codec.state, 'unconfigured')

  // Resetting an unconfigured encoder is a no-op
  codec.reset()
  t.is(codec.state, 'unconfigured')

  // Encode/decode should throw on unconfigured codec
  if ('encode' in codec) {
    t.throws(
      () => {
        ;(codec as VideoEncoder).encode(codecInput as VideoFrame)
      },
      { name: 'InvalidStateError' },
      'encode should throw InvalidStateError on unconfigured',
    )
  } else if ('decode' in codec) {
    t.throws(
      () => {
        ;(codec as VideoDecoder).decode(codecInput as EncodedVideoChunk)
      },
      { name: 'InvalidStateError' },
      'decode should throw InvalidStateError on unconfigured',
    )
  }

  // Flush should reject on unconfigured codec
  await t.throwsAsync(
    codec.flush(),
    { name: 'InvalidStateError' },
    'flush should reject with InvalidStateError on unconfigured',
  )
}

/**
 * Creates a detached ArrayBuffer for testing detached buffer handling.
 * Uses MessageChannel to detach the buffer.
 */
export function makeDetachedArrayBuffer(): ArrayBufferLike {
  const buffer = new ArrayBuffer(10)
  // Transfer the buffer to detach it - this is Node.js specific
  // In Node.js 22+, we can use structuredClone with transfer
  try {
    // Try using structuredClone with transfer (Node.js 17+)
    structuredClone(buffer, { transfer: [buffer] })
  } catch {
    // Fallback: manually detach by resizing to 0 (Node.js 20+)
    try {
      ;(buffer as ArrayBuffer & { resize: (n: number) => void }).resize(0)
    } catch {
      // If neither works, just return an empty ArrayBuffer view
      // The test may need to be skipped on older Node.js versions
    }
  }
  return buffer
}

/**
 * Checks if a VideoFrame is closed by examining its properties.
 */
export function isFrameClosed(frame: VideoFrame): boolean {
  return (
    frame.format === null &&
    frame.codedWidth === 0 &&
    frame.codedHeight === 0 &&
    frame.displayWidth === 0 &&
    frame.displayHeight === 0
  )
}

/**
 * Creates audio data for testing - generates a sine wave.
 */
export function makeAudioData(
  timestamp: number,
  channels: number,
  sampleRate: number,
  frames: number,
  AudioDataClass: new (init: {
    timestamp: number
    data: Float32Array
    numberOfChannels: number
    numberOfFrames: number
    sampleRate: number
    format: string
  }) => AudioData,
): AudioData {
  const data = new Float32Array(frames * channels)

  // Generate samples in planar format
  for (let channel = 0; channel < channels; channel++) {
    const hz = 100 + channel * 50 // sound frequency
    const baseIndex = channel * frames
    for (let i = 0; i < frames; i++) {
      const t = ((i / sampleRate) * hz * Math.PI * 2) % (Math.PI * 2)
      data[baseIndex + i] = Math.sin(t)
    }
  }

  return new AudioDataClass({
    timestamp,
    data,
    numberOfChannels: channels,
    numberOfFrames: frames,
    sampleRate,
    format: 'f32-planar',
  })
}

/**
 * Video color space property sets for comprehensive testing.
 */
export const VIDEO_COLOR_SPACE_SETS = {
  primaries: ['bt709', 'bt470bg', 'smpte170m', 'bt2020', 'smpte432'] as const,
  transfer: ['bt709', 'smpte170m', 'iec61966-2-1', 'srgb', 'linear', 'pq', 'hlg'] as const,
  matrix: ['rgb', 'bt709', 'bt470bg', 'smpte170m', 'bt2020-ncl'] as const,
  fullRange: [true, false] as const,
}

/**
 * Generates all combinations of video color space properties for testing.
 */
export function generateAllColorSpaceCombinations(): Array<import('../../standard.js').VideoColorSpaceInit> {
  const keys = Object.keys(VIDEO_COLOR_SPACE_SETS) as Array<keyof typeof VIDEO_COLOR_SPACE_SETS>
  const colorSpaces: Array<import('../../standard.js').VideoColorSpaceInit> = []
  generateColorSpaceCombinationsHelper(keys, 0, {}, colorSpaces)
  return colorSpaces
}

function generateColorSpaceCombinationsHelper(
  keys: Array<keyof typeof VIDEO_COLOR_SPACE_SETS>,
  keyIndex: number,
  colorSpace: import('../../standard.js').VideoColorSpaceInit,
  results: Array<import('../../standard.js').VideoColorSpaceInit>,
): void {
  if (keyIndex >= keys.length) {
    results.push({ ...colorSpace })
    return
  }

  const prop = keys[keyIndex]
  // case 1: Skip this property
  generateColorSpaceCombinationsHelper(keys, keyIndex + 1, colorSpace, results)
  // case 2: Set this property with a valid value
  for (const val of VIDEO_COLOR_SPACE_SETS[prop]) {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    ;(colorSpace as any)[prop] = val
    generateColorSpaceCombinationsHelper(keys, keyIndex + 1, colorSpace, results)
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    delete (colorSpace as any)[prop]
  }
}

/**
 * Sample format test values for AudioData conversion tests.
 */
export const TEST_VALUES = {
  u8: [0, 255, 191, 64, 128, 256],
  s16: [-32768, 32767, 16383, -16384, 0, 65536],
  s32: [-2147483648, 2147483647, 1073741823, -1073741824, 0, 4294967296],
  f32: [-1.0, 1.0, 0.5, -0.5, 0, 16777216],
}

/**
 * Maps sample format string to TypedArray constructor.
 */
export function typeToArrayType(
  type: 'u8' | 's16' | 's32' | 'f32',
): Uint8ArrayConstructor | Int16ArrayConstructor | Int32ArrayConstructor | Float32ArrayConstructor {
  switch (type) {
    case 'u8':
      return Uint8Array
    case 's16':
      return Int16Array
    case 's32':
      return Int32Array
    case 'f32':
      return Float32Array
    default:
      throw new Error(`Unexpected type: ${type as string}`)
  }
}

/**
 * Infers sample format type from TypedArray instance.
 */
export function arrayTypeToType(
  array: Uint8Array | Int16Array | Int32Array | Float32Array,
): 'u8' | 's16' | 's32' | 'f32' {
  if (array instanceof Uint8Array) return 'u8'
  if (array instanceof Int16Array) return 's16'
  if (array instanceof Int32Array) return 's32'
  if (array instanceof Float32Array) return 'f32'
  throw new Error('Unexpected array type')
}

/**
 * Delay utility for async tests.
 */
export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/**
 * Wait for a condition to be true, with timeout.
 */
export async function waitFor(
  condition: () => boolean,
  description: string,
  timeoutMs = 10000,
  intervalMs = 10,
): Promise<void> {
  const startTime = Date.now()
  while (!condition()) {
    if (Date.now() - startTime > timeoutMs) {
      throw new Error(`Timeout waiting for: ${description}`)
    }
    await delay(intervalMs)
  }
}
