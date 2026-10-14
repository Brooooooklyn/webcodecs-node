/**
 * JS buffer lifetime and trust
 *
 * Native code must not keep a pointer into a JS buffer once JS can run again
 * (a later getter, a callback, returning to the event loop), and must locate a
 * view's bytes with engine calls, never with its overridable `buffer` /
 * `byteOffset` / `byteLength` properties.
 *
 * Crash cases run in a child process: shrinking a resizable ArrayBuffer
 * decommits its pages, so a stale native read faults there instead of quietly
 * reading freed memory, and the crash fails the test instead of killing the
 * ava worker. Expected browser behavior was checked in Chrome 155.
 */

import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import test from 'ava'

import {
  AudioData,
  AudioDecoder,
  EncodedAudioChunk,
  EncodedVideoChunk,
  ImageDecoder,
  MkvDemuxer,
  MkvMuxer,
  Mp4Demuxer,
  Mp4Muxer,
  VideoDecoder,
  WebMDemuxer,
  WebMMuxer,
} from '../index.js'

const __dirname = dirname(fileURLToPath(import.meta.url))
const PNG = readFileSync(join(__dirname, 'fixtures/wpt/four-colors.png'))
const H264_MP4 = readFileSync(join(__dirname, 'fixtures/wpt/h264.mp4'))

// The child's stdout carries the test result, so it must not inherit WEBCODECS_LOG
// (CI sets it, and the log output goes to stdout).
function childEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.WEBCODECS_LOG
  return env
}

// Runs `script` in a child process and returns the JSON it prints last.
function runChild(script: string): unknown {
  const stdout = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    env: childEnv(),
    timeout: 60_000,
    encoding: 'utf8',
  })
  return JSON.parse(stdout.trim().split('\n').pop()!)
}

// Helpers shared by the child scripts.
const CHILD_PRELUDE = `
  const fs = require('node:fs');
  const lib = require('./index.js');
  const resizable = (bytes) => {
    const buffer = new ArrayBuffer(bytes.length, { maxByteLength: bytes.length });
    const view = new Uint8Array(buffer);
    view.set(bytes);
    return view;
  };
  const detach = (buffer) => structuredClone(buffer, { transfer: [buffer] });
  const print = (value) => process.stdout.write(JSON.stringify(value) + '\\n');
`

function own(bytes: Uint8Array): Uint8Array {
  const copy = new Uint8Array(bytes.length)
  copy.set(bytes)
  return copy
}

function detach(buffer: ArrayBuffer): void {
  structuredClone(buffer, { transfer: [buffer] })
}

async function avcConfig() {
  const demuxer = new Mp4Demuxer({ videoOutput: () => {}, error: () => {} })
  await demuxer.loadBuffer(own(H264_MP4))
  const config = demuxer.videoDecoderConfig!
  demuxer.close()
  return { codec: config.codec, description: own(config.description!) }
}

async function decodeSize(decoder: ImageDecoder) {
  const result = await decoder.decode()
  const size = [result.image.codedWidth, result.image.codedHeight]
  result.image.close()
  decoder.close()
  return size
}

// ============================================================================
// Child process: buffers shrunk or detached while native code still needs them
// ============================================================================

