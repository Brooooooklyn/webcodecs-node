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
 * AudioData - represents uncompressed audio data
 *
 * This is a WebCodecs-compliant AudioData implementation backed by FFmpeg.
 */
export declare class AudioData {
  /**
   * Create a new AudioData (W3C WebCodecs spec)
   * Per spec, the constructor takes a single init object containing all parameters including data
   */
  constructor(init: import('./standard').AudioDataInit)
  /** Get sample format */
  get format(): AudioSampleFormat | null
  /**
   * Get sample rate in Hz (W3C spec uses float)
   * Returns 0 after close per W3C spec
   */
  get sampleRate(): number
  /**
   * Get number of frames (samples per channel)
   * Returns 0 after close per W3C spec
   */
  get numberOfFrames(): number
  /**
   * Get number of channels
   * Returns 0 after close per W3C spec
   */
  get numberOfChannels(): number
  /**
   * Get duration in microseconds
   * Returns 0 after close per W3C spec
   */
  get duration(): number
  /**
   * Get timestamp in microseconds
   * Timestamp is preserved after close per W3C spec
   */
  get timestamp(): number
  /** Get whether this AudioData has been closed (W3C WebCodecs spec) */
  get closed(): boolean
  /**
   * Get the number of planes in this AudioData (W3C WebCodecs spec)
   * For interleaved formats: 1
   * For planar formats: numberOfChannels
   */
  get numberOfPlanes(): number
  /**
   * Get the buffer size required for copyTo (W3C WebCodecs spec)
   * Note: options is REQUIRED per spec
   */
  allocationSize(options: AudioDataCopyToOptions): number
  /**
   * Copy audio data to a buffer (W3C WebCodecs spec)
   * Note: Per spec, this is SYNCHRONOUS and returns undefined
   * Accepts AllowSharedBufferSource (any TypedArray, DataView, or ArrayBuffer)
   */
  copyTo(destination: import('./standard').AllowSharedBufferSource, options: AudioDataCopyToOptions): void
  /** Create a copy of this AudioData */
  clone(): AudioData
  /** Close and release resources */
  close(): void
}

/**
 * AudioDecoder - WebCodecs-compliant audio decoder
 *
 * Decodes EncodedAudioChunk objects into AudioData objects using FFmpeg.
 *
 * Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
 *
 * Example:
 * ```javascript
 * const decoder = new AudioDecoder({
 *   output: (data) => { console.log('decoded audio', data); },
 *   error: (e) => { console.error('error', e); }
 * });
 *
 * decoder.configure({
 *   codec: 'opus',
 *   sampleRate: 48000,
 *   numberOfChannels: 2
 * });
 *
 * decoder.decode(chunk);
 * await decoder.flush();
 * ```
 */
export declare class AudioDecoder {
  /**
   * Create a new AudioDecoder with init dictionary (per WebCodecs spec)
   *
   * @param init - Init dictionary containing output and error callbacks
   */
  constructor(init: { output: (data: AudioData) => void; error: (error: Error) => void })
  /** Get decoder state */
  get state(): CodecState
  /** Get number of pending decode operations (per WebCodecs spec) */
  get decodeQueueSize(): number
  /**
   * Set the dequeue event handler (per WebCodecs spec)
   *
   * The dequeue event fires when decodeQueueSize decreases,
   * allowing backpressure management.
   */
  set ondequeue(callback?: (() => unknown) | undefined | null)
  /** Get the dequeue event handler (per WebCodecs spec) */
  get ondequeue(): (() => unknown) | null
  /** Configure the decoder */
  configure(config: AudioDecoderConfig): void
  /** Decode an encoded audio chunk */
  decode(chunk: EncodedAudioChunk): void
  /**
   * Flush the decoder
   * Returns a Promise that resolves when flushing is complete
   *
   * Uses spawn_future_with_callback to check abort flag synchronously in the resolver.
   * This ensures that if reset() is called from a callback, the abort flag is checked
   * AFTER the callback returns, allowing flush() to return AbortError.
   */
  flush(): Promise<void>
  /** Reset the decoder */
  reset(): void
  /** Close the decoder */
  close(): void
  /**
   * Check if a configuration is supported
   * Returns a Promise that resolves with support information
   *
   * W3C WebCodecs spec: Rejects with TypeError for invalid configs,
   * returns { supported: false } for valid but unsupported configs.
   */
  static isConfigSupported(config: AudioDecoderConfig): Promise<AudioDecoderSupport>
  /** Add an event listener for the specified event type */
  addEventListener(
    eventType: string,
    callback: () => unknown,
    options?: AudioDecoderAddEventListenerOptions | undefined | null,
  ): void
  /** Remove an event listener for the specified event type */
  removeEventListener(
    eventType: string,
    callback: () => unknown,
    options?: AudioDecoderEventListenerOptions | undefined | null,
  ): void
  /** Dispatch an event to all registered listeners */
  dispatchEvent(eventType: string): boolean
}

/**
 * AudioEncoder - WebCodecs-compliant audio encoder
 *
 * Encodes AudioData objects into EncodedAudioChunk objects using FFmpeg.
 *
 * Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
 *
 * Example:
 * ```javascript
 * const encoder = new AudioEncoder({
 *   output: (chunk, metadata) => { console.log('encoded chunk', chunk); },
 *   error: (e) => { console.error('error', e); }
 * });
 *
 * encoder.configure({
 *   codec: 'opus',
 *   sampleRate: 48000,
 *   numberOfChannels: 2
 * });
 *
 * encoder.encode(audioData);
 * await encoder.flush();
 * ```
 */
