/**
 * VideoFrame geometry parity with Chrome
 *
 * Display size validation, rotation rounding, orientation after close, decoder
 * displayAspectWidth/displayAspectHeight, and the DOMRectReadOnly interface.
 * Expected values were recorded in Chrome 155.
 */

import test from 'ava'

import { DOMRectReadOnly, VideoDecoder, VideoEncoder, VideoFrame, EncodedVideoChunk } from '../index.js'
import type { VideoDecoderConfigOutput } from '../index.js'
import type { VideoDecoderConfig, VideoEncoderConfig, VideoFrameBufferInit } from '../standard.js'

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

test('VideoFrame: displayHeight converts before displayWidth like Chrome', (t) => {
  // WebIDL converts dictionary members in lexicographic order and stops at the first failure.
  // Chrome 155 reads displayHeight first in both constructors: an invalid displayHeight throws
  // before the displayWidth getter runs, and a throwing displayHeight getter wins over an invalid
  // displayWidth.
  // [invalid member, member whose getter throws, expected error, getter calls]
  const cases: Array<[FrameInit, string, { instanceOf: typeof Error; message?: string }, number]> = [
    [{ displayHeight: -1 }, 'displayWidth', { instanceOf: TypeError }, 0],
    [{ displayWidth: -1 }, 'displayHeight', { instanceOf: Error, message: 'displayHeight getter' }, 1],
  ]
  for (const kind of ['buffer', 'wrap']) {
    for (const [invalid, getter, expected, expectedCalls] of cases) {
      const label = `${kind}: ${JSON.stringify(invalid)}, ${getter} getter throws`
      let calls = 0
      const base = kind === 'buffer' ? { format: 'RGBX', codedWidth: 4, codedHeight: 2, timestamp: 0 } : {}
      const init = Object.defineProperty({ ...base, ...invalid }, getter, {
        enumerable: true,
        get() {
          calls++
          throw new Error(`${getter} getter`)
        },
      }) as VideoFrameBufferInit
      const source = kind === 'buffer' ? new Uint8Array(4 * 2 * 4) : rgbx(4, 2)
      try {
        t.throws(() => new VideoFrame(source, init).close(), expected, label)
      } finally {
        if (source instanceof VideoFrame) source.close()
      }
      t.is(calls, expectedCalls, `${label}: getter calls`)
    }
  }
})

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

type Encoded = { chunks: EncodedVideoChunk[]; decoderConfig: VideoDecoderConfigOutput | undefined }

/** Encode one key frame of `config.width` x `config.height` */
async function encodeKeyFrame(config: VideoEncoderConfig): Promise<Encoded> {
  const chunks: EncodedVideoChunk[] = []
  let decoderConfig: VideoDecoderConfigOutput | undefined
  let failure: Error | null = null
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      decoderConfig ??= metadata?.decoderConfig
    },
    error: (error) => {
      failure = error
    },
  })
  encoder.configure(config)
  const { width, height } = config
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
  return { chunks, decoderConfig }
}

