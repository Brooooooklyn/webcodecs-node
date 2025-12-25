/**
 * VideoEncoder Content Hint Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * - wpt/webcodecs/video-encoder-content-hint.https.any.js
 *
 * Tests that VideoEncoder recognizes the contentHint configuration option.
 */

import test from 'ava'

import { resetHardwareFallbackState, VideoEncoder } from '../../index.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// Content Hint Tests
// WPT: "Test that contentHint is recognized by VideoEncoder"
// ============================================================================

test('VideoEncoder: contentHint text is recognized', async (t) => {
  const config = {
    codec: 'vp8',
    width: 1280,
    height: 720,
    bitrate: 5_000_000,
    bitrateMode: 'constant' as const,
    framerate: 25,
    latencyMode: 'realtime' as const,
    contentHint: 'text',
  }

  const support = await VideoEncoder.isConfigSupported(config)
  t.true(support.supported, 'config with contentHint text should be supported')

  const newConfig = support.config
  t.is(newConfig?.codec, config.codec, 'codec preserved')
  // Note: contentHint may not be in TypeScript types but should be preserved
  t.is((newConfig as unknown as Record<string, unknown>)?.contentHint, 'text', 'contentHint preserved')
})

// ============================================================================
// Additional Content Hint Tests
// ============================================================================

test('VideoEncoder: contentHint motion is recognized', async (t) => {
  const config = {
    codec: 'vp8',
    width: 640,
    height: 480,
    bitrate: 2_000_000,
    contentHint: 'motion',
  }

  const support = await VideoEncoder.isConfigSupported(config)
  t.true(support.supported, 'config with contentHint motion should be supported')
  t.is((support.config as unknown as Record<string, unknown>)?.contentHint, 'motion', 'contentHint motion preserved')
})

test('VideoEncoder: contentHint detail is recognized', async (t) => {
  const config = {
    codec: 'vp8',
    width: 640,
    height: 480,
    bitrate: 2_000_000,
    contentHint: 'detail',
  }

  const support = await VideoEncoder.isConfigSupported(config)
  t.true(support.supported, 'config with contentHint detail should be supported')
  t.is((support.config as unknown as Record<string, unknown>)?.contentHint, 'detail', 'contentHint detail preserved')
})

test('VideoEncoder: config without contentHint is supported', async (t) => {
  const config = {
    codec: 'vp8',
    width: 640,
    height: 480,
    bitrate: 2_000_000,
  }

  const support = await VideoEncoder.isConfigSupported(config)
  t.true(support.supported, 'config without contentHint should be supported')
  // contentHint should be undefined or not present
  t.true(
    (support.config as unknown as Record<string, unknown>)?.contentHint === undefined ||
      (support.config as unknown as Record<string, unknown>)?.contentHint === null,
    'contentHint not set when not provided',
  )
})

test('VideoEncoder: contentHint with H.264', async (t) => {
  const config = {
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 2_000_000,
    contentHint: 'text',
    avc: { format: 'annexb' as const },
  }

  const support = await VideoEncoder.isConfigSupported(config)
  if (!support.supported) {
    t.pass('H.264 not supported - skipping')
    return
  }

  t.is((support.config as unknown as Record<string, unknown>)?.contentHint, 'text', 'contentHint preserved for H.264')
})

test('VideoEncoder: contentHint with VP9', async (t) => {
  const config = {
    codec: 'vp09.00.10.08',
    width: 640,
    height: 480,
    bitrate: 2_000_000,
    contentHint: 'motion',
  }

  const support = await VideoEncoder.isConfigSupported(config)
  if (!support.supported) {
    t.pass('VP9 not supported - skipping')
    return
  }

  t.is((support.config as unknown as Record<string, unknown>)?.contentHint, 'motion', 'contentHint preserved for VP9')
})

test('VideoEncoder: contentHint with AV1', async (t) => {
  const config = {
    codec: 'av01.0.04M.08',
    width: 640,
    height: 480,
    bitrate: 2_000_000,
    contentHint: 'detail',
  }

  const support = await VideoEncoder.isConfigSupported(config)
  if (!support.supported) {
    t.pass('AV1 not supported - skipping')
    return
  }

  t.is((support.config as unknown as Record<string, unknown>)?.contentHint, 'detail', 'contentHint preserved for AV1')
})