export declare class AudioEncoder {
  /**
   * Create a new AudioEncoder with init dictionary (per WebCodecs spec)
   *
   * @param init - Init dictionary containing output and error callbacks
   */
  constructor(init: {
    output: (chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata) => void
    error: (error: Error) => void
  })
  /** Get encoder state */
  get state(): CodecState
  /** Get number of pending encode operations (per WebCodecs spec) */
  get encodeQueueSize(): number
  /**
   * Set the dequeue event handler (per WebCodecs spec)
   *
   * The dequeue event fires when encodeQueueSize decreases,
   * allowing backpressure management.
   */
  set ondequeue(callback?: (() => unknown) | undefined | null)
  /** Get the dequeue event handler (per WebCodecs spec) */
  get ondequeue(): (() => unknown) | null
  /** Configure the encoder */
  configure(config: AudioEncoderConfig): void
  /** Encode audio data */
  encode(data: AudioData): void
  /**
   * Flush the encoder
   * Returns a Promise that resolves when flushing is complete
   *
   * Uses spawn_future_with_callback to check abort flag synchronously in the resolver.
   * This ensures that if reset() is called from a callback, the abort flag is checked
   * AFTER the callback returns, allowing flush() to return AbortError.
   */
  flush(): Promise<void>
  /** Reset the encoder */
  reset(): void
  /** Close the encoder */
  close(): void
  /**
   * Check if a configuration is supported
   * Returns a Promise that resolves with support information
   *
   * W3C WebCodecs spec: Rejects with TypeError for invalid configs,
   * returns { supported: false } for valid but unsupported configs.
   */
  static isConfigSupported(config: AudioEncoderConfig): Promise<AudioEncoderSupport>
  /**
   * Add an event listener for the specified event type
   * Uses separate RwLock to avoid blocking on encode operations
   */
  addEventListener(
    eventType: string,
    callback: () => unknown,
    options?: AudioEncoderAddEventListenerOptions | undefined | null,
  ): void
  /** Remove an event listener for the specified event type */
  removeEventListener(
    eventType: string,
    callback: () => unknown,
    options?: AudioEncoderEventListenerOptions | undefined | null,
  ): void
  /** Dispatch an event to all registered listeners */
  dispatchEvent(eventType: string): boolean
}

/**
 * DOMRectReadOnly - W3C WebCodecs spec compliant rect class
 * Used for codedRect and visibleRect properties
 */
export declare class DOMRectReadOnly {
  /** Create a new DOMRectReadOnly */
  constructor(
    x?: number | undefined | null,
    y?: number | undefined | null,
    width?: number | undefined | null,
    height?: number | undefined | null,
  )
  /** X coordinate */
  get x(): number
  /** Y coordinate */
  get y(): number
  /** Width */
  get width(): number
  /** Height */
  get height(): number
  /** Top edge (same as y) */
  get top(): number
  /** Right edge (x + width) */
  get right(): number
  /** Bottom edge (y + height) */
  get bottom(): number
  /** Left edge (same as x) */
  get left(): number
  /** Convert to JSON (W3C spec uses toJSON) */
  toJSON(): DOMRectInit
}

/**
 * EncodedAudioChunk - represents encoded audio data
 *
 * This is a WebCodecs-compliant EncodedAudioChunk implementation.
 */
export declare class EncodedAudioChunk {
  /** Create a new EncodedAudioChunk */
  constructor(init: import('./standard').EncodedAudioChunkInit)
  /** Get the chunk type */
  get type(): EncodedAudioChunkType
  /** Get the timestamp in microseconds */
  get timestamp(): number
  /** Get the duration in microseconds */
  get duration(): number | null
  /** Get the byte length of the encoded data */
  get byteLength(): number
  /**
   * Copy the encoded data to a BufferSource
   * W3C spec: throws TypeError if destination is too small
   */
  copyTo(destination: import('./standard').BufferSource): void
}

/**
 * EncodedVideoChunk - represents encoded video data
 *
 * This is a WebCodecs-compliant EncodedVideoChunk implementation.
 */
export declare class EncodedVideoChunk {
  /** Create a new EncodedVideoChunk */
  constructor(init: import('./standard').EncodedVideoChunkInit)
  /** Get the chunk type */
  get type(): EncodedVideoChunkType
  /** Get the timestamp in microseconds */
  get timestamp(): number
  /** Get the duration in microseconds */
  get duration(): number | null
  /** Get the byte length of the encoded data */
  get byteLength(): number
  /**
   * Copy the encoded data to a BufferSource
   * W3C spec: throws TypeError if destination is too small
   */
  copyTo(destination: import('./standard').BufferSource): void
}

/**
 * ImageDecoder - WebCodecs-compliant image decoder
 *
 * Decodes image data (JPEG, PNG, WebP, GIF, BMP) into VideoFrame objects.
 *
 * Example:
 * ```javascript
 * const decoder = new ImageDecoder({
 *   data: imageBytes,
 *   type: 'image/png'
 * });
 *
 * const result = await decoder.decode();
 * const frame = result.image;
 * ```
 */