async function encodeVp8(width: number, height: number): Promise<EncodedVideoChunk[]> {
  return (await encodeKeyFrame({ codec: 'vp8', width, height, bitrate: 500_000 })).chunks
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

test('VideoDecoder: invalid displayAspectWidth/displayAspectHeight are a TypeError like Chrome', async (t) => {
  // Chrome 155 takes both as [EnforceRange] unsigned long and requires them together
  const cases: Array<[string, Partial<VideoDecoderConfig>]> = [
    ['lone width', { displayAspectWidth: 16 }],
    ['lone height', { displayAspectHeight: 9 }],
    ['width, undefined height', { displayAspectWidth: 16, displayAspectHeight: undefined }],
    ['lone height 2^32 + 1', { displayAspectHeight: 2 ** 32 + 1 }],
    ['-1:-1', { displayAspectWidth: -1, displayAspectHeight: -1 }],
    ['-1:9', { displayAspectWidth: -1, displayAspectHeight: 9 }],
    ['16:-1', { displayAspectWidth: 16, displayAspectHeight: -1 }],
    ['-1.5:9', { displayAspectWidth: -1.5, displayAspectHeight: 9 }],
    // -0.5 and 0.5 truncate to 0
    ['-0.5:9', { displayAspectWidth: -0.5, displayAspectHeight: 9 }],
    ['0.5:9', { displayAspectWidth: 0.5, displayAspectHeight: 9 }],
    ['2^32:9', { displayAspectWidth: 2 ** 32, displayAspectHeight: 9 }],
    ['2^32 + 1:9', { displayAspectWidth: 2 ** 32 + 1, displayAspectHeight: 9 }],
    ['-(2^32 - 1):9', { displayAspectWidth: -(2 ** 32 - 1), displayAspectHeight: 9 }],
    ['1e20:9', { displayAspectWidth: 1e20, displayAspectHeight: 9 }],
    ['NaN:9', { displayAspectWidth: NaN, displayAspectHeight: 9 }],
    ['16:NaN', { displayAspectWidth: 16, displayAspectHeight: NaN }],
    ['Infinity:9', { displayAspectWidth: Infinity, displayAspectHeight: 9 }],
    ['-Infinity:9', { displayAspectWidth: -Infinity, displayAspectHeight: 9 }],
  ]
  for (const [name, aspect] of cases) {
    const config: VideoDecoderConfig = { codec: 'vp8', codedWidth: 320, codedHeight: 240, ...aspect }
    const decoder = new VideoDecoder({ output: (frame) => frame.close(), error: (error) => t.fail(error.message) })
    t.throws(() => decoder.configure(config), { instanceOf: TypeError }, `configure ${name}`)
    t.is(decoder.state, 'unconfigured', `configure ${name}`)
    decoder.close()
    let support: Promise<unknown> | undefined
    t.notThrows(() => {
      support = VideoDecoder.isConfigSupported(config)
    }, `isConfigSupported ${name} rejects instead of throwing`)
    await t.throwsAsync(support!, { instanceOf: TypeError }, `isConfigSupported ${name}`)
  }
})

test('VideoDecoder: an invalid display aspect stops the config read like Chrome', async (t) => {
  // WebIDL stops a dictionary conversion at the first failure. Chrome 155 reads displayAspectHeight
  // before displayAspectWidth and runs no later getter, so a later exception cannot replace the
  // TypeError, and isConfigSupported() still rejects instead of throwing.
  // [name, invalid aspect members, member read later whose getter throws]
  const cases: Array<[string, Partial<VideoDecoderConfig>, string]> = [
    ['H -1', { displayAspectHeight: -1 }, 'displayAspectWidth'],
    ['W -1', { displayAspectWidth: -1 }, 'flip'],
    ['W -1', { displayAspectWidth: -1 }, 'hardwareAcceleration'],
    ['W -1', { displayAspectWidth: -1 }, 'optimizeForLatency'],
    ['W -1', { displayAspectWidth: -1 }, 'rotation'],
    ['-1:-1', { displayAspectWidth: -1, displayAspectHeight: -1 }, 'rotation'],
  ]
  for (const [name, aspect, later] of cases) {
    const label = `${name}, ${later} getter throws`
    let calls = 0
    const config = () =>
      Object.defineProperty({ codec: 'vp8', ...aspect }, later, {
        enumerable: true,
        get() {
          calls++
          throw new Error(`${later} getter`)
        },
      }) as VideoDecoderConfig
    const decoder = new VideoDecoder({ output: (frame) => frame.close(), error: (error) => t.fail(error.message) })
    t.throws(() => decoder.configure(config()), { instanceOf: TypeError }, `configure ${label}`)
    decoder.close()
    let support: Promise<unknown> | undefined
    t.notThrows(() => {
      support = VideoDecoder.isConfigSupported(config())
    }, `isConfigSupported ${label} rejects instead of throwing`)
    await t.throwsAsync(support!, { instanceOf: TypeError }, `isConfigSupported ${label}`)
    t.is(calls, 0, `${label}: the getter does not run`)
  }
})

test('VideoDecoder: colorSpace is read before the display aspect like Chrome', async (t) => {
  // Chrome 155 reads colorSpace before displayAspectHeight/displayAspectWidth (WebIDL name order),
  // so a colorSpace error comes first, not the display aspect TypeError.
  const icsError = async (config: VideoDecoderConfig) => {
    try {
      await VideoDecoder.isConfigSupported(config)
    } catch (error) {
      return error as Error
    }
    return undefined
  }
  const aspects: Array<[string, Partial<VideoDecoderConfig>]> = [
    ['H -1', { displayAspectWidth: 16, displayAspectHeight: -1 }],
    ['W NaN', { displayAspectWidth: NaN, displayAspectHeight: 9 }],
    ['lone H -1', { displayAspectHeight: -1 }],
  ]
  for (const [name, aspect] of aspects) {
    let calls = 0
    const withGetter = () =>
      Object.defineProperty({ codec: 'vp8', ...aspect }, 'colorSpace', {
        enumerable: true,
        get() {
          calls++
          throw new Error('colorSpace getter')
        },
      }) as VideoDecoderConfig
    const badColorSpace = { codec: 'vp8', ...aspect, colorSpace: { primaries: 'bogus' } } as unknown as VideoDecoderConfig
    const decoder = new VideoDecoder({ output: (frame) => frame.close(), error: (error) => t.fail(error.message) })
    t.throws(() => decoder.configure(withGetter()), { message: 'colorSpace getter' }, `configure ${name}, getter`)
    t.throws(() => decoder.configure(badColorSpace), { message: /primaries/i }, `configure ${name}, bad primaries`)
    decoder.close()
    // isConfigSupported() reports the same error. It throws it instead of rejecting with it: that
    // is a separate difference from Chrome for every throwing getter.
    t.is((await icsError(withGetter()))?.message, 'colorSpace getter', `isConfigSupported ${name}, getter`)
    t.regex(String((await icsError(badColorSpace))?.message), /primaries/i, `isConfigSupported ${name}, bad primaries`)
    t.is(calls, 2, `${name}: the colorSpace getter runs once per call`)
  }
})

test('VideoDecoder: fractional displayAspectWidth/displayAspectHeight truncate like Chrome', async (t) => {
  const chunks = await encodeVp8(320, 240)
  // [width, height, Chrome 155 support.config members, output display size]
  const cases: Array<[number, number, [number, number], string]> = [
    [16.7, 9.2, [16, 9], '427x240'],
    [16.999, 9.999, [16, 9], '427x240'],
    [2 ** 32 - 0.5, 9, [4294967295, 9], '320x240'],
  ]
  for (const [width, height, members, size] of cases) {
    const config: VideoDecoderConfig = {
      codec: 'vp8',
      codedWidth: 320,
      codedHeight: 240,
      displayAspectWidth: width,
      displayAspectHeight: height,
    }
    const support = await VideoDecoder.isConfigSupported(config)
    t.deepEqual(
      [support.config.displayAspectWidth, support.config.displayAspectHeight],
      members,
      `isConfigSupported ${width}:${height}`,
    )
    t.deepEqual(
      await decodeWith(chunks, config),
      { sizes: [size], errors: [], state: 'configured' },
      `configure ${width}:${height}`,
    )
  }
})

test('VideoDecoder: reconfigure with an invalid display aspect throws and keeps the old config', async (t) => {
  const [chunk] = await encodeVp8(320, 240)
  const sizes: string[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      sizes.push(`${frame.displayWidth}x${frame.displayHeight}`)
      frame.close()
    },
    error: (error) => t.fail(error.message),
  })
  decoder.configure({ codec: 'vp8', displayAspectWidth: 16, displayAspectHeight: 9 })
  t.throws(() => decoder.configure({ codec: 'vp8', displayAspectWidth: 1 }), { instanceOf: TypeError })
  t.throws(() => decoder.configure({ codec: 'vp8', displayAspectWidth: -1, displayAspectHeight: -1 }), {
    instanceOf: TypeError,
  })
  t.is(decoder.state, 'configured')
  decoder.decode(chunk)
  await decoder.flush()
  decoder.close()
  t.deepEqual(sizes, ['427x240'])
})

