// Import types from standard.d.ts for use in index.d.ts
import type {
  VideoDecoderConfig,
  VideoEncoderConfig,
  AudioEncoderConfig,
  AudioDecoderConfig,
  VideoFrameBufferInit,
  EncodedVideoChunkInit,
  EncodedAudioChunkInit,
  AudioDataInit,
  VideoColorSpaceInit,
  ImageDecoderInit,
  BufferSource,
  AllowSharedBufferSource,
} from './standard'

// Re-export types from standard.d.ts
export {
  // Config types
  VideoDecoderConfig,
  VideoEncoderConfig,
  AudioEncoderConfig,
  AudioDecoderConfig,
  VideoFrameBufferInit,
  // Init types (used by constructors)
  EncodedVideoChunkInit,
  EncodedAudioChunkInit,
  AudioDataInit,
  VideoColorSpaceInit,
  ImageDecoderInit,
  // Buffer types
  BufferSource,
  AllowSharedBufferSource,
} from './standard'

/**
 * Interface for Canvas-like objects compatible with VideoFrame constructor.
 * Compatible with @napi-rs/canvas Canvas class.
 *
 * @napi-rs/canvas is an optional peer dependency. If installed, Canvas objects
 * can be used as VideoFrame sources. The Canvas pixel data is copied (RGBA format).
 */
export interface CanvasLike {
  readonly width: number
  readonly height: number
  /** Returns raw RGBA pixel data as a Buffer */
  data(): Uint8Array
}

// ============================================================================
// Muxer/Demuxer Types
// ============================================================================

/** Demuxer state */
export type DemuxerState = 'unloaded' | 'ready' | 'demuxing' | 'ended' | 'closed'

/** Muxer state */
export type MuxerState = 'configuring' | 'muxing' | 'finalized' | 'closed'

/** Init options for Mp4Demuxer */
export interface Mp4DemuxerInit {
  /** Callback for video chunks */
  videoOutput?: (chunk: EncodedVideoChunk) => void
  /** Callback for audio chunks */
  audioOutput?: (chunk: EncodedAudioChunk) => void
  /** Error callback (required) */
  error: (error: Error) => void
}

/** Init options for WebMDemuxer */
export interface WebMDemuxerInit {
  /** Callback for video chunks */
  videoOutput?: (chunk: EncodedVideoChunk) => void
  /** Callback for audio chunks */
  audioOutput?: (chunk: EncodedAudioChunk) => void
  /** Error callback (required) */
  error: (error: Error) => void
}

/** Init options for MkvDemuxer */
export interface MkvDemuxerInit {
  /** Callback for video chunks */
  videoOutput?: (chunk: EncodedVideoChunk) => void
  /** Callback for audio chunks */
  audioOutput?: (chunk: EncodedAudioChunk) => void
  /** Error callback (required) */
  error: (error: Error) => void
}

/** Video track config for muxer */
export interface MuxerVideoTrackConfig {
  /** Codec string */
  codec: string
  /** Video width */
  width: number
  /** Video height */
  height: number
  /** Codec description (e.g., avcC for H.264) */
  description?: Uint8Array
}

/** Audio track config for muxer */
export interface MuxerAudioTrackConfig {
  /** Codec string */
  codec: string
  /** Sample rate */
  sampleRate: number
  /** Number of channels */
  numberOfChannels: number
  /** Codec description */
  description?: Uint8Array
}

/** Init options for Mp4Muxer */
export interface Mp4MuxerInit {
  /** Move moov atom to beginning (not compatible with streaming) */
  fastStart?: boolean
  /** Use fragmented MP4 for streaming */
  fragmented?: boolean
  /** Enable streaming output mode */
  streaming?: { bufferCapacity?: number }
}

/** Init options for WebMMuxer */
export interface WebMMuxerInit {
  /** Enable live streaming mode */
  live?: boolean
  /** Enable streaming output mode */
  streaming?: { bufferCapacity?: number }
}

/** Init options for MkvMuxer */
export interface MkvMuxerInit {
  /** Enable live streaming mode */
  live?: boolean
  /** Enable streaming output mode */
  streaming?: { bufferCapacity?: number }
}