export declare class ImageDecoder {
  /**
   * Create a new ImageDecoder
   * Supports both Uint8Array and ReadableStream as data source per W3C spec
   */
  constructor(init: ImageDecoderInit)
  /** Whether the data is fully buffered */
  get complete(): boolean
  /**
   * Promise that resolves when data is fully loaded (per WebCodecs spec)
   * Returns a new promise chained from the stored promise (allows multiple accesses)
   */
  get completed(): Promise<undefined>
  /** Get the MIME type */
  get type(): string
  /** Get the track list */
  get tracks(): ImageTrackList
  /** Decode the image (or a specific frame) */
  decode(this: this, options?: ImageDecodeOptions | undefined | null): Promise<ImageDecodeResult>
  /**
   * Reset the decoder
   * Clears cached frames - next decode() will re-decode from stored data
   */
  reset(): void
  /** Close the decoder */
  close(): void
  /** Whether this ImageDecoder has been closed (W3C WebCodecs spec) */
  get closed(): boolean
  /** Check if a MIME type is supported */
  static isTypeSupported(mimeType: string): Promise<boolean>
}

/**
 * Image decode result
 * Note: W3C spec defines this as a dictionary, but NAPI-RS doesn't support
 * class instances in objects, so we use a class with the same properties.
 */
export declare class ImageDecodeResult {
  /** Get the decoded image */
  get image(): VideoFrame
  /** Get whether the decode is complete */
  get complete(): boolean
}

/** Image track information (W3C spec - class with writable selected property) */
export declare class ImageTrack {
  /** Whether this track is animated */
  get animated(): boolean
  /** Number of frames in this track */
  get frameCount(): number
  /** Number of times the animation repeats (Infinity for infinite) */
  get repetitionCount(): number
  /** Whether this track is currently selected (W3C spec - writable) */
  get selected(): boolean
  /**
   * Set whether this track is selected (W3C spec - writable)
   * Setting to true deselects all other tracks
   */
  set selected(value: boolean)
}

/** Image track list (W3C spec) */
export declare class ImageTrackList {
  /** Get the number of tracks */
  get length(): number
  /** Get the currently selected track (if any) */
  get selectedTrack(): ImageTrack | null
  /** Get the selected track index (W3C spec: returns -1 if no track selected) */
  get selectedIndex(): number
  /** Promise that resolves when track metadata is available (W3C spec) */
  get ready(): Promise<void>
  /** Get track at specified index (W3C spec) */
  item(index: number): ImageTrack | null
}

/** Video color space parameters (WebCodecs spec) - as a class per spec */
export declare class VideoColorSpace {
  /** Create a new VideoColorSpace */
  constructor(init?: import('./standard').VideoColorSpaceInit)
  /** Get color primaries */
  get primaries(): VideoColorPrimaries | null
  /** Get transfer characteristics */
  get transfer(): VideoTransferCharacteristics | null
  /** Get matrix coefficients */
  get matrix(): VideoMatrixCoefficients | null
  /** Get full range flag */
  get fullRange(): boolean | null
  /**
   * Convert to JSON-compatible object (W3C spec uses toJSON)
   *
   * Per W3C spec, toJSON() returns explicit null for unset fields.
   */
  toJSON(): object
}

/**
 * VideoDecoder - WebCodecs-compliant video decoder
 *
 * Decodes EncodedVideoChunk objects into VideoFrame objects using FFmpeg.
 *
 * Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
 *
 * Example:
 * ```javascript
 * const decoder = new VideoDecoder({
 *   output: (frame) => { console.log('decoded frame', frame); },
 *   error: (e) => { console.error('error', e); }
 * });
 *
 * decoder.configure({
 *   codec: 'avc1.42001E'
 * });
 *
 * decoder.decode(chunk);
 * await decoder.flush();
 * ```
 */
export declare class VideoDecoder {
  /**
   * Create a new VideoDecoder with init dictionary (per WebCodecs spec)
   *
   * @param init - Init dictionary containing output and error callbacks
   */
  constructor(init: { output: (frame: VideoFrame) => void; error: (error: Error) => void })
  /** Get decoder state */
  get state(): CodecState
  /** Get number of pending decode operations (per WebCodecs spec) */
  get decodeQueueSize(): number
  /**
   * Set the dequeue event handler (per WebCodecs spec)
   *
   * The dequeue event fires when decodeQueueSize decreases,
   * allowing backpressure management.
   */
  set ondequeue(callback?: (() => unknown) | undefined | null)
  /** Get the dequeue event handler (per WebCodecs spec) */
  get ondequeue(): (() => unknown) | null
  /**
   * Configure the decoder
   *
   * Implements Chromium-aligned hardware acceleration behavior:
   * - `prefer-hardware`: Try hardware only, report error if fails
   * - `no-preference`: Try hardware first, silently fall back to software
   * - `prefer-software`: Use software only
   */
  configure(config: VideoDecoderConfig): void
  /** Decode an encoded video chunk */
  decode(chunk: EncodedVideoChunk): void
  /**
   * Flush the decoder
   * Returns a Promise that resolves when flushing is complete
   *
   * Uses spawn_future_with_callback to check abort flag synchronously in the resolver.
   * This ensures that if reset() is called from a callback, the abort flag is checked
   * AFTER the callback returns, allowing flush() to return AbortError.
   */
  flush(): Promise<void>
  /** Reset the decoder */
  reset(): void
  /** Close the decoder */
  close(): void
  /**
   * Check if a configuration is supported
   * Returns a Promise that resolves with support information
   *
   * W3C WebCodecs spec: Throws TypeError for invalid configs,
   * returns { supported: false } for valid but unsupported configs.
   */
  static isConfigSupported(config: VideoDecoderConfig): Promise<VideoDecoderSupport>
  /**
   * Add an event listener for the specified event type
   * Uses separate RwLock to avoid blocking on decode operations
   */
  addEventListener(
    eventType: string,
    callback: () => unknown,
    options?: VideoDecoderAddEventListenerOptions | undefined | null,
  ): void
  /** Remove an event listener for the specified event type */
  removeEventListener(
    eventType: string,
    callback: () => unknown,
    options?: VideoDecoderEventListenerOptions | undefined | null,
  ): void
  /** Dispatch an event to all registered listeners */
  dispatchEvent(eventType: string): boolean
}

