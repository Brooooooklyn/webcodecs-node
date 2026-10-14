/**
 * VideoFrame Canvas API Conformance Tests
 *
 * Tests VideoFrame creation from @napi-rs/canvas Canvas objects.
 *
 * Skipped on aarch64-windows where @napi-rs/canvas is not available.
 */

import { execFileSync } from 'node:child_process'

import test from 'ava'

import {
  VideoFrame,
  VideoEncoder,
  EncodedVideoChunk,
  createVideoFrameFromRGBA16Float,
  copyVideoFrameRGBA16Float,
  registerVideoFrameCloseObserver,
} from '../index.js'

// Skip all canvas tests on aarch64-windows (no @napi-rs/canvas support)
const isAarch64Windows = process.platform === 'win32' && process.arch === 'arm64'

const runTest = isAarch64Windows ? test.skip : test

// Dynamic import to avoid loading canvas on unsupported platforms
let createCanvas: typeof import('@napi-rs/canvas').createCanvas
if (!isAarch64Windows) {
  const canvas = await import('@napi-rs/canvas')
  createCanvas = canvas.createCanvas
}

// ============================================================================
// Constructor Tests - Canvas Source
// ============================================================================

runTest('VideoFrame: constructor from Canvas with valid timestamp', (t) => {
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  // Fill with red
  ctx.fillStyle = '#FF0000'
  ctx.fillRect(0, 0, 320, 240)

  const frame = new VideoFrame(canvas, { timestamp: 0 })

  t.is(frame.format, 'RGBA')
  t.is(frame.codedWidth, 320)
  t.is(frame.codedHeight, 240)
  t.is(frame.displayWidth, 320)
  t.is(frame.displayHeight, 240)
  t.is(frame.timestamp, 0)

  frame.close()
})

runTest('VideoFrame: Canvas requires timestamp in init', (t) => {
  const canvas = createCanvas(100, 100)

  // Missing init entirely
  t.throws(
    () => {
      new VideoFrame(canvas, undefined as never)
    },
    { message: /timestamp is required/ },
  )

  // Init without timestamp
  t.throws(
    () => {
      new VideoFrame(canvas, {} as never)
    },
    { message: /timestamp is required/ },
  )
})

// Note: @napi-rs/canvas clamps zero dimensions to default values (e.g., 0 -> 350),
// so we use mock canvas objects to test our validation code.

runTest('VideoFrame: Canvas with zero width throws', (t) => {
  // Mock canvas object with zero width
  const mockCanvas = {
    width: 0,
    height: 100,
    data: () => Buffer.alloc(0),
  }

  t.throws(
    () => {
      new VideoFrame(mockCanvas as never, { timestamp: 0 })
    },
    { message: /Canvas width must be greater than 0/ },
  )
})

runTest('VideoFrame: Canvas with zero height throws', (t) => {
  // Mock canvas object with zero height
  const mockCanvas = {
    width: 100,
    height: 0,
    data: () => Buffer.alloc(0),
  }

  t.throws(
    () => {
      new VideoFrame(mockCanvas as never, { timestamp: 0 })
    },
    { message: /Canvas height must be greater than 0/ },
  )
})

runTest('VideoFrame: Canvas with zero width and height throws', (t) => {
  // Mock canvas object with both zero dimensions
  const mockCanvas = {
    width: 0,
    height: 0,
    data: () => Buffer.alloc(0),
  }

  // Should throw for width first (checked before height)
  t.throws(
    () => {
      new VideoFrame(mockCanvas as never, { timestamp: 0 })
    },
    { message: /Canvas width must be greater than 0/ },
  )
})

runTest('VideoFrame: Canvas with all init options', (t) => {
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  // Draw something
  ctx.fillStyle = '#0000FF'
  ctx.fillRect(0, 0, 320, 240)

  const frame = new VideoFrame(canvas, {
    timestamp: 1000,
    duration: 33333,
    displayWidth: 640,
    displayHeight: 480,
  })

  t.is(frame.format, 'RGBA')
  t.is(frame.codedWidth, 320)
  t.is(frame.codedHeight, 240)
  t.is(frame.timestamp, 1000)
  t.is(frame.duration, 33333)
  t.is(frame.displayWidth, 640)
  t.is(frame.displayHeight, 480)

  frame.close()
})

