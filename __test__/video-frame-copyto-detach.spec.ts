/**
 * VideoFrame.copyTo() and a destination that JS detaches, transfers or shrinks.
 *
 * Real Chrome 155 copies the bytes before copyTo() returns, so detaching the destination afterwards
 * keeps the copied bytes in the moved buffer, and detaching it from an options getter rejects with
 * a TypeError. It rejects views on resizable or growable buffers before it reads the options.
 * The expectations below are what Chrome 155 does for the same scripts.
 *
 * The cases that used to write into freed memory run in a child process, so a regression fails the
 * test instead of crashing the ava worker.
 */

import { execFileSync } from 'node:child_process'

import test from 'ava'

import { VideoFrame, type VideoFrameCopyToOptions, type VideoPixelFormat } from '../index.js'

const SENTINEL = 0xa5

function childEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.WEBCODECS_LOG
  return env
}

// Runs `body` in a child Node process after PRELUDE, and `beforeLoad` before it (PRELUDE loads the
// module). The body stores its results in `out`.
function runChild(body: string, beforeLoad = ''): Record<string, unknown> {
  const script = `${beforeLoad}
${PRELUDE}
(async () => {
${body}
  fs.writeSync(1, JSON.stringify(out));
})().catch((e) => { fs.writeSync(2, String(e && e.stack || e)); process.exit(3); });`
  const stdout = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    env: childEnv(),
    timeout: 60_000,
    encoding: 'utf8',
  })
  return JSON.parse(stdout)
}

// 1080p frames: big enough that a freed destination is returned to the OS.
const PRELUDE = `
const fs = require('node:fs');
const { VideoFrame } = require('./index.js');
const W = 1920, H = 1080, SENTINEL = ${SENTINEL};
const KINDS = ['RGBA', 'I420', 'I420->RGBA', 'RGBA->RGBA'];
const out = {};
function frameOf(kind) {
  const rgba = kind.startsWith('RGBA');
  const data = new Uint8Array(rgba ? W * H * 4 : (W * H * 3) / 2);
  for (let i = 0; i < data.length; i++) data[i] = (i * 7 + (i >>> 12)) & 255;
  return new VideoFrame(data, { format: rgba ? 'RGBA' : 'I420', codedWidth: W, codedHeight: H, timestamp: 0 });
}
const optionsOf = (kind) => (kind.endsWith('->RGBA') ? { format: 'RGBA' } : undefined);
async function reference(kind) {
  const frame = frameOf(kind);
  const bytes = new Uint8Array(frame.allocationSize(optionsOf(kind)));
  await frame.copyTo(bytes, optionsOf(kind));
  frame.close();
  return bytes;
}
const same = (a, b) => Buffer.compare(Buffer.from(a.buffer, a.byteOffset, a.length), Buffer.from(b.buffer, b.byteOffset, b.length)) === 0;
async function settle(promise) {
  try {
    await promise;
    return 'resolved';
  } catch (e) {
    return e.constructor.name + ': ' + e.message;
  }
}
`

test('copyTo: detaching the destination after the call keeps the copied bytes, like Chrome', (t) => {
  const out = runChild(`
  for (const kind of KINDS) {
    const ref = await reference(kind);
    for (const how of ['transfer', 'structuredClone', 'postMessage', 'transfer(0)']) {
      const frame = frameOf(kind);
      const dest = new Uint8Array(ref.length).fill(SENTINEL);
      const result = frame.copyTo(dest, optionsOf(kind));
      // Read two bytes before detaching: Chrome has written them by now.
      const writtenAtReturn = dest[0] === ref[0] && dest[dest.length - 1] === ref[ref.length - 1];
      let moved = null;
      if (how === 'transfer') moved = dest.buffer.transfer();
      else if (how === 'structuredClone') moved = structuredClone(dest.buffer, { transfer: [dest.buffer] });
      else if (how === 'postMessage') {
        const { port1, port2 } = new MessageChannel();
        moved = new Promise((resolve) => {
          port2.onmessage = (e) => { port1.close(); port2.close(); resolve(e.data); };
        });
        port1.postMessage(dest.buffer, [dest.buffer]);
      } else dest.buffer.transfer(0);
      const outcome = await settle(result);
      moved = await moved;
      out[kind + ' ' + how] = { outcome, writtenAtReturn, movedHasFrame: moved ? same(new Uint8Array(moved), ref) : null, destLength: dest.length };
      frame.close();
    }
  }
`)
  for (const kind of ['RGBA', 'I420', 'I420->RGBA', 'RGBA->RGBA']) {
    for (const how of ['transfer', 'structuredClone', 'postMessage']) {
      t.deepEqual(
        out[`${kind} ${how}`],
        { outcome: 'resolved', writtenAtReturn: true, movedHasFrame: true, destLength: 0 },
        `${kind} ${how}`,
      )
    }
    t.deepEqual(
      out[`${kind} transfer(0)`],
      { outcome: 'resolved', writtenAtReturn: true, movedHasFrame: null, destLength: 0 },
      kind,
    )
  }
})

