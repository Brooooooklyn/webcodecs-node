/**
 * VideoFrame geometry parity with Chrome
 *
 * Display size validation, rotation rounding and orientation after close.
 * Expected values were recorded in Chrome 155.
 */

import test from 'ava'

import { VideoFrame } from '../index.js'

type FrameInit = {
  visibleRect?: { x: number; y: number; width: number; height: number }
  displayWidth?: number
  displayHeight?: number
  rotation?: number
  flip?: boolean
}

function rgbx(width: number, height: number, init: FrameInit = {}): VideoFrame {
  return new VideoFrame(new Uint8Array(width * height * 4), {
    format: 'RGBX',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
    ...init,
  })
}

// Builds a frame through both constructors: from a buffer, and by wrapping a plain frame.
const constructors: Array<[string, (init: FrameInit) => VideoFrame]> = [
  ['buffer', (init) => rgbx(4, 2, init)],
  [
    'wrap',
    (init) => {
      const source = rgbx(4, 2)
      try {
        return new VideoFrame(source, init)
      } finally {
        source.close()
      }
    },
  ],
]

// ============================================================================
// Display size validation (Chromium ParseAndValidateDisplaySize)
// ============================================================================

const invalidDisplaySizes: Array<[string, FrameInit]> = [
  ['displayWidth without displayHeight', { displayWidth: 8 }],
  ['displayHeight without displayWidth', { displayHeight: 8 }],
  ['zero displayWidth without displayHeight', { displayWidth: 0 }],
  ['zero displayWidth', { displayWidth: 0, displayHeight: 4 }],
  ['zero displayHeight', { displayWidth: 4, displayHeight: 0 }],
  ['negative displayWidth', { displayWidth: -1, displayHeight: 4 }],
  ['NaN displayWidth', { displayWidth: NaN, displayHeight: 4 }],
  ['infinite displayHeight', { displayWidth: 4, displayHeight: Infinity }],
  ['displayWidth 2^32', { displayWidth: 2 ** 32, displayHeight: 4 }],
  ['displayWidth 2^32 + 1', { displayWidth: 2 ** 32 + 1, displayHeight: 4 }],
  ['displayWidth 2^32 - 1', { displayWidth: 2 ** 32 - 1, displayHeight: 4 }],
  ['displayWidth above 32767', { displayWidth: 32768, displayHeight: 1 }],
  ['displayHeight above 32767', { displayWidth: 1, displayHeight: 32768 }],
  ['display area above 2^29 - 1', { displayWidth: 23171, displayHeight: 23171 }],
  ['non-finite rotation', { rotation: NaN }],
  ['infinite rotation', { rotation: Infinity }],
  ['negative infinite rotation', { rotation: -Infinity }],
]

for (const [kind, make] of constructors) {
  for (const [name, init] of invalidDisplaySizes) {
    test(`VideoFrame ${kind}: ${name} throws TypeError`, (t) => {
      t.throws(() => make(init).close(), { instanceOf: TypeError })
    })
  }

  test(`VideoFrame ${kind}: display sizes at the limits are accepted`, (t) => {
    for (const [width, height] of [
      [32767, 16384],
      [23170, 23170],
      [1, 32767],
    ]) {
      const frame = make({ displayWidth: width, displayHeight: height })
      t.deepEqual([frame.displayWidth, frame.displayHeight], [width, height])
      frame.close()
    }
    // [EnforceRange] truncates fractions
    const frame = make({ displayWidth: 1.9, displayHeight: 4.5 })
    t.deepEqual([frame.displayWidth, frame.displayHeight], [1, 4])
    frame.close()
  })

  test(`VideoFrame ${kind}: invalid display size errors match Chrome`, (t) => {
    t.throws(() => make({ displayWidth: 8 }), {
      instanceOf: TypeError,
      message: 'displayWidth specified without displayHeight.',
    })
    t.throws(() => make({ displayHeight: 8 }), {
      instanceOf: TypeError,
      message: 'displayHeight specified without displayWidth.',
    })
    t.throws(() => make({ displayWidth: 4, displayHeight: 0 }), {
      instanceOf: TypeError,
      message: 'displayHeight must be nonzero.',
    })
    t.throws(() => make({ displayWidth: 4294967295, displayHeight: 4 }), {
      instanceOf: TypeError,
      message: 'Invalid display size (4294967295, 4); exceeds implementation limit.',
    })
  })
}

// ============================================================================
// Computed display size of a cropped wrap (spec step 8.5)
// ============================================================================

