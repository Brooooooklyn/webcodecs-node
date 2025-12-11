/**
 * VideoColorSpace Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests VideoColorSpace construction and toJSON method.
 */

import test from 'ava'

import { resetHardwareFallbackState, VideoColorSpace } from '../../index.js'
import { generateAllColorSpaceCombinations, VIDEO_COLOR_SPACE_SETS } from '../helpers/wpt-utils.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// Construction Tests
// ============================================================================

test('VideoColorSpace: default construction', (t) => {
  const colorSpace = new VideoColorSpace()

  // W3C spec: properties return null for unset values
  t.is(colorSpace.primaries, null)
  t.is(colorSpace.transfer, null)
  t.is(colorSpace.matrix, null)
  t.is(colorSpace.fullRange, null)
})

test('VideoColorSpace: construction with empty object', (t) => {
  const colorSpace = new VideoColorSpace({})

  // W3C spec: properties return null for unset values
  t.is(colorSpace.primaries, null)
  t.is(colorSpace.transfer, null)
  t.is(colorSpace.matrix, null)
  t.is(colorSpace.fullRange, null)
})

test('VideoColorSpace: construction with all fields', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt709',
    transfer: 'bt709',
    matrix: 'bt709',
    fullRange: true,
  })

  t.is(colorSpace.primaries, 'bt709')
  t.is(colorSpace.transfer, 'bt709')
  t.is(colorSpace.matrix, 'bt709')
  t.is(colorSpace.fullRange, true)
})

test('VideoColorSpace: construction with fullRange false', (t) => {
  const colorSpace = new VideoColorSpace({
    fullRange: false,
  })

  t.is(colorSpace.fullRange, false)
})

// ============================================================================
// Primaries Tests
// ============================================================================

for (const primaries of VIDEO_COLOR_SPACE_SETS.primaries) {
  test(`VideoColorSpace: primaries ${primaries}`, (t) => {
    const colorSpace = new VideoColorSpace({ primaries })
    t.is(colorSpace.primaries, primaries)
  })
}

test('VideoColorSpace: invalid primaries throws', (t) => {
  t.throws(
    () => {
      new VideoColorSpace({ primaries: 'invalid' as any })
    },
    { instanceOf: TypeError },
  )
})

// ============================================================================
// Transfer Tests
// ============================================================================

for (const transfer of VIDEO_COLOR_SPACE_SETS.transfer) {
  test(`VideoColorSpace: transfer ${transfer}`, (t) => {
    const colorSpace = new VideoColorSpace({ transfer })
    t.is(colorSpace.transfer, transfer)
  })
}

test('VideoColorSpace: invalid transfer throws', (t) => {
  t.throws(
    () => {
      new VideoColorSpace({ transfer: 'invalid' as any })
    },
    { instanceOf: TypeError },
  )
})

// ============================================================================
// Matrix Tests
// ============================================================================

for (const matrix of VIDEO_COLOR_SPACE_SETS.matrix) {
  test(`VideoColorSpace: matrix ${matrix}`, (t) => {
    const colorSpace = new VideoColorSpace({ matrix })
    t.is(colorSpace.matrix, matrix)
  })
}

test('VideoColorSpace: invalid matrix throws', (t) => {
  t.throws(
    () => {
      new VideoColorSpace({ matrix: 'invalid' as any })
    },
    { instanceOf: TypeError },
  )
})

// ============================================================================
// toJSON Tests
// ============================================================================

test('VideoColorSpace: toJSON default', (t) => {
  const colorSpace = new VideoColorSpace()
  const json = colorSpace.toJSON()

  t.deepEqual(json, {
    primaries: null,
    transfer: null,
    matrix: null,
    fullRange: null,
  })
})

test('VideoColorSpace: toJSON with values', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt709',
    transfer: 'srgb',
    matrix: 'rgb',
    fullRange: true,
  })
  const json = colorSpace.toJSON()

  t.deepEqual(json, {
    primaries: 'bt709',
    transfer: 'srgb',
    matrix: 'rgb',
    fullRange: true,
  })
})

test('VideoColorSpace: toJSON roundtrip', (t) => {
  const original = new VideoColorSpace({
    primaries: 'bt2020',
    transfer: 'pq',
    matrix: 'bt2020-ncl',
    fullRange: false,
  })

  const json = original.toJSON()
  const recreated = new VideoColorSpace(json)

  t.is(recreated.primaries, original.primaries)
  t.is(recreated.transfer, original.transfer)
  t.is(recreated.matrix, original.matrix)
  t.is(recreated.fullRange, original.fullRange)
})

// ============================================================================
// Common Combinations Tests
// ============================================================================

test('VideoColorSpace: BT.709 standard', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt709',
    transfer: 'bt709',
    matrix: 'bt709',
    fullRange: false,
  })

  t.is(colorSpace.primaries, 'bt709')
  t.is(colorSpace.transfer, 'bt709')
  t.is(colorSpace.matrix, 'bt709')
  t.is(colorSpace.fullRange, false)
})

test('VideoColorSpace: BT.2020 HDR PQ', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt2020',
    transfer: 'pq',
    matrix: 'bt2020-ncl',
    fullRange: false,
  })

  t.is(colorSpace.primaries, 'bt2020')
  t.is(colorSpace.transfer, 'pq')
  t.is(colorSpace.matrix, 'bt2020-ncl')
  t.is(colorSpace.fullRange, false)
})