test('copyTo: detaching the destination in an options getter rejects with TypeError, like Chrome', (t) => {
  const out = runChild(`
  for (const kind of KINDS) {
    for (const member of ['colorSpace', 'format', 'layout', 'rect']) {
      const frame = frameOf(kind);
      const base = optionsOf(kind) || {};
      const dest = new Uint8Array(frame.allocationSize(optionsOf(kind))).fill(SENTINEL);
      const value = base[member];
      const options = { ...base };
      delete options[member];
      Object.defineProperty(options, member, {
        enumerable: true,
        get() {
          dest.buffer.transfer(0);
          return value;
        },
      });
      out[kind + ' ' + member] = await settle(frame.copyTo(dest, options));
      frame.close();
    }
  }
`)
  for (const kind of ['RGBA', 'I420', 'I420->RGBA', 'RGBA->RGBA']) {
    for (const member of ['colorSpace', 'format', 'layout', 'rect']) {
      t.is(out[`${kind} ${member}`], 'TypeError: destination is detached', `${kind} ${member}`)
    }
  }
})

test('copyTo: resizable and growable destinations reject with TypeError before the options are read, like Chrome', (t) => {
  const out = runChild(`
  for (const kind of KINDS) {
    const size = frameOf(kind).allocationSize(optionsOf(kind));
    const cases = {
      'length-tracking, shrunk after': () => new Uint8Array(new ArrayBuffer(size, { maxByteLength: size })),
      'fixed length, shrunk after': () => new Uint8Array(new ArrayBuffer(size, { maxByteLength: size }), 0, size),
      'growable SharedArrayBuffer': () => new Uint8Array(new SharedArrayBuffer(size, { maxByteLength: size * 2 })),
    };
    for (const [name, make] of Object.entries(cases)) {
      const frame = frameOf(kind);
      const dest = make().fill(SENTINEL);
      const result = frame.copyTo(dest, optionsOf(kind));
      if (dest.buffer.resizable) dest.buffer.resize(0);
      out[kind + ' ' + name] = await settle(result);
      frame.close();
    }
    const frame = frameOf(kind);
    const buffer = new ArrayBuffer(size, { maxByteLength: size });
    let getterRuns = 0;
    const options = {
      ...optionsOf(kind),
      get rect() {
        getterRuns++;
        buffer.resize(0);
        return undefined;
      },
    };
    const outcome = await settle(frame.copyTo(new Uint8Array(buffer), options));
    out[kind + ' shrunk in a getter'] = { outcome, getterRuns, shrunk: buffer.byteLength !== size };
    frame.close();
  }
`)
  const notResizable = 'TypeError: The provided ArrayBufferView value must not be resizable.'
  for (const kind of ['RGBA', 'I420', 'I420->RGBA', 'RGBA->RGBA']) {
    for (const name of ['length-tracking, shrunk after', 'fixed length, shrunk after', 'growable SharedArrayBuffer']) {
      t.is(out[`${kind} ${name}`], notResizable, `${kind} ${name}`)
    }
    t.deepEqual(out[`${kind} shrunk in a getter`], { outcome: notResizable, getterRuns: 0, shrunk: false }, kind)
  }
})

