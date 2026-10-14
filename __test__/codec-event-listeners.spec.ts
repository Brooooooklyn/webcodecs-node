/**
 * Listener storage and dequeue events.
 *
 * Codec event listeners and VideoFrame close observers are kept alive by a store
 * that user code cannot reach, so it can neither drop them nor make them leak.
 *
 * Dequeue events follow the WebCodecs "schedule dequeue event" steps, as Chrome
 * does: dequeues that happen before a queued dequeue event runs share that event,
 * and an error that drops every queued item fires one event.
 *
 * Each test runs in a child process: some replace builtins, and the GC tests
 * need --expose-gc.
 */

import { execFileSync } from 'node:child_process'

import test from 'ava'

// The child's stdout carries the test result, so it must not inherit WEBCODECS_LOG
// (CI sets it, and the log output goes to stdout).
function childEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.WEBCODECS_LOG
  return env
}

// Runs `script` in a child Node process with --expose-gc. The script prints one
// JSON line with fs.writeSync and exits; that line is returned parsed.
function runChild<T>(script: string): T {
  const output = execFileSync(process.execPath, ['--expose-gc', '-e', script], {
    cwd: process.cwd(),
    env: childEnv(),
    timeout: 60_000,
    encoding: 'utf8',
  })
  return JSON.parse(output.trim().split('\n').pop()!) as T
}

// Shared child-side helpers.
const PRELUDE = `
  const fs = require('node:fs');
  const done = (value) => { fs.writeSync(1, JSON.stringify(value) + '\\n'); process.exit(0); };
  const fail = (error) => { fs.writeSync(1, JSON.stringify({ failed: String(error && error.stack) }) + '\\n'); process.exit(0); };
  const tick = () => new Promise((resolve) => setImmediate(resolve));
  const fullGc = async () => { for (let i = 0; i < 6; i++) { globalThis.gc(); await tick(); } };
  const CODECS = ['VideoEncoder', 'VideoDecoder', 'AudioEncoder', 'AudioDecoder'];
`

// Builds one owner of each kind: the four codecs and a VideoFrame.
const OWNERS = `
  const makeOwner = (m, kind) =>
    kind === 'VideoFrame'
      ? new m.VideoFrame(new Uint8Array(64), { format: 'RGBA', codedWidth: 4, codedHeight: 4, timestamp: 0 })
      : new m[kind]({ output: () => {}, error: () => {} });
  // Registers callbacks that nothing else references; returns how many fire later.
  const register = (m, kind, owner, onCall) => {
    if (kind === 'VideoFrame') {
      m.registerVideoFrameCloseObserver(owner, () => onCall());
      return 1;
    }
    owner.addEventListener('dequeue', () => onCall());
    owner.ondequeue = () => onCall();
    return 2;
  };
  const trigger = (kind, owner) => {
    if (kind === 'VideoFrame') owner.close();
    else owner.dispatchEvent('dequeue');
  };
  const KINDS = [...CODECS, 'VideoFrame'];
`

