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

export type TypedArray =
  | Int8Array
  | Uint8Array
  | Uint8ClampedArray
  | Int16Array
  | Uint16Array
  | Int32Array
  | Uint32Array
  | Float32Array
  | Float64Array
  | BigInt64Array
  | BigUint64Array

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

/** Track type for demuxer */
export type DemuxerTrackType = 'video' | 'audio' | 'subtitle' | 'data'

/** Track info from demuxer */
export interface DemuxerTrackInfo {
  /** Track index */
  index: number
  /** Track type */
  trackType: DemuxerTrackType
  /** Codec string */
  codec?: string
  /** Coded width (video only) */
  codedWidth?: number
  /** Coded height (video only) */
  codedHeight?: number
  /** Sample rate (audio only) */
  sampleRate?: number
  /** Number of channels (audio only) */
  numberOfChannels?: number
  /** Duration in microseconds */
  duration?: number
}

/** Video decoder config from demuxer */
export interface DemuxerVideoDecoderConfig {
  /** Codec string */
  codec: string
  /** Coded width */
  codedWidth: number
  /** Coded height */
  codedHeight: number
  /** Codec description (e.g., avcC for H.264) */
  description?: Uint8Array
}

/** Audio decoder config from demuxer */
export interface DemuxerAudioDecoderConfig {
  /** Codec string */
  codec: string
  /** Sample rate */
  sampleRate: number
  /** Number of channels */
  numberOfChannels: number
  /** Codec description */
  description?: Uint8Array
}

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

/**
 * MP4 Demuxer - reads encoded video/audio from MP4 containers
 */
export declare class Mp4Demuxer {
  constructor(init: Mp4DemuxerInit)
  /** Load from file path */
  load(path: string): Promise<void>
  /** Load from buffer */
  loadBuffer(data: Uint8Array): Promise<void>
  /** Get all tracks */
  get tracks(): DemuxerTrackInfo[]
  /** Get container duration in microseconds */
  get duration(): number | null
  /** Get video decoder configuration */
  get videoDecoderConfig(): DemuxerVideoDecoderConfig | null
  /** Get audio decoder configuration */
  get audioDecoderConfig(): DemuxerAudioDecoderConfig | null
  /** Select video track by index */
  selectVideoTrack(trackIndex: number): void
  /** Select audio track by index */
  selectAudioTrack(trackIndex: number): void
  /** Start demuxing (optional packet count limit) */
  demux(count?: number): void
  /** Seek to timestamp in microseconds */
  seek(timestampUs: number): void
  /** Close and release resources */
  close(): void
  /** Get current state */
  get state(): DemuxerState
}

/**
 * WebM Demuxer - reads encoded video/audio from WebM containers
 */
export declare class WebMDemuxer {
  constructor(init: WebMDemuxerInit)
  /** Load from file path */
  load(path: string): Promise<void>
  /** Load from buffer */
  loadBuffer(data: Uint8Array): Promise<void>
  /** Get all tracks */
  get tracks(): DemuxerTrackInfo[]
  /** Get container duration in microseconds */
  get duration(): number | null
  /** Get video decoder configuration */
  get videoDecoderConfig(): DemuxerVideoDecoderConfig | null
  /** Get audio decoder configuration */
  get audioDecoderConfig(): DemuxerAudioDecoderConfig | null
  /** Select video track by index */
  selectVideoTrack(trackIndex: number): void
  /** Select audio track by index */
  selectAudioTrack(trackIndex: number): void
  /** Start demuxing (optional packet count limit) */
  demux(count?: number): void
  /** Seek to timestamp in microseconds */
  seek(timestampUs: number): void
  /** Close and release resources */
  close(): void
  /** Get current state */
  get state(): DemuxerState
}

/**
 * MKV Demuxer - reads encoded video/audio from MKV containers
 */
export declare class MkvDemuxer {
  constructor(init: MkvDemuxerInit)
  /** Load from file path */
  load(path: string): Promise<void>
  /** Load from buffer */
  loadBuffer(data: Uint8Array): Promise<void>
  /** Get all tracks */
  get tracks(): DemuxerTrackInfo[]
  /** Get container duration in microseconds */
  get duration(): number | null
  /** Get video decoder configuration */
  get videoDecoderConfig(): DemuxerVideoDecoderConfig | null
  /** Get audio decoder configuration */
  get audioDecoderConfig(): DemuxerAudioDecoderConfig | null
  /** Select video track by index */
  selectVideoTrack(trackIndex: number): void
  /** Select audio track by index */
  selectAudioTrack(trackIndex: number): void
  /** Start demuxing (optional packet count limit) */
  demux(count?: number): void
  /** Seek to timestamp in microseconds */
  seek(timestampUs: number): void
  /** Close and release resources */
  close(): void
  /** Get current state */
  get state(): DemuxerState
}

/**
 * MP4 Muxer - writes encoded video/audio to MP4 containers
 */
export declare class Mp4Muxer {
  constructor(init?: Mp4MuxerInit)
  /** Add video track */
  addVideoTrack(config: MuxerVideoTrackConfig): void
  /** Add audio track */
  addAudioTrack(config: MuxerAudioTrackConfig): void
  /** Add encoded video chunk */
  addVideoChunk(chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata): void
  /** Add encoded audio chunk */
  addAudioChunk(chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata): void
  /** Flush pending data */
  flush(): Promise<void>
  /** Finalize and get output data */
  finalize(): Uint8Array
  /** Read available data (streaming mode) */
  read(): Uint8Array | null
  /** Check if finished (streaming mode) */
  get isFinished(): boolean
  /** Close and release resources */
  close(): void
  /** Get current state */
  get state(): MuxerState
}

/**
 * WebM Muxer - writes encoded video/audio to WebM containers
 */
export declare class WebMMuxer {
  constructor(init?: WebMMuxerInit)
  /** Add video track */
  addVideoTrack(config: MuxerVideoTrackConfig): void
  /** Add audio track */
  addAudioTrack(config: MuxerAudioTrackConfig): void
  /** Add encoded video chunk */
  addVideoChunk(chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata): void
  /** Add encoded audio chunk */
  addAudioChunk(chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata): void
  /** Flush pending data */
  flush(): Promise<void>
  /** Finalize and get output data */
  finalize(): Uint8Array
  /** Read available data (streaming mode) */
  read(): Uint8Array | null
  /** Check if finished (streaming mode) */
  get isFinished(): boolean
  /** Close and release resources */
  close(): void
  /** Get current state */
  get state(): MuxerState
}

/**
 * MKV Muxer - writes encoded video/audio to MKV containers
 */
export declare class MkvMuxer {
  constructor(init?: MkvMuxerInit)
  /** Add video track */
  addVideoTrack(config: MuxerVideoTrackConfig): void
  /** Add audio track */
  addAudioTrack(config: MuxerAudioTrackConfig): void
  /** Add encoded video chunk */
  addVideoChunk(chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata): void
  /** Add encoded audio chunk */
  addAudioChunk(chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata): void
  /** Flush pending data */
  flush(): Promise<void>
  /** Finalize and get output data */
  finalize(): Uint8Array
  /** Read available data (streaming mode) */
  read(): Uint8Array | null
  /** Check if finished (streaming mode) */
  get isFinished(): boolean
  /** Close and release resources */
  close(): void
  /** Get current state */
  get state(): MuxerState
}