runTest('VideoFrame: Canvas with visibleRect', (t) => {
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  ctx.fillStyle = '#00FF00'
  ctx.fillRect(0, 0, 320, 240)

  const frame = new VideoFrame(canvas, {
    timestamp: 0,
    visibleRect: { x: 10, y: 10, width: 300, height: 220 },
  })

  t.is(frame.codedWidth, 320)
  t.is(frame.codedHeight, 240)

  const visibleRect = frame.visibleRect
  if (!visibleRect) throw new Error('live canvas frame has no visible rectangle')
  t.is(visibleRect.x, 10)
  t.is(visibleRect.y, 10)
  t.is(visibleRect.width, 300)
  t.is(visibleRect.height, 220)

  frame.close()
})

runTest('VideoFrame: Canvas pixel data is RGBA', async (t) => {
  const canvas = createCanvas(2, 2)
  const ctx = canvas.getContext('2d')

  // Fill with red (RGBA: 255, 0, 0, 255)
  ctx.fillStyle = '#FF0000'
  ctx.fillRect(0, 0, 2, 2)

  const frame = new VideoFrame(canvas, { timestamp: 0 })

  // Copy the data to verify format
  const buffer = new Uint8Array(2 * 2 * 4) // RGBA = 4 bytes per pixel
  await frame.copyTo(buffer)

  // Check first pixel is red (RGBA)
  t.is(buffer[0], 255, 'R should be 255')
  t.is(buffer[1], 0, 'G should be 0')
  t.is(buffer[2], 0, 'B should be 0')
  t.is(buffer[3], 255, 'A should be 255')

  frame.close()
})

runTest('VideoFrame: Canvas with drawn graphics', async (t) => {
  const canvas = createCanvas(100, 100)
  const ctx = canvas.getContext('2d')

  // Fill with blue background
  ctx.fillStyle = '#0000FF'
  ctx.fillRect(0, 0, 100, 100)

  // Draw a white line
  ctx.strokeStyle = '#FFFFFF'
  ctx.lineWidth = 2
  ctx.beginPath()
  ctx.moveTo(0, 0)
  ctx.lineTo(100, 100)
  ctx.stroke()

  const frame = new VideoFrame(canvas, { timestamp: 5000 })

  t.is(frame.format, 'RGBA')
  t.is(frame.codedWidth, 100)
  t.is(frame.codedHeight, 100)
  t.is(frame.timestamp, 5000)

  frame.close()
})

runTest('VideoFrame: Canvas various sizes', (t) => {
  const sizes = [
    [1, 1],
    [16, 16],
    [640, 480],
    [1920, 1080],
  ] as const

  for (const [width, height] of sizes) {
    const canvas = createCanvas(width, height)

    const frame = new VideoFrame(canvas, { timestamp: 0 })

    t.is(frame.codedWidth, width, `Width should be ${width}`)
    t.is(frame.codedHeight, height, `Height should be ${height}`)
    t.is(frame.format, 'RGBA')

    frame.close()
  }
})

runTest('VideoFrame: Canvas sRGB color space default', (t) => {
  const canvas = createCanvas(100, 100)

  const frame = new VideoFrame(canvas, { timestamp: 0 })

  // Per W3C spec, RGBA defaults to sRGB color space
  const colorSpace = frame.colorSpace
  t.is(colorSpace.primaries, 'bt709')
  t.is(colorSpace.transfer, 'iec61966-2-1') // sRGB
  t.is(colorSpace.fullRange, true)

  frame.close()
})

runTest('VideoFrame: Canvas clone preserves data', async (t) => {
  const canvas = createCanvas(10, 10)
  const ctx = canvas.getContext('2d')

  // Fill with green
  ctx.fillStyle = '#00FF00'
  ctx.fillRect(0, 0, 10, 10)

  const frame1 = new VideoFrame(canvas, { timestamp: 100 })
  const frame2 = new VideoFrame(frame1, { timestamp: 200 })

  t.is(frame1.timestamp, 100)
  t.is(frame2.timestamp, 200)
  t.is(frame1.format, frame2.format)
  t.is(frame1.codedWidth, frame2.codedWidth)
  t.is(frame1.codedHeight, frame2.codedHeight)

  // Verify pixel data is the same
  const buffer1 = new Uint8Array(10 * 10 * 4)
  const buffer2 = new Uint8Array(10 * 10 * 4)
  await frame1.copyTo(buffer1)
  await frame2.copyTo(buffer2)

  t.deepEqual(buffer1, buffer2, 'Cloned frame should have same pixel data')

  frame1.close()
  frame2.close()
})

// ============================================================================
// Integration Tests
// ============================================================================

