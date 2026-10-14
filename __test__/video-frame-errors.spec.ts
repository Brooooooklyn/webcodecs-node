/**
 * VideoFrame error types, argument conversion and promise rejection.
 *
 * The expected results were checked in headless Chrome 155 (155.0.8059.79):
 * - copyTo() never throws: argument, option and frame errors reject the returned promise.
 * - allocationSize(), metadata() and the constructor throw real TypeError / DOMException objects.
 * - Options are read once each, in lexicographic order (WebIDL dictionaries), before the frame is
 *   checked, so a getter exception wins over InvalidStateError.
 */

import test, { type ExecutionContext } from 'ava'

import { VideoFrame, type VideoFrameCopyToOptions } from '../index.js'

type Settled = { sync: boolean; resolved: boolean; value?: unknown; error?: unknown }

/** Calls `copyTo` with any arguments and reports whether it threw, resolved or rejected. */
async function settleCopyTo(frame: VideoFrame, ...args: unknown[]): Promise<Settled> {
  let promise: Promise<unknown>
  try {
    promise = (frame.copyTo as (...a: unknown[]) => Promise<unknown>).apply(frame, args)
  } catch (error) {
    return { sync: true, resolved: false, error }
  }
  try {
    return { sync: false, resolved: true, value: await promise }
  } catch (error) {
    return { sync: false, resolved: false, error }
  }
}

function allocationSize(frame: VideoFrame, ...args: unknown[]): number {
  return (frame.allocationSize as (...a: unknown[]) => number).apply(frame, args)
}

function isTypeError(t: ExecutionContext, error: unknown, label: string) {
  t.true(error instanceof TypeError, `${label}: ${String(error)} should be a TypeError`)
  t.false(error instanceof DOMException, `${label}: not a DOMException`)
  t.is((error as Error)?.name, 'TypeError', label)
  t.false(/^\w+Error: /.test((error as Error)?.message ?? ''), `${label}: no name prefix in the message`)
}

function isDOMException(t: ExecutionContext, error: unknown, name: string, label: string) {
  t.true(error instanceof DOMException, `${label}: ${String(error)} should be a DOMException`)
  t.is((error as DOMException)?.name, name, label)
  t.false(/^\w+Error: /.test((error as Error)?.message ?? ''), `${label}: no name prefix in the message`)
}

async function rejectsTypeError(t: ExecutionContext, label: string, frame: VideoFrame, ...args: unknown[]) {
  const settled = await settleCopyTo(frame, ...args)
  t.false(settled.sync, `${label}: copyTo must not throw synchronously`)
  t.false(settled.resolved, `${label}: copyTo must reject`)
  isTypeError(t, settled.error, label)
}

async function rejectsDOMException(
  t: ExecutionContext,
  name: string,
  label: string,
  frame: VideoFrame,
  ...args: unknown[]
) {
  const settled = await settleCopyTo(frame, ...args)
  t.false(settled.sync, `${label}: copyTo must not throw synchronously`)
  t.false(settled.resolved, `${label}: copyTo must reject`)
  isDOMException(t, settled.error, name, label)
}

function throwsTypeError(t: ExecutionContext, label: string, fn: () => unknown) {
  const error = t.throws(fn, undefined, label)
  isTypeError(t, error, label)
}

function throwsDOMException(t: ExecutionContext, name: string, label: string, fn: () => unknown) {
  const error = t.throws(fn, undefined, label)
  isDOMException(t, error, name, label)
}

// 4x2 I420: the frame WPT's videoFrame-copyTo.any.js uses
const I420_DATA = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])
const makeI420_4x2 = () => new VideoFrame(I420_DATA, { format: 'I420', codedWidth: 4, codedHeight: 2, timestamp: 0 })
const makeRGBX_4x4 = () =>
  new VideoFrame(new Uint8Array(64), { format: 'RGBX', codedWidth: 4, codedHeight: 4, timestamp: 0 })

// ============================================================================
// WPT videoFrame-copyTo.any.js
// ============================================================================

test('WPT copyTo: Test closed frame.', async (t) => {
  const frame = makeI420_4x2()
  frame.close()
  throwsDOMException(t, 'InvalidStateError', 'allocationSize()', () => frame.allocationSize())
  await rejectsDOMException(t, 'InvalidStateError', 'copyTo()', frame, new Uint8Array(12))
})

test('WPT copyTo: Test undersized buffer.', async (t) => {
  const frame = makeI420_4x2()
  await rejectsTypeError(t, 'copyTo', frame, new Uint8Array(11))
  frame.close()
})