/**
 * VideoEncoder - WebCodecs-compliant video encoder
 *
 * Encodes VideoFrame objects into EncodedVideoChunk objects using FFmpeg.
 *
 * Per the WebCodecs spec, the constructor takes an init dictionary with callbacks.
 *
 * Example:
 * ```javascript
 * const encoder = new VideoEncoder({
 *   output: (chunk, metadata) => { console.log('encoded chunk', chunk); },
 *   error: (e) => { console.error('error', e); }
 * });
 *
 * encoder.configure({
 *   codec: 'avc1.42001E',
 *   width: 1920,
 *   height: 1080,
 *   bitrate: 5_000_000
 * });
 *
 * encoder.encode(frame);
 * await encoder.flush();
 * ```
 */
export declare class VideoEncoder {
  /**
   * Create a new VideoEncoder with init dictionary (per WebCodecs spec)
   *
   * @param init - Init dictionary containing output and error callbacks
   */
  constructor(init: {
    output: (chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata) => void
    error: (error: Error) => void
  })
  /** Get encoder state */
  get state(): CodecState
  /** Get number of pending encode operations (per WebCodecs spec) */
  get encodeQueueSize(): number
  /**
   * Set the dequeue event handler (per WebCodecs spec)
   *
   * The dequeue event fires when encodeQueueSize decreases,
   * allowing backpressure management.
   */
  set ondequeue(callback?: (() => unknown) | undefined | null)
  /** Get the dequeue event handler (per WebCodecs spec) */
  get ondequeue(): (() => unknown) | null
  /** Configure the encoder */
  configure(config: VideoEncoderConfig): void
  /** Encode a frame */
  encode(frame: VideoFrame, options?: VideoEncoderEncodeOptions | undefined | null): void
  /**
   * Flush the encoder
   * Returns a Promise that resolves when flushing is complete
   *
   * Uses spawn_future_with_callback to check abort flag synchronously in the resolver.
   * This ensures that if reset() is called from a callback, the abort flag is checked
   * AFTER the callback returns, allowing flush() to return AbortError.
   */
  flush(): Promise<void>
  /** Reset the encoder */
  reset(): void
  /** Close the encoder */
  close(): void
  /**
   * Add an event listener for the specified event type
   * Uses separate RwLock to avoid blocking on encode operations
   */
  addEventListener(
    eventType: string,
    callback: () => unknown,
    options?: AddEventListenerOptions | undefined | null,
  ): void
  /** Remove an event listener for the specified event type */
  removeEventListener(
    eventType: string,
    callback: () => unknown,
    options?: EventListenerOptions | undefined | null,
  ): void
  /** Dispatch an event to all registered listeners */
  dispatchEvent(eventType: string): boolean
  /**
   * Check if a configuration is supported
   * Returns a Promise that resolves with support information
   *
   * W3C WebCodecs spec: Throws TypeError for invalid configs,
   * returns { supported: false } for valid but unsupported configs.
   *
   * Note: The config parameter is validated via FromNapiValue which throws
   * native TypeError for missing required fields.
   */
  static isConfigSupported(config: VideoEncoderConfig): Promise<VideoEncoderSupport>
}

/**
 * VideoFrame - represents a frame of video
 *
 * This is a WebCodecs-compliant VideoFrame implementation backed by FFmpeg.
 */