test('Mp4Demuxer, MkvDemuxer and WebMDemuxer.loadBuffer copy the bytes', (t) => {
  const result = runChild(`${CHILD_PRELUDE}
    const { Mp4Demuxer, MkvDemuxer, WebMDemuxer, MkvMuxer } = lib;
    async function demux(Demuxer, bytes, change) {
      let packets = 0;
      const demuxer = new Demuxer({ videoOutput: () => packets++, audioOutput: () => packets++, error: (e) => { throw e; } });
      const view = resizable(bytes);
      const loading = demuxer.loadBuffer(view);
      if (change === 'detach while loading') detach(view.buffer);
      await loading;
      if (change === 'shrink after load') view.buffer.resize(0);
      await demuxer.demuxAsync();
      demuxer.close();
      return packets;
    }
    (async () => {
      const mp4 = fs.readFileSync('__test__/fixtures/small_buck_bunny.mp4');
      // A large Matroska file (remuxed H.264, no encoding), so demuxing reads
      // well past what loading parsed.
      const chunks = [];
      const source = new Mp4Demuxer({ videoOutput: (chunk) => chunks.push(chunk), error: (e) => { throw e; } });
      await source.loadBuffer(new Uint8Array(mp4));
      const config = source.videoDecoderConfig;
      await source.demuxAsync();
      source.close();
      const muxer = new MkvMuxer();
      muxer.addVideoTrack({ codec: config.codec, width: config.codedWidth, height: config.codedHeight, description: config.description });
      for (const chunk of chunks) muxer.addVideoChunk(chunk);
      await muxer.flush();
      const mkv = muxer.finalize();
      muxer.close();
      const out = {};
      for (const [name, Demuxer, bytes] of [['mp4', Mp4Demuxer, mp4], ['mkv', MkvDemuxer, mkv], ['webm', WebMDemuxer, mkv]]) {
        out[name] = [];
        for (const change of ['none', 'detach while loading', 'shrink after load']) out[name].push(await demux(Demuxer, bytes, change));
      }
      print(out);
    })();
  `) as Record<string, number[]>
  for (const [name, counts] of Object.entries(result)) {
    t.true(counts[0] > 0, name)
    t.deepEqual(counts, [counts[0], counts[0], counts[0]], name)
  }
})

test('loadBuffer rejects a loaded or closed demuxer without copying the input first', (t) => {
  // Peak RSS shows whether the rejected call copied the 256 MiB input: the
  // copy would raise it by 256 MiB, nothing else in the child comes close.
  const result = runChild(`${CHILD_PRELUDE}
    const { Mp4Demuxer } = lib;
    (async () => {
      const demuxer = new Mp4Demuxer({ videoOutput: () => {}, error: () => {} });
      await demuxer.loadBuffer(new Uint8Array(fs.readFileSync('__test__/fixtures/wpt/h264.mp4')));
      const big = new Uint8Array(256 * 1024 * 1024).fill(1);
      const peakBefore = process.resourceUsage().maxRSS;
      const loaded = await demuxer.loadBuffer(big).then(() => 'resolved', (e) => e.message);
      demuxer.close();
      const closed = await demuxer.loadBuffer(big).then(() => 'resolved', (e) => e.message);
      const peakGrowthMiB = Math.round((process.resourceUsage().maxRSS - peakBefore) / 1024);
      print({ loaded, closed, copied: peakGrowthMiB >= 128 });
    })();
  `)
  t.deepEqual(result, {
    loaded: 'Demuxer already loaded. Call close() first.',
    closed: 'Demuxer already loaded. Call close() first.',
    copied: false,
  })
})

test('ImageDecoder copies BufferSource data and ReadableStream chunks before JS runs again', (t) => {
  const result = runChild(`${CHILD_PRELUDE}
    const { ImageDecoder } = lib;
    const size = async (decoder) => {
      const { image } = await decoder.decode();
      const out = [image.codedWidth, image.codedHeight];
      image.close();
      decoder.close();
      return out;
    };
    (async () => {
      const png = fs.readFileSync('__test__/fixtures/wpt/four-colors.png');
      const gif = fs.readFileSync('__test__/fixtures/wpt/four-colors.gif');
      const view = resizable(png);
      const buffered = new ImageDecoder({ data: view, type: 'image/png' });
      view.buffer.resize(0);
      // highWaterMark 0: each chunk answers a pending read, and the producer
      // shrinks it two microtasks later, before the decode task copies it.
      const chunks = [];
      for (let i = 0; i < gif.length; i += 512) chunks.push(resizable(gif.subarray(i, i + 512)));
      let next = 0;
      const stream = new ReadableStream({
        pull(controller) {
          if (next === chunks.length) return controller.close();
          const chunk = chunks[next++];
          controller.enqueue(chunk);
          queueMicrotask(() => queueMicrotask(() => chunk.buffer.resize(0)));
        },
      }, { highWaterMark: 0 });
      const streamed = new ImageDecoder({ data: stream, type: 'image/gif' });
      await streamed.completed;
      print({ buffered: await size(buffered), streamed: await size(streamed) });
    })();
  `)
  t.deepEqual(result, { buffered: [320, 240], streamed: [320, 240] })
})

