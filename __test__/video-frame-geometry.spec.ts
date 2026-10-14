/**
 * VideoFrame geometry parity with Chrome
 *
 * Display size validation, rotation rounding, orientation after close, and decoder
 * displayAspectWidth/displayAspectHeight.
 * Expected values were recorded in Chrome 155.
 */

import test from 'ava'

import { VideoDecoder, VideoEncoder, VideoFrame, EncodedVideoChunk } from '../index.js'
import type { VideoDecoderConfig } from '../standard.js'

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

// ============================================================================
// VideoDecoder displayAspectWidth/displayAspectHeight
// ============================================================================

async function encodeVp8(width: number, height: number): Promise<EncodedVideoChunk[]> {
  const chunks: EncodedVideoChunk[] = []
  let failure: Error | null = null
  const encoder = new VideoEncoder({
    output: (chunk) => chunks.push(chunk),
    error: (error) => {
      failure = error
    },
  })
  encoder.configure({ codec: 'vp8', width, height, bitrate: 500_000 })
  const frame = new VideoFrame(new Uint8Array(width * height * 4).fill(96), {
    format: 'RGBA',
    codedWidth: width,
    codedHeight: height,
    timestamp: 0,
  })
  encoder.encode(frame, { keyFrame: true })
  frame.close()
  await encoder.flush()
  encoder.close()
  if (failure) throw failure
  return chunks
}

type Decoded = { sizes: string[]; errors: string[]; state: string }

async function decodeWith(chunks: EncodedVideoChunk[], config: VideoDecoderConfig): Promise<Decoded> {
  const sizes: string[] = []
  const errors: string[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      sizes.push(`${frame.displayWidth}x${frame.displayHeight}`)
      frame.close()
    },
    error: (error) => errors.push(`${error.constructor.name}:${error.name}`),
  })
  decoder.configure(config)
  for (const chunk of chunks) decoder.decode(chunk)
  await decoder.flush().catch(() => {})
  const state = decoder.state
  if (state !== 'closed') decoder.close()
  return { sizes, errors, state }
}

test('VideoDecoder: displayAspectWidth/displayAspectHeight grow the output display size like Chrome', async (t) => {
  const chunks = await encodeVp8(320, 240)
  // [aspect, rotation, Chrome 155 displayWidth x displayHeight]
  const cases: Array<[[number, number] | null, number, string]> = [
    [null, 0, '320x240'],
    [[16, 9], 0, '427x240'],
    [[4, 3], 0, '320x240'],
    [[1, 1], 0, '320x320'],
    [[1, 2], 0, '320x640'],
    [[3, 1], 0, '720x240'],
    [[7, 5], 0, '336x240'],
    [[33, 17], 0, '466x240'],
    [[16, 9], 90, '240x427'],
    [[16, 9], 180, '427x240'],
    // Chromium casts the members to int: these ratios are invalid and change nothing
    [[2 ** 31, 1], 0, '320x240'],
    [[4294967295, 1], 0, '320x240'],
    // ...and 2^32 - 1 is -1 twice, a square ratio
    [[4294967295, 4294967295], 0, '320x320'],
  ]
  for (const [aspect, rotation, expected] of cases) {
    const config: VideoDecoderConfig = { codec: 'vp8', codedWidth: 320, codedHeight: 240, rotation }
    if (aspect) {
      config.displayAspectWidth = aspect[0]
      config.displayAspectHeight = aspect[1]
    }
    const result = await decodeWith(chunks, config)
    t.deepEqual(
      result,
      { sizes: [expected], errors: [], state: 'configured' },
      `aspect ${aspect?.join(':') ?? 'none'} rotation ${rotation}`,
    )
  }
})

test('VideoDecoder: display aspect rounding follows Chrome', async (t) => {
  const chunks = await encodeVp8(100, 30)
  const cases: Array<[number, number, string]> = [
    [7, 2, '105x30'],
    [5, 2, '100x40'],
    [3, 7, '100x233'],
    [1092, 1, '32760x30'],
  ]
  for (const [width, height, expected] of cases) {
    const result = await decodeWith(chunks, { codec: 'vp8', displayAspectWidth: width, displayAspectHeight: height })
    t.deepEqual(result.sizes, [expected], `aspect ${width}:${height}`)
  }
})

test('VideoDecoder: a display aspect beyond the size limits fails the decode with EncodingError', async (t) => {
  const chunks = await encodeVp8(100, 30)
  for (const [width, height] of [
    [1093, 1],
    [1, 1092],
  ]) {
    const result = await decodeWith(chunks, { codec: 'vp8', displayAspectWidth: width, displayAspectHeight: height })
    t.deepEqual(
      result,
      { sizes: [], errors: ['DOMException:EncodingError'], state: 'closed' },
      `aspect ${width}:${height}`,
    )
  }
})

test('VideoDecoder: reconfigure applies the new display aspect', async (t) => {
  const [chunk] = await encodeVp8(320, 240)
  const sizes: string[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      sizes.push(`${frame.displayWidth}x${frame.displayHeight}`)
      frame.close()
    },
    error: (error) => t.fail(error.message),
  })
  for (const aspect of [
    { displayAspectWidth: 16, displayAspectHeight: 9 },
    {},
    { displayAspectWidth: 1, displayAspectHeight: 2 },
  ]) {
    decoder.configure({ codec: 'vp8', codedWidth: 320, codedHeight: 240, ...aspect })
    decoder.decode(chunk)
    await decoder.flush()
  }
  decoder.close()
  t.deepEqual(sizes, ['427x240', '320x240', '320x640'])
})
