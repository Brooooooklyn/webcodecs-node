/**
 * Codec teardown tests: close()/reset() preempt queued work, flush() after an
 * error, and memory a closed encoder keeps.
 *
 * Expected behavior is what Chrome 155 does:
 * - close()/reset() return without waiting for queued work. No output or error
 *   callback for earlier work runs afterwards; one dequeue event follows when
 *   work was still queued or a dequeue event was still on its way.
 * - A codec only counts as closed by an error once its error callback ran.
 *   Before that, close()/reset() succeed and drop the error, and a flush fails
 *   with that error. After it, flush() fails with InvalidStateError.
 * - Except: reset() keeps the NotSupportedError configure() reports for an
 *   unsupported config. It still arrives and closes the codec; close() drops it.
 */

import { execFileSync } from 'node:child_process'

import test from 'ava'

import {
  AudioData,
  AudioDecoder,
  AudioEncoder,
  EncodedAudioChunk,
  EncodedVideoChunk,
  VideoDecoder,
  VideoEncoder,
  VideoFrame,
  createVideoFrameFromRGBA16Float,
  type AudioDecoderConfig,
  type VideoDecoderConfig,
} from '../index.js'
import { delay, waitFor } from './helpers/index.js'

interface TeardownCodec {
  readonly state: string
  readonly encodeQueueSize?: number
  readonly decodeQueueSize?: number
  ondequeue: ((event: Event) => unknown) | null
  close(): void
  reset(): void
  flush(): Promise<void>
}

// Lets the deferred send microtasks hand queued work to the worker.
async function sendQueued() {
  await Promise.resolve()
  await Promise.resolve()
}

// Blocks the JS thread without running the event loop, so the worker can
// finish work whose callbacks cannot be delivered yet.
function blockJsThread(ms: number) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms)
}

function i420Frame(width: number, height: number, index: number) {
  const data = new Uint8Array((width * height * 3) / 2)
  for (let i = 0; i < data.length; i++) data[i] = (i * 7 + index * 13) & 0xff
  return new VideoFrame(data, { format: 'I420', codedWidth: width, codedHeight: height, timestamp: index * 33_333 })
}

function audioData(index: number) {
  const frames = 960
  const data = new Float32Array(frames * 2)
  for (let i = 0; i < data.length; i++) data[i] = Math.sin((index * frames + i) * 0.01) * 0.5
  return new AudioData({
    format: 'f32',
    sampleRate: 48_000,
    numberOfChannels: 2,
    numberOfFrames: frames,
    timestamp: index * 20_000,
    data,
  })
}

async function encodeVp8Chunks(count: number) {
  const chunks: EncodedVideoChunk[] = []
  let config: VideoDecoderConfig | undefined
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      config ??= metadata?.decoderConfig as VideoDecoderConfig | undefined
    },
    error: (error) => {
      throw error
    },
  })
  encoder.configure({ codec: 'vp8', width: 320, height: 240, hardwareAcceleration: 'prefer-software' })
  for (let i = 0; i < count; i++) {
    const frame = i420Frame(320, 240, i)
    encoder.encode(frame, { keyFrame: true })
    frame.close()
  }
  await encoder.flush()
  encoder.close()
  return { chunks, config: config! }
}

async function encodeOpusChunks(count: number) {
  const chunks: EncodedAudioChunk[] = []
  let config: AudioDecoderConfig | undefined
  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      config ??= metadata?.decoderConfig as AudioDecoderConfig | undefined
    },
    error: (error) => {
      throw error
    },
  })
  encoder.configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 })
  for (let i = 0; i < count; i++) {
    const data = audioData(i)
    encoder.encode(data)
    data.close()
  }
  await encoder.flush()
  encoder.close()
  return { chunks, config: config! }
}

type Record = (entry: string) => void

interface CodecCase {
  name: string
  // Creates a configured codec whose callbacks write to `record`.
  make(record: Record): TeardownCodec
  // Queues work synchronously.
  queue(codec: TeardownCodec): void
  // Queues one work item synchronously.
  queueOne(codec: TeardownCodec): void
}

