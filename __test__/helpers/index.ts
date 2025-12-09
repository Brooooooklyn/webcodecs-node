/**
 * Test helpers index
 *
 * Re-exports all test utilities for convenient importing.
 */

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
