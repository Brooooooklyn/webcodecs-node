/**
 * Test helpers index
 *
 * Re-exports all test utilities for convenient importing.
 */

export * from './frame-generator.js'
export * from './frame-comparator.js'
export * from './codec-matrix.js'
export * from './audio-generator.js'

import {
  EncodedVideoChunk,
  EncodedAudioChunk,
  EncodedVideoChunkOutput as NativeEncodedVideoChunkOutput,
  EncodedVideoChunkMetadata as NativeEncodedVideoChunkMetadata,
  EncodedAudioChunkMetadata as NativeEncodedAudioChunkMetadata,
} from '../../index.js'

// Re-export types from the native module
export type { EncodedVideoChunkOutput } from '../../index.js'
export type { EncodedVideoChunkMetadata } from '../../index.js'
export type { EncodedAudioChunkMetadata } from '../../index.js'

/**
 * Output type from AudioEncoder callback
 * (plain object, not class instance)
 *
 * AudioEncoder passes EncodedAudioChunk class instance directly,
 * not a plain object like VideoEncoder.
 */
export interface EncodedAudioChunkOutput {
  type: EncodedAudioChunk['type']
  timestamp: number
  duration?: number | null
  data: Buffer
  byteLength: number
}

/**
 * Tuple type returned from VideoEncoder output callback
 */
export type VideoEncoderOutput = [NativeEncodedVideoChunkOutput, NativeEncodedVideoChunkMetadata]

/**
 * Tuple type returned from AudioEncoder output callback
 */
export type AudioEncoderOutput = [EncodedAudioChunkOutput, NativeEncodedAudioChunkMetadata]

/**
 * Reconstruct an EncodedVideoChunk from callback data.
 *
 * VideoEncoder now passes EncodedVideoChunkOutput plain objects through
 * ThreadsafeFunction callbacks (with CalleeHandled: false). This helper
 * reconstructs a proper EncodedVideoChunk instance from that output.
 */
export function reconstructVideoChunk(chunk: NativeEncodedVideoChunkOutput): EncodedVideoChunk {
  // Handle case where chunk properties might be null
  const chunkType = chunk.type
  const timestamp = chunk.timestamp
  const duration = chunk.duration
  const data = chunk.data

  if (chunkType === null || timestamp === null || data === null) {
    throw new Error(
      `EncodedVideoChunk has null properties: type=${chunkType}, timestamp=${timestamp}, data=${data === null ? 'null' : 'ok'}`,
    )
  }

  return new EncodedVideoChunk({
    type: chunkType,
    timestamp: timestamp,
    duration: duration ?? undefined,
    data: data,
  })
}

/**
 * Reconstruct an EncodedAudioChunk from callback data.
 *
 * AudioEncoder now passes EncodedAudioChunk instances through
 * ThreadsafeFunction callbacks. This helper reconstructs a proper instance.
 */
export function reconstructAudioChunk(chunk: EncodedAudioChunkOutput): EncodedAudioChunk {
  return new EncodedAudioChunk({
    type: chunk.type,
    timestamp: chunk.timestamp,
    duration: chunk.duration ?? undefined,
    data: chunk.data,
  })
}