async function codecCases(): Promise<CodecCase[]> {
  const video = await encodeVp8Chunks(12)
  const audio = await encodeOpusChunks(400)
  return [
    {
      name: 'VideoEncoder',
      make: (record) => {
        const codec = new VideoEncoder({ output: () => record('output'), error: (e) => record('error:' + e.name) })
        codec.configure({ codec: 'vp8', width: 640, height: 480, hardwareAcceleration: 'prefer-software' })
        return codec
      },
      queue: (codec) => {
        for (let i = 0; i < 12; i++) {
          const frame = i420Frame(640, 480, i)
          ;(codec as VideoEncoder).encode(frame, { keyFrame: true })
          frame.close()
        }
      },
      queueOne: (codec) => {
        const frame = i420Frame(640, 480, 0)
        ;(codec as VideoEncoder).encode(frame, { keyFrame: true })
        frame.close()
      },
    },
    {
      name: 'VideoDecoder',
      make: (record) => {
        const codec = new VideoDecoder({
          output: (frame) => {
            record('output')
            frame.close()
          },
          error: (e) => record('error:' + e.name),
        })
        codec.configure({ ...video.config, hardwareAcceleration: 'prefer-software' })
        return codec
      },
      queue: (codec) => {
        for (const chunk of video.chunks) (codec as VideoDecoder).decode(chunk)
      },
      queueOne: (codec) => (codec as VideoDecoder).decode(video.chunks[0]),
    },
    {
      name: 'AudioEncoder',
      make: (record) => {
        const codec = new AudioEncoder({ output: () => record('output'), error: (e) => record('error:' + e.name) })
        codec.configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 })
        return codec
      },
      queue: (codec) => {
        for (let i = 0; i < 400; i++) {
          const data = audioData(i)
          ;(codec as AudioEncoder).encode(data)
          data.close()
        }
      },
      queueOne: (codec) => {
        const data = audioData(0)
        ;(codec as AudioEncoder).encode(data)
        data.close()
      },
    },
    {
      name: 'AudioDecoder',
      make: (record) => {
        const codec = new AudioDecoder({
          output: (data) => {
            record('output')
            data.close()
          },
          error: (e) => record('error:' + e.name),
        })
        codec.configure(audio.config)
        return codec
      },
      queue: (codec) => {
        for (const chunk of audio.chunks) (codec as AudioDecoder).decode(chunk)
      },
      queueOne: (codec) => (codec as AudioDecoder).decode(audio.chunks[0]),
    },
  ]
}

// Records each callback with whether it ran before or after the teardown call returned.
function phasedLog() {
  const entries: string[] = []
  let after = false
  return {
    record: (entry: string) => entries.push(`${entry}@${after ? 'after' : 'before'}`),
    markTeardown: () => {
      after = true
    },
    entries: () => [...entries],
    after: () => entries.filter((entry) => entry.endsWith('@after')),
  }
}

function queueSize(codec: TeardownCodec) {
  return codec.encodeQueueSize ?? codec.decodeQueueSize ?? 0
}

