/**
 * VideoEncoder Configuration Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests invalid configurations, unsupported configurations, and valid configurations
 * for VideoEncoder.isConfigSupported() and VideoEncoder.configure().
 */

import test from 'ava'

import { resetHardwareFallbackState, VideoEncoder } from '../../index.js'
import { endAfterEventLoopTurn, getDefaultCodecInit } from '../helpers/wpt-utils.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// Invalid Configurations - Should throw TypeError
// ============================================================================

const invalidConfigs = [
  {
    comment: 'Missing codec',
    config: {
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'Empty codec',
    config: {
      codec: '',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'Width is 0',
    config: {
      codec: 'vp8',
      width: 0,
      height: 480,
    },
  },
  {
    comment: 'Height is 0',
    config: {
      codec: 'vp8',
      width: 640,
      height: 0,
    },
  },
  {
    comment: 'displayWidth is 0',
    config: {
      codec: 'vp8',
      displayWidth: 0,
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'displayHeight is 0',
    config: {
      codec: 'vp8',
      width: 640,
      displayHeight: 0,
      height: 480,
    },
  },
  {
    comment: 'bitrate is present but zero',
    config: {
      codec: 'vp8',
      width: 640,
      height: 480,
      bitrate: 0,
    },
  },
]

// Test isConfigSupported() rejects invalid configs
for (const entry of invalidConfigs) {
  test(`VideoEncoder.isConfigSupported() rejects invalid config: ${entry.comment}`, async (t) => {
    await t.throwsAsync(
      VideoEncoder.isConfigSupported(entry.config as Parameters<typeof VideoEncoder.isConfigSupported>[0]),
      { instanceOf: TypeError },
      `isConfigSupported should throw TypeError for: ${entry.comment}`,
    )
  })
}

// Test configure() throws on invalid configs
for (const entry of invalidConfigs) {
  test(`VideoEncoder.configure() throws TypeError for invalid config: ${entry.comment}`, (t) => {
    const encoder = new VideoEncoder(getDefaultCodecInit(t))

    t.throws(
      () => {
        encoder.configure(entry.config as Parameters<typeof encoder.configure>[0])
      },
      { instanceOf: TypeError },
      `configure should throw TypeError for: ${entry.comment}`,
    )

    // Encoder should remain unconfigured after invalid configure
    t.is(encoder.state, 'unconfigured')
    encoder.close()
  })
}

// ============================================================================
// Valid but Unsupported Configurations
// ============================================================================

const validButUnsupportedConfigs = [
  {
    comment: 'Invalid scalability mode',
    config: { codec: 'vp8', width: 640, height: 480, scalabilityMode: 'ABC' },
  },
  {
    comment: 'Unrecognized codec',
    config: {
      codec: 'bogus',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'VP8 codec string not accepted by WebCodecs',
    config: {
      codec: 'vp08.00.10.08',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'Codec with bad casing',
    config: {
      codec: 'vP8',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'Width is too large',
    config: {
      codec: 'vp8',
      width: 1000000,
      height: 480,
    },
  },
  {
    comment: 'Height is too large',
    config: {
      codec: 'vp8',
      width: 640,
      height: 1000000,
    },
  },
  {
    comment: 'Possible future H264 codec string',
    config: {
      codec: 'avc1.FF000b',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'Possible future HEVC codec string',
    config: {
      codec: 'hvc1.C99.6FFFFFF.L93',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'Possible future VP9 codec string',
    config: {
      codec: 'vp09.99.99.08',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'Possible future AV1 codec string',
    config: {
      codec: 'av01.9.99M.08',
      width: 640,
      height: 480,
    },
  },
  {
    comment: 'codec with spaces',
    config: {
      codec: '  vp09.00.10.08  ',
      width: 640,
      height: 480,
    },
  },
]

// Test isConfigSupported() returns supported: false for unsupported configs
for (const entry of validButUnsupportedConfigs) {
  test(`VideoEncoder.isConfigSupported() returns false for: ${entry.comment}`, async (t) => {
    const config = entry.config
    const support = await VideoEncoder.isConfigSupported(config)
    t.false(support.supported, `isConfigSupported should return false for: ${entry.comment}`)

    // Verify config is echoed back
    const newConfig = support.config
    t.is(newConfig.codec, config.codec, 'codec should be echoed')
    t.is(newConfig.width, config.width, 'width should be echoed')
    t.is(newConfig.height, config.height, 'height should be echoed')
    if ('bitrate' in config && config.bitrate) {
      t.is(newConfig.bitrate, config.bitrate, 'bitrate should be echoed')
    }
    if ('framerate' in config && config.framerate) {
      t.is(newConfig.framerate, config.framerate, 'framerate should be echoed')
    }
  })
}

// Test configure() triggers error callback for unsupported configs
for (const entry of validButUnsupportedConfigs) {
  test(`VideoEncoder.configure() triggers error for: ${entry.comment}`, async (t) => {
    let isErrorCallbackCalled = false
    let errorReceived: Error | null = null

    const encoder = new VideoEncoder({
      output: () => t.fail('unexpected output'),
      error: (e) => {
        isErrorCallbackCalled = true
        errorReceived = e
      },
    })

    encoder.configure(entry.config)

    // Flush should reject
    const error = await t.throwsAsync(encoder.flush())

    t.true(isErrorCallbackCalled, 'error callback should be called')
    t.truthy(errorReceived, 'error should be received')
    t.true(errorReceived!.message.includes('NotSupportedError'), 'error should be NotSupportedError')
    t.is(encoder.state, 'closed', 'encoder should be closed after error')
    t.truthy(error, 'flush should reject')
  })
}

// ============================================================================
// Valid Configurations
// ============================================================================

const validConfigs = [
  {
    comment: 'H.264 with annexb format',
    config: {
      codec: 'avc1.42001E',
      hardwareAcceleration: 'no-preference' as const,
      width: 640,
      height: 480,
      bitrate: 5000000,
      framerate: 24,
      avc: { format: 'annexb' as const },
    },
  },
  {
    comment: 'VP8 basic',
    config: {
      codec: 'vp8',
      hardwareAcceleration: 'no-preference' as const,
      width: 800,
      height: 600,
      bitrate: 7000000,
      bitrateMode: 'variable' as const,
      framerate: 60,
      scalabilityMode: 'L1T2',
      latencyMode: 'quality' as const,
    },
  },
  {
    comment: 'VP9 with constant bitrate',
    config: {
      codec: 'vp09.00.10.08',
      hardwareAcceleration: 'no-preference' as const,
      width: 1280,
      height: 720,
      bitrate: 7000000,
      bitrateMode: 'constant' as const,
      framerate: 25,
      latencyMode: 'realtime' as const,
      alpha: 'discard' as const,
    },
  },
]

for (const entry of validConfigs) {
  test(`VideoEncoder.isConfigSupported() supports: ${entry.comment}`, async (t) => {
    const config = entry.config
    const support = await VideoEncoder.isConfigSupported(config)

    // Skip if not supported on this platform
    if (!support.supported) {
      t.pass(`Codec ${config.codec} not supported on this platform`)
      return
    }

    const newConfig = support.config

    // Future config features should be stripped
    t.false('futureConfigFeature' in newConfig, 'futureConfigFeature should be stripped')

    // Core properties should be echoed
    t.is(newConfig.codec, config.codec, 'codec')
    t.is(newConfig.width, config.width, 'width')
    t.is(newConfig.height, config.height, 'height')

    if (config.bitrate) {
      t.is(newConfig.bitrate, config.bitrate, 'bitrate')
    }
    if (config.framerate) {
      t.is(newConfig.framerate, config.framerate, 'framerate')
    }
    if (config.bitrateMode) {
      t.is(newConfig.bitrateMode, config.bitrateMode, 'bitrateMode')
    }
    if (config.latencyMode) {
      t.is(newConfig.latencyMode, config.latencyMode, 'latencyMode')
    }
    if (config.alpha) {
      t.is(newConfig.alpha, config.alpha, 'alpha')
    }
    if (config.avc) {
      t.is(newConfig.avc?.format, config.avc.format, 'avc.format')
    }
  })
}

// ============================================================================
// Construction Tests
// ============================================================================

test('VideoEncoder: constructor requires init dictionary', async (t) => {
  // Missing required fields
  t.throws(
    () => {
      // @ts-expect-error - Testing missing init
      new VideoEncoder({})
    },
    { instanceOf: TypeError },
  )

  // Valid init
  const encoder = new VideoEncoder(getDefaultCodecInit(t))
  t.is(encoder.state, 'unconfigured')
  encoder.close()

  await endAfterEventLoopTurn()
})

// ============================================================================
// Additional Config Validation Tests
// ============================================================================

test('VideoEncoder.isConfigSupported() with unknown properties are ignored', async (t) => {
  const config = {
    codec: 'vp8',
    width: 640,
    height: 480,
    futureConfigFeature: 'foo',
  }

  const support = await VideoEncoder.isConfigSupported(config)
  // Should not throw, unknown properties are ignored
  t.truthy(support.config)
  t.false('futureConfigFeature' in support.config, 'unknown property should be stripped')
})

test('VideoEncoder.configure() can reconfigure with different settings', async (t) => {
  const chunks: unknown[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => chunks.push(chunk),
    error: () => {},
  })

  // First configure
  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
  })
  t.is(encoder.state, 'configured')

  // Reconfigure
  encoder.configure({
    codec: 'vp8',
    width: 640,
    height: 480,
  })
  t.is(encoder.state, 'configured')

  encoder.close()
})

test('VideoEncoder.configure() after reset works', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
  })
  t.is(encoder.state, 'configured')

  encoder.reset()
  t.is(encoder.state, 'unconfigured')

  // Should be able to configure again
  encoder.configure({
    codec: 'vp8',
    width: 640,
    height: 480,
  })
  t.is(encoder.state, 'configured')

  encoder.close()
})
