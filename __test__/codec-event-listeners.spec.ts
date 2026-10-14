/**
 * Listener storage.
 *
 * Codec event listeners and VideoFrame close observers are kept alive by a store
 * that user code cannot reach, so it can neither drop them nor make them leak.
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
