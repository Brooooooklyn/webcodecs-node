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
  ["format 'I420\\0' (NUL in the quoted value)", { format: 'I420\0' }],
  ["colorSpace 'foo'", { format: 'RGBA', colorSpace: 'foo' }],
  ["colorSpace 'srgb\\0'", { colorSpace: 'srgb\0' }],
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

// ============================================================================
// layout: sequence<PlaneLayout> (results from Chrome 155)
// ============================================================================

const L = (offset: number, stride: number) => ({ offset, stride })
const I420_LAYOUT = () => [L(0, 4), L(8, 2), L(10, 2)]
// A valid I420 4x2 layout in another order; it needs 36 bytes.
const I420_LAYOUT_SHUFFLED = () => [L(16, 8), L(4, 2), L(32, 4)]
const I420_SHUFFLED_BYTES = [
  0, 0, 0, 0, 9, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8, 0, 0, 0, 0, 11, 12, 0, 0, 0, 0,
  0, 0,
]
function* yieldShuffled() {
  yield* I420_LAYOUT_SHUFFLED()
}

test('layout takes any iterable, not only an Array', async (t) => {
  const frame = makeI420_4x2()
  const cases: [string, () => unknown][] = [
    ['Set', () => new Set(I420_LAYOUT())],
    [
      'generator',
      () =>
        (function* () {
          yield* I420_LAYOUT()
        })(),
    ],
    [
      'arguments',
      () =>
        Reflect.apply(
          function () {
            return arguments
          },
          undefined,
          I420_LAYOUT(),
        ),
    ],
    [
      'function with @@iterator',
      () =>
        Object.assign(() => {}, {
          *[Symbol.iterator]() {
            yield* I420_LAYOUT()
          },
        }),
    ],
    ['Proxy of an Array', () => new Proxy(I420_LAYOUT(), {})],
  ]
  for (const [label, layout] of cases) {
    t.is(allocationSize(frame, { layout: layout() }), 12, label)
    const data = new Uint8Array(12)
    const settled = await settleCopyTo(frame, data, { layout: layout() })
    t.deepEqual(settled.value, I420_LAYOUT(), label)
    t.deepEqual(Array.from(data), Array.from(I420_DATA), label)
  }
  // The iteration order is the plane order.
  t.is(allocationSize(frame, { layout: new Set(I420_LAYOUT_SHUFFLED()) }), 36)
  const data = new Uint8Array(40)
  const settled = await settleCopyTo(frame, data, { layout: new Set(I420_LAYOUT_SHUFFLED()) })
  t.deepEqual(settled.value, I420_LAYOUT_SHUFFLED())
  t.deepEqual(Array.from(data), I420_SHUFFLED_BYTES)
  // An iterator that yields 4 entries, or none, gives the wrong plane count.
  for (const [label, layout] of [
    ['4 entries', () => new Set([...I420_LAYOUT(), L(20, 2)])],
    ['empty Set', () => new Set()],
  ] as const) {
    throwsTypeError(t, `allocationSize ${label}`, () => allocationSize(frame, { layout: layout() }))
    await rejectsTypeError(t, `copyTo ${label}`, frame, new Uint8Array(64), { layout: layout() })
  }
  frame.close()
})