for (const teardown of ['close', 'reset'] as const) {
  test(`${teardown}() drops queued work: no output or error callback runs afterwards`, async (t) => {
    for (const codecCase of await codecCases()) {
      const log = phasedLog()
      const codec = codecCase.make(log.record)
      codec.ondequeue = () => log.record('dequeue')
      codecCase.queue(codec)
      await sendQueued()

      t.notThrows(() => codec[teardown](), `${codecCase.name}.${teardown}()`)
      log.markTeardown()
      await delay(300)

      const late = log.after()
      t.deepEqual(
        late.filter((entry) => !entry.startsWith('dequeue')),
        [],
        `${codecCase.name}: callbacks for work queued before ${teardown}()`,
      )
      t.true(
        late.length <= 1,
        `${codecCase.name}: at most one dequeue event after ${teardown}(), got ${late.join(' ')}`,
      )
      t.is(codec.state, teardown === 'close' ? 'closed' : 'unconfigured')
      if (teardown === 'reset') codec.close()
    }
  })

  test(`${teardown}() with work still queued fires one dequeue event`, async (t) => {
    for (const codecCase of await codecCases()) {
      const log = phasedLog()
      const codec = codecCase.make(log.record)
      codec.ondequeue = () => log.record('dequeue')
      // No microtask runs before the teardown, so all of this work is still queued.
      codecCase.queue(codec)
      codec[teardown]()
      log.markTeardown()
      // Wait for the event itself: a fixed delay can end first when another test
      // blocks the JS thread, because timers run before queued native callbacks.
      await waitFor(() => log.after().length > 0, `${codecCase.name}: dequeue event after ${teardown}()`)
      await delay(100)

      t.deepEqual(log.after(), ['dequeue@after'], codecCase.name)
      if (teardown === 'reset') codec.close()
    }
  })

  test(`${teardown}() after the worker emptied the queue still fires the dequeue event JS has not seen`, async (t) => {
    for (const codecCase of await codecCases()) {
      const log = phasedLog()
      const codec = codecCase.make(log.record)
      codec.ondequeue = () => log.record('dequeue')
      codecCase.queue(codec)
      // Only microtasks run in this loop, so the dequeue events the worker fires
      // cannot be delivered before the teardown.
      const deadline = Date.now() + 60_000
      while (queueSize(codec) > 0 && Date.now() < deadline) await Promise.resolve()
      t.is(queueSize(codec), 0, `${codecCase.name}: the worker emptied the queue`)
      t.deepEqual(log.entries(), [], `${codecCase.name}: nothing delivered before ${teardown}()`)

      codec[teardown]()
      log.markTeardown()
      await waitFor(() => log.after().length > 0, `${codecCase.name}: dequeue event after ${teardown}()`)
      // A second event would be queued with the first one; give it time to arrive.
      await delay(100)

      t.deepEqual(log.after(), ['dequeue@after'], codecCase.name)
      if (teardown === 'reset') codec.close()
    }
  })

  // The event being dispatched is already visible, so tearing down from its
  // handler must not replace it. The queue is empty by then, so no event follows.
  // Chrome's encoders match; its decoders fire one more dequeue event on every
  // close() and reset() from the configured state, even an idle one, which the
  // spec only asks for when decodeQueueSize > 0.
  test(`${teardown}() from the ondequeue handler fires no second event`, async (t) => {
    for (const codecCase of await codecCases()) {
      const events: string[] = []
      const errors: string[] = []
      const codec = codecCase.make((entry) => {
        if (entry !== 'output') errors.push(entry)
      })
      codec.ondequeue = () => {
        events.push(`dequeue:${codec.state}`)
        if (events.length === 1) codec[teardown]()
      }
      codecCase.queueOne(codec)
      await waitFor(() => events.length > 0, `${codecCase.name}: dequeue event`)
      // A second event would be queued by the teardown; give it time to arrive.
      await delay(100)

      t.deepEqual(events, ['dequeue:configured'], codecCase.name)
      t.deepEqual(errors, [], codecCase.name)
      t.is(codec.state, teardown === 'close' ? 'closed' : 'unconfigured', codecCase.name)
      if (teardown === 'reset') codec.close()
    }
  })
}

// Worker errors that JS has not seen yet: the error callback cannot run while the
// JS thread is blocked, so the worker has already closed the codec natively.
function opaqueFrameEncoder(record: Record) {
  const encoder = new VideoEncoder({ output: () => record('output'), error: (e) => record('error:' + e.name) })
  encoder.configure({ codec: 'vp8', width: 16, height: 16, hardwareAcceleration: 'prefer-software' })
  const frame = createVideoFrameFromRGBA16Float(new Uint16Array(16 * 16 * 4), {
    codedWidth: 16,
    codedHeight: 16,
    timestamp: 0,
  })
  encoder.encode(frame)
  frame.close()
  return encoder
}

function junkAacDecoder(record: Record) {
  const decoder = new AudioDecoder({
    output: (data) => {
      record('output')
      data.close()
    },
    error: (e) => record('error:' + e.name),
  })
  // AAC-LC 48 kHz stereo AudioSpecificConfig; the chunks are noise
  decoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48_000,
    numberOfChannels: 2,
    description: new Uint8Array([0x11, 0x90]),
  })
  for (let i = 0; i < 4; i++) {
    const data = new Uint8Array(400)
    for (let j = 0; j < data.length; j++) data[j] = (j * 151 + i * 89 + 7) & 0xff
    decoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: i * 21_333, data }))
  }
  return decoder
}

const workerErrorCases: Array<[string, (record: Record) => TeardownCodec, string]> = [
  ['VideoEncoder opaque frame', opaqueFrameEncoder, 'error:OperationError'],
  ['AudioDecoder corrupt AAC', junkAacDecoder, 'error:EncodingError'],
]