export declare class VideoFrame {
  /**
   * Create a new VideoFrame from buffer data or another VideoFrame (W3C WebCodecs spec)
   *
   * Two constructor forms per W3C spec:
   * 1. `new VideoFrame(data, init)` - from BufferSource with VideoFrameBufferInit
   * 2. `new VideoFrame(source, init?)` - from another VideoFrame with optional VideoFrameInit
   */
  constructor(source: VideoFrame | Uint8Array, init?: VideoFrameBufferInit | VideoFrameInit)
  /** Get the pixel format */
  get format(): VideoPixelFormat | null
  /** Get the coded width in pixels (returns 0 when closed per W3C spec) */
  get codedWidth(): number
  /** Get the coded height in pixels (returns 0 when closed per W3C spec) */
  get codedHeight(): number
  /** Get the display width in pixels (returns 0 when closed per W3C spec) */
  get displayWidth(): number
  /** Get the display height in pixels (returns 0 when closed per W3C spec) */
  get displayHeight(): number
  /**
   * Get the coded rect (the region containing valid pixel data)
   * Returns DOMRectReadOnly per W3C WebCodecs spec
   * Throws InvalidStateError if the VideoFrame is closed
   */
  get codedRect(): DOMRectReadOnly
  /**
   * Get the visible rect (the region of coded data that should be displayed)
   * Returns DOMRectReadOnly per W3C WebCodecs spec
   * Throws InvalidStateError if the VideoFrame is closed
   */
  get visibleRect(): DOMRectReadOnly
  /**
   * Get the presentation timestamp in microseconds
   * Per W3C spec: "The timestamp getter steps are to return [[timestamp]]"
   * The timestamp is preserved even after close() - only resource reference is cleared
   */
  get timestamp(): number
  /**
   * Get the duration in microseconds
   * Per W3C spec: "The duration getter steps are to return [[duration]]"
   * The duration is preserved even after close() - only resource reference is cleared
   */
  get duration(): number | null
  /** Get the color space parameters */
  get colorSpace(): VideoColorSpace
  /** Get whether this VideoFrame has been closed (W3C WebCodecs spec) */
  get closed(): boolean
  /**
   * Get the number of planes in this VideoFrame (W3C WebCodecs spec)
   * The number depends on the pixel format:
   * - RGBA, RGBX, BGRA, BGRX: 1 plane
   * - NV12, NV21: 2 planes
   * - I420, I422, I444: 3 planes
   * - I420A, I422A, I444A: 4 planes
   */
  get numberOfPlanes(): number
  /** Get the rotation in degrees clockwise (0, 90, 180, 270) - W3C WebCodecs spec */
  get rotation(): number
  /** Get whether horizontal flip is applied - W3C WebCodecs spec */
  get flip(): boolean
  /**
   * Get the metadata associated with this VideoFrame - W3C WebCodecs spec
   * Currently returns an empty metadata object as members are defined in the registry
   */
  metadata(): VideoFrameMetadata
  /** Calculate the allocation size needed for copyTo */
  allocationSize(options?: VideoFrameCopyToOptions | undefined | null): number
  /**
   * Copy frame data to a Uint8Array
   *
   * Returns a Promise that resolves with an array of PlaneLayout objects.
   * Options can specify target format and rect for cropped copy.
   */
  copyTo(destination: Uint8Array, options?: VideoFrameCopyToOptions | undefined | null): Promise<Array<PlaneLayout>>
  /** Clone this VideoFrame */
  clone(): VideoFrame
  /**
   * Close and release resources
   * Per W3C spec "Close VideoFrame" algorithm:
   * 1. Assign null to frame's [[resource reference]]
   * 2. Assign true to frame's [[Detached]]
   * Note: Metadata (timestamp, duration, etc.) remains accessible after close
   */
  close(): void
}

/** AAC bitstream format (W3C WebCodecs AAC Registration) */
export type AacBitstreamFormat = /** Raw AAC frames - metadata in description */
  | 'aac'
  /** ADTS frames - metadata in each frame */
  | 'adts'

/** AAC encoder configuration (W3C WebCodecs AAC Registration) */
export interface AacEncoderConfig {
  /** Bitstream format (default: "aac") */
  format?: AacBitstreamFormat
}

/** Options for addEventListener (W3C DOM spec) */
export interface AddEventListenerOptions {
  capture?: boolean
  once?: boolean
  passive?: boolean
}

/**
 * Alpha channel handling option (W3C WebCodecs spec)
 * Default is "discard" per spec
 */
export type AlphaOption = /** Keep alpha channel if present */
  | 'keep'
  /** Discard alpha channel (default per W3C spec) */
  | 'discard'

/** Options for copyTo operation */
export interface AudioDataCopyToOptions {
  /** The index of the audio plane to copy */
  planeIndex: number
  /** The offset in frames to start copying from (optional) */
  frameOffset?: number
  /** The number of frames to copy (optional, defaults to all remaining) */
  frameCount?: number
  /** Target format for conversion (optional) */
  format?: AudioSampleFormat
}

/** Options for addEventListener (W3C DOM spec) */
export interface AudioDecoderAddEventListenerOptions {
  capture?: boolean
  once?: boolean
  passive?: boolean
}

/** Decoder configuration output (for passing to decoder) */
export interface AudioDecoderConfigOutput {
  /** Codec string */
  codec: string
  /** Sample rate - W3C spec uses float */
  sampleRate?: number
  /** Number of channels */
  numberOfChannels?: number
  /** Codec description (e.g., AudioSpecificConfig for AAC) - Uint8Array per spec */
  description?: Uint8Array
}

/** Options for removeEventListener (W3C DOM spec) */
export interface AudioDecoderEventListenerOptions {
  capture?: boolean
}

/** Audio decoder support information */
export interface AudioDecoderSupport {
  /** Whether the configuration is supported */
  supported: boolean
  /** The configuration that was tested */
  config: AudioDecoderConfig
}

/** Options for addEventListener (W3C DOM spec) */
export interface AudioEncoderAddEventListenerOptions {
  capture?: boolean
  once?: boolean
  passive?: boolean
}

/** Encode options for audio */
export interface AudioEncoderEncodeOptions {}

/** Options for removeEventListener (W3C DOM spec) */
export interface AudioEncoderEventListenerOptions {
  capture?: boolean
}

/** Audio encoder support information */
export interface AudioEncoderSupport {
  /** Whether the configuration is supported */
  supported: boolean
  /** The configuration that was tested */
  config: AudioEncoderConfig
}

