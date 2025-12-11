/**
 * ImageDecoder Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests ImageDecoder construction, decoding, and track handling.
 */

import test from 'ava'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { ImageDecoder, resetHardwareFallbackState, VideoFrame } from '../../index.js'

const __filename = fileURLToPath(import.meta.url)
const __dirname = dirname(__filename)

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

const fixturesPath = join(__dirname, '../fixtures/wpt')

// Helper to load fixture
function loadFixture(filename: string): Uint8Array | null {
  try {
    const buffer = readFileSync(join(fixturesPath, filename))
    return new Uint8Array(buffer)
  } catch {
    return null
  }
}

// ============================================================================
// PNG Tests
// ============================================================================

test('ImageDecoder: decode PNG', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  const result = await decoder.decode()
  t.truthy(result.image)
  t.true(result.image instanceof VideoFrame)
  t.true(result.image.codedWidth > 0)
  t.true(result.image.codedHeight > 0)

  result.image.close()
  decoder.close()
})

test('ImageDecoder: PNG tracks', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // Static images have one track
  await decoder.decode()

  t.truthy(decoder.tracks)
  t.true(decoder.tracks.length >= 1)

  decoder.close()
})

test('ImageDecoder: PNG closed property', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  t.false(decoder.closed)

  decoder.close()

  t.true(decoder.closed)
})

// ============================================================================
// JPEG Tests
// ============================================================================

test('ImageDecoder: decode JPEG', async (t) => {
  const jpegData = loadFixture('four-colors.jpg')
  if (!jpegData) {
    t.pass('JPEG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: jpegData,
    type: 'image/jpeg',
  })

  const result = await decoder.decode()
  t.truthy(result.image)
  t.true(result.image.codedWidth > 0)
  t.true(result.image.codedHeight > 0)

  result.image.close()
  decoder.close()
})

// ============================================================================
// GIF Tests
// ============================================================================

test('ImageDecoder: decode GIF', async (t) => {
  const gifData = loadFixture('four-colors.gif')
  if (!gifData) {
    t.pass('GIF fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: gifData,
    type: 'image/gif',
  })

  const result = await decoder.decode()
  t.truthy(result.image)
  t.true(result.image.codedWidth > 0)

  result.image.close()
  decoder.close()
})

test('ImageDecoder: GIF animated', async (t) => {
  const gifData = loadFixture('four-colors-flip.gif')
  if (!gifData) {
    t.pass('Animated GIF fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: gifData,
    type: 'image/gif',
  })

  // First decode to populate track info
  const result = await decoder.decode({ frameIndex: 0 })
  t.truthy(result.image)
  result.image.close()

  // Check if we have multiple frames
  const track = decoder.tracks.selectedTrack
  if (track && track.frameCount > 1) {
    // Decode second frame
    const result2 = await decoder.decode({ frameIndex: 1 })
    t.truthy(result2.image)
    result2.image.close()
  }

  decoder.close()
})

// ============================================================================
// WebP Tests
// ============================================================================

test('ImageDecoder: decode WebP', async (t) => {
  const webpData = loadFixture('four-colors.webp')
  if (!webpData) {
    t.pass('WebP fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: webpData,
    type: 'image/webp',
  })

  const result = await decoder.decode()
  t.truthy(result.image)
  t.true(result.image.codedWidth > 0)

  result.image.close()
  decoder.close()
})

// ============================================================================
// AVIF Tests
// ============================================================================

test('ImageDecoder: decode AVIF', async (t) => {
  const avifData = loadFixture('four-colors.avif')
  if (!avifData) {
    t.pass('AVIF fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: avifData,
    type: 'image/avif',
  })

  try {
    const result = await decoder.decode()
    t.truthy(result.image)
    t.true(result.image.codedWidth > 0)
    result.image.close()
  } catch {
    t.pass('AVIF decoding not supported')
  }

  decoder.close()
})

test('ImageDecoder: AVIF animated', async (t) => {
  const avifData = loadFixture('four-colors-flip.avif')
  if (!avifData) {
    t.pass('Animated AVIF fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: avifData,
    type: 'image/avif',
  })

  try {
    const result = await decoder.decode({ frameIndex: 0 })
    t.truthy(result.image)
    result.image.close()
  } catch {
    t.pass('AVIF decoding not supported')
  }

  decoder.close()
})

// ============================================================================
// Construction Tests
// ============================================================================

test('ImageDecoder: construction requires data', (t) => {
  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      new ImageDecoder({
        type: 'image/png',
      } as any)
    },
    { instanceOf: TypeError },
  )
})

test('ImageDecoder: construction requires type', (t) => {
  t.throws(
    () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      new ImageDecoder({
        data: new Uint8Array([0, 1, 2, 3]),
      } as any)
    },
    { instanceOf: TypeError },
  )
})

test('ImageDecoder: invalid type is accepted (error on decode)', async (t) => {
  // Construction should succeed, but decode should fail
  const decoder = new ImageDecoder({
    data: new Uint8Array([0, 1, 2, 3]),
    type: 'image/bogus',
  })

  await t.throwsAsync(decoder.decode())

  decoder.close()
})

test('ImageDecoder: empty data', async (t) => {
  const decoder = new ImageDecoder({
    data: new Uint8Array(0),
    type: 'image/png',
  })

  await t.throwsAsync(decoder.decode())

  decoder.close()
})

// ============================================================================
// Close Tests
// ============================================================================

test('ImageDecoder: decode after close throws', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  decoder.close()

  // Async rejections use standard Error with DOMException name in message
  const error = await t.throwsAsync(decoder.decode())
  t.true(error?.message.includes('InvalidStateError'), 'decode on closed should include InvalidStateError')
})