runTest('VideoFrame: Canvas can be encoded (smoke test)', async (t) => {
  // This is a basic smoke test to verify Canvas frames work with encoders
  // Full encoder tests are in other test files
  const canvas = createCanvas(320, 240)
  const ctx = canvas.getContext('2d')

  ctx.fillStyle = '#FF0000'
  ctx.fillRect(0, 0, 320, 240)

  const chunks: InstanceType<typeof EncodedVideoChunk>[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'vp8',
    width: 320,
    height: 240,
    bitrate: 1_000_000,
  })

  // Create frame from canvas and encode
  const frame = new VideoFrame(canvas, { timestamp: 0 })
  encoder.encode(frame)
  frame.close()

  await encoder.flush()
  encoder.close()

  t.true(chunks.length > 0, 'Should have encoded at least one chunk')
  t.is(chunks[0]!.type, 'key', 'First chunk should be keyframe')
})

test('VideoFrame: native float resource owns half bits across clone and close', (t) => {
  const expected = new Uint16Array([0x3000, 0x3802, 0x3b00, 0x3800, 0x3d00, 0x3400, 0xb000, 0x3800])
  const backing = new Uint16Array(expected.length + 4)
  backing.set(expected, 2)
  const frame = createVideoFrameFromRGBA16Float(backing.subarray(2, 10), {
    codedWidth: 2,
    codedHeight: 1,
    timestamp: 123,
    duration: 45,
    colorSpace: { primaries: 'smpte432', transfer: 'iec61966-2-1', matrix: 'rgb', fullRange: true },
  })
  const clone = frame.clone()
  const closed: string[] = []
  registerVideoFrameCloseObserver(frame, () => {
    closed.push('source')
  })
  registerVideoFrameCloseObserver(clone, () => {
    closed.push('clone')
  })
  try {
    backing.fill(0)
    t.is(frame.format, null)
    t.deepEqual(copyVideoFrameRGBA16Float(frame), expected)
    copyVideoFrameRGBA16Float(frame).fill(0)
    frame.close()
    frame.close()
    t.deepEqual(closed, ['source'])
    t.deepEqual(copyVideoFrameRGBA16Float(clone), expected)
    t.is(clone.timestamp, 123)
    t.is(clone.duration, 45)
    t.is(clone.colorSpace.primaries, 'smpte432')
    t.throws(() => frame.clone(), { instanceOf: DOMException, name: 'InvalidStateError' })
    t.throws(() => copyVideoFrameRGBA16Float(frame), { instanceOf: DOMException, name: 'InvalidStateError' })
  } finally {
    clone.close()
    frame.close()
  }
  t.deepEqual(closed, ['source', 'clone'])
})

test('VideoFrame: opaque float resource rejects public copy formats without losing metadata', async (t) => {
  const frame = createVideoFrameFromRGBA16Float(new Uint16Array([0x3000, 0x3802, 0x3b00, 0x3800]), {
    codedWidth: 1,
    codedHeight: 1,
    timestamp: 123,
    duration: 45,
  })
  try {
    t.throws(() => frame.allocationSize(), { instanceOf: DOMException, name: 'NotSupportedError' })
    t.throws(() => frame.allocationSize({ format: 'RGBA' }), { instanceOf: DOMException, name: 'NotSupportedError' })
    await t.throwsAsync(frame.copyTo(new Uint8Array(8)), { instanceOf: DOMException, name: 'NotSupportedError' })
    await t.throwsAsync(frame.copyTo(new Uint8Array(8), { format: 'RGBA' }), {
      instanceOf: DOMException,
      name: 'NotSupportedError',
    })
    frame.close()
    t.is(frame.timestamp, 123)
    t.is(frame.duration, 45)
    t.deepEqual(frame.colorSpace.toJSON(), { primaries: null, transfer: null, matrix: null, fullRange: null })
    t.is(frame.codedWidth, 0)
    t.is(frame.codedRect, null)
    t.is(frame.visibleRect, null)
    t.is(frame.format, null)
  } finally {
    frame.close()
  }
})

