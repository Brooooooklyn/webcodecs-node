/**
 * Hardware Acceleration API Tests
 *
 * Tests hardware accelerator detection and usage.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import test from 'ava'

import {
  getHardwareAccelerators,
  getAvailableHardwareAccelerators,
  getPreferredHardwareAccelerator,
  isHardwareAcceleratorAvailable,
  VideoEncoder,
} from '../index.js'
import {
  generateSolidColorI420Frame,
  TestColors,
  type EncodedVideoChunkOutput,
  type VideoEncoderOutput,
} from './helpers/index.js'
import { createEncoderConfig } from './helpers/codec-matrix.js'

// Helper to create test encoder with callbacks
function createTestEncoder() {
  const chunks: EncodedVideoChunkOutput[] = []
  const errors: Error[] = []

  const encoder = new VideoEncoder(
    (result: VideoEncoderOutput) => {
      // VideoEncoder callback receives [chunk, metadata] tuple
      const [chunk] = result
      chunks.push(chunk)
    },
    (e) => errors.push(e),
  )

  return { encoder, chunks, errors }
}

// ============================================================================
// getHardwareAccelerators() Tests
// ============================================================================

test('getHardwareAccelerators: returns array of accelerators', (t) => {
  const accelerators = getHardwareAccelerators()

  t.true(Array.isArray(accelerators))
  t.true(accelerators.length > 0, 'Should list known accelerators')
})

test('getHardwareAccelerators: each accelerator has required properties', (t) => {
  const accelerators = getHardwareAccelerators()

  for (const accel of accelerators) {
    t.is(typeof accel.name, 'string', 'name should be string')
    t.true(accel.name.length > 0, 'name should not be empty')

    t.is(typeof accel.description, 'string', 'description should be string')
    t.true(accel.description.length > 0, 'description should not be empty')

    t.is(typeof accel.available, 'boolean', 'available should be boolean')
  }
})

test('getHardwareAccelerators: includes known accelerator names', (t) => {
  const accelerators = getHardwareAccelerators()
  const names = accelerators.map((a) => a.name)

  // These should always be listed (even if not available)
  const knownAccelerators = ['videotoolbox', 'cuda', 'vaapi', 'd3d11va']

  for (const known of knownAccelerators) {
    t.true(names.includes(known), `Should include ${known} in accelerator list`)
  }
})

// ============================================================================
// getAvailableHardwareAccelerators() Tests
// ============================================================================

test('getAvailableHardwareAccelerators: returns array of strings', (t) => {
  const available = getAvailableHardwareAccelerators()

  t.true(Array.isArray(available))

  for (const name of available) {
    t.is(typeof name, 'string')
  }
})

test('getAvailableHardwareAccelerators: is subset of all accelerators', (t) => {
  const all = getHardwareAccelerators()
  const available = getAvailableHardwareAccelerators()

  const allNames = new Set(all.map((a) => a.name))

  for (const name of available) {
    t.true(allNames.has(name), `Available accelerator ${name} should be in full list`)
  }
})

test('getAvailableHardwareAccelerators: matches available flag', (t) => {
  const all = getHardwareAccelerators()
  const available = getAvailableHardwareAccelerators()
  const availableSet = new Set(available)

  for (const accel of all) {
    if (accel.available) {
      t.true(availableSet.has(accel.name), `${accel.name} marked available but not in getAvailableHardwareAccelerators()`)
    } else {
      t.false(availableSet.has(accel.name), `${accel.name} marked unavailable but in getAvailableHardwareAccelerators()`)
    }
  }
})

// ============================================================================
// isHardwareAcceleratorAvailable() Tests
// ============================================================================

test('isHardwareAcceleratorAvailable: returns boolean', (t) => {
  const result = isHardwareAcceleratorAvailable('videotoolbox')
  t.is(typeof result, 'boolean')
})

test('isHardwareAcceleratorAvailable: matches getHardwareAccelerators()', (t) => {
  const accelerators = getHardwareAccelerators()

  for (const accel of accelerators) {
    const isAvailable = isHardwareAcceleratorAvailable(accel.name)
    t.is(isAvailable, accel.available, `Availability mismatch for ${accel.name}`)
  }
})

test('isHardwareAcceleratorAvailable: returns false for unknown accelerator', (t) => {
  const result = isHardwareAcceleratorAvailable('nonexistent-accelerator')
  t.false(result)
})

test('isHardwareAcceleratorAvailable: handles aliases', (t) => {
  // nvenc is an alias for cuda
  const cudaAvailable = isHardwareAcceleratorAvailable('cuda')
  const nvencAvailable = isHardwareAcceleratorAvailable('nvenc')

  t.is(cudaAvailable, nvencAvailable, 'cuda and nvenc should have same availability')
})

// ============================================================================
// getPreferredHardwareAccelerator() Tests
// ============================================================================

test('getPreferredHardwareAccelerator: returns string or null', (t) => {
  const preferred = getPreferredHardwareAccelerator()

  t.true(preferred === null || typeof preferred === 'string')
})

test('getPreferredHardwareAccelerator: returns available accelerator if any', (t) => {
  const preferred = getPreferredHardwareAccelerator()
  const available = getAvailableHardwareAccelerators()

  if (preferred !== null) {
    t.true(available.includes(preferred), `Preferred ${preferred} should be in available list`)
  }
})

test('getPreferredHardwareAccelerator: returns null when none available', (t) => {
  const available = getAvailableHardwareAccelerators()
  const preferred = getPreferredHardwareAccelerator()

  if (available.length === 0) {
    t.is(preferred, null, 'Should return null when no accelerators available')
  } else {
    t.truthy(preferred, 'Should return a preferred accelerator when some are available')
  }
})

// ============================================================================
// Platform-Specific Tests
// ============================================================================

test('platform: macOS should report videotoolbox', (t) => {
  if (process.platform !== 'darwin') {
    t.pass('Skipping macOS-specific test')
    return
  }

  const accelerators = getHardwareAccelerators()
  const vt = accelerators.find((a) => a.name === 'videotoolbox')

  t.truthy(vt, 'videotoolbox should be listed on macOS')

  // On most Macs, VideoToolbox should be available
  // (but could fail on very old hardware or VMs)
  if (vt) {
    t.log(`videotoolbox available: ${vt.available}`)
  }
})

test('platform: preferred accelerator matches platform', (t) => {
  const preferred = getPreferredHardwareAccelerator()

  if (preferred === null) {
    t.pass('No hardware accelerator available')
    return
  }

  switch (process.platform) {
    case 'darwin':
      // macOS should prefer videotoolbox
      t.is(preferred, 'videotoolbox', 'macOS should prefer videotoolbox')
      break
    case 'linux':
      // Linux could prefer vaapi or cuda
      t.true(['vaapi', 'cuda', 'qsv'].includes(preferred), `Linux should prefer vaapi, cuda, or qsv, got ${preferred}`)
      break
    case 'win32':
      // Windows could prefer d3d11va, cuda, or qsv
      t.true(['d3d11va', 'cuda', 'qsv'].includes(preferred), `Windows should prefer d3d11va, cuda, or qsv, got ${preferred}`)
      break
    default:
      t.pass(`Unknown platform: ${process.platform}`)
  }
})

// ============================================================================
// Hardware Encoding Tests (conditional)
// ============================================================================

test('hardware encoding: H.264 with prefer-hardware', async (t) => {
  const preferred = getPreferredHardwareAccelerator()

  if (preferred === null) {
    t.pass('No hardware accelerator available, skipping')
    return
  }

  const { encoder, chunks } = createTestEncoder()

  const config = createEncoderConfig('h264', 320, 240, {
    hardwareAcceleration: 'prefer-hardware',
  })

  t.notThrows(() => {
    encoder.configure(config)
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)

  t.notThrows(() => {
    encoder.encode(frame, { keyFrame: true })
  })

  frame.close()
  await encoder.flush()

  // Should produce output
  t.true(chunks.length > 0, 'Hardware encoder should produce output')

  encoder.close()
})

test('hardware encoding: prefer-software still works', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  const config = createEncoderConfig('h264', 320, 240, {
    hardwareAcceleration: 'prefer-software',
  })

  t.notThrows(() => {
    encoder.configure(config)
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, 0)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()

  t.true(chunks.length > 0)

  encoder.close()
})

test('hardware encoding: no-preference fallback', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  const config = createEncoderConfig('h264', 320, 240, {
    hardwareAcceleration: 'no-preference',
  })

  t.notThrows(() => {
    encoder.configure(config)
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)

  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await encoder.flush()

  t.true(chunks.length > 0)

  encoder.close()
})