test('VideoDecoder: config rotation converts like Chrome (unsigned long) before it rounds', async (t) => {
  const [chunk] = await encodeVp8(32, 16)
  // [config rotation, Chrome 155 output frame rotation]. Chrome applies ToUint32 first, so
  // -90 is 4294967206 (180 after rounding) and 2^32 + 90 is 90.
  const cases: Array<[number, number]> = [
    [90, 90],
    [-90, 180],
    [-315, 270],
    [-45, 180],
    [-135, 90],
    [-225, 0],
    [-405, 180],
    [44.9, 0],
    [1e20, 0],
    [-1e20, 180],
    [2 ** 32 + 90, 90],
    [2 ** 32 * 3 + 180, 180],
    [-(2 ** 32), 0],
    [2 ** 53, 0],
    [NaN, 0],
    [Infinity, 0],
    [-Infinity, 0],
  ]
  const rotations: number[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      rotations.push(frame.rotation)
      frame.close()
    },
    error: (error) => t.fail(error.message),
  })
  for (const [rotation] of cases) {
    decoder.configure({ codec: 'vp8', rotation })
    decoder.decode(chunk)
    await decoder.flush()
  }
  decoder.close()
  t.deepEqual(
    rotations,
    cases.map(([, expected]) => expected),
  )
})