test('a description shrunk by a later config getter is never read from freed pages', (t) => {
  const result = runChild(`${CHILD_PRELUDE}
    const { Mp4Demuxer, VideoDecoder, Mp4Muxer, WebMMuxer, EncodedVideoChunk } = lib;
    (async () => {
      const source = new Mp4Demuxer({ videoOutput: () => {}, error: () => {} });
      await source.loadBuffer(new Uint8Array(fs.readFileSync('__test__/fixtures/wpt/h264.mp4')));
      const { codec, description } = source.videoDecoderConfig;
      source.close();
      const out = {};

      // Each getter below runs after the description was read and shrinks it.
      // The decoder copies the description after its getters ran (like
      // Chrome), the muxers when they read it: neither reads the freed pages.
      let desc = resizable(description);
      const decoder = new VideoDecoder({ output: (frame) => frame.close(), error: () => {} });
      decoder.configure({ codec, description: desc, get rotation() { desc.buffer.resize(0); return 0; } });
      out.decoder = decoder.state;
      decoder.close();

      desc = resizable(description);
      const mp4 = new Mp4Muxer();
      mp4.addVideoTrack({ codec, width: 320, height: 240, description: desc, get sampleEntry() { desc.buffer.resize(0); return undefined; } });
      desc = resizable(description);
      mp4.addVideoChunk(new EncodedVideoChunk({ type: 'key', timestamp: 0, data: new Uint8Array(16) }), {
        decoderConfig: { codec, description: desc, get codedWidth() { desc.buffer.resize(0); return 320; } },
      });
      mp4.close();
      out.mp4 = 'ok';

      desc = resizable(new Uint8Array(64).fill(1));
      const webm = new WebMMuxer();
      webm.addVideoTrack({ codec: 'vp8', width: 320, height: 240, description: desc, get alpha() { desc.buffer.resize(0); return false; } });
      webm.close();
      out.webm = 'ok';
      print(out);
    })();
  `)
  t.deepEqual(result, { decoder: 'configured', mp4: 'ok', webm: 'ok' })
})

test('copyTo ignores an oversized own byteLength and never calls destination getters', (t) => {
  // Before the fix an oversized byteLength made Rust slice past the buffer and
  // abort the process, and a byteLength getter that closed the AudioData
  // deadlocked on the AudioData lock.
  const result = runChild(`${CHILD_PRELUDE}
    const { AudioData, EncodedVideoChunk, EncodedAudioChunk } = lib;
    const sources = () => ({
      audio: new AudioData({ data: new Float32Array(16).fill(0.5), format: 'f32', sampleRate: 48000, numberOfFrames: 16, numberOfChannels: 1, timestamp: 0 }),
      video: new EncodedVideoChunk({ type: 'key', timestamp: 0, data: new Uint8Array(64).fill(9) }),
      audioChunk: new EncodedAudioChunk({ type: 'key', timestamp: 0, data: new Uint8Array(64).fill(9) }),
    });
    const copy = (source, destination) => source instanceof AudioData ? source.copyTo(destination, { planeIndex: 0 }) : source.copyTo(destination);
    const attempt = (fn) => { try { fn(); return 'ok'; } catch (e) { return e.name; } };
    const out = {};
    for (const [name, source] of Object.entries(sources())) {
      const backing = new Uint8Array(4096).fill(7);
      const view = backing.subarray(0, 64);
      Object.defineProperty(view, 'byteLength', { value: 4096 });
      const lyingView = attempt(() => copy(source, view));
      const small = new ArrayBuffer(16);
      Object.defineProperty(small, 'byteLength', { value: 4096 });
      out[name] = { lyingView, wroteOutside: backing.subarray(64).some((b) => b !== 7), lyingArrayBuffer: attempt(() => copy(source, small)) };
    }
    const audio = sources().audio;
    const view = new Uint8Array(64);
    Object.defineProperty(view, 'byteLength', { get() { audio.close(); return 64; } });
    out.closingGetter = attempt(() => copy(audio, view));
    print(out);
  `)
  t.deepEqual(result, {
    audio: { lyingView: 'ok', wroteOutside: false, lyingArrayBuffer: 'RangeError' },
    video: { lyingView: 'ok', wroteOutside: false, lyingArrayBuffer: 'TypeError' },
    audioChunk: { lyingView: 'ok', wroteOutside: false, lyingArrayBuffer: 'TypeError' },
    closingGetter: 'ok',
  })
})