test('listener storage: nothing is stored on the object, and user code cannot drop the callbacks', (t) => {
  const result = runChild<{
    ownKeys: Record<string, string[]>
    fired: Record<string, number>
    expected: Record<string, number>
  }>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${OWNERS}
    (async () => {
      const owners = [];
      const fired = {};
      const expected = {};
      for (const kind of KINDS) {
        const owner = makeOwner(m, kind);
        fired[kind] = 0;
        expected[kind] = register(m, kind, owner, () => fired[kind]++);
        owners.push([kind, owner]);
      }
      const ownKeys = {};
      for (const [kind, owner] of owners) {
        ownKeys[kind] = Reflect.ownKeys(owner).map(String);
        // Empty and remove anything the object exposes, then plant a decoy.
        for (const key of Reflect.ownKeys(owner)) {
          const value = owner[key];
          if (value instanceof Map || value instanceof Set) value.clear();
          delete owner[key];
        }
        owner[Symbol.for('webcodecs.listeners')] = new Map();
      }
      // Builtins replaced after the module loaded are never used by it.
      for (const C of [Map, WeakMap]) {
        for (const name of ['get', 'set', 'delete', 'has']) {
          C.prototype[name] = function () { throw new Error('replaced ' + C.name + '.' + name); };
        }
      }
      await fullGc();
      for (const [kind, owner] of owners) {
        trigger(kind, owner);
        if (kind !== 'VideoFrame') owner.close();
      }
      done({ ownKeys, fired, expected });
    })().catch(fail);
  `)
  for (const kind of Object.keys(result.expected)) {
    t.deepEqual(result.ownKeys[kind], [], `${kind} exposes no listener storage`)
  }
  t.deepEqual(result.fired, result.expected)
})

test('listener storage: a value planted under the old listener key cannot make a callback leak', (t) => {
  // A callback that captures its owner forms a cycle; the cycle must stay collectable.
  const result = runChild<Record<string, boolean>>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${OWNERS}
    (async () => {
      const collected = {};
      const registry = new FinalizationRegistry((kind) => { collected[kind] = true; });
      for (const kind of KINDS) {
        collected[kind] = false;
        (() => {
          const owner = makeOwner(m, kind);
          owner[Symbol.for('webcodecs.listeners')] = {};
          register(m, kind, owner, () => owner.timestamp ?? owner.state);
          registry.register(owner, kind);
        })();
      }
      for (let i = 0; i < 20 && Object.values(collected).includes(false); i++) await fullGc();
      done(collected);
    })().catch(fail);
  `)
  t.deepEqual(result, {
    VideoEncoder: true,
    VideoDecoder: true,
    AudioEncoder: true,
    AudioDecoder: true,
    VideoFrame: true,
  })
})

test('listener storage: frozen, sealed and non-extensible objects take callbacks that fire and stay collectable', (t) => {
  // Chrome accepts listeners on a frozen VideoEncoder: nothing is added to the object.
  const result = runChild<
    Record<string, { threw: string | null; fired: number; expected: number; collected: boolean }>
  >(`
    ${PRELUDE}
    const m = require('./index.js');
    ${OWNERS}
    (async () => {
      const out = {};
      const registry = new FinalizationRegistry((key) => { out[key].collected = true; });
      for (const lock of ['freeze', 'seal', 'preventExtensions']) {
        for (const kind of KINDS) {
          const key = lock + ':' + kind;
          const entry = { threw: null, fired: 0, expected: 0, collected: false };
          out[key] = entry;
          const owner = makeOwner(m, kind);
          Object[lock](owner);
          try {
            entry.expected = register(m, kind, owner, () => entry.fired++);
          } catch (error) {
            entry.threw = error.name;
          }
          await fullGc();
          trigger(kind, owner);
          if (kind !== 'VideoFrame') owner.close();
          // A second owner whose callback captures it must still be collected.
          (() => {
            const other = makeOwner(m, kind);
            Object[lock](other);
            try { register(m, kind, other, () => other.timestamp ?? other.state); } catch {}
            registry.register(other, key);
          })();
        }
      }
      for (let i = 0; i < 20 && Object.values(out).some((entry) => !entry.collected); i++) await fullGc();
      done(out);
    })().catch(fail);
  `)
  for (const [key, entry] of Object.entries(result)) {
    t.is(entry.threw, null, `${key}: registering does not throw`)
    t.is(entry.fired, entry.expected, `${key}: every callback fires`)
    t.true(entry.collected, `${key}: a callback that captures its owner is collected with it`)
  }
})

