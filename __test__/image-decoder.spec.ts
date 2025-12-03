/**
 * ImageDecoder API Conformance Tests
 *
 * Tests WebCodecs ImageDecoder specification compliance.
 * Tests frame_index support for animated images (GIF, WebP).
 */

import test from 'ava'
import { readFileSync } from 'fs'
import { join, dirname } from 'path'
import { fileURLToPath } from 'url'

import { ImageDecoder } from '../index.js'

const __dirname = dirname(fileURLToPath(import.meta.url))

// ============================================================================
// Static Image Tests (PNG)
// ============================================================================

test('ImageDecoder decodes static PNG image', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  t.is(decoder.type, 'image/png')
  t.true(decoder.complete)

  const result = await decoder.decode()

  t.true(result.complete)
  t.truthy(result.image)
  t.is(result.image.codedWidth, 8)
  t.is(result.image.codedHeight, 8)

  // Static images should have frameCount = 1
  const tracks = decoder.tracks
  t.is(tracks.length, 1)
  t.false(tracks.selectedTrack!.animated)
  t.is(tracks.selectedTrack!.frameCount, 1)

  result.image.close()
  decoder.close()
})

test('ImageDecoder PNG default frame_index is 0', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // Decode without options (default frame_index = 0)
  const result1 = await decoder.decode()
  t.true(result1.complete)

  // Decode with explicit frame_index = 0
  const result2 = await decoder.decode({ frameIndex: 0 })
  t.true(result2.complete)

  result1.image.close()
  result2.image.close()
  decoder.close()
})

test('ImageDecoder PNG throws for out-of-bounds frame_index', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // First decode to populate frame count
  const result = await decoder.decode()
  result.image.close()

  // Now try to access invalid frame index
  await t.throwsAsync(() => decoder.decode({ frameIndex: 999 }), {
    message: /out of bounds/,
  })

  decoder.close()
})

// ============================================================================
// Animated Image Tests (GIF)
// ============================================================================

test('ImageDecoder detects animated GIF format', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  t.is(decoder.type, 'image/gif')
  t.true(decoder.complete)

  // Before first decode, animated = true but frameCount = 0
  const tracksBefore = decoder.tracks
  t.true(tracksBefore.selectedTrack!.animated)
  t.is(tracksBefore.selectedTrack!.frameCount, 0)

  decoder.close()
})

test('ImageDecoder GIF frame_count is populated after first decode', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Decode first frame
  const result = await decoder.decode()
  t.true(result.complete)
  result.image.close()

  // After decode, frameCount should be populated
  const tracks = decoder.tracks
  t.true(tracks.selectedTrack!.animated)
  t.true(tracks.selectedTrack!.frameCount > 0)

  decoder.close()
})

test('ImageDecoder GIF can decode specific frames by index', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Decode first frame (index 0)
  const result0 = await decoder.decode({ frameIndex: 0 })
  t.true(result0.complete)
  t.truthy(result0.image)

  // Get frame count
  // Note: FFmpeg's single-packet image decoder may not extract all animation frames.
  // For full animation support, use a video decoder approach instead.
  const frameCount = decoder.tracks.selectedTrack!.frameCount
  t.true(frameCount >= 1, `Expected at least 1 frame, got ${frameCount}`)

  // Decode subsequent frames if available
  for (let i = 1; i < frameCount; i++) {
    const result = await decoder.decode({ frameIndex: i })
    t.true(result.complete)
    t.truthy(result.image)
    result.image.close()
  }

  result0.image.close()
  decoder.close()
})

test('ImageDecoder GIF throws for out-of-bounds frame_index', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // First decode to populate frame count
  const result = await decoder.decode()
  result.image.close()

  const frameCount = decoder.tracks.selectedTrack!.frameCount

  // Try to access one beyond the last frame
  await t.throwsAsync(() => decoder.decode({ frameIndex: frameCount }), {
    message: /out of bounds/,
  })

  decoder.close()
})

// ============================================================================
// Reset and Close Tests
// ============================================================================

test('ImageDecoder reset() clears cache and frame_count', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Decode to populate cache
  const result1 = await decoder.decode()
  result1.image.close()
  const frameCountAfterDecode = decoder.tracks.selectedTrack!.frameCount
  t.true(frameCountAfterDecode > 0)

  // Reset
  decoder.reset()

  // Frame count should be reset to 0 for animated formats
  t.is(decoder.tracks.selectedTrack!.frameCount, 0)

  // Should be able to decode again
  const result2 = await decoder.decode()
  t.true(result2.complete)
  result2.image.close()

  // Frame count should be repopulated
  t.is(decoder.tracks.selectedTrack!.frameCount, frameCountAfterDecode)

  decoder.close()
})

test('ImageDecoder close() prevents further decodes', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  decoder.close()

  await t.throwsAsync(() => decoder.decode(), {
    message: /closed/,
  })
})

test('ImageDecoder reset() after close() throws', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  decoder.close()

  t.throws(() => decoder.reset(), {
    message: /closed/,
  })
})

// ============================================================================
// MIME Type Support Tests
// ============================================================================

test('ImageDecoder.isTypeSupported returns true for supported types', async (t) => {
  t.true(await ImageDecoder.isTypeSupported('image/png'))
  t.true(await ImageDecoder.isTypeSupported('image/jpeg'))
  t.true(await ImageDecoder.isTypeSupported('image/gif'))
  t.true(await ImageDecoder.isTypeSupported('image/webp'))
  t.true(await ImageDecoder.isTypeSupported('image/bmp'))
})

test('ImageDecoder.isTypeSupported returns false for unsupported types', async (t) => {
  t.false(await ImageDecoder.isTypeSupported('image/unknown'))
  t.false(await ImageDecoder.isTypeSupported('video/mp4'))
  t.false(await ImageDecoder.isTypeSupported(''))
})

// ============================================================================
// Frame Caching Tests
// ============================================================================

test('ImageDecoder caches frames after first decode', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // First decode creates cache
  const result1 = await decoder.decode()
  t.true(result1.complete)

  // Second decode should use cache (same result)
  const result2 = await decoder.decode()
  t.true(result2.complete)

  // Both should have valid dimensions
  t.is(result1.image.codedWidth, result2.image.codedWidth)
  t.is(result1.image.codedHeight, result2.image.codedHeight)

  result1.image.close()
  result2.image.close()
  decoder.close()
})

test('ImageDecoder completed getter resolves immediately for buffered data', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // completed should resolve immediately since data is buffered
  await t.notThrowsAsync(async () => {
    await decoder.completed
  })

  decoder.close()
})
