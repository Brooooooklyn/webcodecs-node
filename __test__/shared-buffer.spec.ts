import { execFileSync } from 'node:child_process'
import { Worker } from 'node:worker_threads'

import test from 'ava'

import { AudioData, EncodedAudioChunk, EncodedVideoChunk } from '../index.js'

function bytes(length: number): Uint8Array {
  return Uint8Array.from({ length }, (_, i) => (i * 37 + 11) & 255)
}

// Exercise direct SABs at runtime without widening BufferSource declarations.
const asBufferSource = (buffer: SharedArrayBuffer) => buffer as unknown as ArrayBuffer

test.serial('direct SharedArrayBuffer and shared views are accepted for input and output', (t) => {
  const source = bytes(16)
  const shared = new SharedArrayBuffer(source.length)
  new Uint8Array(shared).set(source)

  const audio = new AudioData({
    data: asBufferSource(shared),
    format: 'u8',
    sampleRate: 48000,
    numberOfFrames: source.length,
    numberOfChannels: 1,
    timestamp: 0,
  })
  const audioOutput = new Uint8Array(new SharedArrayBuffer(source.length))
  audio.copyTo(audioOutput, { planeIndex: 0, format: 'u8' })
  t.deepEqual(Array.from(audioOutput), Array.from(source))
  const convertedOutput = new SharedArrayBuffer(source.length * 2)
  audio.copyTo(convertedOutput, { planeIndex: 0, format: 's16' })
  t.deepEqual(
    Array.from(new Int16Array(convertedOutput)),
    Array.from(source, (sample) => (sample - 128) * 256),
  )
  audio.close()

  const audioChunk = new EncodedAudioChunk({ type: 'key', timestamp: 0, data: asBufferSource(shared) })
  const audioChunkOutput = new DataView(new SharedArrayBuffer(source.length))
  audioChunk.copyTo(audioChunkOutput)
  t.deepEqual(Array.from(new Uint8Array(audioChunkOutput.buffer)), Array.from(source))

  const inputBacking = new SharedArrayBuffer(source.length + 8)
  new Uint8Array(inputBacking).set(source, 4)
  const videoChunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 0,
    data: new DataView(inputBacking, 4, source.length),
  })
  const outputBacking = new SharedArrayBuffer(source.length + 8)
  const outputBytes = new Uint8Array(outputBacking).fill(0xee)
  videoChunk.copyTo(new Uint8Array(outputBacking, 4, source.length))
  t.deepEqual(Array.from(outputBytes.subarray(4, 4 + source.length)), Array.from(source))
  t.true(outputBytes.subarray(0, 4).every((byte) => byte === 0xee))
  t.true(outputBytes.subarray(4 + source.length).every((byte) => byte === 0xee))
})

test.serial('shared copies do not call a later Uint8Array.prototype.set replacement', (t) => {
  const source = bytes(32)
  const shared = new SharedArrayBuffer(source.length)
  new Uint8Array(shared).set(source)
  const output = new Uint8Array(new SharedArrayBuffer(source.length))
  const descriptor = Object.getOwnPropertyDescriptor(Uint8Array.prototype, 'set')
  let calls = 0
  Object.defineProperty(Uint8Array.prototype, 'set', {
    configurable: true,
    writable: true,
    value() {
      calls++
      throw new Error('user set called')
    },
  })
  try {
    const chunk = new EncodedAudioChunk({ type: 'key', timestamp: 0, data: asBufferSource(shared) })
    chunk.copyTo(output)
    t.deepEqual(Array.from(output), Array.from(source))
    t.is(calls, 0)
  } finally {
    if (descriptor) Object.defineProperty(Uint8Array.prototype, 'set', descriptor)
    else Reflect.deleteProperty(Uint8Array.prototype, 'set')
  }
})

test.serial('shared copies keep working after a second native load', (t) => {
  const output = execFileSync(
    process.execPath,
    [
      '-e',
      `const fs = require('node:fs');
       const first = require('./index.js');
       const file = Object.keys(require.cache).find((name) => name.endsWith('.node'));
       const second = { exports: {} };
       process.dlopen(second, file);
       const shared = new SharedArrayBuffer(8);
       new Uint8Array(shared).fill(7);
       const chunk = new first.EncodedAudioChunk({ type: 'key', timestamp: 0, data: shared });
       const output = new Uint8Array(new SharedArrayBuffer(8));
       chunk.copyTo(output);
       fs.writeSync(1, String(output[0]));`,
    ],
    { cwd: process.cwd(), encoding: 'utf8', timeout: 60_000 },
  )
  t.is(output, '7')
})

for (const kind of ['Int32Array', 'BigInt64Array'] as const) {
  test.serial(`shared input and output tolerate concurrent ${kind} worker reads and writes`, async (t) => {
    const source = bytes(4096)
    const shared = new SharedArrayBuffer(source.length)
    new Uint8Array(shared).set(source)
    const output = new Uint8Array(new SharedArrayBuffer(source.length))
    const state = new Int32Array(new SharedArrayBuffer(8))
    const worker = new Worker(
      `const { workerData } = require('node:worker_threads');
       const input = workerData.kind === 'Int32Array' ? new Int32Array(workerData.input) : new BigInt64Array(workerData.input);
       const output = workerData.kind === 'Int32Array' ? new Int32Array(workerData.output) : new BigInt64Array(workerData.output);
       const state = new Int32Array(workerData.state);
       let i = 0;
       while (Atomics.load(state, 0) !== 2) {
         const index = i % input.length;
         const value = workerData.kind === 'Int32Array' ? i : BigInt(i);
         Atomics.store(input, index, value);
         Atomics.store(output, index, Atomics.load(output, index) ^ value);
         Atomics.add(state, 1, 1);
         i++;
         if (i === 1) {
           Atomics.store(state, 0, 1);
           Atomics.notify(state, 0);
         }
       }`,
      { eval: true, workerData: { input: shared, output: output.buffer, state: state.buffer, kind } },
    )
    let audio: AudioData | undefined
    try {
      Atomics.wait(state, 0, 0, 5000)
      t.is(Atomics.load(state, 0), 1, 'worker started')
      audio = new AudioData({
        data: asBufferSource(shared),
        format: 'u8',
        sampleRate: 48000,
        numberOfFrames: source.length,
        numberOfChannels: 1,
        timestamp: 0,
      })
      for (let i = 0; i < 500; i++) {
        const chunk = new EncodedAudioChunk({ type: 'key', timestamp: 0, data: asBufferSource(shared) })
        chunk.copyTo(output)
        const video = new EncodedVideoChunk({ type: 'key', timestamp: 0, data: new DataView(shared) })
        video.copyTo(output)
        audio.copyTo(output, { planeIndex: 0, format: 'u8' })
      }
      t.true(Atomics.load(state, 1) > 0, 'worker accessed both shared buffers')
    } finally {
      Atomics.store(state, 0, 2)
      await worker.terminate()
      audio?.close()
    }
  })
}
