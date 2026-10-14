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

import { VideoFrame } from '../index.js'

const SENTINEL = 0xa5

function childEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.WEBCODECS_LOG
  return env
}

// Runs `body` in a child Node process after PRELUDE. The body stores its results in `out`.
function runChild(body: string): Record<string, unknown> {
  const script = `${PRELUDE}
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