// ============================================================================
// In process: view geometry comes from the engine, not from JS properties
// ============================================================================

function copySources() {
  return {
    AudioData: new AudioData({
      data: new Float32Array(16).fill(0.5),
      format: 'f32',
      sampleRate: 48000,
      numberOfFrames: 16,
      numberOfChannels: 1,
      timestamp: 0,
    }),
    EncodedVideoChunk: new EncodedVideoChunk({ type: 'key', timestamp: 0, data: new Uint8Array(64).fill(9) }),
    EncodedAudioChunk: new EncodedAudioChunk({ type: 'key', timestamp: 0, data: new Uint8Array(64).fill(9) }),
  }
}

function copyInto(source: AudioData | EncodedVideoChunk | EncodedAudioChunk, destination: unknown) {
  if (source instanceof AudioData) {
    source.copyTo(destination as Uint8Array, { planeIndex: 0 })
  } else {
    source.copyTo(destination as Uint8Array)
  }
}

test('copyTo writes into the real view whatever its own properties claim', (t) => {
  for (const [name, source] of Object.entries(copySources())) {
    for (const spoof of ['byteOffset', 'buffer'] as const) {
      const backing = new Uint8Array(512).fill(7)
      const view = backing.subarray(0, 64)
      const elsewhere = new Uint8Array(512)
      Object.defineProperty(view, spoof, { value: spoof === 'byteOffset' ? 256 : elsewhere.buffer })
      copyInto(source, view)
      t.true(
        view.every((b) => b !== 7),
        `${name} ${spoof}: view written`,
      )
      t.true(
        backing.subarray(64).every((b) => b === 7),
        `${name} ${spoof}: nothing written after the view`,
      )
      t.true(
        elsewhere.every((b) => b === 0),
        `${name} ${spoof}: other buffer untouched`,
      )
    }
  }
})

test('copyTo accepts any BufferSource and rejects look-alike objects', (t) => {
  for (const [name, source] of Object.entries(copySources())) {
    const expected = new Uint8Array(64)
    copyInto(source, expected)
    const backing = new ArrayBuffer(256)
    for (const view of [new DataView(backing, 100, 64), new Uint16Array(backing, 100, 32)]) {
      const bytes = new Uint8Array(backing).fill(0xee)
      copyInto(source, view)
      t.deepEqual(Array.from(bytes.subarray(100, 164)), Array.from(expected), `${name}: view written`)
      t.true(
        bytes.subarray(0, 100).every((b) => b === 0xee),
        `${name}: nothing written before the view`,
      )
      t.true(
        bytes.subarray(164).every((b) => b === 0xee),
        `${name}: nothing written after the view`,
      )
    }
    t.throws(
      () => copyInto(source, { buffer: backing, byteOffset: 0, byteLength: 64 }),
      { instanceOf: TypeError },
      name,
    )
  }
})

test('chunk and AudioData constructors copy the real view bytes', (t) => {
  const backing = new Uint8Array(256).map((_, i) => i)
  const views = () => {
    const f32 = new Float32Array(backing.buffer, 0, 16)
    Object.defineProperty(f32, 'byteOffset', { value: 128 })
    const dataView = new DataView(backing.buffer, 0, 64)
    Object.defineProperty(dataView, 'buffer', { value: new ArrayBuffer(256) })
    return [f32, dataView]
  }
  for (const data of views()) {
    for (const Chunk of [EncodedVideoChunk, EncodedAudioChunk]) {
      const chunk = new Chunk({ type: 'key', timestamp: 0, data })
      const out = new Uint8Array(chunk.byteLength)
      chunk.copyTo(out)
      t.deepEqual(Array.from(out), Array.from(backing.subarray(0, 64)), Chunk.name)
    }
    const audio = new AudioData({
      data,
      format: 'u8',
      sampleRate: 48000,
      numberOfFrames: 64,
      numberOfChannels: 1,
      timestamp: 0,
    })
    const out = new Uint8Array(64)
    audio.copyTo(out, { planeIndex: 0 })
    t.deepEqual(Array.from(out), Array.from(backing.subarray(0, 64)), 'AudioData')
    audio.close()
  }
  const fake = { buffer: backing.buffer, byteOffset: 0, byteLength: 64 }
  t.throws(() => new EncodedVideoChunk({ type: 'key', timestamp: 0, data: fake as unknown as Uint8Array }), {
    instanceOf: TypeError,
  })
  t.throws(() => new EncodedAudioChunk({ type: 'key', timestamp: 0, data: fake as unknown as Uint8Array }), {
    instanceOf: TypeError,
  })
  t.throws(
    () =>
      new AudioData({
        data: fake as unknown as Uint8Array,
        format: 'u8',
        sampleRate: 48000,
        numberOfFrames: 64,
        numberOfChannels: 1,
        timestamp: 0,
      }),
    { instanceOf: TypeError },
  )
})