test('VideoDecoder.isConfigSupported returns the rotation converted like Chrome (unsigned long)', async (t) => {
  const [chunk] = await encodeVp8(32, 16)
  // [config rotation, Chrome 155 support.config.rotation]
  const cases: Array<[number, number]> = [
    [0, 0],
    [-0, 0],
    [90, 90],
    [270, 270],
    [45, 45],
    [450, 450],
    [90.5, 90],
    [44.9, 44],
    [-0.5, 0],
    [-90, 4294967206],
    [1e20, 1661992960],
    [-1e20, 2632974336],
    [2 ** 32 + 90, 90],
    [2 ** 53, 0],
    [NaN, 0],
    [Infinity, 0],
    [-Infinity, 0],
  ]
  const echoed: number[] = []
  const configs: VideoDecoderConfig[] = []
  for (const [rotation] of cases) {
    const support = await VideoDecoder.isConfigSupported({ codec: 'vp8', rotation })
    t.true(support.supported)
    echoed.push(support.config.rotation as number)
    configs.push(support.config)
  }
  t.deepEqual(
    echoed,
    cases.map(([, expected]) => expected),
  )
  t.true(Object.is(echoed[1], 0), '-0 is +0')

  // The returned config decodes like the original one
  const rotations: Array<[number, number]> = []
  let original = 0
  const decoder = new VideoDecoder({
    output: (frame) => {
      rotations.push([original, frame.rotation])
      frame.close()
    },
    error: (error) => t.fail(error.message),
  })
  for (const [index, [rotation]] of cases.entries()) {
    for (const config of [{ codec: 'vp8', rotation }, configs[index]]) {
      original = rotation
      decoder.configure(config)
      decoder.decode(chunk)
      await decoder.flush()
    }
  }
  decoder.close()
  t.is(rotations.length, cases.length * 2)
  for (let i = 0; i < rotations.length; i += 2) {
    t.deepEqual(rotations[i + 1], rotations[i], `rotation ${rotations[i][0]}`)
  }
})

// ============================================================================
// VideoEncoder displayWidth/displayHeight
// ============================================================================

test('VideoEncoder: a lone displayWidth or displayHeight is a TypeError like Chrome', async (t) => {
  const message = 'Invalid display size; both height and width must be set together.'
  // Chrome 155 checks the pair after codec, size and bitrate, and before a display size of 0,
  // the framerate and codec support
  const cases: Array<[string, Partial<VideoEncoderConfig>]> = [
    ['lone width', { displayWidth: 640 }],
    ['lone height', { displayHeight: 360 }],
    ['width, undefined height', { displayWidth: 640, displayHeight: undefined }],
    ['lone width 0', { displayWidth: 0 }],
    ['lone height 0', { displayHeight: 0 }],
    ['lone width, framerate 0', { displayWidth: 640, framerate: 0 }],
    ['lone width, unknown codec', { displayWidth: 640, codec: 'bogus' }],
    ['lone width, unknown scalabilityMode', { displayWidth: 640, scalabilityMode: 'L9T9' }],
  ]
  for (const [name, display] of cases) {
    const config: VideoEncoderConfig = { codec: 'vp8', width: 320, height: 240, ...display }
    const encoder = new VideoEncoder({ output: () => {}, error: (error) => t.fail(error.message) })
    t.throws(() => encoder.configure(config), { instanceOf: TypeError, message }, `configure ${name}`)
    t.is(encoder.state, 'unconfigured', `configure ${name}`)
    encoder.close()
    let support: Promise<unknown> | undefined
    t.notThrows(() => {
      support = VideoEncoder.isConfigSupported(config)
    }, `isConfigSupported ${name} rejects instead of throwing`)
    await t.throwsAsync(support!, { instanceOf: TypeError, message }, `isConfigSupported ${name}`)
  }

  // A bitrate of 0 is reported before the lone display size, as in Chrome
  const config: VideoEncoderConfig = { codec: 'vp8', width: 320, height: 240, displayWidth: 640, bitrate: 0 }
  const encoder = new VideoEncoder({ output: () => {}, error: (error) => t.fail(error.message) })
  t.throws(() => encoder.configure(config), { instanceOf: TypeError, message: /bitrate/ })
  encoder.close()
  await t.throwsAsync(VideoEncoder.isConfigSupported(config), { instanceOf: TypeError, message: /bitrate/ })
})

