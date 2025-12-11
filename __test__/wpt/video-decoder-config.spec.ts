/**
 * VideoDecoder Configuration Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests invalid configurations, unsupported configurations, and valid configurations
 * for VideoDecoder.isConfigSupported() and VideoDecoder.configure().
 */

import test from 'ava'

import { resetHardwareFallbackState, VideoDecoder } from '../../index.js'
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
    config: {},
  },
  {
    comment: 'Empty codec',
    config: { codec: '' },
  },
]

// Test isConfigSupported() rejects invalid configs
for (const entry of invalidConfigs) {
  test(`VideoDecoder.isConfigSupported() rejects invalid config: ${entry.comment}`, async (t) => {
    await t.throwsAsync(
      VideoDecoder.isConfigSupported(entry.config as Parameters<typeof VideoDecoder.isConfigSupported>[0]),
      { instanceOf: TypeError },
      `isConfigSupported should throw TypeError for: ${entry.comment}`,
    )
  })
}

// Test configure() throws on invalid configs
for (const entry of invalidConfigs) {
  test(`VideoDecoder.configure() throws TypeError for invalid config: ${entry.comment}`, (t) => {
    const decoder = new VideoDecoder(getDefaultCodecInit(t))

    t.throws(
      () => {
        decoder.configure(entry.config as Parameters<typeof decoder.configure>[0])
      },
      { instanceOf: TypeError },
      `configure should throw TypeError for: ${entry.comment}`,
    )

    t.is(decoder.state, 'unconfigured')
    decoder.close()
  })
}

// ============================================================================
// Valid but Unsupported Configurations
// ============================================================================

const validButUnsupportedConfigs = [
  {
    comment: 'Unrecognized codec',
    config: { codec: 'bogus' },
  },
  {
    comment: 'Unrecognized codec with dataview description',
    config: {
      codec: '7󠎢ﷺ۹.9',
      description: new DataView(new ArrayBuffer(12583)),
    },
  },
  {
    comment: 'Audio codec',
    config: { codec: 'vorbis' },
  },
  // Note: W3C WPT considers 'vp9' ambiguous, but we accept it for compatibility
  // with webcodecs-harness and browser implementations that accept the short form.
  // Original WPT: { comment: 'Ambiguous codec', config: { codec: 'vp9' } },
  {
    comment: 'Codec with bad casing',
    config: { codec: 'Vp09.00.10.08' },
  },
  {
    comment: 'Codec with MIME type',
    config: { codec: 'video/webm; codecs="vp8"' },
  },
  {
    comment: 'Possible future H264 codec string',
    config: { codec: 'avc1.FF000b' },
  },
  {
    comment: 'Possible future H264 codec string (level 2.9)',
    config: { codec: 'avc1.4D401D' },
  },
  {
    comment: 'Possible future HEVC codec string',
    config: { codec: 'hvc1.C99.6FFFFFF.L93' },
  },
  {
    comment: 'Possible future VP9 codec string',
    config: { codec: 'vp09.99.99.08' },
  },
  {
    comment: 'Possible future AV1 codec string',
    config: { codec: 'av01.9.99M.08' },
  },
  {
    comment: 'codec with spaces',
    config: { codec: '  vp09.00.10.08  ' },
  },
]

// Test isConfigSupported() returns supported: false
for (const entry of validButUnsupportedConfigs) {
  test(`VideoDecoder.isConfigSupported() returns false for: ${entry.comment}`, async (t) => {
    const support = await VideoDecoder.isConfigSupported(entry.config)
    t.false(support.supported, `isConfigSupported should return false for: ${entry.comment}`)
  })
}

// Test configure() triggers error callback
for (const entry of validButUnsupportedConfigs) {
  test(`VideoDecoder.configure() triggers error for: ${entry.comment}`, async (t) => {
    let isErrorCallbackCalled = false
    let errorReceived: Error | null = null

    const decoder = new VideoDecoder({
      output: () => t.fail('unexpected output'),
      error: (e) => {
        isErrorCallbackCalled = true
        errorReceived = e
      },
    })

    decoder.configure(entry.config)

    // Flush should reject
    try {
      await decoder.flush()
      t.fail('flush should reject')
    } catch (error) {
      t.truthy(error, 'flush should reject with error')
    }

    t.true(isErrorCallbackCalled, 'error callback should be called')
    t.truthy(errorReceived, 'error should be received')
    t.true(errorReceived!.message.includes('NotSupportedError'), 'error should be NotSupportedError')
    t.is(decoder.state, 'closed', 'decoder should be closed after error')
  })
}

// ============================================================================
// Valid Configurations
// ============================================================================

const validConfigs = [
  {
    comment: 'variant 1 of h264 codec string',
    config: { codec: 'avc3.42001E' },
  },
  {
    comment: 'variant 2 of h264 codec string',
    config: { codec: 'avc1.42001E' },
  },
]