for (const [name, options] of [
  ['Test incorrect plane count.', { layout: [{ offset: 0, stride: 4 }] }],
  [
    'Test invalid stride.',
    {
      layout: [
        { offset: 0, stride: 1 },
        { offset: 8, stride: 2 },
        { offset: 10, stride: 2 },
      ],
    },
  ],
  [
    'Test address overflow.',
    {
      layout: [
        { offset: 0, stride: 4 },
        { offset: 8, stride: 2 },
        { offset: 2 ** 32 - 2, stride: 2 },
      ],
    },
  ],
  ['Test empty rect.', { rect: { x: 0, y: 0, width: 4, height: 0 } }],
  ['Test invalid rect.', { rect: { x: 0, y: 0, width: 4, height: 4 } }],
] as const) {
  test(`WPT copyTo: ${name}`, async (t) => {
    const frame = makeI420_4x2()
    throwsTypeError(t, 'allocationSize', () => allocationSize(frame, options))
    await rejectsTypeError(t, 'copyTo', frame, new Uint8Array(12), options)
    frame.close()
  })
}

test('WPT copyTo: Test codedRect.', async (t) => {
  const frame = makeI420_4x2()
  // A DOMRectReadOnly is a valid DOMRectInit
  const options = { rect: frame.codedRect! }
  t.is(frame.allocationSize(options), 12)
  const data = new Uint8Array(12)
  const layout = await frame.copyTo(data, options)
  t.deepEqual(
    layout.map(({ offset, stride }) => ({ offset, stride })),
    [
      { offset: 0, stride: 4 },
      { offset: 8, stride: 2 },
      { offset: 10, stride: 2 },
    ],
  )
  t.deepEqual(Array.from(data), Array.from(I420_DATA))
  frame.close()
})

// WPT videoFrame-copyTo-rgb.any.js builds these frames from a Uint32Array; this uses a Uint8Array.
for (const format of ['I420', 'I420P10', 'I420P12', 'I420A', 'I422', 'I422A', 'I444', 'I444A', 'NV12'] as const) {
  test(`WPT copyTo-rgb: Unsupported format ${format}`, async (t) => {
    const frame = makeRGBX_4x4()
    const options = { format }
    throwsDOMException(t, 'NotSupportedError', 'allocationSize', () => frame.allocationSize(options))
    await rejectsDOMException(t, 'NotSupportedError', 'copyTo', frame, new Uint8Array(64), options)
    frame.close()
  })
}

// ============================================================================
// Argument conversion (C2)
// ============================================================================

test('copyTo rejects with TypeError instead of throwing when the destination is not a Uint8Array', async (t) => {
  const frame = makeI420_4x2()
  for (const [label, args] of [
    ['no arguments', []],
    ['5', [5]],
    ['null', [null]],
    ['plain object', [{}]],
    ['array', [[1, 2, 3]]],
  ] as const) {
    await rejectsTypeError(t, label, frame, ...args)
  }
  // The destination converts first: the options are not read.
  let reads = 0
  const options = {
    get format() {
      reads++
      return 'RGBA'
    },
  }
  await rejectsTypeError(t, 'bad destination with options', frame, 5, options)
  t.is(reads, 0, 'options getters must not run')
  frame.close()
})

const BAD_OPTIONS: [string, unknown][] = [
  ['options 5', 5],
  ["options 'str'", 'str'],
  ['options true', true],
  ['options Symbol', Symbol('options')],
  ['options 1n', 1n],
  ["format 'FOO'", { format: 'FOO' }],
  ["format 'rgba'", { format: 'rgba' }],
  ['format null', { format: null }],
  ['format 5', { format: 5 }],
  ['format Symbol', { format: Symbol('format') }],
  ["format 'I420AP12' (spec only)", { format: 'I420AP12' }],
  ["colorSpace 'foo'", { format: 'RGBA', colorSpace: 'foo' }],
  ['colorSpace null', { format: 'RGBA', colorSpace: null }],
  ['rect 5', { rect: 5 }],
  ['rect null (all members 0)', { rect: null }],
  ['rect {} (all members 0)', { rect: {} }],
  ['rect []', { rect: [] }],
  ['rect.x Symbol', { rect: { x: Symbol('x'), y: 0, width: 2, height: 2 } }],
  ["layout 'abc'", { layout: 'abc' }],
  ['layout null', { layout: null }],
  ['layout {length: 3}', { layout: { length: 3 } }],
  ['layout entry 5', { layout: [5, { offset: 8, stride: 2 }, { offset: 10, stride: 2 }] }],
  ['layout entry null', { layout: [null, { offset: 8, stride: 2 }, { offset: 10, stride: 2 }] }],
  ['layout stride missing', { layout: [{ offset: 0 }, { offset: 8, stride: 2 }, { offset: 10, stride: 2 }] }],
  [
    'layout offset NaN',
    {
      layout: [
        { offset: Number.NaN, stride: 4 },
        { offset: 8, stride: 2 },
        { offset: 10, stride: 2 },
      ],
    },
  ],
  [
    'layout offset -1',
    {
      layout: [
        { offset: -1, stride: 4 },
        { offset: 8, stride: 2 },
        { offset: 10, stride: 2 },
      ],
    },
  ],
  [
    'layout offset 2^32',
    {
      layout: [
        { offset: 2 ** 32, stride: 4 },
        { offset: 8, stride: 2 },
        { offset: 10, stride: 2 },
      ],
    },
  ],
]