test('copyTo: the resizable, growable and detached checks run no page code, like Chrome', (t) => {
  // Chrome reads these flags inside the engine. Patched prototype getters and own properties on a
  // buffer must not run or change the outcome. They are patched after the module has loaded.
  const out = runChild(`
  const { Worker } = require('node:worker_threads');
  const SIZE = 64 * 32 * 4;
  const frame = () => new VideoFrame(new Uint8Array(SIZE), { format: 'RGBA', codedWidth: 64, codedHeight: 32, timestamp: 0 });
  const detached = () => { const d = new Uint8Array(SIZE); d.buffer.transfer(); return d; };
  const rab = () => new ArrayBuffer(SIZE, { maxByteLength: SIZE });
  const growable = () => new SharedArrayBuffer(SIZE, { maxByteLength: SIZE * 2 });
  let runs = 0;
  async function patched(proto, key, get, dest) {
    const original = Object.getOwnPropertyDescriptor(proto, key);
    Object.defineProperty(proto, key, { configurable: true, get() { runs++; return get.call(this); } });
    try {
      let result;
      try {
        result = frame().copyTo(dest());
      } catch (e) {
        return 'threw ' + e.constructor.name + ': ' + e.message;
      }
      return await settle(result);
    } finally {
      if (original) Object.defineProperty(proto, key, original);
      else delete proto[key];
    }
  }
  const own = (buffer, key, value) => Object.defineProperty(buffer, key, { get: () => value });
  const throws = () => { throw new RangeError('patched'); };
  const AB = ArrayBuffer.prototype, SAB = SharedArrayBuffer.prototype;
  const kept = new Uint8Array(SIZE);
  out['AB.resizable throws'] = await patched(AB, 'resizable', throws, () => new Uint8Array(SIZE));
  out['AB.resizable says true'] = await patched(AB, 'resizable', () => true, () => new Uint8Array(SIZE));
  out['AB.resizable says false, resizable dest'] = await patched(AB, 'resizable', () => false, () => new Uint8Array(rab()));
  out['AB.resizable detaches dest'] = await patched(AB, 'resizable', function () { this.transfer(); return false; }, () => kept);
  out['dest length after'] = kept.length;
  out['AB.growable throws'] = await patched(AB, 'growable', throws, () => new Uint8Array(SIZE));
  out['SAB.growable throws'] = await patched(SAB, 'growable', throws, () => new Uint8Array(new SharedArrayBuffer(SIZE)));
  out['SAB.growable says false, growable dest'] = await patched(SAB, 'growable', () => false, () => new Uint8Array(growable()));
  out['AB.detached throws, detached dest'] = await patched(AB, 'detached', throws, detached);
  out['AB.detached says false, detached dest'] = await patched(AB, 'detached', () => false, detached);
  out['AB.detached says true, small dest'] = await patched(AB, 'detached', () => true, () => new Uint8Array(16));
  out['getter runs'] = runs;
  out['own resizable=false, resizable dest'] = await settle(frame().copyTo(new Uint8Array(own(rab(), 'resizable', false))));
  out['own resizable=true'] = await settle(frame().copyTo(new Uint8Array(own(new ArrayBuffer(SIZE), 'resizable', true))));
  out['own growable=false, growable dest'] = await settle(frame().copyTo(new Uint8Array(own(growable(), 'growable', false))));
  out['own growable=true'] = await settle(frame().copyTo(new Uint8Array(own(new SharedArrayBuffer(SIZE), 'growable', true))));
  const spoofed = detached();
  own(spoofed.buffer, 'detached', false);
  out['own detached=false, detached dest'] = await settle(frame().copyTo(spoofed));
  out['own detached=true, small dest'] = await settle(frame().copyTo(new Uint8Array(own(new ArrayBuffer(16), 'detached', true))));
  // A worker is a separate env: it captures the getters when it loads the module.
  out['worker, AB.resizable throws'] = await new Promise((resolve, reject) => {
    const worker = new Worker(\`
      const { parentPort } = require('node:worker_threads');
      const { VideoFrame } = require(\${JSON.stringify(require.resolve('./index.js'))});
      Object.defineProperty(ArrayBuffer.prototype, 'resizable', { configurable: true, get() { throw new RangeError('patched'); } });
      const frame = new VideoFrame(new Uint8Array(16), { format: 'RGBA', codedWidth: 2, codedHeight: 2, timestamp: 0 });
      Promise.resolve()
        .then(() => frame.copyTo(new Uint8Array(16)))
        .then(() => 'resolved', (e) => e.constructor.name + ': ' + e.message)
        .then((r) => parentPort.postMessage(r));
    \`, { eval: true });
    worker.once('message', resolve);
    worker.once('error', reject);
  });
`)
  const notResizable = 'TypeError: The provided ArrayBufferView value must not be resizable.'
  const tooSmall = 'TypeError: destination buffer too small: need 8192 bytes, got 16'
  t.deepEqual(out, {
    'AB.resizable throws': 'resolved',
    'AB.resizable says true': 'resolved',
    'AB.resizable says false, resizable dest': notResizable,
    'AB.resizable detaches dest': 'resolved',
    'dest length after': 8192,
    'AB.growable throws': 'resolved',
    'SAB.growable throws': 'resolved',
    'SAB.growable says false, growable dest': notResizable,
    'AB.detached throws, detached dest': 'TypeError: destination is detached',
    'AB.detached says false, detached dest': 'TypeError: destination is detached',
    'AB.detached says true, small dest': tooSmall,
    'getter runs': 0,
    'own resizable=false, resizable dest': notResizable,
    'own resizable=true': 'resolved',
    'own growable=false, growable dest': notResizable,
    'own growable=true': 'resolved',
    'own detached=false, detached dest': 'TypeError: destination is detached',
    'own detached=true, small dest': tooSmall,
    'worker, AB.resizable throws': 'resolved',
  })
})