// Node.js before 24.13 (and Node.js 22 with --js-float16array) has Float16Array
// but does not report its type to native code, so its byte length is unknown
// there: such a view must be rejected, never read at a guessed length.
test('a Float16Array is read at its full byte length, or rejected where Node-API cannot size it', (t) => {
  const Float16 = (
    globalThis as { Float16Array?: new (buffer: ArrayBuffer, byteOffset: number, length: number) => ArrayBufferView }
  ).Float16Array
  if (!Float16) {
    t.pass('this runtime has no Float16Array')
    return
  }
  const backing = new Uint8Array(64).map((_, i) => i)
  const sized = (fn: () => number) => {
    try {
      return fn()
    } catch (e) {
      t.true(e instanceof TypeError, String(e))
      return 'TypeError'
    }
  }
  const read = [
    sized(
      () => new EncodedVideoChunk({ type: 'key', timestamp: 0, data: new Float16(backing.buffer, 0, 8) }).byteLength,
    ),
    sized(
      () => new EncodedAudioChunk({ type: 'key', timestamp: 0, data: new Float16(backing.buffer, 0, 8) }).byteLength,
    ),
  ]
  const chunk = new EncodedVideoChunk({ type: 'key', timestamp: 0, data: backing.subarray(0, 16) })
  const destination = new Uint8Array(32)
  const written = sized(() => {
    chunk.copyTo(new Float16(destination.buffer, 0, 8))
    return destination.findLastIndex((b) => b !== 0) + 1
  })
  const reported = typeof read[0] === 'number'
  t.deepEqual(read, reported ? [16, 16] : ['TypeError', 'TypeError'])
  t.is(written, reported ? 16 : 'TypeError')
})

// ============================================================================
// ImageDecoder: copy at construction, like Chrome
// ============================================================================

test('ImageDecoder keeps its own copy of BufferSource data', async (t) => {
  const changed = own(PNG)
  const mutated = new ImageDecoder({ data: changed, type: 'image/png' })
  changed.fill(0)
  t.deepEqual(await decodeSize(mutated), [320, 240])

  const gone = own(PNG)
  const detachedAfter = new ImageDecoder({ data: gone, type: 'image/png' })
  detach(gone.buffer as ArrayBuffer)
  t.deepEqual(await decodeSize(detachedAfter), [320, 240])

  t.deepEqual(
    await decodeSize(new ImageDecoder({ data: own(PNG).buffer as ArrayBuffer, type: 'image/png' })),
    [320, 240],
  )
  t.deepEqual(
    await decodeSize(new ImageDecoder({ data: new DataView(own(PNG).buffer as ArrayBuffer), type: 'image/png' })),
    [320, 240],
  )

  const detachedBefore = own(PNG)
  detach(detachedBefore.buffer as ArrayBuffer)
  t.throws(() => new ImageDecoder({ data: detachedBefore, type: 'image/png' }), {
    instanceOf: TypeError,
    message: 'No image data provided',
  })
})

// ============================================================================
// Decoder configs: description is copied, and a detached one is invalid
// ============================================================================