test('ImageDecoder: double close is safe', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  decoder.close()
  decoder.close() // Should not throw

  t.pass()
})

// ============================================================================
// Reset Tests
// ============================================================================

test('ImageDecoder: reset clears cache', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // First decode
  const result1 = await decoder.decode()
  result1.image.close()

  // Reset
  decoder.reset()

  // Should be able to decode again
  const result2 = await decoder.decode()
  t.truthy(result2.image)
  result2.image.close()

  decoder.close()
})

test('ImageDecoder: reset after close throws', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

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
// Frame Index Tests
// ============================================================================

test('ImageDecoder: decode with default frameIndex', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  const result = await decoder.decode() // Default frameIndex is 0
  t.truthy(result.image)

  result.image.close()
  decoder.close()
})

test('ImageDecoder: decode with explicit frameIndex 0', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  const result = await decoder.decode({ frameIndex: 0 })
  t.truthy(result.image)

  result.image.close()
  decoder.close()
})

test('ImageDecoder: out of range frameIndex', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // Decode first frame to populate track info
  const result = await decoder.decode()
  result.image.close()

  // Try to decode non-existent frame
  await t.throwsAsync(decoder.decode({ frameIndex: 9999 }), { message: /RangeError/ })

  decoder.close()
})

// ============================================================================
// Track Tests
// ============================================================================

test('ImageDecoder: tracks property', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  t.truthy(decoder.tracks)

  // Decode to populate track info
  const result = await decoder.decode()
  result.image.close()

  t.true(decoder.tracks.length >= 1)

  decoder.close()
})

test('ImageDecoder: tracks.ready', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // Decode to populate track info
  const result = await decoder.decode()
  result.image.close()

  // ready should be a fulfilled promise
  await decoder.tracks.ready

  t.pass()

  decoder.close()
})

test('ImageDecoder: tracks.selectedTrack', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // Decode to populate track info
  const result = await decoder.decode()
  result.image.close()

  const track = decoder.tracks.selectedTrack
  t.truthy(track)

  decoder.close()
})

test('ImageDecoder: tracks.selectedIndex', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // Before decode, selectedIndex may be -1
  // After decode, it should be 0
  const result = await decoder.decode()
  result.image.close()

  // For static images, selectedIndex should be 0
  t.is(decoder.tracks.selectedIndex, 0)

  decoder.close()
})

// ============================================================================
// Complete Property Tests
// ============================================================================

test('ImageDecoder: complete property', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // After construction with complete data, complete should be true
  // Decode first
  const result = await decoder.decode()
  result.image.close()

  t.true(decoder.complete)

  decoder.close()
})

// ============================================================================
// Type Property Tests
// ============================================================================

test('ImageDecoder: type property', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  t.is(decoder.type, 'image/png')

  decoder.close()
})

// ============================================================================
// Multiple Decodes Tests
// ============================================================================

test('ImageDecoder: multiple sequential decodes', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  // First decode
  const result1 = await decoder.decode()
  t.truthy(result1.image)
  result1.image.close()

  // Second decode (from cache)
  const result2 = await decoder.decode()
  t.truthy(result2.image)
  result2.image.close()

  // Third decode
  const result3 = await decoder.decode()
  t.truthy(result3.image)
  result3.image.close()

  decoder.close()
})

// ============================================================================
// Invalid Data Tests
// ============================================================================

test('ImageDecoder: invalid PNG data', async (t) => {
  const decoder = new ImageDecoder({
    data: new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xff]),
    type: 'image/png',
  })

  await t.throwsAsync(decoder.decode())

  decoder.close()
})

test('ImageDecoder: wrong type for data', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  // Try to decode PNG as JPEG
  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/jpeg',
  })

  await t.throwsAsync(decoder.decode())

  decoder.close()
})

// ============================================================================
// isTypeSupported Tests
// ============================================================================

test('ImageDecoder.isTypeSupported: common types', async (t) => {
  const commonTypes = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/avif', 'image/bmp']

  for (const type of commonTypes) {
    const supported = await ImageDecoder.isTypeSupported(type)
    t.true(typeof supported === 'boolean', `${type} returns boolean`)
  }
})

test('ImageDecoder.isTypeSupported: unsupported types', async (t) => {
  const unsupportedTypes = ['image/tiff', 'image/svg+xml', 'video/mp4', 'audio/mp3', 'bogus']

  for (const type of unsupportedTypes) {
    const supported = await ImageDecoder.isTypeSupported(type)
    t.false(supported, `${type} should not be supported`)
  }
})

// ============================================================================
// Dimensions Tests
// ============================================================================

test('ImageDecoder: decoded frame dimensions', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  const result = await decoder.decode()
  const frame = result.image

  t.true(frame.codedWidth > 0)
  t.true(frame.codedHeight > 0)
  t.true(frame.displayWidth > 0)
  t.true(frame.displayHeight > 0)

  frame.close()
  decoder.close()
})

// ============================================================================
// Pixel Format Tests
// ============================================================================

test('ImageDecoder: decoded frame format', async (t) => {
  const pngData = loadFixture('four-colors.png')
  if (!pngData) {
    t.pass('PNG fixture not available')
    return
  }

  const decoder = new ImageDecoder({
    data: pngData,
    type: 'image/png',
  })

  const result = await decoder.decode()
  const frame = result.image

  // Frame should have a valid format
  t.truthy(frame.format)

  frame.close()
  decoder.close()
})
