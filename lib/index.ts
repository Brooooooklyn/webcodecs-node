/**
 * WebCodecs Node.js - Extended API with Async Iterators and Streams
 *
 * This module re-exports the native WebCodecs implementation and adds:
 * - Async iterator support for encoders/decoders
 * - Node.js Transform stream wrappers
 * - Promise-based flush methods
 */

// Re-export all native types and classes
export * from '../index.js'

import {
  VideoEncoder as NativeVideoEncoder,
  VideoDecoder as NativeVideoDecoder,
  AudioEncoder as NativeAudioEncoder,
  AudioDecoder as NativeAudioDecoder,
  VideoFrame,
  AudioData,
  EncodedVideoChunk,
  EncodedAudioChunk,
  type VideoEncoderConfig,
  type VideoDecoderConfig,
  type AudioEncoderConfig,
  type AudioDecoderConfig,
  type VideoEncoderEncodeOptions,
  type EncodedVideoChunkMetadata,
  type EncodedAudioChunkMetadata,
} from '../index.js'

/**
 * Extended VideoEncoder with async iterator support
 *
 * Example:
 * ```typescript
 * const encoder = new VideoEncoderAsync()
 * encoder.configure({ codec: 'avc1.42001E', width: 640, height: 480 })
 *
 * for (const frame of frames) {
 *   encoder.encode(frame)
 * }
 *
 * for await (const chunk of encoder.flush()) {
 *   await muxer.write(chunk)
 * }
 * ```
 */
export class VideoEncoderAsync extends NativeVideoEncoder {
  /**
   * Async iterator that yields encoded chunks as they become available
   */
  async *[Symbol.asyncIterator](): AsyncIterableIterator<EncodedVideoChunk> {
    while (this.hasOutput()) {
      const chunk = this.takeNextChunk()
      if (chunk) {
        yield chunk
      }
    }
  }

  /**
   * Flush the encoder and return all remaining chunks as an async iterable
   */
  async *flushAsync(): AsyncIterableIterator<EncodedVideoChunk> {
    super.flush()
    yield* this
  }

  /**
   * Flush and collect all remaining chunks into an array
   */
  async flushAll(): Promise<EncodedVideoChunk[]> {
    super.flush()
    return this.takeEncodedChunks()
  }

  /**
   * Drain all currently available output chunks
   */
  drain(): EncodedVideoChunk[] {
    return this.takeEncodedChunks()
  }
}

/**
 * Extended VideoDecoder with async iterator support
 */
export class VideoDecoderAsync extends NativeVideoDecoder {
  /**
   * Async iterator that yields decoded frames as they become available
   */
  async *[Symbol.asyncIterator](): AsyncIterableIterator<VideoFrame> {
    while (this.hasOutput()) {
      const frame = this.takeNextFrame()
      if (frame) {
        yield frame
      }
    }
  }

  /**
   * Flush the decoder and return all remaining frames as an async iterable
   */
  async *flushAsync(): AsyncIterableIterator<VideoFrame> {
    super.flush()
    yield* this
  }

  /**
   * Flush and collect all remaining frames into an array
   */
  async flushAll(): Promise<VideoFrame[]> {
    super.flush()
    return this.takeDecodedFrames()
  }

  /**
   * Drain all currently available output frames
   */
  drain(): VideoFrame[] {
    return this.takeDecodedFrames()
  }
}

/**
 * Extended AudioEncoder with async iterator support
 */
export class AudioEncoderAsync extends NativeAudioEncoder {
  /**
   * Async iterator that yields encoded chunks as they become available
   */
  async *[Symbol.asyncIterator](): AsyncIterableIterator<EncodedAudioChunk> {
    while (this.hasOutput()) {
      const chunk = this.takeNextChunk()
      if (chunk) {
        yield chunk
      }
    }
  }

  /**
   * Flush the encoder and return all remaining chunks as an async iterable
   */
  async *flushAsync(): AsyncIterableIterator<EncodedAudioChunk> {
    super.flush()
    yield* this
  }

  /**
   * Flush and collect all remaining chunks into an array
   */
  async flushAll(): Promise<EncodedAudioChunk[]> {
    super.flush()
    return this.takeEncodedChunks()
  }

  /**
   * Drain all currently available output chunks
   */
  drain(): EncodedAudioChunk[] {
    return this.takeEncodedChunks()
  }
}

/**
 * Extended AudioDecoder with async iterator support
 */
export class AudioDecoderAsync extends NativeAudioDecoder {
  /**
   * Async iterator that yields decoded audio data as it becomes available
   */
  async *[Symbol.asyncIterator](): AsyncIterableIterator<AudioData> {
    while (this.hasOutput()) {
      const data = this.takeNextAudio()
      if (data) {
        yield data
      }
    }
  }

  /**
   * Flush the decoder and return all remaining data as an async iterable
   */
  async *flushAsync(): AsyncIterableIterator<AudioData> {
    super.flush()
    yield* this
  }

  /**
   * Flush and collect all remaining data into an array
   */
  async flushAll(): Promise<AudioData[]> {
    super.flush()
    return this.takeDecodedAudio()
  }

  /**
   * Drain all currently available output data
   */
  drain(): AudioData[] {
    return this.takeDecodedAudio()
  }
}

// Re-export type aliases for convenience
export type {
  VideoEncoderConfig,
  VideoDecoderConfig,
  AudioEncoderConfig,
  AudioDecoderConfig,
  VideoEncoderEncodeOptions,
  EncodedVideoChunkMetadata,
  EncodedAudioChunkMetadata,
}