test('VideoDecoder and AudioDecoder reject a description detached by a later getter', async (t) => {
  const { codec, description } = await avcConfig()

  const videoDecoder = new VideoDecoder({ output: (frame) => frame.close(), error: () => {} })
  const videoDescription = own(description)
  t.throws(
    () =>
      videoDecoder.configure({
        codec,
        description: videoDescription,
        get rotation() {
          detach(videoDescription.buffer as ArrayBuffer)
          return 0
        },
      } as VideoDecoderConfigWithRotation),
    { instanceOf: TypeError, message: 'Invalid config, description is detached.' },
  )
  t.is(videoDecoder.state, 'unconfigured')
  videoDecoder.close()

  const detachedDescription = own(description)
  detach(detachedDescription.buffer as ArrayBuffer)
  await t.throwsAsync(VideoDecoder.isConfigSupported({ codec, description: detachedDescription }), {
    instanceOf: TypeError,
  })

  const audioDecoder = new AudioDecoder({ output: (data) => data.close(), error: () => {} })
  const audioDescription = new Uint8Array([0x12, 0x10])
  t.throws(
    () =>
      audioDecoder.configure({
        codec: 'mp4a.40.2',
        get sampleRate() {
          detach(audioDescription.buffer as ArrayBuffer)
          return 44100
        },
        numberOfChannels: 2,
        description: audioDescription,
      }),
    { instanceOf: TypeError, message: 'Invalid config; description is detached.' },
  )
  audioDecoder.close()
})

type VideoDecoderConfigWithRotation = Parameters<VideoDecoder['configure']>[0] & { rotation?: number }

test('AudioDecoder rejects description: null with a TypeError, like Chrome', async (t) => {
  const config = { codec: 'opus', sampleRate: 48000, numberOfChannels: 2 }
  const nullDescription = { ...config, description: null as unknown as Uint8Array }
  const decoder = new AudioDecoder({ output: (data) => data.close(), error: () => {} })
  t.throws(() => decoder.configure(nullDescription), { instanceOf: TypeError })
  t.is(decoder.state, 'unconfigured')
  // An absent description is still fine.
  decoder.configure({ ...config, description: undefined })
  t.is(decoder.state, 'configured')
  decoder.close()
  // Chrome rejects; the async wrapper also accepts a synchronous throw.
  await t.throwsAsync(async () => AudioDecoder.isConfigSupported(nullDescription), { instanceOf: TypeError })
})

test('VideoDecoder copies the description after the other config getters ran, like Chrome', async (t) => {
  const { codec, description } = await avcConfig()
  // Chrome 155 reads every member first and copies the bytes when it uses the
  // config, so a getter that runs after description was read changes them.
  for (const getter of ['codedWidth', 'optimizeForLatency', 'rotation', 'flip']) {
    const changed = own(description)
    const config = { codec, description: changed }
    Object.defineProperty(config, getter, {
      enumerable: true,
      get() {
        changed[0] = 0x77
        return undefined
      },
    })
    const support = await VideoDecoder.isConfigSupported(config)
    t.is(new Uint8Array(support.config.description as ArrayBuffer)[0], 0x77, getter)
  }
})

test('isConfigSupported returns a copy of the description, as an ArrayBuffer', async (t) => {
  const { codec, description } = await avcConfig()
  const video = await VideoDecoder.isConfigSupported({ codec, description })
  t.true(video.config.description instanceof ArrayBuffer)
  t.deepEqual(Array.from(new Uint8Array(video.config.description as ArrayBuffer)), Array.from(description))

  const audioDescription = new Uint8Array([0x12, 0x10])
  const pending = AudioDecoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 44100,
    numberOfChannels: 2,
    description: new DataView(audioDescription.buffer),
  })
  audioDescription[0] = 0xff
  const audio = await pending
  t.true(audio.config.description instanceof ArrayBuffer)
  t.deepEqual(Array.from(new Uint8Array(audio.config.description as ArrayBuffer)), [0x12, 0x10])
})

// ============================================================================
// Muxers: a description from isConfigSupported (an ArrayBuffer) still muxes
// ============================================================================