test('copyTo rejects bad options with TypeError; allocationSize throws TypeError', async (t) => {
  const frame = makeI420_4x2()
  for (const [label, options] of BAD_OPTIONS) {
    await rejectsTypeError(t, `copyTo ${label}`, frame, new Uint8Array(64), options)
    throwsTypeError(t, `allocationSize ${label}`, () => allocationSize(frame, options))
  }
  frame.close()
})

test('an invalid option is a TypeError even on a closed frame', async (t) => {
  const frame = makeI420_4x2()
  frame.close()
  await rejectsTypeError(t, 'copyTo', frame, new Uint8Array(64), { format: 'FOO' })
  throwsTypeError(t, 'allocationSize', () => allocationSize(frame, { format: 'FOO' }))
})

test('valid options still convert like WebIDL', async (t) => {
  const frame = makeI420_4x2()
  for (const [label, options] of [
    ['undefined', undefined],
    ['null', null],
    ['{}', {}],
    ['[]', []],
    ['function', () => {}],
  ] as const) {
    const settled = await settleCopyTo(frame, new Uint8Array(12), options)
    t.true(settled.resolved, `copyTo options ${label}`)
    t.is(allocationSize(frame, options), 12)
  }
  // Numbers and enum strings are converted with ToNumber / ToString.
  const converted = {
    format: { toString: () => 'RGBA' },
    rect: { x: '0', y: '0', width: '2', height: '2' },
  }
  t.is(allocationSize(frame, converted), 16)
  t.true((await settleCopyTo(frame, new Uint8Array(16), converted)).resolved)
  const layout = {
    layout: [
      { offset: '0', stride: '4' },
      { offset: 8.9, stride: 2 },
      { offset: 10, stride: 2 },
    ],
  }
  t.is(allocationSize(frame, layout), 12)
  const settled = await settleCopyTo(frame, new Uint8Array(12), layout)
  t.deepEqual(settled.value, [
    { offset: 0, stride: 4 },
    { offset: 8, stride: 2 },
    { offset: 10, stride: 2 },
  ])
  frame.close()
})

test('an exception thrown while reading the options rejects copyTo with that exception', async (t) => {
  const boom = new Error('boom')
  const cases: [string, unknown][] = [
    [
      'format getter',
      {
        get format() {
          throw boom
        },
      },
    ],
    [
      'format toString',
      {
        format: {
          toString() {
            throw boom
          },
        },
      },
    ],
    [
      'rect.x valueOf',
      {
        rect: {
          x: {
            valueOf() {
              throw boom
            },
          },
          y: 0,
          width: 2,
          height: 2,
        },
      },
    ],
    [
      'layout stride getter',
      {
        layout: [
          {
            offset: 0,
            get stride() {
              throw boom
            },
          },
          { offset: 8, stride: 2 },
          { offset: 10, stride: 2 },
        ],
      },
    ],
  ]
  const open = makeI420_4x2()
  const closed = makeI420_4x2()
  closed.close()
  for (const frame of [open, closed]) {
    const which = frame === open ? 'open' : 'closed'
    for (const [label, options] of cases) {
      const settled = await settleCopyTo(frame, new Uint8Array(64), options)
      t.false(settled.sync, `${which} ${label}: copyTo must not throw`)
      t.is(settled.error, boom, `${which} ${label}: copyTo rejects with the thrown value`)
      t.is(
        t.throws(() => allocationSize(frame, options)),
        boom,
        `${which} ${label}: allocationSize throws it`,
      )
    }
  }
  // A thrown non-Error value is passed through as is.
  const settled = await settleCopyTo(open, new Uint8Array(64), {
    get rect() {
      throw 42 // eslint-disable-line no-throw-literal
    },
  })
  t.is(settled.error, 42)
  open.close()
})