test('VideoEncoder: displayWidth/displayHeight reach the decoded frame through decoderConfig like Chrome', async (t) => {
  // [display size, Chrome 155 decoderConfig displayAspectWidth/displayAspectHeight, decoded display size]
  const cases: Array<[[number, number] | null, [number, number] | null, string]> = [
    [null, null, '320x240'],
    [[640, 360], [640, 360], '427x240'],
    [[320, 240], [320, 240], '320x240'],
    [[100, 500], [100, 500], '320x1600'],
    [[640.7, 360.2], [640, 360], '427x240'],
  ]
  for (const [display, aspect, size] of cases) {
    const name = display?.join('x') ?? 'none'
    const config: VideoEncoderConfig = { codec: 'vp8', width: 320, height: 240, bitrate: 500_000 }
    if (display) {
      config.displayWidth = display[0]
      config.displayHeight = display[1]
    }
    const { chunks, decoderConfig } = await encodeKeyFrame(config)
    if (!decoderConfig) {
      t.fail(`no decoderConfig for ${name}`)
      continue
    }
    t.deepEqual(
      ['displayAspectWidth', 'displayAspectHeight'].filter((key) => key in decoderConfig),
      aspect ? ['displayAspectWidth', 'displayAspectHeight'] : [],
      `decoderConfig members ${name}`,
    )
    t.deepEqual(
      [decoderConfig.displayAspectWidth, decoderConfig.displayAspectHeight],
      aspect ?? [undefined, undefined],
      `decoderConfig ${name}`,
    )
    t.deepEqual(
      await decodeWith(chunks, decoderConfig),
      { sizes: [size], errors: [], state: 'configured' },
      `decode ${name}`,
    )
  }
})

// ============================================================================
// DOMRectReadOnly interface shape
// ============================================================================

test('DOMRectReadOnly: Object.prototype.toString reports DOMRectReadOnly', (t) => {
  t.is(Object.prototype.toString.call(new DOMRectReadOnly(1, 2, 3, 4)), '[object DOMRectReadOnly]')
  t.is(Object.prototype.toString.call(DOMRectReadOnly.prototype), '[object DOMRectReadOnly]')
  const descriptor = Object.getOwnPropertyDescriptor(DOMRectReadOnly.prototype, Symbol.toStringTag)
  t.deepEqual(descriptor, { value: 'DOMRectReadOnly', writable: false, enumerable: false, configurable: true })
  const frame = rgbx(4, 2)
  t.is(Object.prototype.toString.call(frame.visibleRect), '[object DOMRectReadOnly]')
  frame.close()
})

test('DOMRectReadOnly: members enumerate in IDL order', (t) => {
  const keys: string[] = []
  for (const key in new DOMRectReadOnly(1, 2, 3, 4)) keys.push(key)
  t.deepEqual(keys, ['x', 'y', 'width', 'height', 'top', 'right', 'bottom', 'left', 'toJSON'])
  t.deepEqual(Object.getOwnPropertyNames(DOMRectReadOnly.prototype), [
    'x',
    'y',
    'width',
    'height',
    'top',
    'right',
    'bottom',
    'left',
    'toJSON',
    'constructor',
  ])
  // Moving the members keeps them working
  const rect = new DOMRectReadOnly(1, 2, -3, 4)
  t.deepEqual(rect.toJSON(), { x: 1, y: 2, width: -3, height: 4, top: 2, right: 1, bottom: 6, left: -2 })
  t.is(DOMRectReadOnly.prototype.constructor, DOMRectReadOnly)
})