test('listener storage: WeakMap replaced before the module loads can drop callbacks but cannot crash', (t) => {
  // Documented limit: code that runs before the module loads is handed the store.
  // A dropped callback is collected and skipped; nothing reads freed memory.
  const result = runChild<{ handedStore: boolean; fired: number; afterward: number }>(`
    ${PRELUDE}
    let store = null;
    const set = WeakMap.prototype.set;
    WeakMap.prototype.set = function (key, value) { store = this; return set.call(this, key, value); };
    const m = require('./index.js');
    WeakMap.prototype.set = set;
    ${OWNERS}
    (async () => {
      let fired = 0;
      const owners = KINDS.map((kind) => {
        const owner = makeOwner(m, kind);
        register(m, kind, owner, () => fired++);
        return [kind, owner];
      });
      const handedStore = store !== null;
      for (const [, owner] of owners) store && store.delete(owner);
      await fullGc();
      for (const [kind, owner] of owners) trigger(kind, owner);
      // The owners keep working after their callbacks were dropped.
      let afterward = 0;
      for (const [kind, owner] of owners) {
        if (kind === 'VideoFrame') continue;
        owner.addEventListener('dequeue', () => afterward++);
        owner.dispatchEvent('dequeue');
        owner.close();
      }
      done({ handedStore, fired, afterward });
    })().catch(fail);
  `)
  t.deepEqual(result, { handedStore: true, fired: 0, afterward: 4 })
})

test('module load: the listener store and the copyTo buffer checks are both set up when the module loads', (t) => {
  // napi-rs keeps one load hook per crate, so all load-time work must run from it.
  // Builtins replaced after the load must reach neither.
  const result = runChild<{ collected: Record<string, boolean>; patchedGetterCalls: number; copied: boolean }>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${OWNERS}
    for (const name of ['get', 'set', 'delete', 'has']) {
      WeakMap.prototype[name] = function () { throw new Error('replaced WeakMap.' + name); };
    }
    let patchedGetterCalls = 0;
    for (const key of ['resizable', 'detached']) {
      const { get } = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, key);
      Object.defineProperty(ArrayBuffer.prototype, key, {
        configurable: true,
        get() { patchedGetterCalls++; return get.call(this); },
      });
    }
    (async () => {
      // Without the store, callbacks are held strongly and a cycle is never collected.
      const collected = {};
      const registry = new FinalizationRegistry((kind) => { collected[kind] = true; });
      for (const kind of KINDS) {
        collected[kind] = false;
        (() => {
          const owner = makeOwner(m, kind);
          register(m, kind, owner, () => owner.timestamp ?? owner.state);
          registry.register(owner, kind);
        })();
      }
      for (let i = 0; i < 20 && Object.values(collected).includes(false); i++) await fullGc();
      // Without the load-time getters, copyTo would look up the replaced ones.
      const frame = new m.VideoFrame(new Uint8Array(64).fill(7), { format: 'RGBA', codedWidth: 4, codedHeight: 4, timestamp: 0 });
      const destination = new Uint8Array(64);
      await frame.copyTo(destination);
      frame.close();
      done({ collected, patchedGetterCalls, copied: destination.every((byte) => byte === 7) });
    })().catch(fail);
  `)
  t.deepEqual(result, {
    collected: { VideoEncoder: true, VideoDecoder: true, AudioEncoder: true, AudioDecoder: true, VideoFrame: true },
    patchedGetterCalls: 0,
    copied: true,
  })
})

// ============================================================================
// Dequeue event coalescing
// ============================================================================

// Child-side setup: encoded VP8 and Opus chunks for the decoders, and a burst runner.
const BURST = `
  const N = 6;
  const videoFrame = (i) =>
    new m.VideoFrame(new Uint8Array(64 * 64 * 4).fill(i * 20), { format: 'RGBA', codedWidth: 64, codedHeight: 64, timestamp: i * 33333 });
  const audioData = (i) =>
    new m.AudioData({ format: 'f32-planar', sampleRate: 48000, numberOfFrames: 960, numberOfChannels: 2, timestamp: i * 20000, data: new Float32Array(1920).fill(0.1) });
  const VIDEO = { codec: 'vp8', width: 64, height: 64, bitrate: 200000 };
  const AUDIO = { codec: 'opus', sampleRate: 48000, numberOfChannels: 2 };
  async function encodeAll(Codec, config, make) {
    const chunks = [];
    const encoder = new Codec({ output: (chunk) => chunks.push(chunk), error: fail });
    encoder.configure(config);
    for (let i = 0; i < N; i++) { const input = make(i); encoder.encode(input); input.close(); }
    await encoder.flush();
    encoder.close();
    return chunks;
  }
  const queueSize = (codec) => codec.encodeQueueSize ?? codec.decodeQueueSize;
  // Queues N items in one task, then keeps the JS thread busy until the worker has
  // dequeued all of them, so no dequeue event can run in between.
  async function burst(codec, submit) {
    const seen = [];
    codec.addEventListener('dequeue', () => seen.push(queueSize(codec)));
    submit();
    // encode()/decode() hand their work to the worker from microtasks; let them run.
    await Promise.resolve();
    const deadline = Date.now() + 30000;
    while (queueSize(codec) > 0 && Date.now() < deadline) {}
    await codec.flush();
    for (let i = 0; i < 20 && seen.length === 0; i++) await new Promise((resolve) => setTimeout(resolve, 10));
    // Room for any extra event to arrive.
    await new Promise((resolve) => setTimeout(resolve, 50));
    codec.close();
    return seen;
  }
`

test('dequeue: items dequeued before the dequeue event runs share one event, for every codec', (t) => {
  // WebCodecs "schedule dequeue event": a dequeue while an event is already
  // queued does not queue another one (Chrome 155 does the same).
  const result = runChild<Record<string, number[]>>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${BURST}
    (async () => {
      const videoChunks = await encodeAll(m.VideoEncoder, VIDEO, videoFrame);
      const audioChunks = await encodeAll(m.AudioEncoder, AUDIO, audioData);
      const out = {};
      {
        const encoder = new m.VideoEncoder({ output: () => {}, error: fail });
        encoder.configure(VIDEO);
        out.VideoEncoder = await burst(encoder, () => {
          for (let i = 0; i < N; i++) { const frame = videoFrame(i); encoder.encode(frame); frame.close(); }
        });
      }
      {
        const decoder = new m.VideoDecoder({ output: (frame) => frame.close(), error: fail });
        decoder.configure({ codec: 'vp8', codedWidth: 64, codedHeight: 64 });
        out.VideoDecoder = await burst(decoder, () => { for (const chunk of videoChunks) decoder.decode(chunk); });
      }
      {
        const encoder = new m.AudioEncoder({ output: () => {}, error: fail });
        encoder.configure(AUDIO);
        out.AudioEncoder = await burst(encoder, () => {
          for (let i = 0; i < N; i++) { const data = audioData(i); encoder.encode(data); data.close(); }
        });
      }
      {
        const decoder = new m.AudioDecoder({ output: (data) => data.close(), error: fail });
        decoder.configure(AUDIO);
        out.AudioDecoder = await burst(decoder, () => { for (const chunk of audioChunks) decoder.decode(chunk); });
      }
      done(out);
    })().catch(fail);
  `)
  t.deepEqual(result, { VideoEncoder: [0], VideoDecoder: [0], AudioEncoder: [0], AudioDecoder: [0] })
})