test('an Array layout is read by index, without its @@iterator, like Chrome', async (t) => {
  const frame = makeI420_4x2()
  const check = async (label: string, layout: () => unknown) => {
    t.is(allocationSize(frame, { layout: layout() }), 12, label)
    t.deepEqual((await settleCopyTo(frame, new Uint8Array(64), { layout: layout() })).value, I420_LAYOUT(), label)
  }
  // Chrome 155 does not use an Array's own @@iterator, whatever it is.
  for (const [label, iterator] of [
    ['a generator', yieldShuffled],
    ['undefined', undefined],
    ['5', 5],
  ] as const) {
    await check(`own @@iterator ${label}`, () =>
      Object.defineProperty(I420_LAYOUT(), Symbol.iterator, { value: iterator }),
    )
  }
  const invalidItems = () => Object.defineProperty([5, 5, 5], Symbol.iterator, { value: yieldShuffled })
  throwsTypeError(t, 'allocationSize: the items are read, not the iterator', () =>
    allocationSize(frame, { layout: invalidItems() }),
  )
  await rejectsTypeError(t, 'copyTo: the items are read, not the iterator', frame, new Uint8Array(64), {
    layout: invalidItems(),
  })

  // Nor a patched Array.prototype[@@iterator] or array iterator next(). Patched only while the
  // calls run synchronously.
  const iteratorDescriptor = Object.getOwnPropertyDescriptor(Array.prototype, Symbol.iterator)!
  const arrayIteratorPrototype = Object.getPrototypeOf([][Symbol.iterator]()) as { next: () => unknown }
  const nextDescriptor = Object.getOwnPropertyDescriptor(arrayIteratorPrototype, 'next')!
  let nextCalls = 0
  let size = 0
  let pending!: Promise<Settled>
  const layout = [L(0, 4), L(8, 2), L(10, 2)]
  Object.defineProperty(Array.prototype, Symbol.iterator, { value: yieldShuffled, configurable: true, writable: true })
  Object.defineProperty(arrayIteratorPrototype, 'next', {
    value(this: unknown) {
      nextCalls++
      return (nextDescriptor.value as () => unknown).call(this)
    },
    configurable: true,
    writable: true,
  })
  try {
    size = frame.allocationSize({ layout })
    pending = settleCopyTo(frame, new Uint8Array(64), { layout })
  } finally {
    Object.defineProperty(Array.prototype, Symbol.iterator, iteratorDescriptor)
    Object.defineProperty(arrayIteratorPrototype, 'next', nextDescriptor)
  }
  t.is(size, 12)
  t.deepEqual((await pending).value, I420_LAYOUT())
  t.is(nextCalls, 0)

  // `length` is read again before each entry: growing or shrinking the array while an entry
  // converts changes the plane count.
  for (const [label, change] of [
    ['grown', (array: unknown[]) => array.push(L(20, 2))],
    ['shrunk', (array: unknown[]) => (array.length = 1)],
  ] as const) {
    const layout = () => {
      const array: unknown[] = I420_LAYOUT()
      array[0] = {
        offset: 0,
        get stride() {
          change(array)
          return 4
        },
      }
      return array
    }
    throwsTypeError(t, `allocationSize ${label}`, () => allocationSize(frame, { layout: layout() }))
    await rejectsTypeError(t, `copyTo ${label}`, frame, new Uint8Array(64), { layout: layout() })
  }
  frame.close()
})

test('layout iterator errors: a thrown value passes through, a broken protocol is a TypeError', async (t) => {
  const boom = new Error('boom')
  const iterable = (next: () => unknown) => ({ [Symbol.iterator]: () => ({ next }) })
  const throwing: [string, () => unknown][] = [
    [
      '@@iterator getter',
      () => ({
        get [Symbol.iterator]() {
          throw boom
        },
      }),
    ],
    [
      '@@iterator()',
      () => ({
        [Symbol.iterator]() {
          throw boom
        },
      }),
    ],
    [
      'next getter',
      () => ({
        [Symbol.iterator]: () => ({
          get next() {
            throw boom
          },
        }),
      }),
    ],
    [
      'next()',
      () =>
        iterable(() => {
          throw boom
        }),
    ],
    [
      'done getter',
      () =>
        iterable(() => ({
          value: L(0, 4),
          get done() {
            throw boom
          },
        })),
    ],
    [
      'value getter',
      () =>
        iterable(() => ({
          done: false,
          get value() {
            throw boom
          },
        })),
    ],
  ]
  const open = makeI420_4x2()
  const closed = makeI420_4x2()
  closed.close()
  for (const frame of [open, closed]) {
    const which = frame === open ? 'open' : 'closed'
    for (const [label, layout] of throwing) {
      const settled = await settleCopyTo(frame, new Uint8Array(64), { layout: layout() })
      t.false(settled.sync, `${which} ${label}: copyTo must not throw`)
      t.is(settled.error, boom, `${which} ${label}: copyTo rejects with the thrown value`)
      t.is(
        t.throws(() => allocationSize(frame, { layout: layout() })),
        boom,
        `${which} ${label}: allocationSize throws it`,
      )
    }
  }

  const typeErrors: [string, () => unknown][] = [
    ['{length: 3} without @@iterator', () => ({ length: 3, 0: L(0, 4), 1: L(8, 2), 2: L(10, 2) })],
    ["'abc'", () => 'abc'],
    ["new String('abc')", () => new String('abc')],
    ['Uint32Array', () => new Uint32Array([0, 4])],
    ['Map', () => new Map([[1, 2]])],
    ['@@iterator null', () => ({ [Symbol.iterator]: null })],
    ['@@iterator 5', () => ({ [Symbol.iterator]: 5 })],
    ['@@iterator() returns 5', () => ({ [Symbol.iterator]: () => 5 })],
    ['next 5', () => ({ [Symbol.iterator]: () => ({ next: 5 }) })],
    ['next() returns 5', () => iterable(() => 5)],
    ['done: 1 ends the sequence at once', () => iterable(() => ({ done: 1, value: L(0, 4) }))],
    [
      'an undefined entry',
      () =>
        (function* () {
          yield L(0, 4)
          yield undefined
          yield L(10, 2)
        })(),
    ],
  ]
  for (const [label, layout] of typeErrors) {
    await rejectsTypeError(t, `copyTo ${label}`, open, new Uint8Array(64), { layout: layout() })
    throwsTypeError(t, `allocationSize ${label}`, () => allocationSize(open, { layout: layout() }))
  }
  // A valid iterable still converts before the closed check.
  await rejectsDOMException(t, 'InvalidStateError', 'closed Set', closed, new Uint8Array(64), {
    layout: new Set(I420_LAYOUT()),
  })
  open.close()
})