test('VideoColorSpace: BT.2020 HDR HLG', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt2020',
    transfer: 'hlg',
    matrix: 'bt2020-ncl',
    fullRange: false,
  })

  t.is(colorSpace.primaries, 'bt2020')
  t.is(colorSpace.transfer, 'hlg')
  t.is(colorSpace.matrix, 'bt2020-ncl')
})

test('VideoColorSpace: sRGB', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt709',
    transfer: 'srgb',
    matrix: 'rgb',
    fullRange: true,
  })

  t.is(colorSpace.primaries, 'bt709')
  t.is(colorSpace.transfer, 'srgb')
  t.is(colorSpace.matrix, 'rgb')
  t.is(colorSpace.fullRange, true)
})

test('VideoColorSpace: SMPTE 170M (NTSC)', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'smpte170m',
    transfer: 'smpte170m',
    matrix: 'smpte170m',
    fullRange: false,
  })

  t.is(colorSpace.primaries, 'smpte170m')
  t.is(colorSpace.transfer, 'smpte170m')
  t.is(colorSpace.matrix, 'smpte170m')
})

test('VideoColorSpace: BT.470BG (PAL)', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt470bg',
    transfer: 'bt709',
    matrix: 'bt470bg',
    fullRange: false,
  })

  t.is(colorSpace.primaries, 'bt470bg')
  t.is(colorSpace.matrix, 'bt470bg')
})

// ============================================================================
// Partial Construction Tests
// ============================================================================

test('VideoColorSpace: only primaries', (t) => {
  const colorSpace = new VideoColorSpace({ primaries: 'bt709' })

  t.is(colorSpace.primaries, 'bt709')
  t.is(colorSpace.transfer, null)
  t.is(colorSpace.matrix, null)
  t.is(colorSpace.fullRange, null)
})

test('VideoColorSpace: only transfer', (t) => {
  const colorSpace = new VideoColorSpace({ transfer: 'pq' })

  t.is(colorSpace.primaries, null)
  t.is(colorSpace.transfer, 'pq')
  t.is(colorSpace.matrix, null)
  t.is(colorSpace.fullRange, null)
})

test('VideoColorSpace: only matrix', (t) => {
  const colorSpace = new VideoColorSpace({ matrix: 'rgb' })

  t.is(colorSpace.primaries, null)
  t.is(colorSpace.transfer, null)
  t.is(colorSpace.matrix, 'rgb')
  t.is(colorSpace.fullRange, null)
})

test('VideoColorSpace: only fullRange', (t) => {
  const colorSpace = new VideoColorSpace({ fullRange: true })

  t.is(colorSpace.primaries, null)
  t.is(colorSpace.transfer, null)
  t.is(colorSpace.matrix, null)
  t.is(colorSpace.fullRange, true)
})

// ============================================================================
// All Combinations Tests
// ============================================================================

// Test a subset of all valid combinations
const combinations = generateAllColorSpaceCombinations().slice(0, 20) // Test first 20 combinations

for (const combo of combinations) {
  const name = `VideoColorSpace: combo ${combo.primaries || 'null'}/${combo.transfer || 'null'}/${combo.matrix || 'null'}/${combo.fullRange ?? 'null'}`
  test(name, (t) => {
    const colorSpace = new VideoColorSpace(combo)

    t.is(colorSpace.primaries, combo.primaries ?? null)
    t.is(colorSpace.transfer, combo.transfer ?? null)
    t.is(colorSpace.matrix, combo.matrix ?? null)
    t.is(colorSpace.fullRange, combo.fullRange ?? null)

    // Verify toJSON roundtrip
    const json = colorSpace.toJSON()
    const recreated = new VideoColorSpace(json)
    t.is(recreated.primaries, colorSpace.primaries)
    t.is(recreated.transfer, colorSpace.transfer)
    t.is(recreated.matrix, colorSpace.matrix)
    t.is(recreated.fullRange, colorSpace.fullRange)
  })
}

// ============================================================================
// Immutability Tests
// ============================================================================

test('VideoColorSpace: properties are readonly', (t) => {
  const colorSpace = new VideoColorSpace({
    primaries: 'bt709',
    transfer: 'bt709',
    matrix: 'bt709',
    fullRange: false,
  })

  // In strict mode (ESM), assigning to getter-only properties throws TypeError
  t.throws(
    () => {
      // @ts-expect-error - Testing immutability
      colorSpace.primaries = 'bt2020'
    },
    { instanceOf: TypeError },
  )
  t.throws(
    () => {
      // @ts-expect-error - Testing immutability
      colorSpace.transfer = 'pq'
    },
    { instanceOf: TypeError },
  )
  t.throws(
    () => {
      // @ts-expect-error - Testing immutability
      colorSpace.matrix = 'rgb'
    },
    { instanceOf: TypeError },
  )
  t.throws(
    () => {
      // @ts-expect-error - Testing immutability
      colorSpace.fullRange = true
    },
    { instanceOf: TypeError },
  )

  // Values should remain unchanged
  t.is(colorSpace.primaries, 'bt709')
  t.is(colorSpace.transfer, 'bt709')
  t.is(colorSpace.matrix, 'bt709')
  t.is(colorSpace.fullRange, false)
})
