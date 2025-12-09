/**
 * Codec configuration matrix
 *
 * Provides standard encoder/decoder configurations for testing
 * across different codecs and quality levels.
 */

import type { VideoEncoderConfig, VideoDecoderConfig } from '../../standard.js'

/** Supported codec types */
export type CodecType = 'h264' | 'h265' | 'vp8' | 'vp9' | 'av1'

/** Codec configuration with metadata */
export interface CodecConfig {
  /** Codec identifier */
  codec: CodecType
  /** WebCodecs codec string */
  codecString: string
  /** Human-readable name */
  name: string
  /** Whether this codec supports hardware acceleration */
  supportsHardwareAcceleration: boolean
  /** MIME type for this codec */
  mimeType: string
  /** Minimum supported width (must be even for YUV) */
  minWidth: number
  /** Minimum supported height */
  minHeight: number
}

/** All supported codecs and their metadata */
export const CodecRegistry: Record<CodecType, CodecConfig> = {
  h264: {
    codec: 'h264',
    codecString: 'avc1.42001E', // Baseline profile, level 3.0
    name: 'H.264/AVC',
    supportsHardwareAcceleration: true,
    mimeType: 'video/avc',
    minWidth: 16,
    minHeight: 16,
  },
  h265: {
    codec: 'h265',
    codecString: 'hev1.1.6.L93.B0', // Main profile
    name: 'H.265/HEVC',
    supportsHardwareAcceleration: true,
    mimeType: 'video/hevc',
    minWidth: 16,
    minHeight: 16,
  },
  vp8: {
    codec: 'vp8',
    codecString: 'vp8',
    name: 'VP8',
    supportsHardwareAcceleration: false,
    mimeType: 'video/vp8',
    minWidth: 16,
    minHeight: 16,
  },
  vp9: {
    codec: 'vp9',
    codecString: 'vp09.00.10.08', // Profile 0, level 1.0
    name: 'VP9',
    supportsHardwareAcceleration: true,
    mimeType: 'video/vp9',
    minWidth: 2,
    minHeight: 2,
  },
  av1: {
    codec: 'av1',
    codecString: 'av01.0.01M.08', // Main profile, level 2.0
    name: 'AV1',
    supportsHardwareAcceleration: true,
    mimeType: 'video/av1',
    minWidth: 2,
    minHeight: 2,
  },
}

/** Quality preset */
export type QualityPreset = 'low' | 'medium' | 'high'

/** Resolution preset */
export interface ResolutionPreset {
  width: number
  height: number
  name: string
}

/** Common resolution presets */
export const Resolutions: Record<string, ResolutionPreset> = {
  qvga: { width: 320, height: 240, name: 'QVGA' },
  vga: { width: 640, height: 480, name: 'VGA' },
  hd720: { width: 1280, height: 720, name: '720p' },
  hd1080: { width: 1920, height: 1080, name: '1080p' },
  uhd4k: { width: 3840, height: 2160, name: '4K UHD' },
  // Test sizes
  tiny: { width: 128, height: 96, name: 'Tiny' },
  small: { width: 256, height: 144, name: 'Small' },
}

/** Bitrate recommendations per resolution and quality */
const BitrateTable: Record<string, Record<QualityPreset, number>> = {
  tiny: { low: 100_000, medium: 200_000, high: 400_000 },
  small: { low: 200_000, medium: 400_000, high: 800_000 },
  qvga: { low: 300_000, medium: 600_000, high: 1_200_000 },
  vga: { low: 500_000, medium: 1_000_000, high: 2_000_000 },
  hd720: { low: 1_500_000, medium: 3_000_000, high: 6_000_000 },
  hd1080: { low: 3_000_000, medium: 6_000_000, high: 12_000_000 },
  uhd4k: { low: 10_000_000, medium: 20_000_000, high: 40_000_000 },
}

/**
 * Get recommended bitrate for a resolution and quality level
 */
export function getRecommendedBitrate(width: number, height: number, quality: QualityPreset = 'medium'): number {
  const pixels = width * height

  // Find closest resolution preset
  const entries = Object.entries(Resolutions)
  let closestKey = 'vga'
  let closestDiff = Infinity

  for (const [key, res] of entries) {
    const diff = Math.abs(res.width * res.height - pixels)
    if (diff < closestDiff) {
      closestDiff = diff
      closestKey = key
    }
  }

  return BitrateTable[closestKey]?.[quality] ?? BitrateTable.vga[quality]
}

/**
 * Create an encoder configuration for a specific codec
 */