test('options are read once each, in lexicographic order', async (t) => {
  const log: string[] = []
  const logged = <T extends object>(prefix: string, target: T) =>
    new Proxy(target, {
      get(object, key, receiver) {
        if (typeof key === 'string') log.push(prefix + key)
        return Reflect.get(object, key, receiver)
      },
    })
  const options = () =>
    logged('', {
      colorSpace: 'srgb',
      format: 'RGBA',
      layout: [logged('layout[0].', { offset: 0, stride: 16 })],
      rect: logged('rect.', { x: 0, y: 0, width: 4, height: 2 }),
    }) as VideoFrameCopyToOptions
  const expected = [
    'colorSpace',
    'format',
    'layout',
    'layout[0].offset',
    'layout[0].stride',
    'rect',
    'rect.height',
    'rect.width',
    'rect.x',
    'rect.y',
  ]
  const frame = makeI420_4x2()
  t.is(frame.allocationSize(options()), 32)
  t.deepEqual(log, expected, 'allocationSize')
  log.length = 0
  await frame.copyTo(new Uint8Array(32), options())
  t.deepEqual(log, expected, 'copyTo')
  frame.close()
})

test('rect members default to 0 in copyTo, not to the visible rect', async (t) => {
  const source = new VideoFrame(
    new Uint8Array(64).map((_, i) => i),
    { format: 'RGBA', codedWidth: 4, codedHeight: 4, timestamp: 0 },
  )
  // Cropping through the VideoFrame constructor keeps the 4x4 coded size, as in Chrome.
  const frame = new VideoFrame(source, { visibleRect: { x: 2, y: 2, width: 2, height: 2 } })
  const data = new Uint8Array(16)
  t.deepEqual(await frame.copyTo(data, { rect: { width: 2, height: 2 } }), [{ offset: 0, stride: 8 }])
  // Chrome 155: the rect starts at (0, 0)
  t.deepEqual(Array.from(data), [0, 1, 2, 3, 4, 5, 6, 7, 16, 17, 18, 19, 20, 21, 22, 23])
  frame.close()
  source.close()
})

test("colorSpace values Chrome's enum has but copyTo cannot target are NotSupportedError", async (t) => {
  const frame = makeI420_4x2()
  for (const colorSpace of [
    'rec2020',
    'srgb-linear',
    'display-p3-linear',
    'rec2100-hlg',
    'rec2100-pq',
    'rec2100-linear',
  ]) {
    for (const options of [{ format: 'RGBA', colorSpace }, { colorSpace }]) {
      const label = JSON.stringify(options)
      await rejectsDOMException(t, 'NotSupportedError', `copyTo ${label}`, frame, new Uint8Array(64), options)
      throwsDOMException(t, 'NotSupportedError', `allocationSize ${label}`, () => allocationSize(frame, options))
    }
    // Checked before the destination size and the rect, like Chrome.
    await rejectsDOMException(t, 'NotSupportedError', `${colorSpace} small destination`, frame, new Uint8Array(1), {
      format: 'RGBA',
      colorSpace,
    })
    await rejectsDOMException(t, 'NotSupportedError', `${colorSpace} bad rect`, frame, new Uint8Array(64), {
      colorSpace,
      rect: { x: 0, y: 0, width: 8, height: 8 },
    })
  }
  for (const colorSpace of ['srgb', 'display-p3']) {
    t.true((await settleCopyTo(frame, new Uint8Array(32), { format: 'RGBA', colorSpace })).resolved, colorSpace)
  }
  // The closed check comes first.
  frame.close()
  await rejectsDOMException(t, 'InvalidStateError', 'closed rec2020', frame, new Uint8Array(64), {
    format: 'RGBA',
    colorSpace: 'rec2020',
  })
  throwsDOMException(t, 'InvalidStateError', 'closed rec2020 allocationSize', () =>
    allocationSize(frame, { colorSpace: 'rec2020' }),
  )
})

// ============================================================================
// Explicit non-RGB format (C3)
// ============================================================================