for (const teardown of ['close', 'reset'] as const) {
  test(`${teardown}() succeeds and drops a worker error whose callback has not run`, async (t) => {
    for (const [name, make] of workerErrorCases) {
      const log: string[] = []
      const codec = make((entry) => log.push(entry))
      await sendQueued()
      blockJsThread(300)

      t.notThrows(() => codec[teardown](), `${name}: ${teardown}()`)
      await delay(300)
      t.deepEqual(log, [], `${name}: no callback after ${teardown}()`)
      t.is(codec.state, teardown === 'close' ? 'closed' : 'unconfigured', name)
      if (teardown === 'reset') codec.close()
    }
  })
}

test('reset() leaves an encoder usable after dropping a worker error', async (t) => {
  const log: string[] = []
  const encoder = opaqueFrameEncoder((entry) => log.push(entry))
  await sendQueued()
  blockJsThread(300)
  encoder.reset()

  encoder.configure({ codec: 'vp8', width: 16, height: 16, hardwareAcceleration: 'prefer-software' })
  const frame = i420Frame(16, 16, 0)
  encoder.encode(frame)
  frame.close()
  await encoder.flush()
  t.deepEqual(log, ['output'])
  encoder.close()
})

for (const [name, make, expected] of workerErrorCases) {
  test(`${name}: a flush pending when the error lands rejects with that error after the error callback`, async (t) => {
    const log: string[] = []
    const codec = make((entry) => log.push(entry))
    await codec.flush().catch((error: unknown) => {
      log.push(`flush:${(error as DOMException).name}:${error instanceof DOMException}`)
    })
    t.deepEqual(log, [expected, `flush:${expected.slice('error:'.length)}:true`])
  })
}

// Work that succeeds, then work that fails, then flush(). The flush buffers the
// outputs of the first work; they must reach JS before the error callback, never
// after the error closed the codec. Chrome: outputs, error, flush rejection.
interface FailAfterOutputCase {
  name: string
  // Configures the codec, queues `good` successful work items and one failing
  // item, and returns the codec. Synchronous, so a flush() right after it is
  // pending before the worker starts.
  run(record: Record, good: number): TeardownCodec
  error: string
}

async function failAfterOutputCases(): Promise<FailAfterOutputCase[]> {
  const video = await encodeVp8Chunks(3)
  const audio = await encodeOpusChunks(3)
  return [
    {
      name: 'VideoEncoder: opaque frame',
      error: 'OperationError',
      run: (record, good) => {
        const encoder = new VideoEncoder({ output: () => record('output'), error: (e) => record('error:' + e.name) })
        encoder.configure({ codec: 'vp8', width: 16, height: 16, hardwareAcceleration: 'prefer-software' })
        for (let i = 0; i < good; i++) {
          const frame = i420Frame(16, 16, i)
          encoder.encode(frame, { keyFrame: true })
          frame.close()
        }
        const opaque = createVideoFrameFromRGBA16Float(new Uint16Array(16 * 16 * 4), {
          codedWidth: 16,
          codedHeight: 16,
          timestamp: good * 33_333,
        })
        encoder.encode(opaque)
        opaque.close()
        return encoder
      },
    },
    {
      name: 'VideoDecoder: corrupt chunk',
      error: 'EncodingError',
      run: (record, good) => {
        const decoder = new VideoDecoder({
          output: (frame) => {
            record('output')
            frame.close()
          },
          error: (e) => record('error:' + e.name),
        })
        // Low latency: each chunk decodes on its own, so the corrupt one fails in decode()
        decoder.configure({ ...video.config, optimizeForLatency: true, hardwareAcceleration: 'prefer-software' })
        for (let i = 0; i < good; i++) decoder.decode(video.chunks[i])
        decoder.decode(new EncodedVideoChunk({ type: 'key', timestamp: good * 33_333, data: new Uint8Array([0]) }))
        return decoder
      },
    },
    {
      // Chrome resamples Opus input to 48 kHz; this encoder (libopus) cannot open at
      // 44.1 kHz, so the queued reconfigure fails on the worker.
      name: 'AudioEncoder: reconfigure the encoder cannot open',
      error: 'EncodingError',
      run: (record, good) => {
        const encoder = new AudioEncoder({ output: () => record('output'), error: (e) => record('error:' + e.name) })
        encoder.configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 })
        for (let i = 0; i < good; i++) {
          const data = audioData(i)
          encoder.encode(data)
          data.close()
        }
        encoder.configure({ codec: 'opus', sampleRate: 44_100, numberOfChannels: 2 })
        return encoder
      },
    },
    {
      name: 'AudioDecoder: corrupt chunk',
      error: 'EncodingError',
      run: (record, good) => {
        const decoder = new AudioDecoder({
          output: (data) => {
            record('output')
            data.close()
          },
          error: (e) => record('error:' + e.name),
        })
        decoder.configure(audio.config)
        for (let i = 0; i < good; i++) decoder.decode(audio.chunks[i])
        // An Opus code 3 packet that declares zero frames
        decoder.decode(
          new EncodedAudioChunk({ type: 'key', timestamp: good * 20_000, data: new Uint8Array([0x03, 0x00]) }),
        )
        return decoder
      },
    },
  ]
}