test('copyTo: a flag getter patched before the module loads can change the resizable check, not memory safety', (t) => {
  // Known limit: the module captures the flag getters from the globals when it loads, so a getter
  // patched before that is the one it calls. Here it lets a real resizable buffer through and
  // resizes it. copyTo still looks the destination up after that JS has run, so nothing is written
  // outside it. A build that wrote through a pointer read before the getters crashed with SIGBUS.
  const out = runChild(
    `
  Object.defineProperty(ArrayBuffer.prototype, 'resizable', { configurable: true, get: realResizable });
  for (const kind of KINDS) {
    const ref = await reference(kind);
    const size = ref.length;
    const result = {};
    const copy = async (name, getters, shrinkInCheck) => {
      const buffer = new ArrayBuffer(size, { maxByteLength: size });
      const dest = new Uint8Array(buffer);
      const frame = frameOf(kind);
      // Copy the getters as getters: a spread would run them now.
      const options = Object.defineProperties({ ...optionsOf(kind) }, Object.getOwnPropertyDescriptors(getters(buffer)));
      globalThis.onResizableCheck = (b) => { if (shrinkInCheck && b === buffer) b.resize(0); };
      const promise = frame.copyTo(dest, options);
      globalThis.onResizableCheck = null;
      result[name] = await settle(promise);
      if (result[name] === 'resolved' && dest.length === size) result[name + ', same bytes'] = same(dest, ref);
      frame.close();
    };
    await copy('passes the check', () => ({}));
    await copy('shrunk in an options getter', (buffer) => ({ get rect() { buffer.resize(0); } }));
    await copy('shrunk in the patched getter', () => ({}), true);
    await copy('shrunk in the patched getter, grown back in an options getter', (buffer) => ({ get rect() { buffer.resize(size); } }), true);
    const buffer = new ArrayBuffer(size, { maxByteLength: size });
    const frame = frameOf(kind);
    const promise = frame.copyTo(new Uint8Array(buffer), optionsOf(kind));
    buffer.resize(0);
    result['shrunk after the call'] = await settle(promise);
    frame.close();
    out[kind] = result;
  }
`,
    `
const realResizable = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'resizable').get;
Object.defineProperty(ArrayBuffer.prototype, 'resizable', {
  configurable: true,
  get() {
    if (globalThis.onResizableCheck) globalThis.onResizableCheck(this);
    return false;
  },
});
`,
  )
  for (const kind of ['RGBA', 'I420', 'I420->RGBA', 'RGBA->RGBA']) {
    const size = kind === 'I420' ? (1920 * 1080 * 3) / 2 : 1920 * 1080 * 4
    const shrunk = `TypeError: destination buffer too small: need ${size} bytes, got 0`
    t.deepEqual(
      out[kind],
      {
        'passes the check': 'resolved',
        'passes the check, same bytes': true,
        'shrunk in an options getter': shrunk,
        'shrunk in the patched getter': shrunk,
        'shrunk in the patched getter, grown back in an options getter': 'resolved',
        'shrunk in the patched getter, grown back in an options getter, same bytes': true,
        'shrunk after the call': 'resolved',
      },
      kind,
    )
  }
})