test('VideoFrame: a crop whose computed display size rounds to 0 throws TypeError', (t) => {
  const narrow = rgbx(4, 2, { displayWidth: 1, displayHeight: 1 })
  const short = rgbx(4, 4, { displayWidth: 4, displayHeight: 1 })
  try {
    t.throws(() => new VideoFrame(narrow, { visibleRect: { x: 0, y: 0, width: 1, height: 1 } }), {
      instanceOf: TypeError,
      message: 'computed displayWidth must be nonzero',
    })
    t.throws(() => new VideoFrame(short, { visibleRect: { x: 0, y: 0, width: 4, height: 1 } }), {
      instanceOf: TypeError,
      message: 'computed displayHeight must be nonzero',
    })
    // An explicit display size is not computed
    const explicit = new VideoFrame(narrow, {
      visibleRect: { x: 0, y: 0, width: 1, height: 1 },
      displayWidth: 3,
      displayHeight: 3,
    })
    t.deepEqual([explicit.displayWidth, explicit.displayHeight], [3, 3])
    explicit.close()
  } finally {
    narrow.close()
    short.close()
  }
})

test('VideoFrame: a crop whose computed display size exceeds the limits throws OperationError', (t) => {
  const source = rgbx(4, 4)
  const max = new VideoFrame(source, {
    visibleRect: { x: 0, y: 0, width: 1, height: 1 },
    displayWidth: 8191,
    displayHeight: 1,
  })
  const over = new VideoFrame(source, {
    visibleRect: { x: 0, y: 0, width: 1, height: 1 },
    displayWidth: 8192,
    displayHeight: 1,
  })
  try {
    const grown = new VideoFrame(max, { visibleRect: { x: 0, y: 0, width: 4, height: 1 } })
    t.deepEqual([grown.displayWidth, grown.displayHeight], [32764, 1])
    grown.close()
    const error = t.throws(() => new VideoFrame(over, { visibleRect: { x: 0, y: 0, width: 4, height: 1 } }), {
      instanceOf: DOMException,
    })
    t.is(error?.name, 'OperationError')
  } finally {
    source.close()
    max.close()
    over.close()
  }
})

test('VideoFrame: wrapping without a visibleRect keeps a display size beyond the limits', (t) => {
  // Node only: this binding accepts coded sizes above Chrome's 32767 limit, and such a frame
  // must stay usable when re-wrapped without a crop.
  const wide = new VideoFrame(new Uint8Array(40000 * 4), {
    format: 'RGBA',
    codedWidth: 40000,
    codedHeight: 1,
    timestamp: 0,
  })
  try {
    for (const init of [{ timestamp: 5 }, { rotation: 90 }]) {
      const wrapped = new VideoFrame(wide, init)
      t.deepEqual([wrapped.displayWidth, wrapped.displayHeight], init.rotation ? [1, 40000] : [40000, 1])
      wrapped.close()
    }
  } finally {
    wide.close()
  }
})

// ============================================================================
// Rotation rounding (spec Parse Rotation: ties towards positive infinity)
// ============================================================================

// [input, Chrome 155 rotation]
const rotations: Array<[number, number]> = [
  [-405, 0],
  [-315, 90],
  [-270, 90],
  [-225, 180],
  [-135, 270],
  [-45, 0],
  [-44.99999999999999, 0],
  [-0, 0],
  [-5e-324, 0],
  [44.99999999999999, 0],
  [45, 90],
  [89.5, 90],
  [135, 180],
  [225, 270],
  [315, 0],
  [405, 90],
  [1350000000045, 90],
  [-1350000000045, 0],
  [1e20, 270],
  [-1e20, 90],
  [Number.MAX_VALUE, 90],
  [-Number.MAX_VALUE, 270],
]

test('VideoFrame: rotation rounds to the nearest quarter turn like Chrome', (t) => {
  for (const [input, expected] of rotations) {
    const buffer = rgbx(4, 2, { rotation: input })
    t.is(buffer.rotation, expected, `buffer rotation ${input}`)
    const swapped = expected === 90 || expected === 270
    t.deepEqual([buffer.displayWidth, buffer.displayHeight], swapped ? [2, 4] : [4, 2])
    buffer.close()

    const plain = rgbx(4, 2)
    const wrapped = new VideoFrame(plain, { rotation: input })
    t.is(wrapped.rotation, expected, `wrapped rotation ${input}`)
    wrapped.close()
    plain.close()

    // A flipped base subtracts the delta (spec Add Rotations)
    const flipped = rgbx(4, 2, { rotation: 90, flip: true })
    const combined = new VideoFrame(flipped, { rotation: input })
    t.is(combined.rotation, (90 - expected + 360) % 360, `flipped base + rotation ${input}`)
    t.true(combined.flip)
    combined.close()
    flipped.close()
  }
})

// ============================================================================
// Close VideoFrame resets rotation and flip
// ============================================================================

test('VideoFrame: close resets rotation and flip but keeps them on a clone', (t) => {
  const frame = rgbx(4, 2, { rotation: 270, flip: true })
  const clone = frame.clone()
  frame.close()
  t.is(frame.rotation, 0)
  t.false(frame.flip)
  t.is(clone.rotation, 270)
  t.true(clone.flip)
  clone.close()
  t.is(clone.rotation, 0)
  t.false(clone.flip)
})