test('outputs of work before a failure reach JS before the error callback, and the pending flush rejects after it', async (t) => {
  for (const failCase of await failAfterOutputCases()) {
    const log: string[] = []
    const codec = failCase.run((entry) => log.push(entry), 2)
    await codec.flush().catch((error: unknown) => log.push(`flush:${(error as DOMException).name}`))
    await delay(100)
    t.deepEqual(
      log,
      ['output', 'output', `error:${failCase.error}`, `flush:${failCase.error}`],
      failCase.name,
    )
    t.is(codec.state, 'closed', failCase.name)
  }
})

test('close() in an output callback that runs before the error callback drops the error', async (t) => {
  for (const failCase of await failAfterOutputCases()) {
    const log: string[] = []
    let codec: TeardownCodec | undefined
    let closed = false
    codec = failCase.run((entry) => {
      log.push(entry)
      if (entry === 'output' && !closed) {
        closed = true
        codec!.close()
      }
    }, 2)
    // The first output callback closes the codec: the second output and the
    // error are dropped, and the flush is aborted.
    const flush = await codec.flush().then(
      () => 'ok',
      (error: unknown) => (error as DOMException).name,
    )
    await delay(100)
    t.deepEqual(log, ['output'], failCase.name)
    t.is(flush, 'AbortError', failCase.name)
    t.is(codec.state, 'closed', failCase.name)
  }
})

// ============================================================================
// flush() after the error callback ran: InvalidStateError
// ============================================================================

const badConfigCases: Array<[string, (record: Record) => TeardownCodec]> = [
  [
    'VideoEncoder',
    (record) => {
      const codec = new VideoEncoder({ output: () => record('output'), error: (e) => record('error:' + e.name) })
      codec.configure({ codec: 'bogus', width: 64, height: 64 })
      return codec
    },
  ],
  [
    'VideoDecoder',
    (record) => {
      const codec = new VideoDecoder({ output: (f) => f.close(), error: (e) => record('error:' + e.name) })
      codec.configure({ codec: 'bogus' })
      return codec
    },
  ],
  [
    'AudioEncoder',
    (record) => {
      const codec = new AudioEncoder({ output: () => record('output'), error: (e) => record('error:' + e.name) })
      codec.configure({ codec: 'bogus', sampleRate: 48_000, numberOfChannels: 2 })
      return codec
    },
  ],
  [
    'AudioDecoder',
    (record) => {
      const codec = new AudioDecoder({ output: (d) => d.close(), error: (e) => record('error:' + e.name) })
      codec.configure({ codec: 'bogus', sampleRate: 48_000, numberOfChannels: 2 })
      return codec
    },
  ],
]

test('flush() after the error callback ran rejects with InvalidStateError', async (t) => {
  const cases: Array<[string, (record: Record) => TeardownCodec]> = [
    ...badConfigCases,
    ...workerErrorCases.map(([name, make]): [string, (record: Record) => TeardownCodec] => [name, make]),
  ]
  for (const [name, make] of cases) {
    const log: string[] = []
    const codec = make((entry) => log.push(entry))
    await waitFor(() => log.some((entry) => entry.startsWith('error:')), `${name} error callback`)

    const error = await t.throwsAsync(codec.flush(), undefined, name)
    t.true(error instanceof DOMException, `${name}: DOMException`)
    t.is((error as DOMException).name, 'InvalidStateError', name)
    t.throws(() => codec.close(), { name: 'InvalidStateError' }, `${name}: close() after the error callback`)
  }
})

test('decoders reject a flush issued before the error callback with that error', async (t) => {
  for (const [name, make] of badConfigCases.filter(([name]) => name.endsWith('Decoder'))) {
    const log: string[] = []
    const codec = make((entry) => log.push(entry))
    await codec.flush().catch((error: unknown) => log.push(`flush:${(error as DOMException).name}`))
    t.deepEqual(log, ['error:NotSupportedError', 'flush:NotSupportedError'], name)
  }
})