// ============================================================================
// In-process cases: nothing here can write into freed memory
// ============================================================================

const W = 64
const H = 48
const KINDS = ['RGBA', 'I420', 'I420->RGBA', 'RGBA->RGBA'] as const
type Kind = (typeof KINDS)[number]

function frameOf(kind: Kind): VideoFrame {
  const rgba = kind.startsWith('RGBA')
  const data = new Uint8Array(rgba ? W * H * 4 : (W * H * 3) / 2)
  for (let i = 0; i < data.length; i++) data[i] = (i * 7 + 3) & 255
  return new VideoFrame(data, { format: rgba ? 'RGBA' : 'I420', codedWidth: W, codedHeight: H, timestamp: 0 })
}

const optionsOf = (kind: Kind) => (kind.endsWith('->RGBA') ? ({ format: 'RGBA' } as const) : undefined)

// ES2024 ArrayBuffer APIs, which the ES2023 lib of this repo does not type.
const detach = (buffer: ArrayBufferLike) =>
  (buffer as ArrayBuffer & { transfer(length: number): ArrayBuffer }).transfer(0)
const ResizableArrayBuffer = ArrayBuffer as unknown as new (
  length: number,
  options: { maxByteLength: number },
) => ArrayBuffer

async function reference(kind: Kind): Promise<Uint8Array> {
  const frame = frameOf(kind)
  const bytes = new Uint8Array(frame.allocationSize(optionsOf(kind)))
  await frame.copyTo(bytes, optionsOf(kind))
  frame.close()
  return bytes
}

for (const kind of KINDS) {
  test(`copyTo ${kind}: the bytes are in the destination when copyTo returns, like Chrome`, async (t) => {
    const ref = await reference(kind)
    const frame = frameOf(kind)
    const dest = new Uint8Array(ref.length).fill(SENTINEL)
    const result = frame.copyTo(dest, optionsOf(kind))
    const atReturn = dest.slice()
    t.deepEqual(atReturn, ref)
    t.true((await result).length > 0)
    frame.close()
  })

  test(`copyTo ${kind}: a SharedArrayBuffer destination is written before copyTo returns`, async (t) => {
    const ref = await reference(kind)
    const frame = frameOf(kind)
    const dest = new Uint8Array(new SharedArrayBuffer(ref.length)).fill(SENTINEL)
    const result = frame.copyTo(dest, optionsOf(kind))
    const atReturn = dest.slice()
    t.deepEqual(atReturn, ref)
    await t.notThrowsAsync(result)
    frame.close()
  })

  test(`copyTo ${kind}: close() right after copyTo still resolves with the data, like Chrome`, async (t) => {
    const ref = await reference(kind)
    const frame = frameOf(kind)
    const dest = new Uint8Array(ref.length).fill(SENTINEL)
    const result = frame.copyTo(dest, optionsOf(kind))
    frame.close()
    await t.notThrowsAsync(result)
    t.deepEqual(dest, ref)
  })
}

test('copyTo: a destination detached before the call rejects with TypeError', async (t) => {
  const frame = frameOf('RGBA')
  const dest = new Uint8Array(frame.allocationSize())
  detach(dest.buffer)
  await t.throwsAsync(frame.copyTo(dest), { instanceOf: TypeError, message: 'destination is detached' })
  frame.close()
})

test('copyTo: error order matches Chrome (resizable, then closed, then detached)', async (t) => {
  const closed = frameOf('RGBA')
  const size = closed.allocationSize()
  closed.close()
  const resizable = new Uint8Array(new ResizableArrayBuffer(size, { maxByteLength: size }))
  await t.throwsAsync(closed.copyTo(resizable), { instanceOf: TypeError, message: /must not be resizable/ })
  const detached = new Uint8Array(size)
  detach(detached.buffer)
  const error = await t.throwsAsync(closed.copyTo(detached))
  t.is(error?.name, 'InvalidStateError')
})