for (const entry of validConfigs) {
  test(`VideoDecoder.isConfigSupported() accepts: ${entry.comment}`, async (t) => {
    try {
      const support = await VideoDecoder.isConfigSupported(entry.config)

      if (!support.supported) {
        t.pass(`Codec ${entry.config.codec} not supported on this platform`)
        return
      }

      // Configure should succeed
      const decoder = new VideoDecoder(getDefaultCodecInit(t))
      const config = {
        ...entry.config,
        codedWidth: 1280,
        codedHeight: 720,
      }
      decoder.configure(config)

      await decoder.flush()
      t.is(decoder.state, 'configured', 'decoder should be configured')
      decoder.close()
    } catch (e) {
      t.fail(`${entry.comment} should not throw: ${e instanceof Error ? e.message : String(e)}`)
    }
  })
}

// ============================================================================
// Construction Tests
// ============================================================================

test('VideoDecoder: constructor requires init dictionary', async (t) => {
  // Missing required fields
  t.throws(
    () => {
      // @ts-expect-error - Testing missing init
      new VideoDecoder({})
    },
    { instanceOf: TypeError },
  )

  // Valid init
  const decoder = new VideoDecoder(getDefaultCodecInit(t))
  t.is(decoder.state, 'unconfigured')
  decoder.close()

  await endAfterEventLoopTurn()
})

// ============================================================================
// isConfigSupported Returns Parsed Configuration
// ============================================================================

test('VideoDecoder.isConfigSupported() returns a parsed configuration', async (t) => {
  const config = {
    codec: 'vp8',
    codedWidth: 640,
    codedHeight: 480,
    displayAspectWidth: 800,
    displayAspectHeight: 600,
    colorSpace: { primaries: 'bt709' },
    futureConfigFeature: 'foo',
  }

  const support = await VideoDecoder.isConfigSupported(config)

  if (!support.supported) {
    t.pass('VP8 not supported on this platform')
    return
  }

  t.is(support.config.codec, config.codec, 'codec')
  t.is(support.config.codedWidth, config.codedWidth, 'codedWidth')
  t.is(support.config.codedHeight, config.codedHeight, 'codedHeight')
  t.is(support.config.displayAspectWidth, config.displayAspectWidth, 'displayAspectWidth')
  t.is(support.config.displayAspectHeight, config.displayAspectHeight, 'displayAspectHeight')

  // Color space properties
  if (support.config.colorSpace) {
    t.is(support.config.colorSpace.primaries, config.colorSpace.primaries, 'color primaries')
  }

  // Future config features should be stripped
  t.false('futureConfigFeature' in support.config, 'futureConfigFeature should be stripped')
})

// ============================================================================
// Invalid Dimension Tests
// ============================================================================

test('VideoDecoder.isConfigSupported() rejects zero codedWidth', async (t) => {
  await t.throwsAsync(
    VideoDecoder.isConfigSupported({
      codec: 'vp8',
      codedWidth: 0,
      codedHeight: 480,
    }),
    { instanceOf: TypeError },
  )
})

test('VideoDecoder.isConfigSupported() rejects zero displayAspectWidth', async (t) => {
  await t.throwsAsync(
    VideoDecoder.isConfigSupported({
      codec: 'vp8',
      codedWidth: 640,
      codedHeight: 480,
      displayAspectWidth: 0,
    }),
    { instanceOf: TypeError },
  )
})

test('VideoDecoder.configure() throws on zero codedWidth', (t) => {
  const decoder = new VideoDecoder(getDefaultCodecInit(t))

  t.throws(
    () => {
      decoder.configure({
        codec: 'vp8',
        codedWidth: 0,
        codedHeight: 480,
      })
    },
    { instanceOf: TypeError },
  )

  t.is(decoder.state, 'unconfigured')
  decoder.close()
})

test('VideoDecoder.configure() throws on zero displayAspectWidth', (t) => {
  const decoder = new VideoDecoder(getDefaultCodecInit(t))

  t.throws(
    () => {
      decoder.configure({
        codec: 'vp8',
        codedWidth: 640,
        codedHeight: 480,
        displayAspectWidth: 0,
      })
    },
    { instanceOf: TypeError },
  )

  t.is(decoder.state, 'unconfigured')
  decoder.close()
})

// ============================================================================
// State Transition Tests
// ============================================================================

test('VideoDecoder: configure then reset then configure', async (t) => {
  const support = await VideoDecoder.isConfigSupported({ codec: 'vp8' })
  if (!support.supported) {
    t.pass('VP8 not supported')
    return
  }

  const decoder = new VideoDecoder({
    output: () => {},
    error: () => {},
  })

  decoder.configure({ codec: 'vp8' })
  t.is(decoder.state, 'configured')

  decoder.reset()
  t.is(decoder.state, 'unconfigured')

  decoder.configure({ codec: 'vp8' })
  t.is(decoder.state, 'configured')

  decoder.close()
})
