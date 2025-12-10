/**
 * Standard WebCodecs type definitions
 * W3C WebCodecs Spec: https://w3c.github.io/webcodecs/
 *
 * These types match the W3C WebCodecs specification for interoperability.
 */

// ============================================================================
// BufferSource Types
// ============================================================================

/**
 * BufferSource per WebIDL spec - union of ArrayBuffer and ArrayBufferView
 */
export type BufferSource = ArrayBufferView | ArrayBuffer

// ============================================================================
// EncodedVideoChunk Types
// ============================================================================

/**
 * Type of encoded video chunk
 * @see https://w3c.github.io/webcodecs/#enumdef-encodedvideochunktype
 */
export type EncodedVideoChunkType = 'key' | 'delta'

/**
 * Init dictionary for EncodedVideoChunk constructor
 * @see https://w3c.github.io/webcodecs/#dictdef-encodedvideochunkinit
 */
export interface EncodedVideoChunkInit {
  /** Chunk type - 'key' for keyframes, 'delta' for dependent frames */
  type: EncodedVideoChunkType
  /** Timestamp in microseconds */
  timestamp: number
  /** Duration in microseconds (optional) */
  duration?: number
  /** Encoded video data */
  data: BufferSource
  /** ArrayBuffers to transfer (optional, for zero-copy) */
  transfer?: ArrayBuffer[]
}

// ============================================================================
// EncodedAudioChunk Types
// ============================================================================

/**
 * Type of encoded audio chunk
 * @see https://w3c.github.io/webcodecs/#enumdef-encodedaudiochunktype
 */
export type EncodedAudioChunkType = 'key' | 'delta'

/**
 * Init dictionary for EncodedAudioChunk constructor
 * @see https://w3c.github.io/webcodecs/#dictdef-encodedaudiochunkinit
 */
export interface EncodedAudioChunkInit {
  /** Chunk type - 'key' for keyframes, 'delta' for dependent frames */
  type: EncodedAudioChunkType
  /** Timestamp in microseconds */
  timestamp: number
  /** Duration in microseconds (optional) */
  duration?: number
  /** Encoded audio data */
  data: BufferSource
  /** ArrayBuffers to transfer (optional, for zero-copy) */
  transfer?: ArrayBuffer[]
}

// ============================================================================
// VideoEncoder Types
// ============================================================================

/**
 * Hardware acceleration preference
 * @see https://w3c.github.io/webcodecs/#enumdef-hardwareacceleration
 */
export type HardwareAcceleration = 'no-preference' | 'prefer-hardware' | 'prefer-software'

/**
 * Latency mode for encoding
 * @see https://w3c.github.io/webcodecs/#enumdef-latencymode
 */
export type LatencyMode = 'quality' | 'realtime'

/**
 * Bitrate mode for video encoding
 * @see https://w3c.github.io/webcodecs/#enumdef-videoencoderbitratemode
 */
export type VideoEncoderBitrateMode = 'constant' | 'variable' | 'quantizer'

/**
 * Alpha channel handling
 * @see https://w3c.github.io/webcodecs/#enumdef-alphaoption
 */
export type AlphaOption = 'discard' | 'keep'

/**
 * VideoEncoder configuration
 * @see https://w3c.github.io/webcodecs/#dictdef-videoencoderconfig
 */
export interface VideoEncoderConfig {
  /** Codec string (e.g., 'avc1.42001E', 'vp8', 'vp09.00.10.08') */
  codec: string
  /** Coded width in pixels */
  width: number
  /** Coded height in pixels */
  height: number
  /** Display width (optional, defaults to width) */
  displayWidth?: number
  /** Display height (optional, defaults to height) */
  displayHeight?: number
  /** Target bitrate in bits per second */
  bitrate?: number
  /** Target framerate */
  framerate?: number
  /** Hardware acceleration preference */
  hardwareAcceleration?: HardwareAcceleration
  /** Alpha channel handling */
  alpha?: AlphaOption
  /** Scalability mode (e.g., 'L1T1', 'L1T2') */
  scalabilityMode?: string
  /** Bitrate mode */
  bitrateMode?: VideoEncoderBitrateMode
  /** Latency mode */
  latencyMode?: LatencyMode
  /** AVC-specific configuration */
  avc?: AvcEncoderConfig
  /** HEVC-specific configuration */
  hevc?: HevcEncoderConfig
}

/**
 * AVC (H.264) encoder configuration
 * @see https://w3c.github.io/webcodecs/avc_codec_registration.html
 */
export interface AvcEncoderConfig {
  /** Bitstream format */
  format?: 'avc' | 'annexb'
}

/**
 * HEVC (H.265) encoder configuration
 * @see https://w3c.github.io/webcodecs/hevc_codec_registration.html
 */
