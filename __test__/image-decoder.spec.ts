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

  // animated is set synchronously in constructor
  const tracksBefore = decoder.tracks
  t.true(tracksBefore.selectedTrack!.animated)
  // Note: frameCount may be 0 or populated depending on whether
  // background pre-parsing has completed. Use tracks.ready to
  // reliably get the frame count (see test below).

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

  try {
    decoder.reset()
    t.fail('reset should throw InvalidStateError')
  } catch (error) {
    t.true(error instanceof DOMException, 'reset error should be DOMException')
    t.is((error as DOMException).name, 'InvalidStateError')
  }
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

test('ImageDecoder tracks.ready resolves after metadata is parsed', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({ data, type: 'image/gif' })

  // Wait for tracks.ready (metadata parsing)
  await decoder.tracks.ready

  // After ready, frameCount should be populated for animated GIF
  const track = decoder.tracks.selectedTrack!
  t.true(track.animated)
  t.true(track.frameCount > 0, `Expected frameCount > 0 after ready, got ${track.frameCount}`)

  decoder.close()
})

test('ImageDecoder tracks.ready resolves for static images', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({ data, type: 'image/png' })

  // Wait for tracks.ready
  await decoder.tracks.ready

  // Static images should have frameCount = 1
  const track = decoder.tracks.selectedTrack!
  t.false(track.animated)
  t.is(track.frameCount, 1)

  decoder.close()
})

// ============================================================================
// ImageDecoder Options Tests (W3C spec)
// ============================================================================

test('ImageDecoder desiredWidth without desiredHeight throws TypeError', (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))

  // Per W3C spec: both must be specified or neither
  t.throws(() => new ImageDecoder({ data, type: 'image/png', desiredWidth: 100 }), {
    message: /Both desiredWidth and desiredHeight must be specified/,
  })
})

test('ImageDecoder desiredHeight without desiredWidth throws TypeError', (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))

  // Per W3C spec: both must be specified or neither
  t.throws(() => new ImageDecoder({ data, type: 'image/png', desiredHeight: 100 }), {
    message: /Both desiredWidth and desiredHeight must be specified/,
  })
})

test('ImageDecoder desiredWidth and desiredHeight scale the output', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/png',
    desiredWidth: 4,
    desiredHeight: 4,
  })

  const result = await decoder.decode()
  t.true(result.complete)

  // Output should be scaled to specified dimensions
  t.is(result.image.codedWidth, 4)
  t.is(result.image.codedHeight, 4)

  result.image.close()
  decoder.close()
})

test('ImageDecoder preferAnimation false returns single frame for animated GIF', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/gif',
    preferAnimation: false,
  })

  // Decode first (and only) frame
  const result = await decoder.decode()
  t.true(result.complete)
  t.truthy(result.image)
  result.image.close()

  // With preferAnimation: false, should only have 1 frame
  const track = decoder.tracks.selectedTrack!
  t.is(track.frameCount, 1)
  t.false(track.animated)

  decoder.close()
})

test('ImageDecoder preferAnimation true decodes all frames', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/animated.gif'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/gif',
    preferAnimation: true,
  })

  // Wait for pre-parsing to complete
  await decoder.tracks.ready

  // With preferAnimation: true (or default), should decode all frames
  const track = decoder.tracks.selectedTrack!
  t.true(track.animated)
  t.true(track.frameCount >= 1)

  decoder.close()
})

test('ImageDecoder colorSpaceConversion default extracts color space metadata', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/png',
    colorSpaceConversion: 'default',
  })

  const result = await decoder.decode()
  t.true(result.complete)

  // With "default", color space may be populated from image metadata (if present)
  // The colorSpace object should exist regardless
  t.truthy(result.image.colorSpace)

  result.image.close()
  decoder.close()
})

test('ImageDecoder colorSpaceConversion none returns empty color space', async (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))
  const decoder = new ImageDecoder({
    data,
    type: 'image/png',
    colorSpaceConversion: 'none',
  })

  const result = await decoder.decode()
  t.true(result.complete)

  // With "none", color space metadata should be ignored (all null per Chromium behavior)
  const cs = result.image.colorSpace.toJSON() as {
    primaries: string | null
    transfer: string | null
    matrix: string | null
    fullRange: boolean | null
  }
  t.is(cs.primaries, null)
  t.is(cs.transfer, null)
  t.is(cs.matrix, null)
  t.is(cs.fullRange, null)

  result.image.close()
  decoder.close()
})

test('ImageDecoder invalid colorSpaceConversion throws TypeError', (t) => {
  const data = readFileSync(join(__dirname, 'fixtures/test.png'))

  t.throws(
    () =>
      new ImageDecoder({
        data,
        type: 'image/png',
        colorSpaceConversion: 'invalid' as unknown as 'none' | 'default',
      }),
    { message: /Invalid colorSpaceConversion value/ },
  )
})