test('layout iteration order matches Chrome', async (t) => {
  const log: string[] = []
  const logged = <T extends object>(prefix: string, target: T) =>
    new Proxy(target, {
      get(object, key, receiver) {
        log.push(prefix + String(key))
        return Reflect.get(object, key, receiver)
      },
    })
  // An iterable that logs each protocol step.
  const loggingIterable = (items: unknown[]) => ({
    [Symbol.iterator]() {
      log.push('@@iterator()')
      let index = 0
      return {
        next(...args: unknown[]) {
          log.push(`next(${args.length})`)
          const done = index >= items.length
          const value = done ? undefined : items[index++]
          return {
            get done() {
              log.push('done')
              return done
            },
            get value() {
              log.push('value')
              return value
            },
          }
        },
        return() {
          log.push('return()')
          return {}
        },
      }
    },
  })
  const loggedLayout = () => [logged('[0].', L(0, 4)), logged('[1].', L(8, 2)), logged('[2].', L(10, 2))]
  const frame = makeI420_4x2()
  const expectLog = async (
    label: string,
    layout: () => unknown,
    expected: string[],
    outcome: 'resolve' | 'TypeError' | 'boom',
  ) => {
    for (const method of ['allocationSize', 'copyTo'] as const) {
      log.length = 0
      if (method === 'allocationSize') {
        if (outcome === 'resolve') t.is(allocationSize(frame, { layout: layout() }), 12, `${label} ${method}`)
        else t.throws(() => allocationSize(frame, { layout: layout() }), undefined, `${label} ${method}`)
      } else {
        const settled = await settleCopyTo(frame, new Uint8Array(64), { layout: layout() })
        if (outcome === 'resolve') t.true(settled.resolved, `${label} ${method}`)
        else if (outcome === 'TypeError') isTypeError(t, settled.error, `${label} ${method}`)
        else t.is((settled.error as Error)?.message, 'boom', `${label} ${method}`)
      }
      t.deepEqual(log, expected, `${label} ${method}`)
    }
  }
  // Each step reads `value` before `done`, and the entry converts before the next step.
  await expectLog(
    'iterable',
    () => loggingIterable(loggedLayout()),
    [
      '@@iterator()',
      'next(0)',
      'value',
      'done',
      '[0].offset',
      '[0].stride',
      'next(0)',
      'value',
      'done',
      '[1].offset',
      '[1].stride',
      'next(0)',
      'value',
      'done',
      '[2].offset',
      '[2].stride',
      'next(0)',
      'value',
      'done',
    ],
    'resolve',
  )
  // A failed entry stops the iteration without calling return().
  await expectLog(
    'invalid entry',
    () => loggingIterable([5, ...loggedLayout().slice(1)]),
    ['@@iterator()', 'next(0)', 'value', 'done'],
    'TypeError',
  )
  await expectLog(
    'throwing entry',
    () =>
      loggingIterable([
        logged('[0].', L(0, 4)),
        {
          offset: 8,
          get stride() {
            log.push('[1].stride')
            throw new Error('boom')
          },
        },
        L(10, 2),
      ]),
    ['@@iterator()', 'next(0)', 'value', 'done', '[0].offset', '[0].stride', 'next(0)', 'value', 'done', '[1].stride'],
    'boom',
  )
  // A Proxy is not an Array: it goes through @@iterator.
  await expectLog(
    'Proxy of an Array',
    () => logged('', I420_LAYOUT()),
    ['Symbol(Symbol.iterator)', 'length', '0', 'length', '1', 'length', '2', 'length'],
    'resolve',
  )
  // An Array reads each index right before converting that entry.
  await expectLog(
    'Array with index getters',
    () => {
      const items = loggedLayout()
      const array: unknown[] = []
      items.forEach((item, index) =>
        Object.defineProperty(array, index, {
          get() {
            log.push(`get ${index}`)
            return item
          },
          enumerable: true,
        }),
      )
      return array
    },
    ['get 0', '[0].offset', '[0].stride', 'get 1', '[1].offset', '[1].stride', 'get 2', '[2].offset', '[2].stride'],
    'resolve',
  )
  // layout converts between format and rect.
  const options = () => ({
    get format() {
      log.push('format')
      return undefined
    },
    get layout() {
      log.push('layout')
      return loggingIterable(I420_LAYOUT())
    },
    get rect() {
      log.push('rect')
      return undefined
    },
  })
  const expected = ['format', 'layout', '@@iterator()']
  for (let step = 0; step < 4; step++) expected.push('next(0)', 'value', 'done')
  expected.push('rect')
  log.length = 0
  t.is(allocationSize(frame, options()), 12)
  t.deepEqual(log, expected, 'allocationSize member order')
  log.length = 0
  t.true((await settleCopyTo(frame, new Uint8Array(64), options())).resolved)
  t.deepEqual(log, expected, 'copyTo member order')
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

test('copyTo rejects, never throws, when the global DOMException cannot build the error', async (t) => {
  // Chrome 155 builds the DOMException from its own constructor, so it rejects with a real
  // DOMException here. This package reads the global; it rejects with what it can build.
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'DOMException')!
  const boom = new Error('boom')
  const isPlainError = (error: unknown, name: string, message: string, label: string) => {
    t.true(error instanceof Error, `${label}: ${String(error)} should be an Error`)
    t.false(error instanceof DOMException, label)
    t.is((error as Error)?.name, name, label)
    t.is((error as Error)?.message, message, label)
  }
  const replacements: [
    string,
    PropertyDescriptor,
    (error: unknown, name: string, message: string, label: string) => void,
  ][] = [
    ['undefined', { value: undefined, writable: true, configurable: true }, isPlainError],
    [
      'a throwing getter',
      {
        get() {
          throw boom
        },
        configurable: true,
      },
      (error, _name, _message, label) => t.is(error, boom, label),
    ],
    [
      'a non-constructor',
      { value: () => {}, writable: true, configurable: true },
      (error, _name, _message, label) => t.true(error instanceof TypeError, `${label}: ${String(error)}`),
    ],
    ['a plain object', { value: {}, writable: true, configurable: true }, isPlainError],
  ]
  const open = makeI420_4x2()
  const closed = makeI420_4x2()
  closed.close()
  const calls: [string, VideoFrame, unknown, string, string][] = [
    ['closed frame', closed, undefined, 'InvalidStateError', 'VideoFrame is closed'],
    [
      "colorSpace 'rec2020'",
      open,
      { colorSpace: 'rec2020' },
      'NotSupportedError',
      'This pixel conversion to this color space is not supported.',
    ],
  ]
  for (const [what, replacement, check] of replacements) {
    for (const [call, frame, options, name, message] of calls) {
      const label = `DOMException is ${what}, ${call}`
      let pending!: Promise<Settled>
      Object.defineProperty(globalThis, 'DOMException', replacement)
      try {
        // copyTo runs synchronously, before settleCopyTo first awaits.
        pending = settleCopyTo(frame, new Uint8Array(64), options)
      } finally {
        Object.defineProperty(globalThis, 'DOMException', descriptor)
      }
      const settled = await pending
      t.false(settled.sync, `${label}: copyTo must not throw synchronously`)
      t.false(settled.resolved, `${label}: copyTo must reject`)
      check(settled.error, name, message, label)
    }
  }
  t.is(globalThis.DOMException, descriptor.value)
  await rejectsDOMException(t, 'InvalidStateError', 'restored', closed, new Uint8Array(64))
  open.close()
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