export interface HevcEncoderConfig {
  /** Bitstream format */
  format?: 'hevc' | 'annexb'
}

/**
 * VideoEncoder encode options
 * @see https://w3c.github.io/webcodecs/#dictdef-videoencoderencodeptions
 */
export interface VideoEncoderEncodeOptions {
  /** Force keyframe */
  keyFrame?: boolean
}

// ============================================================================
// VideoDecoder Types
// ============================================================================

/**
 * VideoDecoder configuration
 * @see https://w3c.github.io/webcodecs/#dictdef-videodecoderconfig
 */
export interface VideoDecoderConfig {
  /** Codec string */
  codec: string
  /** Coded width (optional for some codecs) */
  codedWidth?: number
  /** Coded height (optional for some codecs) */
  codedHeight?: number
  /** Display aspect width */
  displayAspectWidth?: number
  /** Display aspect height */
  displayAspectHeight?: number
  /** Color space information */
  colorSpace?: VideoColorSpaceInit
  /** Hardware acceleration preference */
  hardwareAcceleration?: HardwareAcceleration
  /** Optimize for latency */
  optimizeForLatency?: boolean
  /** Codec-specific description (e.g., avcC box for H.264) */
  description?: BufferSource
  /** Rotation in degrees clockwise (0, 90, 180, 270) - W3C WebCodecs spec */
  rotation?: number
  /** Horizontal flip - W3C WebCodecs spec */
  flip?: boolean
}

// ============================================================================
// AudioEncoder Types
// ============================================================================

/**
 * Bitrate mode for audio encoding
 * @see https://w3c.github.io/webcodecs/#enumdef-bitratemode
 */
export type BitrateMode = 'constant' | 'variable'

/**
 * AudioEncoder configuration
 * @see https://w3c.github.io/webcodecs/#dictdef-audioencoderconfig
 */
export interface AudioEncoderConfig {
  /** Codec string (e.g., 'opus', 'mp4a.40.2') */
  codec: string
  /** Sample rate in Hz */
  sampleRate: number
  /** Number of audio channels */
  numberOfChannels: number
  /** Target bitrate in bits per second */
  bitrate?: number
  /** Bitrate mode */
  bitrateMode?: BitrateMode
  /** Opus-specific configuration */
  opus?: OpusEncoderConfig
  /** AAC-specific configuration */
  aac?: AacEncoderConfig
}

/**
 * Opus encoder configuration
 * @see https://w3c.github.io/webcodecs/opus_codec_registration.html
 */
export interface OpusEncoderConfig {
  /** Opus application type */
  application?: 'voip' | 'audio' | 'lowdelay'
  /** Frame duration in microseconds */
  frameDuration?: number
  /** Complexity (0-10) */
  complexity?: number
  /** Use DTX (discontinuous transmission) */
  usedtx?: boolean
  /** Use in-band FEC */
  useinbandfec?: boolean
  /** Packet loss percentage hint */
  packetlossperc?: number
}

/**
 * AAC encoder configuration
 * @see https://w3c.github.io/webcodecs/aac_codec_registration.html
 */
export interface AacEncoderConfig {
  /** AAC format */
  format?: 'aac' | 'adts'
}

// ============================================================================
// AudioDecoder Types
// ============================================================================

/**
 * AudioDecoder configuration
 * @see https://w3c.github.io/webcodecs/#dictdef-audiodecoderconfig
 */
export interface AudioDecoderConfig {
  /** Codec string */
  codec: string
  /** Sample rate in Hz */
  sampleRate: number
  /** Number of audio channels */
  numberOfChannels: number
  /** Codec-specific description */
  description?: BufferSource
}

// ============================================================================
// VideoFrame Types
// ============================================================================

/**
 * Pixel format for video frames
 * @see https://w3c.github.io/webcodecs/#enumdef-videoframepixelformat
 */
export type VideoPixelFormat = 'I420' | 'I420A' | 'I422' | 'I444' | 'NV12' | 'RGBA' | 'RGBX' | 'BGRA' | 'BGRX'

/**
 * VideoFrame buffer init
 * @see https://w3c.github.io/webcodecs/#dictdef-videoframebufferinit
 */
export interface VideoFrameBufferInit {
  /** Pixel format */
  format: VideoPixelFormat
  /** Coded width */
  codedWidth: number
  /** Coded height */
  codedHeight: number
  /** Timestamp in microseconds */
  timestamp: number
  /** Duration in microseconds */
  duration?: number
  /** Display width */
  displayWidth?: number
  /** Display height */
  displayHeight?: number
  /** Color space */
  colorSpace?: VideoColorSpaceInit
}

/**
 * VideoFrame copy-to options
 * @see https://w3c.github.io/webcodecs/#dictdef-videoframecopytoptions
 */