/** Audio sample format (WebCodecs spec) */
export type AudioSampleFormat = /** Unsigned 8-bit integer samples| interleaved */
  | 'u8'
  /** Signed 16-bit integer samples| interleaved */
  | 's16'
  /** Signed 32-bit integer samples| interleaved */
  | 's32'
  /** 32-bit float samples| interleaved */
  | 'f32'
  /** Unsigned 8-bit integer samples| planar */
  | 'u8-planar'
  /** Signed 16-bit integer samples| planar */
  | 's16-planar'
  /** Signed 32-bit integer samples| planar */
  | 's32-planar'
  /** 32-bit float samples| planar */
  | 'f32-planar'

/** AVC (H.264) bitstream format (W3C WebCodecs AVC Registration) */
export type AvcBitstreamFormat = /** AVC format with parameter sets in description (ISO 14496-15) */
  | 'avc'
  /** Annex B format with parameter sets in bitstream */
  | 'annexb'

/** AVC (H.264) encoder configuration (W3C WebCodecs AVC Registration) */
export interface AvcEncoderConfig {
  /** Bitstream format (default: "avc") */
  format?: AvcBitstreamFormat
}

/** Bitrate mode for audio encoding (W3C WebCodecs spec) */
export type BitrateMode = /** Variable bitrate (default) */
  | 'variable'
  /** Constant bitrate */
  | 'constant'

/** Encoder state per WebCodecs spec */
export type CodecState = /** Encoder not configured */
  | 'unconfigured'
  /** Encoder configured and ready */
  | 'configured'
  /** Encoder closed */
  | 'closed'

/** ColorSpaceConversion for ImageDecoder (W3C WebCodecs spec) */
export type ColorSpaceConversion = /** Apply default color space conversion (spec default) */
  | 'default'
  /** No color space conversion */
  | 'none'

/** DOMRectInit for specifying regions */
export interface DOMRectInit {
  x?: number
  y?: number
  width?: number
  height?: number
}

/** Output callback metadata for audio */
export interface EncodedAudioChunkMetadata {
  /** Decoder configuration for this chunk */
  decoderConfig?: AudioDecoderConfigOutput
}

/** Type of encoded audio chunk */
export type EncodedAudioChunkType = /** Key chunk - can be decoded independently */
  | 'key'
  /** Delta chunk - depends on previous chunks */
  | 'delta'

/** Output callback metadata per WebCodecs spec */
export interface EncodedVideoChunkMetadata {
  /** Decoder configuration for this chunk (only present for keyframes) */
  decoderConfig?: VideoDecoderConfigOutput
  /** SVC metadata (temporal layer info) */
  svc?: SvcOutputMetadata
  /** Alpha channel side data (when alpha option is "keep") */
  alphaSideData?: Uint8Array
}

/** Type of encoded video chunk */
export type EncodedVideoChunkType = /** Keyframe - can be decoded independently */
  | 'key'
  /** Delta frame - depends on previous frames */
  | 'delta'

/** Options for removeEventListener (W3C DOM spec) */
export interface EventListenerOptions {
  capture?: boolean
}

/** FLAC encoder configuration (W3C WebCodecs FLAC Registration) */
export interface FlacEncoderConfig {
  /** Block size (0 = auto, default: 0) */
  blockSize?: number
  /** Compression level 0-8 (default: 5) */
  compressLevel?: number
}

/** Get available hardware accelerators (only those that can be used) */
export declare function getAvailableHardwareAccelerators(): Array<string>

/** Get list of all known hardware accelerators and their availability */
export declare function getHardwareAccelerators(): Array<HardwareAccelerator>

/** Get the preferred hardware accelerator for the current platform */
export declare function getPreferredHardwareAccelerator(): string | null

/** Hardware acceleration preference (W3C WebCodecs spec) */
export type HardwareAcceleration = /** No preference - may use hardware or software */
  | 'no-preference'
  /** Prefer hardware acceleration */
  | 'prefer-hardware'
  /** Prefer software implementation */
  | 'prefer-software'

/** Hardware accelerator information */
export interface HardwareAccelerator {
  /** Internal name (e.g., "videotoolbox", "cuda", "vaapi") */
  name: string
  /** Human-readable description */
  description: string
  /** Whether this accelerator is available on this system */
  available: boolean
}

/** HEVC (H.265) bitstream format (W3C WebCodecs HEVC Registration) */
export type HevcBitstreamFormat = /** HEVC format with parameter sets in description (ISO 14496-15) */
  | 'hevc'
  /** Annex B format with parameter sets in bitstream */
  | 'annexb'

/** HEVC (H.265) encoder configuration (W3C WebCodecs HEVC Registration) */
export interface HevcEncoderConfig {
  /** Bitstream format (default: "hevc") */
  format?: HevcBitstreamFormat
}

/** Image decode options */
export interface ImageDecodeOptions {
  /** Frame index to decode (for animated images) */
  frameIndex?: number
  /** Whether to only decode complete frames */
  completeFramesOnly?: boolean
}

/** Check if a specific hardware accelerator is available */
export declare function isHardwareAcceleratorAvailable(name: string): boolean

/** Latency mode for video encoding (W3C WebCodecs spec) */
export type LatencyMode = /** Optimize for quality (default) */
  | 'quality'
  /** Optimize for low latency */
  | 'realtime'