test('VideoFrame: float image alpha discard and geometry preserve the source resource', (t) => {
  const source = createVideoFrameFromRGBA16Float(new Uint16Array(2 * 2 * 4).fill(0x3800), {
    codedWidth: 2,
    codedHeight: 2,
    timestamp: 1,
  })
  const clone = new VideoFrame(source, {
    timestamp: 2,
    alpha: 'discard',
    rotation: 90,
    displayWidth: 4,
    displayHeight: 6,
    visibleRect: { x: 1, y: 0, width: 1, height: 2 },
  })
  try {
    t.is(clone.format, null)
    t.is(clone.displayWidth, 6)
    t.is(clone.displayHeight, 4)
    const rect = clone.visibleRect
    if (!rect) throw new Error('live float frame has no visible rectangle')
    t.is(rect.x, 1)
    t.is(rect.width, 1)
    t.is(copyVideoFrameRGBA16Float(clone)[3], 0x3c00)
    t.is(copyVideoFrameRGBA16Float(source)[3], 0x3800)
    clone.close()
    t.is(copyVideoFrameRGBA16Float(source)[3], 0x3800)
  } finally {
    clone.close()
    source.close()
  }
})

test('VideoFrame: float image rejects inconsistent dimensions and keeps standard buffer formats', (t) => {
  t.throws(() => createVideoFrameFromRGBA16Float(new Uint16Array(3), { codedWidth: 1, codedHeight: 1, timestamp: 0 }), {
    instanceOf: TypeError,
  })
  t.throws(() => createVideoFrameFromRGBA16Float(new Uint16Array(4), { codedWidth: 0, codedHeight: 1, timestamp: 0 }), {
    instanceOf: TypeError,
  })
  t.throws(
    () =>
      createVideoFrameFromRGBA16Float(new Uint16Array(4), {
        codedWidth: 1,
        codedHeight: 1,
        timestamp: 0,
        displayWidth: 0,
      }),
    { instanceOf: TypeError },
  )
  const frame = new VideoFrame(new Uint8Array([11, 22, 33, 44]), {
    format: 'RGBA',
    codedWidth: 1,
    codedHeight: 1,
    timestamp: 0,
  })
  try {
    t.is(frame.format, 'RGBA')
    t.is(frame.allocationSize(), 4)
    t.throws(() => copyVideoFrameRGBA16Float(frame), { instanceOf: DOMException, name: 'NotSupportedError' })
  } finally {
    frame.close()
  }
})

test('VideoFrame: opaque float encode rejects asynchronously and closes without output', async (t) => {
  const frame = createVideoFrameFromRGBA16Float(new Uint16Array(2 * 2 * 4).fill(0x3800), {
    codedWidth: 2,
    codedHeight: 2,
    timestamp: 123,
  })
  const chunks: EncodedVideoChunk[] = []
  let reportError: (error: Error) => void = () => {
    throw new Error('error promise was not initialized')
  }
  const errorResult = new Promise<Error>((resolve) => {
    reportError = resolve
  })
  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (error) => {
      reportError(error)
    },
  })
  try {
    encoder.configure({ codec: 'vp8', width: 2, height: 2, bitrate: 1_000_000 })
    encoder.encode(frame)
    t.is(encoder.encodeQueueSize, 1)
    frame.close()
    await t.throwsAsync(encoder.flush(), { instanceOf: DOMException, name: 'OperationError' })
    const error = await errorResult
    t.true(error instanceof DOMException)
    t.is(error.name, 'OperationError')
    t.is(encoder.state, 'closed')
    t.is(encoder.encodeQueueSize, 0)
    t.deepEqual(chunks, [])
  } finally {
    frame.close()
    if (encoder.state !== 'closed') encoder.close()
  }
})

// ============================================================================
// RGBA16Float module exports: native lifetimes and argument checks
// ============================================================================

// The child's stdout carries the test result, so it must not inherit WEBCODECS_LOG
// (CI sets it, and the log output goes to stdout).
function childEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.WEBCODECS_LOG
  return env
}

// Runs `script` in a child Node process, so a native crash or abort fails the
// test instead of killing the ava worker. The script loads ./index.js itself
// and writes its result with fs.writeSync before exiting.
function runChild(script: string, env: NodeJS.ProcessEnv = childEnv()): string {
  return execFileSync(process.execPath, ['--expose-gc', '-e', script], {
    cwd: process.cwd(),
    env,
    timeout: 30_000,
    encoding: 'utf8',
  })
}

