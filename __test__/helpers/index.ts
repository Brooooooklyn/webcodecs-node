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
  EncodedAudioChunkOutput as NativeEncodedAudioChunkOutput,
} from '../../index.js'

// Re-export types from the native module
export type { EncodedVideoChunkOutput } from '../../index.js'
export type { EncodedVideoChunkMetadata } from '../../index.js'
export type { EncodedAudioChunkMetadata } from '../../index.js'
export type { EncodedAudioChunkOutput } from '../../index.js'

// Note: Both VideoEncoder and AudioEncoder callbacks now receive separate arguments
// (chunk, metadata) per W3C WebCodecs spec. The old tuple types are no longer used.

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
 * AudioEncoder now passes EncodedAudioChunkOutput plain objects through
 * ThreadsafeFunction callbacks (with FnArgs). This helper reconstructs
 * a proper EncodedAudioChunk instance from that output.
 */
export function reconstructAudioChunk(chunk: NativeEncodedAudioChunkOutput): EncodedAudioChunk {
  return new EncodedAudioChunk({
    type: chunk.type,
    timestamp: chunk.timestamp,
    duration: chunk.duration ?? undefined,
    data: chunk.data,
  })
}