test("copyTo rejects an explicit non-RGB format, even the frame's own, with NotSupportedError", async (t) => {
  const sizes = { w: 4, h: 4 }
  for (const format of [
    'I420',
    'I420P10',
    'I420P12',
    'I420A',
    'I420AP10',
    'I422',
    'I422P10',
    'I422P12',
    'I422A',
    'I422AP10',
    'I444',
    'I444P10',
    'I444P12',
    'I444A',
    'I444AP10',
    'NV12',
  ] as const) {
    const frame = new VideoFrame(new Uint8Array(1024), {
      format,
      codedWidth: sizes.w,
      codedHeight: sizes.h,
      timestamp: 0,
    })
    const size = frame.allocationSize()
    // allocationSize takes the frame's own format (Chrome 155 too)
    t.is(frame.allocationSize({ format }), size, `${format} allocationSize`)
    await rejectsDOMException(t, 'NotSupportedError', `${format} copyTo`, frame, new Uint8Array(size), { format })
    // The destination is checked first.
    await rejectsTypeError(t, `${format} small destination`, frame, new Uint8Array(1), { format })
    // Without a format the copy works.
    t.true((await settleCopyTo(frame, new Uint8Array(size))).resolved, `${format} without format`)
    frame.close()
  }
  for (const format of ['RGBA', 'RGBX', 'BGRA', 'BGRX'] as const) {
    const frame = new VideoFrame(new Uint8Array(64), { format, codedWidth: 4, codedHeight: 4, timestamp: 0 })
    t.true((await settleCopyTo(frame, new Uint8Array(64), { format })).resolved, format)
    frame.close()
  }
})

// ============================================================================
// Synchronous paths (#121 C7, #123 E4)
// ============================================================================

test('allocationSize rect and layout errors are TypeError objects', (t) => {
  const frame = makeI420_4x2()
  for (const [label, options] of [
    ['rect out of bounds', { rect: { x: 2, y: 0, width: 4, height: 2 } }],
    ['negative rect', { rect: { x: -2, y: 0, width: 2, height: 2 } }],
    ['rect NaN', { rect: { x: Number.NaN, y: 0, width: 2, height: 2 } }],
    ['unaligned rect', { rect: { x: 1, y: 0, width: 2, height: 2 } }],
    ['layout count', { layout: [{ offset: 0, stride: 4 }] }],
    [
      'stride overflow',
      {
        layout: [
          { offset: 0, stride: 2 ** 32 - 1 },
          { offset: 8, stride: 2 },
          { offset: 10, stride: 2 },
        ],
      },
    ],
  ] as const) {
    throwsTypeError(t, label, () => allocationSize(frame, options))
  }
  frame.close()
})

test('VideoFrame constructor rect and layout errors are TypeError objects', (t) => {
  const init = { format: 'I420', codedWidth: 4, codedHeight: 2, timestamp: 0 } as const
  for (const [label, extra] of [
    ['visibleRect out of bounds', { visibleRect: { x: 0, y: 0, width: 4, height: 4 } }],
    ['visibleRect unaligned', { visibleRect: { x: 1, y: 0, width: 2, height: 2 } }],
    ['visibleRect empty', { visibleRect: { x: 0, y: 0, width: 0, height: 2 } }],
    ['visibleRect negative', { visibleRect: { x: -2, y: 0, width: 2, height: 2 } }],
    ['visibleRect NaN', { visibleRect: { x: Number.NaN, y: 0, width: 2, height: 2 } }],
    ['layout count', { layout: [{ offset: 0, stride: 4 }] }],
    [
      'layout stride',
      {
        layout: [
          { offset: 0, stride: 3 },
          { offset: 8, stride: 2 },
          { offset: 10, stride: 2 },
        ],
      },
    ],
    [
      'layout past the data',
      {
        layout: [
          { offset: 0, stride: 8 },
          { offset: 8, stride: 2 },
          { offset: 10, stride: 2 },
        ],
      },
    ],
  ] as const) {
    throwsTypeError(t, label, () => new VideoFrame(new Uint8Array(12), { ...init, ...extra }))
  }
  const source = makeI420_4x2()
  for (const [label, visibleRect] of [
    ['clone visibleRect out of bounds', { x: 0, y: 0, width: 8, height: 2 }],
    ['clone visibleRect unaligned', { x: 1, y: 0, width: 2, height: 2 }],
  ] as const) {
    throwsTypeError(t, label, () => new VideoFrame(source, { timestamp: 0, visibleRect }))
  }
  source.close()
})

test('metadata() on a closed frame throws InvalidStateError', (t) => {
  const frame = makeI420_4x2()
  t.deepEqual(frame.metadata(), {})
  frame.close()
  throwsDOMException(t, 'InvalidStateError', 'metadata()', () => frame.metadata())
})