test('VideoEncoder: opaque float error comes first, then one dequeue per queued encode, and the encoder is collected', (t) => {
  const script = `
    const fs = require('node:fs');
    const { VideoEncoder, createVideoFrameFromRGBA16Float } = require('./index.js');
    const tick = () => new Promise((resolve) => setImmediate(resolve));
    const log = [];
    let weak;
    let settle;
    const settled = new Promise((resolve) => { settle = resolve; });
    const record = (entry) => { log.push(entry); if (log.length === 4) settle(); };
    // Only reports a failure: a correct run settles as soon as the four callbacks ran.
    const guard = setTimeout(settle, 10000);
    (() => {
      const encoder = new VideoEncoder({
        output: () => record('output'),
        // Reads the encoder through the WeakRef so the callback does not keep it alive.
        error: (error) => record('error:' + error.name + ':' + weak.deref().encodeQueueSize),
      });
      weak = new WeakRef(encoder);
      encoder.ondequeue = () => record('dequeue');
      encoder.configure({ codec: 'vp8', width: 16, height: 16, bitrate: 100000 });
      const frames = [0, 1, 2].map((timestamp) =>
        createVideoFrameFromRGBA16Float(new Uint16Array(16 * 16 * 4).fill(0x3c00), {
          codedWidth: 16, codedHeight: 16, timestamp,
        }),
      );
      // encode() hands work to the worker from a microtask, so all three are queued first.
      for (const frame of frames) { encoder.encode(frame); frame.close(); }
    })();
    settled.then(async () => {
      clearTimeout(guard);
      let collected = false;
      for (let i = 0; i < 100 && !collected; i++) {
        await tick();
        globalThis.gc();
        await tick();
        collected = weak.deref() === undefined;
      }
      fs.writeSync(1, JSON.stringify({ log, collected }) + '\\n');
      process.exit(0);
    });
  `
  const result = JSON.parse(runChild(script))
  t.deepEqual(result.log, ['error:OperationError:0', 'dequeue', 'dequeue', 'dequeue'])
  t.true(result.collected, 'the closed encoder must be collectable')
})

test('VideoEncoder: opaque float error releases the hardware encoder slot while the encoder is alive', (t) => {
  // The slot counter is only visible in the debug log. The marker is written to
  // the same fd as the native log lines, so lines before it were logged before
  // the error callback ran, while the script still references the encoder.
  const script = `
    const fs = require('node:fs');
    const { VideoEncoder, createVideoFrameFromRGBA16Float } = require('./index.js');
    // Keeps the process alive until the error callback runs; only fires on failure.
    setTimeout(() => process.exit(0), 10000);
    const encoder = new VideoEncoder({
      output: () => {},
      error: (error) => {
        fs.writeSync(1, 'MARKER:' + error.name + '\\n');
        process.exit(0);
      },
    });
    const frame = createVideoFrameFromRGBA16Float(new Uint16Array(640 * 480 * 4), {
      codedWidth: 640, codedHeight: 480, timestamp: 0,
    });
    try {
      encoder.configure({ codec: 'avc1.42001f', width: 640, height: 480, hardwareAcceleration: 'prefer-hardware' });
      encoder.encode(frame);
    } catch (error) {
      fs.writeSync(1, 'THREW:' + error.name + '\\n');
      process.exit(0);
    }
    frame.close();
  `
  const output = runChild(script, { ...process.env, WEBCODECS_LOG: 'webcodecs=debug' })
  const [beforeError, marker = ''] = output.split('MARKER:')
  const slots = [...beforeError.matchAll(/hardware encoder slot \((\d+)\/\d+\)/g)].map((match) => Number(match[1]))
  if (!marker.startsWith('OperationError') || !beforeError.includes('Acquired hardware encoder slot')) {
    t.log('No hardware encoder slot was taken on this machine; nothing to check')
    t.pass()
    return
  }
  t.is(slots.at(-1), 0, 'the slot must be released before the error callback runs')
})

test('createVideoFrameFromRGBA16Float: data detached by an init getter throws TypeError', (t) => {
  const data = new Uint16Array(4 * 4 * 4).fill(0x3c00)
  const init = {
    codedWidth: 4,
    codedHeight: 4,
    get timestamp() {
      structuredClone(data.buffer, { transfer: [data.buffer] })
      return 0
    },
  }
  t.throws(() => createVideoFrameFromRGBA16Float(data, init), { instanceOf: TypeError })
  t.is(data.length, 0)
})

