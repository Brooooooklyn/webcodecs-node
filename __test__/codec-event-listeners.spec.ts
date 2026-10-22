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

test('listener storage: WeakMap methods replaced before the module loads can call back into the same codec or frame', (t) => {
  // The store calls them while it links a new holder, the first time a callback is
  // registered on an object. No lock may be held then: the codec's lock is not
  // reentrant, so a call back into the same codec would wait forever.
  const result = runChild<Record<string, string>>(`
    ${PRELUDE}
    const get = WeakMap.prototype.get;
    const set = WeakMap.prototype.set;
    let target = null;
    let hook = null;
    const runHook = (key) => {
      if (hook && key === target) { const run = hook; hook = null; run(); }
    };
    WeakMap.prototype.get = function (key) { runHook(key); return get.call(this, key); };
    WeakMap.prototype.set = function (key, value) { runHook(key); return set.call(this, key, value); };
    const m = require('./index.js');
    const out = {};
    for (const kind of CODECS) {
      for (const outer of ['addEventListener', 'ondequeue']) {
        for (const inner of ['addEventListener', 'removeEventListener', 'ondequeue', 'dispatchEvent']) {
          const codec = new m[kind]({ output: () => {}, error: () => {} });
          const log = [];
          target = codec;
          hook = () => {
            log.push('hook');
            const listener = () => log.push('inner');
            if (inner === 'addEventListener') codec.addEventListener('dequeue', listener);
            if (inner === 'removeEventListener') {
              codec.addEventListener('dequeue', listener);
              codec.removeEventListener('dequeue', listener);
            }
            if (inner === 'ondequeue') codec.ondequeue = listener;
            if (inner === 'dispatchEvent') {
              codec.addEventListener('dequeue', listener);
              codec.dispatchEvent('dequeue');
            }
          };
          const listener = () => log.push('outer');
          if (outer === 'addEventListener') codec.addEventListener('dequeue', listener);
          else codec.ondequeue = listener;
          hook = null;
          codec.dispatchEvent('dequeue');
          out[kind + ':' + outer + '>' + inner] = log.join(',');
          codec.close();
        }
      }
    }
    const frame = new m.VideoFrame(new Uint8Array(64), { format: 'RGBA', codedWidth: 4, codedHeight: 4, timestamp: 0 });
    const log = [];
    target = frame;
    hook = () => {
      log.push('hook');
      m.registerVideoFrameCloseObserver(frame, () => log.push('inner'));
    };
    m.registerVideoFrameCloseObserver(frame, () => log.push('outer'));
    hook = null;
    frame.close();
    out['VideoFrame:register>register'] = log.join(',');
    done(out);
  `)
  const expected: Record<string, string> = {}
  for (const kind of ['VideoEncoder', 'VideoDecoder', 'AudioEncoder', 'AudioDecoder']) {
    for (const outer of ['addEventListener', 'ondequeue']) {
      // The holder is linked before the outer callback is registered, so callbacks
      // registered from the hook come first.
      expected[`${kind}:${outer}>addEventListener`] = 'hook,inner,outer'
      expected[`${kind}:${outer}>removeEventListener`] = 'hook,outer'
      expected[`${kind}:${outer}>dispatchEvent`] = 'hook,inner,inner,outer'
    }
    expected[`${kind}:addEventListener>ondequeue`] = 'hook,inner,outer'
    // Assigning ondequeue again replaces the handler set from the hook.
    expected[`${kind}:ondequeue>ondequeue`] = 'hook,outer'
  }
  expected['VideoFrame:register>register'] = 'hook,inner,outer'
  t.deepEqual(result, expected)
})

