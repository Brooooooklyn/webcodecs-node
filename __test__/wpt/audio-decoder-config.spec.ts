/**
 * AudioDecoder Configuration Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests invalid configurations, unsupported configurations, and valid configurations
 * for AudioDecoder.isConfigSupported() and AudioDecoder.configure().
 */

import test from 'ava'

import { AudioDecoder, resetHardwareFallbackState } from '../../index.js'
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
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Empty codec',
    config: {
      codec: '',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Missing numberOfChannels',
    config: {
      codec: 'opus',
      sampleRate: 48000,
    },
  },
  {
    comment: 'Zero sampleRate',
    config: {
      codec: 'opus',
      sampleRate: 0,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Zero channels',
    config: {
      codec: 'opus',
      sampleRate: 8000,
      numberOfChannels: 0,
    },
  },
]

// Test isConfigSupported() rejects invalid configs
for (const entry of invalidConfigs) {
  test(`AudioDecoder.isConfigSupported() rejects invalid config: ${entry.comment}`, async (t) => {
    await t.throwsAsync(
      AudioDecoder.isConfigSupported(entry.config as Parameters<typeof AudioDecoder.isConfigSupported>[0]),
      { instanceOf: TypeError },
      `isConfigSupported should throw TypeError for: ${entry.comment}`,
    )
  })
}

// Test configure() throws on invalid configs
for (const entry of invalidConfigs) {
  test(`AudioDecoder.configure() throws TypeError for invalid config: ${entry.comment}`, (t) => {
    const decoder = new AudioDecoder(getDefaultCodecInit(t))

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
    config: {
      codec: 'bogus',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Video codec',
    config: {
      codec: 'vp8',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Ambiguous codec',
    config: {
      codec: 'vp9',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Codec with MIME type',
    config: {
      codec: 'audio/webm; codecs="opus"',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Possible future opus codec string',
    config: {
      codec: 'opus.123',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Possible future aac codec string',
    config: {
      codec: 'mp4a.FF.9',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'codec with spaces',
    config: {
      codec: '  mp3  ',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
]

// Test isConfigSupported() returns supported: false
for (const entry of validButUnsupportedConfigs) {
  test(`AudioDecoder.isConfigSupported() returns false for: ${entry.comment}`, async (t) => {
    const support = await AudioDecoder.isConfigSupported(entry.config)
    t.false(support.supported, `isConfigSupported should return false for: ${entry.comment}`)
  })
}

// Test configure() triggers error callback
for (const entry of validButUnsupportedConfigs) {
  test(`AudioDecoder.configure() triggers error for: ${entry.comment}`, async (t) => {
    let isErrorCallbackCalled = false
    let errorReceived: Error | null = null

    const decoder = new AudioDecoder({
      output: () => t.fail('unexpected output'),
      error: (e) => {
        isErrorCallbackCalled = true
        errorReceived = e
      },
    })

    decoder.configure(entry.config)

    // Flush should reject
    const error = await t.throwsAsync(decoder.flush())

    t.true(isErrorCallbackCalled, 'error callback should be called')
    t.truthy(errorReceived, 'error should be received')
    t.true(errorReceived!.message.includes('NotSupportedError'), 'error should be NotSupportedError')
    t.is(decoder.state, 'closed', 'decoder should be closed after error')
    t.truthy(error, 'flush should reject')
  })
}

// ============================================================================
// Configurations that require description
// ============================================================================

const supportedButErrorOnConfiguration = [
  {
    comment: 'Opus with more than two channels without description',
    config: {
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 3,
    },
  },
  {
    comment: 'Opus with more than two channels and short description',
    config: {
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 3,
      description: new Uint8Array(9), // at least 10 bytes required
    },
  },
  {
    comment: 'vorbis requires a description',
    config: {
      codec: 'vorbis',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'flac requires a description',
    config: {
      codec: 'flac',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
]

for (const entry of supportedButErrorOnConfiguration) {
  test(`AudioDecoder.configure() errors for: ${entry.comment}`, async (t) => {
    // Check if codec is supported at all
    const baseConfig = {
      codec: entry.config.codec,
      sampleRate: entry.config.sampleRate,
      numberOfChannels: Math.min(entry.config.numberOfChannels, 2),
    }

    const support = await AudioDecoder.isConfigSupported(baseConfig)
    if (!support.supported) {
      t.pass(`Codec ${entry.config.codec} not supported on this platform`)
      return
    }

    let isErrorCallbackCalled = false

    const decoder = new AudioDecoder({
      output: () => t.fail('unexpected output'),
      error: () => {
        isErrorCallbackCalled = true
      },
    })

    decoder.configure(entry.config)

    // Flush should reject
    await t.throwsAsync(decoder.flush())

    t.true(isErrorCallbackCalled, 'error callback should be called')
    t.is(decoder.state, 'closed', 'decoder should be closed after error')
  })
}

// ============================================================================
// Valid Configurations
// ============================================================================

const validConfigs = [
  {
    comment: 'Opus basic',
    config: {
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'Opus mono',
    config: {
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 1,
    },
  },
  {
    comment: 'AAC basic',
    config: {
      codec: 'mp4a.40.2',
      sampleRate: 44100,
      numberOfChannels: 2,
    },
  },
  {
    comment: 'MP3',
    config: {
      codec: 'mp3',
      sampleRate: 44100,
      numberOfChannels: 2,
    },
  },
]

for (const entry of validConfigs) {
  test(`AudioDecoder.isConfigSupported() supports: ${entry.comment}`, async (t) => {
    const config = entry.config
    const support = await AudioDecoder.isConfigSupported(config)

    if (!support.supported) {
      t.pass(`Codec ${config.codec} not supported on this platform`)
      return
    }

    const newConfig = support.config
    t.is(newConfig.codec, config.codec, 'codec')
    t.is(newConfig.sampleRate, config.sampleRate, 'sampleRate')
    t.is(newConfig.numberOfChannels, config.numberOfChannels, 'numberOfChannels')
  })
}

// ============================================================================
// Construction Tests
// ============================================================================

test('AudioDecoder: constructor requires init dictionary', async (t) => {
  // Missing required fields
  t.throws(
    () => {
      // @ts-expect-error - Testing missing init
      new AudioDecoder({})
    },
    { instanceOf: TypeError },
  )

  // Valid init
  const decoder = new AudioDecoder(getDefaultCodecInit(t))
  t.is(decoder.state, 'unconfigured')
  decoder.close()

  await endAfterEventLoopTurn()
})

// ============================================================================
// State Transition Tests
// ============================================================================

test('AudioDecoder: configure then reset then configure', async (t) => {
  const support = await AudioDecoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('Opus not supported')
    return
  }

  const decoder = new AudioDecoder({
    output: () => {},
    error: () => {},
  })

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })
  t.is(decoder.state, 'configured')

  decoder.reset()
  t.is(decoder.state, 'unconfigured')

  decoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })
  t.is(decoder.state, 'configured')

  decoder.close()
})

// ============================================================================
// Unconfigured Decoder Tests
// ============================================================================

test('AudioDecoder: unconfigured decoder operations', async (t) => {
  const decoder = new AudioDecoder(getDefaultCodecInit(t))

  t.is(decoder.state, 'unconfigured')

  // Reset on unconfigured is no-op
  decoder.reset()
  t.is(decoder.state, 'unconfigured')

  // Flush should reject
  await t.throwsAsync(decoder.flush(), { message: /InvalidStateError/ })

  decoder.close()
})