// ============================================================================
// An unsupported config: reset() keeps the error, close() drops it
// ============================================================================

interface UnsupportedConfigCase {
  name: string
  // Creates an unconfigured codec. The error callback records the codec state it sees.
  make(record: Record): TeardownCodec
  configureBogus(codec: TeardownCodec): void
  configureGood(codec: TeardownCodec): void
  // Queues work after configureGood().
  queue(codec: TeardownCodec): void
}

const unsupportedConfigCases: UnsupportedConfigCase[] = [
  {
    name: 'VideoEncoder',
    make: (record) => {
      const codec: VideoEncoder = new VideoEncoder({
        output: () => record('output'),
        error: (e) => record(`error:${e.name}:${codec.state}`),
      })
      return codec
    },
    configureBogus: (codec) => (codec as VideoEncoder).configure({ codec: 'bogus', width: 64, height: 48 }),
    configureGood: (codec) =>
      (codec as VideoEncoder).configure({
        codec: 'vp8',
        width: 64,
        height: 48,
        hardwareAcceleration: 'prefer-software',
      }),
    queue: (codec) => {
      for (let i = 0; i < 3; i++) {
        const frame = i420Frame(64, 48, i)
        ;(codec as VideoEncoder).encode(frame, { keyFrame: true })
        frame.close()
      }
    },
  },
  {
    name: 'VideoDecoder',
    make: (record) => {
      const codec: VideoDecoder = new VideoDecoder({
        output: (frame) => {
          record('output')
          frame.close()
        },
        error: (e) => record(`error:${e.name}:${codec.state}`),
      })
      return codec
    },
    configureBogus: (codec) => (codec as VideoDecoder).configure({ codec: 'bogus' }),
    configureGood: (codec) => (codec as VideoDecoder).configure({ codec: 'vp8' }),
    queue: () => {},
  },
  {
    name: 'AudioEncoder',
    make: (record) => {
      const codec: AudioEncoder = new AudioEncoder({
        output: () => record('output'),
        error: (e) => record(`error:${e.name}:${codec.state}`),
      })
      return codec
    },
    configureBogus: (codec) =>
      (codec as AudioEncoder).configure({ codec: 'bogus', sampleRate: 48_000, numberOfChannels: 2 }),
    configureGood: (codec) =>
      (codec as AudioEncoder).configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 }),
    queue: (codec) => {
      for (let i = 0; i < 3; i++) {
        const data = audioData(i)
        ;(codec as AudioEncoder).encode(data)
        data.close()
      }
    },
  },
  {
    name: 'AudioDecoder',
    make: (record) => {
      const codec: AudioDecoder = new AudioDecoder({
        output: (data) => {
          record('output')
          data.close()
        },
        error: (e) => record(`error:${e.name}:${codec.state}`),
      })
      return codec
    },
    configureBogus: (codec) =>
      (codec as AudioDecoder).configure({ codec: 'bogus', sampleRate: 48_000, numberOfChannels: 2 }),
    configureGood: (codec) =>
      (codec as AudioDecoder).configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 }),
    queue: () => {},
  },
]

// Chrome queues the NotSupportedError configure() finds as a task that closes the
// codec when it runs. reset() does not cancel that task; close() makes it a no-op.
test('reset() keeps the NotSupportedError from configure(), which closes the codec when it arrives', async (t) => {
  for (const config of unsupportedConfigCases) {
    const log: string[] = []
    const codec = config.make((entry) => log.push(entry))
    config.configureBogus(codec)
    codec.reset()
    t.is(codec.state, 'unconfigured', `${config.name}: state after reset()`)

    // The codec works again until the error arrives, then the error closes it and
    // drops this work.
    config.configureGood(codec)
    config.queue(codec)
    const flush = await codec.flush().then(
      () => 'ok',
      (error: unknown) => (error as DOMException).name,
    )

    t.is(flush, 'NotSupportedError', `${config.name}: flush`)
    t.deepEqual(log, ['error:NotSupportedError:closed'], config.name)
    t.is(codec.state, 'closed', config.name)
    t.throws(() => codec.reset(), { name: 'InvalidStateError' }, `${config.name}: reset() after the error`)
  }
})