test('createVideoFrameFromRGBA16Float: validation errors are TypeErrors and rotation/flip are accepted', (t) => {
  type Init = Parameters<typeof createVideoFrameFromRGBA16Float>[1]
  const data = () => new Uint16Array(2 * 2 * 4)
  const invalid: Array<Record<string, unknown>> = [
    { codedWidth: 2, codedHeight: 2 },
    { codedHeight: 2, timestamp: 0 },
    { codedWidth: 2, codedHeight: 2, timestamp: 0, format: 'RGBA' },
    { codedWidth: 2, codedHeight: 2, timestamp: 0, alpha: 'bogus' },
    { codedWidth: 2, codedHeight: 2, timestamp: 0, visibleRect: { x: 1, y: 0, width: 2, height: 1 } },
  ]
  for (const init of invalid) {
    t.throws(() => createVideoFrameFromRGBA16Float(data(), init as unknown as Init), { instanceOf: TypeError })
  }
  const frame = createVideoFrameFromRGBA16Float(data(), {
    codedWidth: 2,
    codedHeight: 2,
    timestamp: 0,
    rotation: 90,
    flip: true,
  })
  try {
    t.is(frame.rotation, 90)
    t.true(frame.flip)
  } finally {
    frame.close()
  }
})

test('RGBA16Float exports: another class instance throws TypeError instead of crashing', (t) => {
  const script = `
    const fs = require('node:fs');
    const m = require('./index.js');
    const others = {
      colorSpace: () => new m.VideoColorSpace({ primaries: 'bt709' }),
      rect: () => new m.DOMRectReadOnly(1, 2, 3, 4),
      chunk: () => new m.EncodedVideoChunk({ type: 'key', timestamp: 0, data: new Uint8Array(16) }),
    };
    const calls = {
      copy: (value) => m.copyVideoFrameRGBA16Float(value),
      observe: (value) => m.registerVideoFrameCloseObserver(value, () => {}),
    };
    const results = [];
    for (const [call, run] of Object.entries(calls)) {
      for (const [kind, make] of Object.entries(others)) {
        try { run(make()); results.push(call + ':' + kind + ':returned'); }
        catch (error) { results.push(call + ':' + kind + ':' + (error instanceof TypeError ? 'TypeError' : error.name)); }
      }
    }
    fs.writeSync(1, JSON.stringify(results) + '\\n');
  `
  t.deepEqual(JSON.parse(runChild(script)), [
    'copy:colorSpace:TypeError',
    'copy:rect:TypeError',
    'copy:chunk:TypeError',
    'observe:colorSpace:TypeError',
    'observe:rect:TypeError',
    'observe:chunk:TypeError',
  ])
})

test('registerVideoFrameCloseObserver: JS that closes or registers frames during registration completes', (t) => {
  // Registration stores the callback in a WeakMap that the module captures when it
  // loads, so JS runs in the middle of that step only if WeakMap.prototype.set was
  // replaced before then.
  const script = `
    const fs = require('node:fs');
    const set = WeakMap.prototype.set;
    let hook = null;
    WeakMap.prototype.set = function (key, value) {
      if (hook) hook(key);
      return set.call(this, key, value);
    };
    const m = require('./index.js');
    const make = () => m.createVideoFrameFromRGBA16Float(new Uint16Array(4), { codedWidth: 1, codedHeight: 1, timestamp: 0 });
    const results = [];
    for (const mode of ['close-other', 'register-other', 'close-self']) {
      const other = make();
      const target = make();
      let otherFired = 0;
      let targetFired = 0;
      let reentered = false;
      let outcome = 'returned';
      hook = (key) => {
        if (!reentered && key === target) {
          reentered = true;
          if (mode === 'close-other') other.close();
          if (mode === 'register-other') m.registerVideoFrameCloseObserver(other, () => { otherFired++; });
          if (mode === 'close-self') target.close();
        }
      };
      try {
        m.registerVideoFrameCloseObserver(target, () => { targetFired++; });
      } catch (error) {
        outcome = error.name;
      } finally {
        hook = null;
      }
      target.close();
      other.close();
      results.push([mode, reentered, outcome, targetFired, otherFired].join(':'));
    }
    fs.writeSync(1, JSON.stringify(results) + '\\n');
  `
  t.deepEqual(JSON.parse(runChild(script)), [
    'close-other:true:returned:1:0',
    'register-other:true:returned:1:1',
    'close-self:true:InvalidStateError:0:0',
  ])
})

test('registerVideoFrameCloseObserver: a frozen frame takes a callback that runs on close', (t) => {
  // Nothing is added to the frame, so freezing it does not matter (Chrome also
  // accepts listeners on a frozen VideoEncoder).
  const frame = createVideoFrameFromRGBA16Float(new Uint16Array(4), { codedWidth: 1, codedHeight: 1, timestamp: 0 })
  let fired = 0
  Object.freeze(frame)
  registerVideoFrameCloseObserver(frame, () => {
    fired++
  })
  frame.close()
  t.is(fired, 1)
  frame.close()
  t.is(fired, 1)
})