test('muxers take a description from isConfigSupported (an ArrayBuffer) or any other BufferSource', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const source = new Mp4Demuxer({ videoOutput: (chunk) => chunks.push(chunk), error: () => {} })
  await source.loadBuffer(own(H264_MP4))
  const { codec, codedWidth, codedHeight, description } = source.videoDecoderConfig!
  await source.demuxAsync()
  source.close()
  const support = await VideoDecoder.isConfigSupported({ codec, codedWidth, codedHeight, description })
  t.true(support.config.description instanceof ArrayBuffer)
  const width = codedWidth!
  const height = codedHeight!

  // The description the muxer wrote is the one the demuxer reads back.
  const forms = {
    ArrayBuffer: support.config.description as ArrayBuffer,
    DataView: new DataView(own(description!).buffer),
    Uint16Array: new Uint16Array(own(description!).buffer, 0, description!.length / 2),
  }
  for (const [name, form] of Object.entries(forms)) {
    const expected = Array.from(own(description!).subarray(0, form.byteLength))
    const mp4 = new Mp4Muxer()
    mp4.addVideoTrack({ codec, width, height, description: form })
    chunks.forEach((chunk, i) => mp4.addVideoChunk(chunk, i === 0 ? { decoderConfig: support.config } : undefined))
    mp4.flush()
    const mp4Bytes = mp4.finalize()
    mp4.close()
    const mkv = new MkvMuxer()
    mkv.addVideoTrack({ codec, width, height, description: form })
    chunks.forEach((chunk) => mkv.addVideoChunk(chunk))
    mkv.flush()
    const mkvBytes = mkv.finalize()
    mkv.close()
    for (const [Demuxer, bytes] of [
      [Mp4Demuxer, mp4Bytes],
      [MkvDemuxer, mkvBytes],
    ] as const) {
      const demuxer = new Demuxer({ videoOutput: () => {}, error: () => {} })
      await demuxer.loadBuffer(bytes)
      t.deepEqual(Array.from(demuxer.videoDecoderConfig!.description!), expected, `${Demuxer.name} ${name}`)
      demuxer.close()
    }
  }

  const audioSupport = await AudioDecoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 44100,
    numberOfChannels: 2,
    description: new Uint8Array([0x12, 0x10]),
  })
  const mp4 = new Mp4Muxer()
  t.notThrows(() =>
    mp4.addAudioTrack({
      codec: 'mp4a.40.2',
      sampleRate: 44100,
      numberOfChannels: 2,
      description: audioSupport.config.description,
    }),
  )
  mp4.close()
  const opusHead = new Uint8Array([
    0x4f, 0x70, 0x75, 0x73, 0x48, 0x65, 0x61, 0x64, 1, 2, 0x38, 1, 0x80, 0xbb, 0, 0, 0, 0, 0,
  ])
  const webm = new WebMMuxer()
  t.notThrows(() =>
    webm.addAudioTrack({ codec: 'opus', sampleRate: 48000, numberOfChannels: 2, description: opusHead.buffer }),
  )
  webm.close()

  const rejects = new Mp4Muxer()
  t.throws(() => rejects.addVideoTrack({ codec, width, height, description: {} as ArrayBuffer }), {
    message: /Expected an ArrayBuffer, SharedArrayBuffer, TypedArray or DataView/,
  })
  rejects.close()
})

test('loadBuffer keeps its argument check and its rejection for every demuxer', async (t) => {
  const mkvMuxer = new MkvMuxer()
  const { codec, description } = await avcConfig()
  mkvMuxer.addVideoTrack({ codec, width: 320, height: 240, description })
  mkvMuxer.addVideoChunk(new EncodedVideoChunk({ type: 'key', timestamp: 0, data: new Uint8Array(16) }))
  mkvMuxer.flush()
  const mkv = mkvMuxer.finalize()
  mkvMuxer.close()
  for (const [Demuxer, bytes] of [
    [Mp4Demuxer, own(H264_MP4)],
    [MkvDemuxer, mkv],
    [WebMDemuxer, mkv],
  ] as const) {
    const demuxer = new Demuxer({ videoOutput: () => {}, error: () => {} })
    // Not a Uint8Array: a synchronous throw, as before.
    t.throws(() => demuxer.loadBuffer(new Uint16Array(4) as unknown as Uint8Array), {
      message: 'Expected a Uint8Array',
    })
    await demuxer.loadBuffer(bytes)
    t.truthy(demuxer.videoDecoderConfig, Demuxer.name)
    t.throws(() => demuxer.loadBuffer({} as Uint8Array), { message: 'Expected a Uint8Array' })
    await t.throwsAsync(demuxer.loadBuffer(bytes), { message: 'Demuxer already loaded. Call close() first.' })
    demuxer.close()
    await t.throwsAsync(demuxer.loadBuffer(bytes), { message: 'Demuxer already loaded. Call close() first.' })
  }
})
