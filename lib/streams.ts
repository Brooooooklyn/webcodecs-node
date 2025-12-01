/**
 * WebCodecs Node.js - Transform Stream Wrappers
 *
 * Provides Node.js Transform stream classes for seamless pipeline integration.
 */

import { Transform, TransformCallback, TransformOptions } from 'node:stream'

import {
  VideoEncoder,
  VideoDecoder,
  AudioEncoder,
  AudioDecoder,
  VideoFrame,
  AudioData,
  EncodedVideoChunk,
  EncodedAudioChunk,
  type VideoEncoderConfig,
  type VideoDecoderConfig,
  type AudioEncoderConfig,
  type AudioDecoderConfig,
} from '../index.js'

/**
 * Transform stream that encodes VideoFrames into EncodedVideoChunks
 *
 * Example:
 * ```typescript
 * import { pipeline } from 'node:stream/promises'
 *
 * const encoder = new VideoEncoderStream({
 *   codec: 'avc1.42001E',
 *   width: 1920,
 *   height: 1080,
 *   bitrate: 5_000_000,
 * })
 *
 * await pipeline(frameSource, encoder, muxerSink)
 * ```
 */
export class VideoEncoderStream extends Transform {
  private encoder: VideoEncoder

  constructor(
    config: VideoEncoderConfig,
    options?: TransformOptions,
  ) {
    super({ ...options, objectMode: true })
    this.encoder = new VideoEncoder()
    this.encoder.configure(config)
  }

  _transform(
    frame: VideoFrame,
    _encoding: BufferEncoding,
    callback: TransformCallback,
  ): void {
    try {
      this.encoder.encode(frame)
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  _flush(callback: TransformCallback): void {
    try {
      this.encoder.flush()
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  private drainOutput(): void {
    while (this.encoder.hasOutput()) {
      const chunk = this.encoder.takeNextChunk()
      if (chunk) {
        this.push(chunk)
      }
    }
  }

  /**
   * Get the underlying VideoEncoder instance
   */
  get nativeEncoder(): VideoEncoder {
    return this.encoder
  }
}

/**
 * Transform stream that decodes EncodedVideoChunks into VideoFrames
 */
export class VideoDecoderStream extends Transform {
  private decoder: VideoDecoder

  constructor(
    config: VideoDecoderConfig,
    options?: TransformOptions,
  ) {
    super({ ...options, objectMode: true })
    this.decoder = new VideoDecoder()
    this.decoder.configure(config)
  }

  _transform(
    chunk: EncodedVideoChunk,
    _encoding: BufferEncoding,
    callback: TransformCallback,
  ): void {
    try {
      this.decoder.decode(chunk)
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  _flush(callback: TransformCallback): void {
    try {
      this.decoder.flush()
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  private drainOutput(): void {
    while (this.decoder.hasOutput()) {
      const frame = this.decoder.takeNextFrame()
      if (frame) {
        this.push(frame)
      }
    }
  }

  /**
   * Get the underlying VideoDecoder instance
   */
  get nativeDecoder(): VideoDecoder {
    return this.decoder
  }
}

/**
 * Transform stream that encodes AudioData into EncodedAudioChunks
 */
export class AudioEncoderStream extends Transform {
  private encoder: AudioEncoder

  constructor(
    config: AudioEncoderConfig,
    options?: TransformOptions,
  ) {
    super({ ...options, objectMode: true })
    this.encoder = new AudioEncoder()
    this.encoder.configure(config)
  }

  _transform(
    data: AudioData,
    _encoding: BufferEncoding,
    callback: TransformCallback,
  ): void {
    try {
      this.encoder.encode(data)
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  _flush(callback: TransformCallback): void {
    try {
      this.encoder.flush()
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  private drainOutput(): void {
    while (this.encoder.hasOutput()) {
      const chunk = this.encoder.takeNextChunk()
      if (chunk) {
        this.push(chunk)
      }
    }
  }

  /**
   * Get the underlying AudioEncoder instance
   */
  get nativeEncoder(): AudioEncoder {
    return this.encoder
  }
}

/**
 * Transform stream that decodes EncodedAudioChunks into AudioData
 */
export class AudioDecoderStream extends Transform {
  private decoder: AudioDecoder

  constructor(
    config: AudioDecoderConfig,
    options?: TransformOptions,
  ) {
    super({ ...options, objectMode: true })
    this.decoder = new AudioDecoder()
    this.decoder.configure(config)
  }

  _transform(
    chunk: EncodedAudioChunk,
    _encoding: BufferEncoding,
    callback: TransformCallback,
  ): void {
    try {
      this.decoder.decode(chunk)
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  _flush(callback: TransformCallback): void {
    try {
      this.decoder.flush()
      this.drainOutput()
      callback()
    } catch (error) {
      callback(error as Error)
    }
  }

  private drainOutput(): void {
    while (this.decoder.hasOutput()) {
      const data = this.decoder.takeNextAudio()
      if (data) {
        this.push(data)
      }
    }
  }

  /**
   * Get the underlying AudioDecoder instance
   */
  get nativeDecoder(): AudioDecoder {
    return this.decoder
  }
}