export interface VideoFrameCopyToOptions {
  /** Rectangle to copy */
  rect?: DOMRectInit
  /** Plane layouts */
  layout?: PlaneLayout[]
}

/**
 * Plane layout for video frame data
 * @see https://w3c.github.io/webcodecs/#dictdef-planelayout
 */
export interface PlaneLayout {
  /** Offset in bytes */
  offset: number
  /** Stride in bytes */
  stride: number
}

/**
 * DOMRect init dictionary
 */
export interface DOMRectInit {
  x?: number
  y?: number
  width?: number
  height?: number
}

// ============================================================================
// VideoColorSpace Types
// ============================================================================

/**
 * Color primaries
 * @see https://w3c.github.io/webcodecs/#enumdef-videocolorprimaries
 */
export type VideoColorPrimaries = 'bt709' | 'bt470bg' | 'smpte170m' | 'bt2020' | 'smpte432'

/**
 * Transfer characteristics
 * @see https://w3c.github.io/webcodecs/#enumdef-videotransfercharacteristics
 */
export type VideoTransferCharacteristics = 'bt709' | 'smpte170m' | 'iec61966-2-1' | 'srgb' | 'linear' | 'pq' | 'hlg'

/**
 * Matrix coefficients
 * @see https://w3c.github.io/webcodecs/#enumdef-videomatrixcoefficients
 */
export type VideoMatrixCoefficients = 'rgb' | 'bt709' | 'bt470bg' | 'smpte170m' | 'bt2020-ncl'

/**
 * VideoColorSpace init dictionary
 * @see https://w3c.github.io/webcodecs/#dictdef-videocolorspaceinit
 */
export interface VideoColorSpaceInit {
  /** Color primaries */
  primaries?: VideoColorPrimaries | null
  /** Transfer characteristics */
  transfer?: VideoTransferCharacteristics | null
  /** Matrix coefficients */
  matrix?: VideoMatrixCoefficients | null
  /** Full range flag */
  fullRange?: boolean | null
}

// ============================================================================
// AudioData Types
// ============================================================================

/**
 * Audio sample format
 * @see https://w3c.github.io/webcodecs/#enumdef-audiosampleformat
 */
export type AudioSampleFormat = 'u8' | 's16' | 's32' | 'f32' | 'u8-planar' | 's16-planar' | 's32-planar' | 'f32-planar'

/**
 * AudioData init dictionary
 * @see https://w3c.github.io/webcodecs/#dictdef-audiodatainit
 */
export interface AudioDataInit {
  /** Sample format */
  format: AudioSampleFormat
  /** Sample rate in Hz */
  sampleRate: number
  /** Number of frames (samples per channel) */
  numberOfFrames: number
  /** Number of channels */
  numberOfChannels: number
  /** Timestamp in microseconds */
  timestamp: number
  /** Audio data */
  data: BufferSource
  /** ArrayBuffers to transfer */
  transfer?: ArrayBuffer[]
}

/**
 * AudioData copy-to options
 * @see https://w3c.github.io/webcodecs/#dictdef-audiodatacopytoptions
 */
export interface AudioDataCopyToOptions {
  /** Plane index (for planar formats) */
  planeIndex: number
  /** Frame offset */
  frameOffset?: number
  /** Frame count */
  frameCount?: number
  /** Output format */
  format?: AudioSampleFormat
}

// ============================================================================
// ImageDecoder Types
// ============================================================================

/**
 * ImageDecoder init dictionary
 * @see https://w3c.github.io/webcodecs/#dictdef-imagedecodeinit
 */
export interface ImageDecoderInit {
  /** Image data */
  data: BufferSource | ReadableStream<Uint8Array>
  /** MIME type */
  type: string
  /** Color space conversion */
  colorSpaceConversion?: 'none' | 'default'
  /** Desired width */
  desiredWidth?: number
  /** Desired height */
  desiredHeight?: number
  /** Prefer animation */
  preferAnimation?: boolean
  /** ArrayBuffers to transfer */
  transfer?: ArrayBuffer[]
}

/**
 * ImageDecoder decode options
 * @see https://w3c.github.io/webcodecs/#dictdef-imagedecodeoptions
 */
export interface ImageDecodeOptions {
  /** Frame index */
  frameIndex?: number
  /** Complete frames only */
  completeFramesOnly?: boolean
}

/**
 * ImageDecoder decode result
 * @see https://w3c.github.io/webcodecs/#dictdef-imagedecoderesult
 */
export interface ImageDecodeResult {
  /** Decoded image as VideoFrame */
  image: import('./index').VideoFrame
  /** Whether decoding is complete */
  complete: boolean
}

// ============================================================================
// Codec State
// ============================================================================

/**
 * Codec state
 * @see https://w3c.github.io/webcodecs/#enumdef-codecstate
 */
export type CodecState = 'unconfigured' | 'configured' | 'closed'