/** Opus application mode (W3C WebCodecs Opus Registration) */
export type OpusApplication = /** Optimize for VoIP (speech intelligibility) */
  | 'voip'
  /** Optimize for audio fidelity (default) */
  | 'audio'
  /** Minimize coding delay */
  | 'lowdelay'

/** Opus bitstream format (W3C WebCodecs Opus Registration) */
export type OpusBitstreamFormat = /** Opus packets (RFC 6716) - no metadata needed for decoding */
  | 'opus'
  /** Ogg encapsulation (RFC 7845) - metadata in description */
  | 'ogg'

/** Opus encoder configuration (W3C WebCodecs Opus Registration) */
export interface OpusEncoderConfig {
  /** Bitstream format (default: "opus") */
  format?: OpusBitstreamFormat
  /** Signal type hint (default: "auto") */
  signal?: OpusSignal
  /** Application mode (default: "audio") */
  application?: OpusApplication
  /**
   * Frame duration in microseconds (default: 20000)
   * Note: W3C spec uses unsigned long long, but NAPI-RS uses f64 for JS compatibility
   */
  frameDuration?: number
  /** Encoder complexity 0-10 (default: 5 mobile, 9 desktop) */
  complexity?: number
  /** Expected packet loss percentage 0-100 (default: 0) */
  packetlossperc?: number
  /** Enable in-band FEC (default: false) */
  useinbandfec?: boolean
  /** Enable DTX (default: false) */
  usedtx?: boolean
}

/** Opus signal type hint (W3C WebCodecs Opus Registration) */
export type OpusSignal = /** Auto-detect signal type */
  | 'auto'
  /** Music signal */
  | 'music'
  /** Voice/speech signal */
  | 'voice'

/** Layout information for a single plane per WebCodecs spec */
export interface PlaneLayout {
  /** Byte offset from the start of the buffer to the start of the plane */
  offset: number
  /** Number of bytes per row (stride) */
  stride: number
}

/**
 * Reset all hardware fallback state.
 *
 * This clears all failure counts and re-enables hardware acceleration.
 * Useful for:
 * - Test isolation (call in beforeEach)
 * - Error recovery after fixing hardware issues
 * - Manual reset by users
 */
export declare function resetHardwareFallbackState(): void

/** SVC (Scalable Video Coding) output metadata (W3C WebCodecs spec) */
export interface SvcOutputMetadata {
  /** Temporal layer ID for this frame */
  temporalLayerId?: number
}

/** Video color primaries (W3C WebCodecs spec) */
export type VideoColorPrimaries = /** BT.709 / sRGB primaries */
  | 'bt709'
  /** BT.470 BG (PAL) */
  | 'bt470bg'
  /** SMPTE 170M (NTSC) */
  | 'smpte170m'
  /** BT.2020 (UHD) */
  | 'bt2020'
  /** SMPTE 432 (DCI-P3) */
  | 'smpte432'

/** Options for addEventListener (W3C DOM spec) */
export interface VideoDecoderAddEventListenerOptions {
  capture?: boolean
  once?: boolean
  passive?: boolean
}

/** Decoder configuration output (for passing to decoder) */
export interface VideoDecoderConfigOutput {
  /** Codec string */
  codec: string
  /** Coded width */
  codedWidth?: number
  /** Coded height */
  codedHeight?: number
  /** Codec description (e.g., avcC for H.264) - Uint8Array per spec */
  description?: Uint8Array
  /** Color space information for the video content */
  colorSpace?: VideoColorSpaceInit
  /** Display aspect width (for non-square pixels) */
  displayAspectWidth?: number
  /** Display aspect height (for non-square pixels) */
  displayAspectHeight?: number
  /** Rotation in degrees clockwise (0, 90, 180, 270) per W3C spec */
  rotation?: number
  /** Horizontal flip per W3C spec */
  flip?: boolean
}

/** Options for removeEventListener (W3C DOM spec) */
export interface VideoDecoderEventListenerOptions {
  capture?: boolean
}

/** Result of isConfigSupported per WebCodecs spec */
export interface VideoDecoderSupport {
  /** Whether the configuration is supported */
  supported: boolean
  /** The configuration that was checked */
  config: VideoDecoderConfig
}

/** Bitrate mode for video encoding (W3C WebCodecs spec) */
export type VideoEncoderBitrateMode = /** Variable bitrate (default) */
  | 'variable'
  /** Constant bitrate */
  | 'constant'
  /** Use quantizer parameter from codec-specific options */
  | 'quantizer'

/** Encode options per WebCodecs spec */
export interface VideoEncoderEncodeOptions {
  /** Force this frame to be a keyframe */
  keyFrame?: boolean
  /** AVC (H.264) codec-specific options */
  avc?: VideoEncoderEncodeOptionsForAvc
  /** HEVC (H.265) codec-specific options */
  hevc?: VideoEncoderEncodeOptionsForHevc
  /** VP9 codec-specific options */
  vp9?: VideoEncoderEncodeOptionsForVp9
  /** AV1 codec-specific options */
  av1?: VideoEncoderEncodeOptionsForAv1
}

/** AV1 encode options (W3C WebCodecs AV1 Registration) */
export interface VideoEncoderEncodeOptionsForAv1 {
  /** Per-frame quantizer (0-63, lower = higher quality) */
  quantizer?: number
}

