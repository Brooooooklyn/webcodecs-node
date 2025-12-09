/**
 * AudioEncoder Configuration Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/nicosurjana/nicosurjana.git
 *
 * Tests invalid configurations, unsupported configurations, and valid configurations
 * for AudioEncoder.isConfigSupported() and AudioEncoder.configure().
 */

import test from 'ava'

import { AudioEncoder, resetHardwareFallbackState } from '../../index.js'
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
    comment: 'Missing sampleRate',
    config: {
      codec: 'opus',
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
  test(`AudioEncoder.isConfigSupported() rejects invalid config: ${entry.comment}`, async (t) => {
    await t.throwsAsync(
      AudioEncoder.isConfigSupported(entry.config as Parameters<typeof AudioEncoder.isConfigSupported>[0]),
      { instanceOf: TypeError },
      `isConfigSupported should throw TypeError for: ${entry.comment}`,
    )
  })
}

// Test configure() throws on invalid configs
for (const entry of invalidConfigs) {
  test(`AudioEncoder.configure() throws TypeError for invalid config: ${entry.comment}`, (t) => {
    const encoder = new AudioEncoder(getDefaultCodecInit(t))

    t.throws(
      () => {
        encoder.configure(entry.config as Parameters<typeof encoder.configure>[0])
      },
      { instanceOf: TypeError },
      `configure should throw TypeError for: ${entry.comment}`,
    )

    t.is(encoder.state, 'unconfigured')
    encoder.close()
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
      codec: '  opus  ',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
]

// Test isConfigSupported() returns supported: false
for (const entry of validButUnsupportedConfigs) {
  test(`AudioEncoder.isConfigSupported() returns false for: ${entry.comment}`, async (t) => {
    const support = await AudioEncoder.isConfigSupported(entry.config)
    t.false(support.supported, `isConfigSupported should return false for: ${entry.comment}`)
  })
}

// Test configure() triggers error callback
for (const entry of validButUnsupportedConfigs) {
  test(`AudioEncoder.configure() triggers error for: ${entry.comment}`, async (t) => {
    let isErrorCallbackCalled = false
    let errorReceived: Error | null = null

    const encoder = new AudioEncoder({
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
    comment: 'Opus basic',
    config: {
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 2,
      bitrate: 256000,
    },
  },
  {
    comment: 'Opus mono',
    config: {
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 1,
      bitrate: 128000,
    },
  },
  {
    comment: 'AAC basic',
    config: {
      codec: 'mp4a.40.2',
      sampleRate: 44100,
      numberOfChannels: 2,
      bitrate: 192000,
    },
  },
  {
    comment: 'MP3',
    config: {
      codec: 'mp3',
      sampleRate: 44100,
      numberOfChannels: 2,
      bitrate: 192000,
    },
  },
  {
    comment: 'FLAC',
    config: {
      codec: 'flac',
      sampleRate: 48000,
      numberOfChannels: 2,
    },
  },
]

for (const entry of validConfigs) {
  test(`AudioEncoder.isConfigSupported() supports: ${entry.comment}`, async (t) => {
    const config = entry.config
    const support = await AudioEncoder.isConfigSupported(config)

    if (!support.supported) {
      t.pass(`Codec ${config.codec} not supported on this platform`)
      return
    }

    const newConfig = support.config
    t.is(newConfig.codec, config.codec, 'codec')
    t.is(newConfig.sampleRate, config.sampleRate, 'sampleRate')
    t.is(newConfig.numberOfChannels, config.numberOfChannels, 'numberOfChannels')
    if (config.bitrate) {
      t.is(newConfig.bitrate, config.bitrate, 'bitrate')
    }
  })
}

// ============================================================================
// Construction Tests
// ============================================================================

test('AudioEncoder: constructor requires init dictionary', async (t) => {
  // Missing required fields
  t.throws(
    () => {
      // @ts-expect-error - Testing missing init
      new AudioEncoder({})
    },
    { instanceOf: TypeError },
  )

  // Valid init
  const encoder = new AudioEncoder(getDefaultCodecInit(t))
  t.is(encoder.state, 'unconfigured')
  encoder.close()

  await endAfterEventLoopTurn()
})

// ============================================================================
// Opus-Specific Configuration Tests
// ============================================================================

const opusEncoderConfigs = [
  {
    comment: 'Empty Opus config',
    opus: {},
  },
  {
    comment: 'Opus with frameDuration',
    opus: { frameDuration: 2500 },
  },
  {
    comment: 'Opus with complexity',
    opus: { complexity: 10 },
  },
  {
    comment: 'Opus with useinbandfec',
    opus: {
      packetlossperc: 15,
      useinbandfec: true,
    },
  },
  {
    comment: 'Opus with usedtx',
    opus: { usedtx: true },
  },
  {
    comment: 'Opus mixed parameters',
    opus: {
      frameDuration: 40000,
      complexity: 0,
      packetlossperc: 10,
      useinbandfec: true,
      usedtx: true,
    },
  },
]

for (const entry of opusEncoderConfigs) {
  test(`AudioEncoder.isConfigSupported() with Opus parameters: ${entry.comment}`, async (t) => {
    const config = {
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 2,
      bitrate: 256000,
      opus: entry.opus,
    }

    const support = await AudioEncoder.isConfigSupported(config)

    if (!support.supported) {
      t.pass('Opus not supported on this platform')
      return
    }

    t.true(support.supported)
  })
}

// ============================================================================
// State Transition Tests
// ============================================================================

test('AudioEncoder: configure then reset then configure', async (t) => {
  const support = await AudioEncoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('Opus not supported')
    return
  }

  const encoder = new AudioEncoder({
    output: () => {},
    error: () => {},
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })
  t.is(encoder.state, 'configured')

  encoder.reset()
  t.is(encoder.state, 'unconfigured')

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })
  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: reconfigure with different settings', async (t) => {
  const support = await AudioEncoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  if (!support.supported) {
    t.pass('Opus not supported')
    return
  }

  const encoder = new AudioEncoder({
    output: () => {},
    error: () => {},
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })
  t.is(encoder.state, 'configured')

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 256000,
  })
  t.is(encoder.state, 'configured')

  encoder.close()
})