export function createEncoderConfig(
  codec: CodecType,
  width: number,
  height: number,
  options: {
    bitrate?: number
    framerate?: number
    quality?: QualityPreset
    hardwareAcceleration?: 'no-preference' | 'prefer-hardware' | 'prefer-software'
    latencyMode?: 'quality' | 'realtime'
  } = {},
): VideoEncoderConfig {
  const codecInfo = CodecRegistry[codec]
  const quality = options.quality ?? 'medium'
  const bitrate = options.bitrate ?? getRecommendedBitrate(width, height, quality)

  return {
    codec: codecInfo.codecString,
    width,
    height,
    bitrate,
    framerate: options.framerate ?? 30,
    hardwareAcceleration: options.hardwareAcceleration ?? 'no-preference',
    latencyMode: options.latencyMode ?? 'quality',
  }
}

/**
 * Create a decoder configuration for a specific codec
 */
export function createDecoderConfig(
  codec: CodecType,
  options: {
    codedWidth?: number
    codedHeight?: number
    hardwareAcceleration?: 'no-preference' | 'prefer-hardware' | 'prefer-software'
  } = {},
): VideoDecoderConfig {
  const codecInfo = CodecRegistry[codec]

  return {
    codec: codecInfo.codecString,
    codedWidth: options.codedWidth,
    codedHeight: options.codedHeight,
    hardwareAcceleration: options.hardwareAcceleration ?? 'no-preference',
  }
}

/**
 * Get all codec types for iteration
 */
export function getAllCodecs(): CodecType[] {
  return Object.keys(CodecRegistry) as CodecType[]
}

/**
 * Check if a codec string is supported
 */
export function isCodecSupported(codecString: string): CodecType | null {
  for (const [type, config] of Object.entries(CodecRegistry)) {
    if (config.codecString === codecString) {
      return type as CodecType
    }
  }

  // Check for partial matches (e.g., "avc1" prefix)
  if (codecString.startsWith('avc1')) return 'h264'
  if (codecString.startsWith('hev1') || codecString.startsWith('hvc1')) return 'h265'
  if (codecString === 'vp8') return 'vp8'
  if (codecString.startsWith('vp09') || codecString === 'vp9') return 'vp9'
  if (codecString.startsWith('av01')) return 'av1'

  return null
}

/**
 * Test matrix generator for parametric testing
 *
 * Generates all combinations of codecs and resolutions for comprehensive testing.
 */
export interface TestCase {
  codec: CodecType
  codecInfo: CodecConfig
  resolution: ResolutionPreset
  encoderConfig: VideoEncoderConfig
  decoderConfig: VideoDecoderConfig
  testName: string
}

export function generateTestMatrix(
  codecs: CodecType[] = getAllCodecs(),
  resolutionKeys: (keyof typeof Resolutions)[] = ['tiny', 'qvga'],
): TestCase[] {
  const testCases: TestCase[] = []

  for (const codec of codecs) {
    const codecInfo = CodecRegistry[codec]

    for (const resKey of resolutionKeys) {
      const resolution = Resolutions[resKey]

      // Skip if resolution is below codec minimum
      if (resolution.width < codecInfo.minWidth || resolution.height < codecInfo.minHeight) {
        continue
      }

      testCases.push({
        codec,
        codecInfo,
        resolution,
        encoderConfig: createEncoderConfig(codec, resolution.width, resolution.height),
        decoderConfig: createDecoderConfig(codec, {
          codedWidth: resolution.width,
          codedHeight: resolution.height,
        }),
        testName: `${codecInfo.name} @ ${resolution.name}`,
      })
    }
  }

  return testCases
}

/**
 * Quick test configuration for a single codec
 *
 * Returns encoder/decoder configs suitable for basic testing.
 */
export function getQuickTestConfig(
  codec: CodecType = 'h264',
  width: number = 320,
  height: number = 240,
): { encoder: VideoEncoderConfig; decoder: VideoDecoderConfig } {
  return {
    encoder: createEncoderConfig(codec, width, height, { quality: 'medium' }),
    decoder: createDecoderConfig(codec, { codedWidth: width, codedHeight: height }),
  }
}

/**
 * H.264-specific profile configurations
 */
export const H264Profiles = {
  baseline: 'avc1.42001E', // Baseline profile, level 3.0
  main: 'avc1.4D001E', // Main profile, level 3.0
  high: 'avc1.64001E', // High profile, level 3.0
  high10: 'avc1.6E001E', // High 10 profile (10-bit)
} as const

/**
 * H.265-specific profile configurations
 */
export const H265Profiles = {
  main: 'hev1.1.6.L93.B0', // Main profile
  main10: 'hev1.2.4.L93.B0', // Main 10 profile (10-bit)
} as const

/**
 * VP9-specific profile configurations
 */
export const VP9Profiles = {
  profile0: 'vp09.00.10.08', // Profile 0 (4:2:0, 8-bit)
  profile2: 'vp09.02.10.10', // Profile 2 (4:2:0, 10-bit)
} as const

/**
 * AV1-specific profile configurations
 */
export const AV1Profiles = {
  main: 'av01.0.01M.08', // Main profile, level 2.0
  high: 'av01.1.01M.08', // High profile
} as const