const edges = (rect: DOMRectReadOnly) => [rect.x, rect.y, rect.width, rect.height]

test('DOMRectReadOnly: constructor converts arguments with ToNumber', (t) => {
  const args = (...values: unknown[]) => values as [number, number, number, number]
  t.deepEqual(edges(new DOMRectReadOnly(...args('1', '2', '-3', ' 4 '))), [1, 2, -3, 4])
  t.deepEqual(edges(new DOMRectReadOnly(...args(true, false, '', '0x10'))), [1, 0, 0, 16])
  t.deepEqual(edges(new DOMRectReadOnly(...args(null, undefined, [5], { valueOf: () => 7 }))), [0, 0, 5, 7])
  t.deepEqual(edges(new DOMRectReadOnly(...args('abc', {}))), [NaN, NaN, 0, 0])
  t.deepEqual(edges(new DOMRectReadOnly(NaN, Infinity, -0, -Infinity)), [NaN, Infinity, -0, -Infinity])

  // Arguments convert in order; an exception stops the conversion
  const log: string[] = []
  const tracked = (name: string, value: number) => ({
    valueOf() {
      log.push(name)
      if (name === 'y') throw new RangeError('boom')
      return value
    },
  })
  t.throws(() => new DOMRectReadOnly(...args(tracked('x', 1), tracked('y', 2), tracked('width', 3))), {
    instanceOf: RangeError,
    message: 'boom',
  })
  t.deepEqual(log, ['x', 'y'])
  t.throws(() => new DOMRectReadOnly(...args(Symbol('x'))), { instanceOf: TypeError })
  t.throws(() => new DOMRectReadOnly(...args(1n)), { instanceOf: TypeError })
})

test('DOMRectReadOnly.fromRect converts a DOMRectInit like Chrome', (t) => {
  t.is(typeof DOMRectReadOnly.fromRect, 'function')
  t.is(DOMRectReadOnly.fromRect.length, 0)
  for (const empty of [undefined, null, {}, { top: 9, left: 9 }]) {
    const rect = DOMRectReadOnly.fromRect(empty as never)
    t.true(rect instanceof DOMRectReadOnly)
    t.deepEqual(edges(rect), [0, 0, 0, 0])
  }
  t.deepEqual(edges(DOMRectReadOnly.fromRect()), [0, 0, 0, 0])
  const rect = DOMRectReadOnly.fromRect({ x: 1, y: 2, width: 3, height: 4 })
  t.is(JSON.stringify(rect), '{"x":1,"y":2,"width":3,"height":4,"top":2,"right":4,"bottom":6,"left":1}')
  t.deepEqual(edges(DOMRectReadOnly.fromRect({ x: '1', width: '-3', y: null } as never)), [1, 0, -3, 0])
  t.deepEqual(edges(DOMRectReadOnly.fromRect({ x: 'abc', y: { valueOf: () => 42 } } as never)), [NaN, 42, 0, 0])
  // Another rect reads through its prototype getters
  t.deepEqual(edges(DOMRectReadOnly.fromRect(new DOMRectReadOnly(5, 6, 7, 8))), [5, 6, 7, 8])

  // Members are read in lexicographic order
  const log: string[] = []
  const init = {}
  for (const key of ['y', 'x', 'width', 'height']) {
    Object.defineProperty(init, key, {
      get() {
        log.push(key)
        return 1
      },
      enumerable: true,
    })
  }
  DOMRectReadOnly.fromRect(init)
  t.deepEqual(log, ['height', 'width', 'x', 'y'])

  for (const value of [5, 'x', true]) {
    t.throws(() => DOMRectReadOnly.fromRect(value as never), { instanceOf: TypeError })
  }
  t.throws(() => DOMRectReadOnly.fromRect({ x: Symbol('x') } as never), { instanceOf: TypeError })
  t.throws(() => DOMRectReadOnly.fromRect({ x: 1n } as never), { instanceOf: TypeError })
})