test('listener storage: an async_hooks init hook can call back into the codec while its first listener is added', (t) => {
  // Adding the first listener creates the codec's thread-safe function, which runs
  // async_hooks init hooks. That must also happen with no lock held.
  const result = runChild<Record<string, string>>(`
    ${PRELUDE}
    const asyncHooks = require('node:async_hooks');
    const m = require('./index.js');
    let hook = null;
    asyncHooks.createHook({
      init() { if (hook) { const run = hook; hook = null; run(); } },
    }).enable();
    const out = {};
    for (const kind of CODECS) {
      for (const outer of ['addEventListener', 'ondequeue']) {
        const codec = new m[kind]({ output: () => {}, error: () => {} });
        const log = [];
        hook = () => {
          log.push('hook');
          codec.addEventListener('dequeue', () => log.push('inner'));
        };
        const listener = () => log.push('outer');
        if (outer === 'addEventListener') codec.addEventListener('dequeue', listener);
        else codec.ondequeue = listener;
        hook = null;
        codec.dispatchEvent('dequeue');
        out[kind + ':' + outer] = log.join(',');
        codec.close();
      }
    }
    done(out);
  `)
  const expected: Record<string, string> = {}
  for (const kind of ['VideoEncoder', 'VideoDecoder', 'AudioEncoder', 'AudioDecoder']) {
    for (const outer of ['addEventListener', 'ondequeue']) expected[`${kind}:${outer}`] = 'hook,inner,outer'
  }
  t.deepEqual(result, expected)
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

test('module load: loading the module again leaves callbacks removable and collectable', (t) => {
  // process.dlopen with a fresh module object runs the module's load hook again in
  // the same Node environment. (Node gives that load its own napi_env, so it gets
  // its own store.) Callbacks registered before and after the second load must
  // still be released by removal while their owners stay alive.
  const result = runChild<{ loadedAgain: boolean; collected: Record<string, boolean>; owners: number }>(`
    ${PRELUDE}
    const first = require('./index.js');
    const file = Object.keys(require.cache).find((name) => name.endsWith('.node'));
    const collected = {};
    const registry = new FinalizationRegistry((key) => { collected[key] = true; });
    const owners = [];
    let removers = [];
    const register = (m, kind, key) => {
      collected[key] = false;
      const owner = kind === 'VideoFrame'
        ? new m.VideoFrame(new Uint8Array(64), { format: 'RGBA', codedWidth: 4, codedHeight: 4, timestamp: 0 })
        : new m[kind]({ output: () => {}, error: () => {} });
      owners.push(owner);
      (() => {
        const callback = () => key;
        registry.register(callback, key);
        if (kind === 'VideoFrame') {
          m.registerVideoFrameCloseObserver(owner, callback);
          // close() releases the observers; the frame object stays alive.
          removers.push(() => owner.close());
        } else {
          owner.addEventListener('dequeue', callback);
          owner.ondequeue = callback;
          removers.push(() => { owner.removeEventListener('dequeue', callback); owner.ondequeue = null; });
        }
      })();
    };
    register(first, 'AudioDecoder', 'first load, before:AudioDecoder');
    register(first, 'VideoFrame', 'first load, before:VideoFrame');
    const second = { exports: {} };
    process.dlopen(second, file);
    register(first, 'VideoEncoder', 'first load, after:VideoEncoder');
    register(second.exports, 'AudioDecoder', 'second load:AudioDecoder');
    register(second.exports, 'VideoFrame', 'second load:VideoFrame');
    for (const remove of removers) remove();
    removers = [];
    (async () => {
      for (let i = 0; i < 20 && Object.values(collected).includes(false); i++) await fullGc();
      done({ loadedAgain: first.AudioDecoder !== second.exports.AudioDecoder, collected, owners: owners.length });
    })().catch(fail);
  `)
  t.deepEqual(result, {
    loadedAgain: true,
    collected: {
      'first load, before:AudioDecoder': true,
      'first load, before:VideoFrame': true,
      'first load, after:VideoEncoder': true,
      'second load:AudioDecoder': true,
      'second load:VideoFrame': true,
    },
    owners: 5,
  })
})

test('module load: the first module keeps its iterable layout support after a second load', (t) => {
  // allocationSize reaches ITERATOR_SYMBOL without the separate copyTo getter cache.
  // The child exits normally so execFileSync also checks its env cleanup hooks.
  const result = runChild<{ loadedAgain: boolean; firstSize: number; secondSize: number }>(`
    const fs = require('node:fs');
    const first = require('./index.js');
    const file = Object.keys(require.cache).find((name) => name.endsWith('.node'));
    const second = { exports: {} };
    process.dlopen(second, file);
    const layout = {
      *[Symbol.iterator]() {
        yield { offset: 0, stride: 4 };
        yield { offset: 8, stride: 2 };
        yield { offset: 10, stride: 2 };
      },
    };
    const makeFrame = (m) => new m.VideoFrame(new Uint8Array(12), {
      format: 'I420', codedWidth: 4, codedHeight: 2, timestamp: 0,
    });
    const firstFrame = makeFrame(first);
    const secondFrame = makeFrame(second.exports);
    const firstSize = firstFrame.allocationSize({ layout });
    const secondSize = secondFrame.allocationSize({ layout });
    firstFrame.close();
    secondFrame.close();
    fs.writeSync(1, JSON.stringify({
      loadedAgain: first.VideoFrame !== second.exports.VideoFrame,
      firstSize,
      secondSize,
    }) + '\\n');
  `)
  t.deepEqual(result, { loadedAgain: true, firstSize: 12, secondSize: 12 })
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

test('dequeue: an item dequeued while the first listener is being added still fires its event', (t) => {
  // Adding the first listener makes the codec's thread-safe function with the
  // event state unlocked (its async_hooks init hooks may call back in). The init
  // hook below holds the JS thread there until the worker has dequeued the one
  // queued item. Chrome 155 fires that event: the queue size dropped and its
  // dequeue task has not run before the listener is added.
  const result = runChild<Record<string, string>>(`
    ${PRELUDE}
    const asyncHooks = require('node:async_hooks');
    const m = require('./index.js');
    let armed = false;
    asyncHooks.createHook({
      init() {
        if (!armed) return;
        armed = false;
        const start = Date.now();
        while (Date.now() - start < 200) {}
      },
    }).enable();
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const frame = () =>
      new m.VideoFrame(new Uint8Array(64 * 64 * 4).fill(80), { format: 'RGBA', codedWidth: 64, codedHeight: 64, timestamp: 0 });
    const audio = () =>
      new m.AudioData({ format: 'f32-planar', sampleRate: 48000, numberOfFrames: 960, numberOfChannels: 2, timestamp: 0, data: new Float32Array(1920).fill(0.1) });
    const VIDEO = { codec: 'vp8', width: 64, height: 64, bitrate: 200000 };
    const AUDIO = { codec: 'opus', sampleRate: 48000, numberOfChannels: 2 };
    async function firstChunk(Codec, config, make) {
      const chunks = [];
      const encoder = new Codec({ output: (chunk) => chunks.push(chunk), error: fail });
      encoder.configure(config);
      const input = make();
      encoder.encode(input);
      input.close();
      await encoder.flush();
      encoder.close();
      return chunks[0];
    }
    (async () => {
      const videoChunk = await firstChunk(m.VideoEncoder, VIDEO, frame);
      const audioChunk = await firstChunk(m.AudioEncoder, AUDIO, audio);
      const SUBMIT = {
        VideoEncoder: (codec) => { codec.configure(VIDEO); const input = frame(); codec.encode(input); input.close(); },
        VideoDecoder: (codec) => { codec.configure({ codec: 'vp8', codedWidth: 64, codedHeight: 64 }); codec.decode(videoChunk); },
        AudioEncoder: (codec) => { codec.configure(AUDIO); const input = audio(); codec.encode(input); input.close(); },
        AudioDecoder: (codec) => { codec.configure(AUDIO); codec.decode(audioChunk); },
      };
      const out = {};
      async function run(kind, how) {
        let events = 0;
        let weak;
        await (async () => {
          const codec = new m[kind]({ output: (output) => output.close?.(), error: fail });
          weak = new WeakRef(codec);
          SUBMIT[kind](codec);
          // The codec hands the item to its worker from a microtask.
          await Promise.resolve();
          const listener = () => { events++; };
          armed = true;
          if (how === 'addEventListener') codec.addEventListener('dequeue', listener);
          else codec.ondequeue = listener;
          armed = false;
          await codec.flush();
          await sleep(50);
          codec.close();
        })();
        await fullGc();
        return events + (weak.deref() === undefined ? ' collected' : ' kept');
      }
      for (const kind of CODECS) {
        for (const how of ['addEventListener', 'ondequeue']) {
          // '0 collected' (rare): the worker dequeued the item before the
          // listener was being added at all. That older gap fires no event on
          // main either and is not the one under test, so run the case again.
          // The gap under test gives '0 kept' and is never run again.
          let result = await run(kind, how);
          for (let attempt = 1; attempt < 5 && result === '0 collected'; attempt++) {
            result = await run(kind, how);
          }
          out[kind + ':' + how] = result;
        }
      }
      done(out);
    })().catch(fail);
  `)
  const expected: Record<string, string> = {}
  for (const kind of ['VideoEncoder', 'VideoDecoder', 'AudioEncoder', 'AudioDecoder']) {
    for (const how of ['addEventListener', 'ondequeue']) expected[`${kind}:${how}`] = '1 collected'
  }
  t.deepEqual(result, expected)
})

// Child-side runner for error teardown: records the error (with the queue size and
// state it sees), every dequeue event, and the order of both in log.
const TEARDOWN = `
  const N = 6;
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  async function teardown(Codec, setup, onError) {
    const errors = [];
    const dequeues = [];
    const log = [];
    let codec;
    const queueSize = () => codec.encodeQueueSize ?? codec.decodeQueueSize;
    codec = new Codec({
      output: (out) => out.close?.(),
      error: (error) => {
        errors.push(error.name + ':' + queueSize() + ':' + codec.state);
        log.push('error');
        if (onError) onError(log);
      },
    });
    codec.addEventListener('dequeue', () => {
      dequeues.push('dequeue:' + queueSize());
      log.push(dequeues.at(-1));
    });
    setup(codec);
    const queued = queueSize();
    for (let i = 0; i < 200 && !(errors.length > 0 && dequeues.includes('dequeue:0')); i++) await sleep(10);
    // Room for any extra event to arrive.
    await sleep(100);
    return { queued, state: codec.state, errors, dequeues, log };
  }
`

type Teardown = { queued: number; state: string; errors: string[]; dequeues: string[]; log: string[] }

// The events from the error on: Chrome logs the error, then one dequeue event that
// sees an empty queue.
const fromError = (log: string[]) => log.slice(log.indexOf('error'))

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
    t.deepEqual(entry.log, ['error', 'dequeue:0'], `${codec}: the error comes first, as in Chrome`)
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
  t.deepEqual(fromError(result.log), ['error', 'dequeue:0'], 'one dequeue event follows the error')
})

test('dequeue: a display aspect error on an output frame leaves the VideoDecoder queue empty', (t) => {
  // 1093:1 on 100x30 passes configure() but the output frame would be 32790 wide.
  // Chrome 155 (100 of 100 runs, 64 key chunks): dequeue:63, the error
  // (EncodingError) with a queue of 0 and state closed, then dequeue:0. 64 chunks
  // so that a decoder with frame threads still fails while chunks are queued.
  const result = runChild<Teardown>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${TEARDOWN}
    (async () => {
      const COUNT = 64;
      const chunks = [];
      const encoder = new m.VideoEncoder({ output: (chunk) => chunks.push(chunk), error: fail });
      encoder.configure({ codec: 'vp8', width: 100, height: 30, bitrate: 300000 });
      for (let i = 0; i < COUNT; i++) {
        const frame = new m.VideoFrame(new Uint8Array(100 * 30 * 4).fill(i), { format: 'RGBA', codedWidth: 100, codedHeight: 30, timestamp: i * 33333 });
        encoder.encode(frame, { keyFrame: true });
        frame.close();
      }
      await encoder.flush();
      encoder.close();
      done(await teardown(m.VideoDecoder, (decoder) => {
        decoder.configure({ codec: 'vp8', codedWidth: 100, codedHeight: 30, displayAspectWidth: 1093, displayAspectHeight: 1 });
        for (const chunk of chunks) decoder.decode(chunk);
      }));
    })().catch(fail);
  `)
  t.is(result.queued, 64, 'all chunks were queued')
  t.is(result.state, 'closed', 'the error closed the decoder')
  t.deepEqual(result.errors, ['EncodingError:0:closed'], 'one error, seeing an empty queue')
  t.is(result.dequeues.at(-1), 'dequeue:0', 'the last dequeue event sees an empty queue')
  t.deepEqual(fromError(result.log), ['error', 'dequeue:0'], 'one dequeue event follows the error')
})

test('dequeue: error callback microtasks precede one dequeue with the original payload still queued', (t) => {
  // Without a competing output callback, the error callback runs first and leaves
  // the original dispatcher payload queued. Both payloads must coalesce into one
  // event after the callback's microtasks.
  const result = runChild<Record<string, Teardown>>(`
    ${PRELUDE}
    const m = require('./index.js');
    ${TEARDOWN}
    const audioData = (i) =>
      new m.AudioData({ format: 'f32-planar', sampleRate: 48000, numberOfFrames: 960, numberOfChannels: 2, timestamp: i * 20000, data: new Float32Array(1920) });
    const SETUPS = {
      AudioDecoder(codec) {
        codec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
        for (let i = 0; i < N; i++) codec.decode(new m.EncodedAudioChunk({ type: 'key', timestamp: i, data: new Uint8Array(0) }));
      },
      VideoDecoder(codec, key) {
        codec.configure({ codec: 'vp8', codedWidth: 64, codedHeight: 64 });
        codec.decode(key);
        for (let i = 1; i <= N; i++) codec.decode(new m.EncodedVideoChunk({ type: 'delta', timestamp: i * 33333, data: new Uint8Array(0) }));
      },
      AudioEncoder(codec) {
        codec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
        codec.configure({ codec: 'opus', sampleRate: 44100, numberOfChannels: 2 });
        for (let i = 0; i < N; i++) { const data = audioData(i); codec.encode(data); data.close(); }
      },
      VideoEncoder(codec) {
        codec.configure({ codec: 'vp8', width: 16, height: 16, bitrate: 100000 });
        for (let i = 0; i < 3; i++) {
          const frame = m.createVideoFrameFromRGBA16Float(new Uint16Array(16 * 16 * 4).fill(0x3c00), { codedWidth: 16, codedHeight: 16, timestamp: i });
          codec.encode(frame);
          frame.close();
        }
      },
    };
    const queueMicrotasks = (log) => {
      queueMicrotask(() => log.push('microtask'));
      Promise.resolve().then(() => log.push('promise'));
    };
    (async () => {
      let key;
      const encoder = new m.VideoEncoder({ output: (chunk) => { key = chunk; }, error: fail });
      encoder.configure({ codec: 'vp8', width: 64, height: 64, bitrate: 200000 });
      const frame = new m.VideoFrame(new Uint8Array(64 * 64 * 4), { format: 'RGBA', codedWidth: 64, codedHeight: 64, timestamp: 0 });
      encoder.encode(frame, { keyFrame: true });
      frame.close();
      await encoder.flush();
      encoder.close();
      const out = {};
      for (const kind of CODECS) {
        out[kind] = await teardown(m[kind], (codec) => SETUPS[kind](codec, key), queueMicrotasks);
      }
      done(out);
    })().catch(fail);
  `)
  for (const kind of ['AudioDecoder', 'VideoDecoder', 'AudioEncoder', 'VideoEncoder']) {
    t.deepEqual(
      fromError(result[kind].log),
      ['error', 'microtask', 'promise', 'dequeue:0'],
      `${kind}: microtasks run between the error and its one dequeue event`,
    )
  }
})

test('dequeue: the error callback runs before the dequeue event for the work it dropped, under load', (t) => {
  // Chrome 155 (10 of 10 runs each, same triggers): AudioDecoder, AudioEncoder and
  // VideoEncoder log the error, then dequeue:0; VideoDecoder logs dequeue:6, the
  // error, then dequeue:0.
  //
  // The error and the dequeue event reach JS through two thread-safe functions.
  // Each failure here starts while the JS thread is busy with the output callbacks
  // of another encoder whose thread-safe function was created between the failing
  // codec's error and dequeue ones. The dispatcher consumes the original payload
  // while the error is pending. Check microtask ordering and exactly one replacement
  // event; the deadline only bounds a failing run.
  const result = runChild<Record<string, Record<string, number>>>(`
    ${PRELUDE}
    const m = require('./index.js');
    const N = 6;
    const ROUNDS = 5;
    const PER_KIND = 4;
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const audioData = (i, sampleRate = 48000) =>
      new m.AudioData({ format: 'f32-planar', sampleRate, numberOfFrames: 960, numberOfChannels: 2, timestamp: i * 20000, data: new Float32Array(1920) });
    const SETUPS = {
      AudioDecoder(codec) {
        codec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
        for (let i = 0; i < N; i++) codec.decode(new m.EncodedAudioChunk({ type: 'key', timestamp: i, data: new Uint8Array(0) }));
      },
      VideoDecoder(codec, key) {
        codec.configure({ codec: 'vp8', codedWidth: 64, codedHeight: 64 });
        codec.decode(key);
        for (let i = 1; i <= N; i++) codec.decode(new m.EncodedVideoChunk({ type: 'delta', timestamp: i * 33333, data: new Uint8Array(0) }));
      },
      AudioEncoder(codec) {
        codec.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
        codec.configure({ codec: 'opus', sampleRate: 44100, numberOfChannels: 2 });
        for (let i = 0; i < N; i++) { const data = audioData(i); codec.encode(data); data.close(); }
      },
      VideoEncoder(codec) {
        codec.configure({ codec: 'vp8', width: 16, height: 16, bitrate: 100000 });
        for (let i = 0; i < 3; i++) {
          const frame = m.createVideoFrameFromRGBA16Float(new Uint16Array(16 * 16 * 4).fill(0x3c00), { codedWidth: 16, codedHeight: 16, timestamp: i });
          codec.encode(frame);
          frame.close();
        }
      },
    };
    (async () => {
      let key;
      const encoder = new m.VideoEncoder({ output: (chunk) => { key = chunk; }, error: fail });
      encoder.configure({ codec: 'vp8', width: 64, height: 64, bitrate: 200000 });
      const frame = new m.VideoFrame(new Uint8Array(64 * 64 * 4), { format: 'RGBA', codedWidth: 64, codedHeight: 64, timestamp: 0 });
      encoder.encode(frame, { keyFrame: true });
      frame.close();
      await encoder.flush();
      encoder.close();
      const out = {};
      for (const kind of Object.keys(SETUPS)) out[kind] = {};
      // One budget for all rounds: a failing round uses it up, and the rest still report.
      const deadline = Date.now() + 40000;
      for (let round = 0; round < ROUNDS; round++) {
        const runs = [];
        for (let i = 0; i < PER_KIND; i++) {
          for (const kind of Object.keys(SETUPS)) {
            const run = { kind, log: [] };
            run.codec = new m[kind]({
              output: (o) => o.close?.(),
              error: () => {
                run.log.push('error');
                queueMicrotask(() => run.log.push('microtask'));
                Promise.resolve().then(() => run.log.push('promise'));
              },
            });
            runs.push(run);
          }
        }
        // Each output starts the next failure, then keeps the JS thread busy for a moment.
        let next = 0;
        const busy = new m.AudioEncoder({
          output: () => {
            if (next < runs.length) { const run = runs[next++]; SETUPS[run.kind](run.codec, key); }
            const until = performance.now() + 0.5;
            while (performance.now() < until) {}
          },
          error: fail,
        });
        busy.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2 });
        for (const run of runs) {
          const queueSize = () => run.codec.encodeQueueSize ?? run.codec.decodeQueueSize;
          run.codec.addEventListener('dequeue', () => run.log.push('dequeue:' + queueSize()));
        }
        // No flush: outputs during a flush are delivered by the flush, not by the
        // output callback's thread-safe function.
        for (let i = 0; i < runs.length * 3; i++) { const data = audioData(i); busy.encode(data); data.close(); }
        const finished = (run) => {
          const from = run.log.indexOf('error');
          return from >= 0 && ['microtask', 'promise', 'dequeue:0'].every((entry) => run.log.slice(from).includes(entry));
        };
        while (!runs.every(finished) && Date.now() < deadline) await sleep(10);
        // Room for any extra event to arrive.
        await sleep(100);
        busy.close();
        for (const run of runs) {
          const from = run.log.indexOf('error');
          const tail = from < 0 ? 'no error: ' + run.log.join(',') : run.log.slice(from).join(',');
          out[run.kind][tail] = (out[run.kind][tail] || 0) + 1;
        }
      }
      done(out);
    })().catch(fail);
  `)
  for (const kind of ['AudioDecoder', 'VideoDecoder', 'AudioEncoder', 'VideoEncoder']) {
    t.deepEqual(
      result[kind],
      { 'error,microtask,promise,dequeue:0': 20 },
      `${kind}: the consumed payload is replaced once, after the error callback's microtasks`,
    )
  }
})