// Child-side runner for error teardown: records the error (with the queue size and
// state it sees) and every dequeue event.
//
// Not checked: whether the error comes before the dequeue events. Errors and
// dequeue events reach JS through two thread-safe functions, so their order can
// flip under load (origin/main does the same); Chrome reports the error first.
const TEARDOWN = `
  const N = 6;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  async function teardown(Codec, setup) {
    const errors = [];
    const dequeues = [];
    let codec;
    const queueSize = () => codec.encodeQueueSize ?? codec.decodeQueueSize;
    codec = new Codec({
      output: (out) => out.close?.(),
      error: (error) => errors.push(error.name + ':' + queueSize() + ':' + codec.state),
    });
    codec.addEventListener('dequeue', () => dequeues.push('dequeue:' + queueSize()));
    setup(codec);
    const queued = queueSize();
    for (let i = 0; i < 200 && !(errors.length > 0 && dequeues.includes('dequeue:0')); i++) await sleep(10);
    // Room for any extra event to arrive.
    await sleep(100);
    return { queued, state: codec.state, errors, dequeues };
  }
`

type Teardown = { queued: number; state: string; errors: string[]; dequeues: string[] }

test('dequeue: an error that drops every queued item fires one dequeue event', (t) => {
  // Chrome 155 resets the codec on error: the queue empties at once and one
  // dequeue event reports it.
  const result = runChild<Record<string, Teardown>>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${TEARDOWN}
    (async () => {
      const out = {};
      out.AudioDecoder = await teardown(m.AudioDecoder, (decoder) => {
        decoder.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
        for (let i = 0; i < N; i++) decoder.decode(new m.EncodedAudioChunk({ type: 'key', timestamp: i, data: new Uint8Array(0) }));
      });
      out.AudioEncoder = await teardown(m.AudioEncoder, (encoder) => {
        encoder.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
        encoder.configure({ codec: 'opus', sampleRate: 44100, numberOfChannels: 2 });
        // Chrome fails on 48 kHz data for the 44.1 kHz configuration; this repo
        // cannot open Opus at 44.1 kHz. Both close with an EncodingError.
        for (let i = 0; i < N; i++) {
          const data = new m.AudioData({ format: 'f32-planar', sampleRate: 48000, numberOfFrames: 960, numberOfChannels: 2, timestamp: i * 20000, data: new Float32Array(1920) });
          encoder.encode(data);
          data.close();
        }
      });
      done(out);
    })().catch(fail);
  `)
  for (const [codec, entry] of Object.entries(result)) {
    t.is(entry.queued, 6, `${codec}: all items were queued`)
    t.is(entry.state, 'closed', `${codec}: the error closed the codec`)
    t.deepEqual(
      entry.errors.map((error) => error.split(':')[0]),
      ['EncodingError'],
      `${codec}: one error`,
    )
    t.deepEqual(entry.dequeues, ['dequeue:0'], `${codec}: one dequeue event for all dropped items`)
  }
  // The decode error empties the queue before the error callback runs, as in Chrome.
  // (The AudioEncoder error comes from a failed reconfiguration; on that path the
  // queue is emptied later, so its callback can still see the queued encodes.)
  t.deepEqual(result.AudioDecoder.errors, ['EncodingError:0:closed'])
})

test('dequeue: an error after a decoded chunk leaves the VideoDecoder queue empty', (t) => {
  // Chrome 155 decodes the key chunk, fails on the first empty delta chunk, and logs
  // dequeue:6, the error with a queue of 0, then dequeue:0. Whether the first
  // dequeue joins the teardown event here depends on timing, so the number of
  // events is not checked. (Chrome rejects empty key chunks in decode() itself,
  // so they cannot be the trigger.)
  const result = runChild<Teardown>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${TEARDOWN}
    (async () => {
      const chunks = [];
      const encoder = new m.VideoEncoder({ output: (chunk) => chunks.push(chunk), error: fail });
      encoder.configure({ codec: 'vp8', width: 64, height: 64, bitrate: 200000 });
      const frame = new m.VideoFrame(new Uint8Array(64 * 64 * 4), { format: 'RGBA', codedWidth: 64, codedHeight: 64, timestamp: 0 });
      encoder.encode(frame, { keyFrame: true });
      frame.close();
      await encoder.flush();
      encoder.close();
      done(await teardown(m.VideoDecoder, (decoder) => {
        decoder.configure({ codec: 'vp8', codedWidth: 64, codedHeight: 64 });
        decoder.decode(chunks[0]);
        for (let i = 1; i <= N; i++) decoder.decode(new m.EncodedVideoChunk({ type: 'delta', timestamp: i * 33333, data: new Uint8Array(0) }));
      }));
    })().catch(fail);
  `)
  t.is(result.queued, 7, 'all chunks were queued')
  t.is(result.state, 'closed', 'the error closed the decoder')
  t.deepEqual(result.errors, ['EncodingError:0:closed'], 'one error, seeing an empty queue')
  t.is(result.dequeues.at(-1), 'dequeue:0', 'the last dequeue event sees an empty queue')
})