test('copyTo: bytes outside the planes are left as they were, like Chrome', async (t) => {
  const rgba = frameOf('RGBA')
  const rgbaDest = new Uint8Array(16 + (W * 4 + 8) * H).fill(SENTINEL)
  const layout = [{ offset: 16, stride: W * 4 + 8 }]
  await rgba.copyTo(rgbaDest, { layout })
  t.true(rgbaDest.subarray(0, 16).every((b) => b === SENTINEL))
  t.true(rgbaDest.subarray(16 + W * 4, 16 + W * 4 + 8).every((b) => b === SENTINEL))
  rgbaDest.fill(SENTINEL)
  await rgba.copyTo(rgbaDest, { layout, format: 'RGBA' })
  t.true(rgbaDest.subarray(16 + W * 4, 16 + W * 4 + 8).every((b) => b === SENTINEL))
  rgba.close()

  const i420 = frameOf('I420')
  const i420Dest = new Uint8Array(W * H * 3).fill(SENTINEL)
  await i420.copyTo(i420Dest, {
    layout: [
      { offset: 0, stride: W },
      { offset: W * H + 100, stride: W / 2 },
      { offset: W * H * 2, stride: W / 2 },
    ],
  })
  t.true(i420Dest.subarray(W * H, W * H + 100).every((b) => b === SENTINEL))
  t.true(i420Dest.subarray(W * H * 2 + (W * H) / 4).every((b) => b === SENTINEL))
  i420.close()
})

test('copyTo: a large RGB conversion matches the same frame copied in small strips', async (t) => {
  // A 1080p rect is split into row bands copied on several threads; a 60-row strip is copied in
  // one piece. Strips of every height cover each band edge, whatever the core count.
  const width = 1920
  const height = 1080
  const strip = 60
  const cases: Array<[VideoPixelFormat, VideoFrameCopyToOptions]> = [
    ['I420', { format: 'RGBA' }],
    ['I420', { format: 'BGRA', colorSpace: 'display-p3' }],
    ['RGBA', { format: 'RGBA', colorSpace: 'display-p3' }],
  ]
  for (const [format, options] of cases) {
    const data = new Uint8Array(format === 'I420' ? (width * height * 3) / 2 : width * height * 4)
    for (let i = 0; i < data.length; i++) data[i] = (i * 13 + (i >>> 9)) & 255
    const frame = new VideoFrame(data, {
      format,
      codedWidth: width,
      codedHeight: height,
      timestamp: 0,
      colorSpace: { primaries: 'bt709', transfer: 'bt709', matrix: 'bt709', fullRange: false },
    })
    const whole = new Uint8Array(width * height * 4)
    await frame.copyTo(whole, options)
    const strips = new Uint8Array(width * height * 4)
    for (let y = 0; y < height; y += strip) {
      const part = new Uint8Array(width * strip * 4)
      await frame.copyTo(part, { ...options, rect: { x: 0, y, width, height: strip } })
      strips.set(part, y * width * 4)
    }
    t.true(Buffer.from(whole).equals(Buffer.from(strips)), `${format} ${JSON.stringify(options)}`)

    // The same rect into a padded layout: the bands split the destination by its stride, and the
    // padding keeps its bytes.
    const row = width * 4
    const layout = [{ offset: 32, stride: row + 64 }]
    const padded = new Uint8Array(layout[0].offset + layout[0].stride * height).fill(SENTINEL)
    t.deepEqual(await frame.copyTo(padded, { ...options, layout }), layout)
    let wrongRows = 0
    for (let y = 0; y < height; y++) {
      const start = layout[0].offset + y * layout[0].stride
      const got = Buffer.from(padded.subarray(start, start + row))
      if (!got.equals(Buffer.from(whole.subarray(y * row, (y + 1) * row)))) wrongRows++
      if (!padded.subarray(start + row, start + layout[0].stride).every((b) => b === SENTINEL)) wrongRows++
    }
    t.true(padded.subarray(0, layout[0].offset).every((b) => b === SENTINEL))
    t.is(wrongRows, 0, `${format} ${JSON.stringify(options)} with a padded layout`)
    frame.close()
  }
})