test('close() drops the NotSupportedError from configure(), also after reset()', async (t) => {
  for (const config of unsupportedConfigCases) {
    for (const resetFirst of [false, true]) {
      const log: string[] = []
      const codec = config.make((entry) => log.push(entry))
      config.configureBogus(codec)
      if (resetFirst) codec.reset()
      codec.close()
      await delay(200)
      t.deepEqual(log, [], `${config.name}${resetFirst ? ' after reset()' : ''}`)
      t.is(codec.state, 'closed', config.name)
    }
  }
})

// FLAC needs a description. A configured decoder checks that in configure() like
// an unconfigured one, so the error does not depend on the decoder's state.
test('AudioDecoder: reset() keeps the NotSupportedError for FLAC without a description, also when reconfiguring', async (t) => {
  for (const configuredFirst of [false, true]) {
    const name = configuredFirst ? 'after opus' : 'first configure'
    const log: string[] = []
    const decoder: AudioDecoder = new AudioDecoder({
      output: (data) => data.close(),
      error: (e) => log.push(`error:${e.name}:${decoder.state}`),
    })
    if (configuredFirst) decoder.configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 })
    decoder.configure({ codec: 'flac', sampleRate: 48_000, numberOfChannels: 2 })
    decoder.reset()
    t.is(decoder.state, 'unconfigured', `${name}: state after reset()`)

    await waitFor(() => log.length > 0, `${name}: error callback`)
    await delay(50)
    t.deepEqual(log, ['error:NotSupportedError:closed'], name)
    t.is(decoder.state, 'closed', name)
  }
})

// ============================================================================
// Memory a closed encoder keeps
// ============================================================================

// The child's stdout carries the test result, so it must not inherit WEBCODECS_LOG
// (CI sets it, and the log output goes to stdout).
function childEnv(): NodeJS.ProcessEnv {
  const env = { ...process.env }
  delete env.WEBCODECS_LOG
  return env
}

// RSS only drops when the allocator hands freed blocks back to the OS. macOS does
// that for blocks this large; other allocators may keep them, so the check runs on
// macOS only.
const rssTest = process.platform === 'darwin' ? test : test.skip

rssTest('VideoEncoder: an encoder an error closed frees its codec while it is still referenced', (t) => {
  // Each encoder fills an x264 1080p context, then a failing configure() closes it.
  // The encoders stay referenced, so whatever they still hold adds up in RSS.
  const script = `
    const fs = require('node:fs');
    const { VideoEncoder, VideoFrame } = require('./index.js');
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const rss = () => process.memoryUsage().rss / 1048576;
    const settle = async () => { for (let i = 0; i < 5; i++) { global.gc(); await sleep(50); } };
    (async () => {
      const kept = [];
      const samples = [];
      for (let k = 0; k < 3; k++) {
        const errors = [];
        const encoder = new VideoEncoder({ output: () => {}, error: (e) => errors.push(e.name) });
        kept.push(encoder);
        encoder.configure({ codec: 'avc1.640028', width: 1920, height: 1080, hardwareAcceleration: 'prefer-software' });
        for (let i = 0; i < 2; i++) {
          const frame = new VideoFrame(new Uint8Array(1920 * 1080 * 3 / 2).fill(90 + i), {
            format: 'I420', codedWidth: 1920, codedHeight: 1080, timestamp: i * 33333,
          });
          encoder.encode(frame);
          frame.close();
        }
        while (encoder.encodeQueueSize > 0) await sleep(10);
        encoder.configure({ codec: 'bogus', width: 1920, height: 1080 });
        while (errors.length === 0) await sleep(10);
        await settle();
        samples.push(rss());
      }
      fs.writeSync(1, JSON.stringify({ samples, states: kept.map((e) => e.state) }));
      process.exit(0);
    })();
  `
  const result = JSON.parse(
    execFileSync(process.execPath, ['--expose-gc', '-e', script], {
      cwd: process.cwd(),
      env: childEnv(),
      timeout: 120_000,
      encoding: 'utf8',
    }),
  ) as { samples: number[]; states: string[] }
  t.deepEqual(result.states, ['closed', 'closed', 'closed'])
  // A 1080p x264 context is about 40 MB; a released one is reused by the next encoder.
  const growth = (result.samples[2] - result.samples[0]) / 2
  t.true(growth < 20, `RSS grows ${growth.toFixed(1)} MB per closed encoder (samples ${result.samples.join(', ')})`)
})