/** AVC (H.264) encode options (W3C WebCodecs AVC Registration) */
export interface VideoEncoderEncodeOptionsForAvc {
  /** Per-frame quantizer (0-51, lower = higher quality) */
  quantizer?: number
}

/** HEVC (H.265) encode options (W3C WebCodecs HEVC Registration) */
export interface VideoEncoderEncodeOptionsForHevc {
  /** Per-frame quantizer (0-51, lower = higher quality) */
  quantizer?: number
}

/** VP9 encode options (W3C WebCodecs VP9 Registration) */
export interface VideoEncoderEncodeOptionsForVp9 {
  /** Per-frame quantizer (0-63, lower = higher quality) */
  quantizer?: number
}

/** Result of isConfigSupported per WebCodecs spec */
export interface VideoEncoderSupport {
  /** Whether the configuration is supported */
  supported: boolean
  /** The configuration that was checked */
  config: VideoEncoderConfig
}

/** Options for copyTo operation */
export interface VideoFrameCopyToOptions {
  /** Target pixel format (for format conversion) */
  format?: VideoPixelFormat
  /** Region to copy (not yet implemented) */
  rect?: DOMRectInit
  /** Layout for output planes */
  layout?: Array<PlaneLayout>
}

/** Options for creating a VideoFrame from an image source (VideoFrameInit per spec) */
export interface VideoFrameInit {
  /** Timestamp in microseconds (required per spec when creating from VideoFrame) */
  timestamp?: number
  /** Duration in microseconds (optional) */
  duration?: number
  /** Alpha handling: "keep" (default) or "discard" */
  alpha?: string
  /** Visible rect (optional) */
  visibleRect?: DOMRectInit
  /** Rotation in degrees clockwise (0, 90, 180, 270) - default 0 */
  rotation?: number
  /** Horizontal flip - default false */
  flip?: boolean
  /** Display width (optional) */
  displayWidth?: number
  /** Display height (optional) */
  displayHeight?: number
  /** Metadata associated with the frame */
  metadata?: VideoFrameMetadata
}

/**
 * VideoFrameMetadata - metadata associated with a VideoFrame (W3C spec)
 * Members defined in VideoFrame Metadata Registry - currently empty per spec
 */
export interface VideoFrameMetadata {}

/** Rectangle for specifying a region */
export interface VideoFrameRect {
  x: number
  y: number
  width: number
  height: number
}

/** Video matrix coefficients (W3C WebCodecs spec) */
export type VideoMatrixCoefficients = /** RGB (identity matrix) */
  | 'rgb'
  /** BT.709 */
  | 'bt709'
  /** BT.470 BG */
  | 'bt470bg'
  /** SMPTE 170M */
  | 'smpte170m'
  /** BT.2020 non-constant luminance */
  | 'bt2020-ncl'

/** Video pixel format (WebCodecs spec) */
export type VideoPixelFormat = /** Planar YUV 4:2:0| 12bpp| (1 Cr & Cb sample per 2x2 Y samples) */
  | 'I420'
  /** Planar YUV 4:2:0| 12bpp| with alpha plane */
  | 'I420A'
  /** Planar YUV 4:2:2| 16bpp */
  | 'I422'
  /** Planar YUV 4:2:2| 16bpp| with alpha plane */
  | 'I422A'
  /** Planar YUV 4:4:4| 24bpp */
  | 'I444'
  /** Planar YUV 4:4:4| 24bpp| with alpha plane */
  | 'I444A'
  /** Planar YUV 4:2:0| 10-bit */
  | 'I420P10'
  /** Planar YUV 4:2:0| 10-bit| with alpha plane */
  | 'I420AP10'
  /** Planar YUV 4:2:2| 10-bit */
  | 'I422P10'
  /** Planar YUV 4:2:2| 10-bit| with alpha plane */
  | 'I422AP10'
  /** Planar YUV 4:4:4| 10-bit */
  | 'I444P10'
  /** Planar YUV 4:4:4| 10-bit| with alpha plane */
  | 'I444AP10'
  /** Planar YUV 4:2:0| 12-bit */
  | 'I420P12'
  /** Planar YUV 4:2:2| 12-bit */
  | 'I422P12'
  /** Planar YUV 4:4:4| 12-bit */
  | 'I444P12'
  /** Semi-planar YUV 4:2:0| 12bpp (Y plane + interleaved UV) */
  | 'NV12'
  /** Semi-planar YUV 4:2:0| 12bpp (Y plane + interleaved VU) - per W3C WebCodecs spec */
  | 'NV21'
  /** RGBA 32bpp */
  | 'RGBA'
  /** RGBX 32bpp (alpha ignored) */
  | 'RGBX'
  /** BGRA 32bpp */
  | 'BGRA'
  /** BGRX 32bpp (alpha ignored) */
  | 'BGRX'

/** Video transfer characteristics (W3C WebCodecs spec) */
export type VideoTransferCharacteristics = /** BT.709 transfer */
  | 'bt709'
  /** SMPTE 170M transfer */
  | 'smpte170m'
  /** IEC 61966-2-1 (sRGB) - technical name */
  | 'iec61966-2-1'
  /** sRGB transfer (alias for iec61966-2-1) */
  | 'srgb'
  /** Linear transfer */
  | 'linear'
  /** Perceptual Quantizer (HDR) */
  | 'pq'
  /** Hybrid Log-Gamma (HDR) */
  | 'hlg'
