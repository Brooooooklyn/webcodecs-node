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

async function encodeVp8Chunks(count: number, width = 320, height = 240) {
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
  encoder.configure({ codec: 'vp8', width, height, hardwareAcceleration: 'prefer-software' })
  for (let i = 0; i < count; i++) {
    const frame = i420Frame(width, height, i)
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

  // Serial, because ava runs a file's serial tests before the others: the spin
  // below runs no timers, and in x64 Node under Rosetta at background priority it
  // took about 11 s, longer than the waitFor() of the tests that ran with it.
  test.serial(`${teardown}() after the worker emptied the queue still fires the dequeue event JS has not seen`, async (t) => {
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

  // Building the dropped error's DOMException would read the page's global
  // DOMException, and a throw there would become an uncaught exception. Chrome
  // builds it natively, so no page code runs for a dropped error.
  // Serial: the global stays replaced across an await.
  test.serial(`${teardown}() drops a worker error without reading the global DOMException`, async (t) => {
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'DOMException')!
    for (const [name, make] of workerErrorCases) {
      const log: string[] = []
      const codec = make((entry) => log.push(entry))
      await sendQueued()
      blockJsThread(300)

      codec[teardown]()
      Object.defineProperty(globalThis, 'DOMException', {
        configurable: true,
        get: () => {
          log.push('DOMException read')
          return descriptor.value
        },
      })
      try {
        await delay(300)
      } finally {
        Object.defineProperty(globalThis, 'DOMException', descriptor)
      }
      t.deepEqual(log, [], `${name}: nothing runs after ${teardown}()`)
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

async function failAfterOutputCases(maxGood = 3, videoSize = 320): Promise<FailAfterOutputCase[]> {
  const video = await encodeVp8Chunks(maxGood, videoSize, (videoSize * 3) / 4)
  const audio = await encodeOpusChunks(maxGood)
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

// Some Node-API versions cannot retain a primitive with napi_create_reference.
// Reporting it must not leave a pending exception that stops the remaining
// outputs and the codec's own error callback.
test.serial('primitive output exceptions do not stop later outputs or the codec error callback', async (t) => {
  for (const thrown of ['primitive-output-threw', 42] as const) {
    for (const failCase of await failAfterOutputCases()) {
      const log: string[] = []
      process.setUncaughtExceptionCaptureCallback((error) => {
        log.push(`uncaught:${String(error)}`)
      })
      try {
        const codec = failCase.run((entry) => {
          log.push(entry)
          if (entry === 'output') throw thrown
        }, 2)
        await codec.flush().catch((error: unknown) => log.push(`flush:${(error as DOMException).name}`))
        await delay(20)
      } finally {
        process.setUncaughtExceptionCaptureCallback(null)
      }
      t.deepEqual(
        log,
        [
          'output',
          `uncaught:${String(thrown)}`,
          'output',
          `uncaught:${String(thrown)}`,
          `error:${failCase.error}`,
          `flush:${failCase.error}`,
        ],
        `${String(thrown)}: ${failCase.name}`,
      )
    }
  }
})

// A failed exception scheduler must not leave a second exception pending and
// suppress the codec's own error callback. Report both exceptions.
test.serial('a failed exception scheduler does not stop codec error callbacks', async (t) => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'queueMicrotask')!
  for (const failCase of await failAfterOutputCases()) {
    const log: string[] = []
    process.setUncaughtExceptionCaptureCallback((error) => {
      log.push(`uncaught:${error instanceof Error ? error.message : String(error)}`)
    })
    try {
      Object.defineProperty(globalThis, 'queueMicrotask', {
        configurable: true,
        get() {
          throw new Error('queue-microtask-getter')
        },
      })
      const codec = failCase.run((entry) => {
        log.push(entry)
        if (entry === 'output') throw new Error('output-threw')
      }, 2)
      await codec.flush().catch((error: unknown) => log.push(`flush:${(error as DOMException).name}`))
      await delay(20)
    } finally {
      process.setUncaughtExceptionCaptureCallback(null)
      Object.defineProperty(globalThis, 'queueMicrotask', descriptor)
    }
    t.deepEqual(log.filter((entry) => entry === 'output'), ['output', 'output'], failCase.name)
    t.deepEqual(
      log.filter((entry) => entry.startsWith('uncaught:')).sort(),
      ['uncaught:output-threw', 'uncaught:output-threw', 'uncaught:queue-microtask-getter', 'uncaught:queue-microtask-getter'].sort(),
      failCase.name,
    )
    t.deepEqual(log.slice(-2), [`error:${failCase.error}`, `flush:${failCase.error}`], failCase.name)
  }
})

// A page-provided scheduler can invoke the native reporter synchronously, throw after it, or
// invoke it again. Each output exception owns one native reference, however the scheduler behaves.
test.serial('synchronous exception schedulers consume each output exception once', async (t) => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'queueMicrotask')!
  let retainedReporter: (() => void) | undefined
  const schedulers: {
    name: string
    retain?: boolean
    run(reporter: () => void): void
  }[] = [
    {
      name: 'synchronous reporter',
      run: (reporter) => reporter(),
    },
    {
      name: 'throw after synchronous reporter',
      run: (reporter) => {
        try {
          reporter()
        } catch {}
        throw new Error('queue-microtask-call')
      },
    },
    {
      name: 'repeated synchronous reporter',
      run: (reporter) => {
        let outputError: unknown
        try {
          reporter()
        } catch (error) {
          outputError = error
        }
        reporter()
        throw outputError
      },
    },
    {
      name: 'retained reporter after scheduler failure',
      retain: true,
      run: (reporter) => {
        retainedReporter = reporter
        throw new Error('queue-microtask-call')
      },
    },
  ]
  const failCases = await failAfterOutputCases()
  for (const scheduler of schedulers) {
    for (const failCase of failCases) {
      const label = `${scheduler.name}: ${failCase.name}`
      retainedReporter = undefined
      const log: string[] = []
      process.setUncaughtExceptionCaptureCallback((error) => {
        log.push(`uncaught:${error instanceof Error ? error.message : String(error)}`)
      })
      try {
        Object.defineProperty(globalThis, 'queueMicrotask', {
          configurable: true,
          writable: true,
          value: (reporter: () => void) => scheduler.run(reporter),
        })
        const codec = failCase.run((entry) => {
          log.push(entry)
          if (entry === 'output') throw new Error('output-threw')
        }, 2)
        await codec.flush().catch((error: unknown) => log.push(`flush:${(error as DOMException).name}`))
        await delay(20)
        if (scheduler.retain) {
          t.truthy(retainedReporter, label)
          retainedReporter!()
        }
      } finally {
        process.setUncaughtExceptionCaptureCallback(null)
        Object.defineProperty(globalThis, 'queueMicrotask', descriptor)
      }
      t.deepEqual(log.filter((entry) => entry === 'output'), ['output', 'output'], label)
      const expected = ['uncaught:output-threw', 'uncaught:output-threw']
      t.deepEqual(log.filter((entry) => entry.startsWith('uncaught:')).sort(), expected.sort(), label)
      t.deepEqual(log.slice(-2), [`error:${failCase.error}`, `flush:${failCase.error}`], label)
    }
  }
})

// The flush rejection is built from the global DOMException, which page code can
// replace. When the replacement throws, napi-rs cannot reject the promise while
// that exception is pending, so the flush stayed pending forever. Now it rejects
// with what was thrown, or with what a replaced constructor returned, as it is.
// A value whose cause is itself crashed the process: napi-rs's Error::from
// follows `cause` with no limit. (Chrome builds a native DOMException.)
interface DOMExceptionReplacement {
  name: string
  // Replaces the global DOMException and returns the value the flush must reject with.
  install(): unknown
}

function errorCausedBySelf(message: string): Error {
  const error = new Error(message)
  error.cause = error
  return error
}

function replaceDOMExceptionWithThrowingGetter(thrown: unknown): unknown {
  Object.defineProperty(globalThis, 'DOMException', {
    configurable: true,
    get: () => {
      throw thrown
    },
  })
  return thrown
}

const domExceptionReplacements: DOMExceptionReplacement[] = [
  {
    name: 'getter throws an Error',
    install: () => replaceDOMExceptionWithThrowingGetter(new Error('replaced DOMException')),
  },
  {
    name: 'getter throws an Error whose cause is itself',
    install: () => replaceDOMExceptionWithThrowingGetter(errorCausedBySelf('replaced DOMException')),
  },
  {
    name: 'constructor returns an Error whose cause is itself',
    install: () => {
      const returned = errorCausedBySelf('replaced DOMException')
      Object.defineProperty(globalThis, 'DOMException', {
        configurable: true,
        writable: true,
        value: function ReplacedDOMException() {
          return returned
        },
      })
      return returned
    },
  },
  {
    name: 'getter throws a string',
    install: () => replaceDOMExceptionWithThrowingGetter('replaced DOMException'),
  },
]

// Serial: the global stays replaced across an await.
test.serial('a pending flush still settles when page code replaced DOMException in the error callback', async (t) => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'DOMException')!
  for (const replacement of domExceptionReplacements) {
    for (const failCase of await failAfterOutputCases()) {
      const log: string[] = []
      let expected: unknown = Symbol('not replaced')
      // Cleared before the global is restored, so an error callback that comes
      // later cannot replace it again.
      let armed = true
      try {
        const codec = failCase.run((entry) => {
          log.push(entry)
          if (armed && entry.startsWith('error:')) {
            expected = replacement.install()
          }
        }, 2)
        const flush = codec.flush().then(
          () => 'flush:resolved',
          (error: unknown) => (error === expected ? 'flush:rejected with that value' : `flush:rejected:${String(error)}`),
        )
        // Await the flush with no timer. A flush that never settles (the bug)
        // fails the test: ava reports "Promise returned by test never
        // resolved" when the event loop runs empty, or the test fails at ava's
        // timeout. A timer could fail a flush that settles: if the JS thread is
        // blocked for longer than the timer before the flush's result reaches
        // JS, the overdue timer runs first.
        log.push(await flush)
      } finally {
        armed = false
        Object.defineProperty(globalThis, 'DOMException', descriptor)
      }
      t.deepEqual(
        log,
        ['output', 'output', `error:${failCase.error}`, 'flush:rejected with that value'],
        `${replacement.name}: ${failCase.name}`,
      )
    }
  }
})

const throwingDOMExceptionReplacements = [
  {
    name: 'getter throws',
    install(thrown: Error) {
      replaceDOMExceptionWithThrowingGetter(thrown)
    },
  },
  {
    name: 'constructor throws',
    install(thrown: Error) {
      Object.defineProperty(globalThis, 'DOMException', {
        configurable: true,
        writable: true,
        value: function ReplacedDOMException() {
          throw thrown
        },
      })
    },
  },
]

// Building the callback's DOMException still reports a replacement's exception
// as uncaught. That report must not overtake outputs produced before the error,
// and the pending flush must still reject. The capture callback restores the
// native global so the flush rejection does not depend on the replacement.
test.serial('buffered outputs precede a throwing DOMException replacement and the flush still settles', async (t) => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'DOMException')!
  for (const replacement of throwingDOMExceptionReplacements) {
    for (const failCase of await failAfterOutputCases()) {
      const log: string[] = []
      const thrown = new Error('replacement-threw')
      let resolveUncaught!: () => void
      const uncaught = new Promise<void>((resolve) => {
        resolveUncaught = resolve
      })
      process.setUncaughtExceptionCaptureCallback((error) => {
        log.push(`uncaught:${error instanceof Error ? error.message : String(error)}`)
        Object.defineProperty(globalThis, 'DOMException', descriptor)
        resolveUncaught()
      })
      try {
        replacement.install(thrown)
        const codec = failCase.run((entry) => log.push(entry), 2)
        const flush = codec.flush().then(
          () => 'flush:resolved',
          (error: unknown) => `flush:${error === thrown ? failCase.error : (error as DOMException).name}`,
        )
        const [flushResult] = await Promise.all([flush, uncaught])
        log.push(flushResult)
      } finally {
        process.setUncaughtExceptionCaptureCallback(null)
        Object.defineProperty(globalThis, 'DOMException', descriptor)
      }
      t.deepEqual(
        log,
        ['output', 'output', 'uncaught:replacement-threw', `flush:${failCase.error}`],
        `${replacement.name}: ${failCase.name}`,
      )
    }
  }
})

// No flush pending: the outputs travel to JS on their own, the error on another
// ThreadsafeFunction. Node runs at most 999 calls of one ThreadsafeFunction before
// it moves on to the next, so with more outputs queued the error callback ran
// first and closed the codec, and the gate dropped the rest. Chrome: all outputs,
// then the error.
test('outputs queued behind more than 999 others still reach JS before the error callback', async (t) => {
  const good = 1100
  for (const failCase of await failAfterOutputCases(good, 64)) {
    const log: string[] = []
    const codec = failCase.run((entry) => log.push(entry), good)
    await sendQueued()
    // Keep the JS thread busy until the worker is done: every output and the
    // error are queued before any callback runs.
    const deadline = Date.now() + 20_000
    while (queueSize(codec) > 0 && Date.now() < deadline) blockJsThread(1)
    blockJsThread(200)
    await waitFor(() => log.some((entry) => entry.startsWith('error:')), `${failCase.name} error callback`)
    await delay(100)
    const errorAt = log.findIndex((entry) => entry.startsWith('error:'))
    t.deepEqual(
      { before: errorAt, error: log[errorAt], after: log.length - errorAt - 1 },
      { before: good, error: `error:${failCase.error}`, after: 0 },
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

test.serial('close() drops the configure() NotSupportedError without reading the global DOMException', async (t) => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'DOMException')!
  for (const config of unsupportedConfigCases) {
    const log: string[] = []
    const codec = config.make((entry) => log.push(entry))
    config.configureBogus(codec)
    codec.close()
    Object.defineProperty(globalThis, 'DOMException', {
      configurable: true,
      get: () => {
        log.push('DOMException read')
        return descriptor.value
      },
    })
    try {
      await delay(200)
    } finally {
      Object.defineProperty(globalThis, 'DOMException', descriptor)
    }
    t.deepEqual(log, [], config.name)
  }
})

// The same as "a pending flush still settles when page code replaced DOMException in
// the error callback", for a flush that is aborted: by reset() or close(), or by the
// NotSupportedError a configure() before reset() reports, which closes the codec
// configured since. That rejection was built through napi-rs's Error::from too, so
// the flush stayed pending or the process crashed. (Chrome rejects with a native
// AbortError, or with the NotSupportedError before the error callback runs.)
const abortTriggers = ['reset()', 'close()', 'configure() error after reset()'] as const

// Serial: the global stays replaced across an await.
test.serial('an aborted flush still settles when page code replaced DOMException', async (t) => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'DOMException')!
  for (const trigger of abortTriggers) {
    for (const replacement of domExceptionReplacements) {
      for (const config of unsupportedConfigCases) {
        const log: string[] = []
        let expected: unknown = Symbol('not replaced')
        // Cleared before the global is restored, so an error callback that comes
        // later cannot replace it again.
        let armed = trigger === 'configure() error after reset()'
        try {
          const codec = config.make((entry) => {
            log.push(entry)
            if (armed && entry.startsWith('error:')) {
              expected = replacement.install()
            }
          })
          if (trigger === 'configure() error after reset()') {
            config.configureBogus(codec)
            codec.reset()
          }
          config.configureGood(codec)
          config.queue(codec)
          const flush = codec.flush().then(
            () => 'flush:resolved',
            (error: unknown) =>
              error === expected ? 'flush:rejected with that value' : `flush:rejected:${String(error)}`,
          )
          if (trigger === 'reset()') {
            expected = replacement.install()
            codec.reset()
          } else if (trigger === 'close()') {
            expected = replacement.install()
            codec.close()
          }
          // Awaited with no timer, for the reason given in that test.
          log.push(await flush)
        } finally {
          armed = false
          Object.defineProperty(globalThis, 'DOMException', descriptor)
        }
        t.deepEqual(
          log,
          trigger === 'configure() error after reset()'
            ? ['error:NotSupportedError:closed', 'flush:rejected with that value']
            : ['flush:rejected with that value'],
          `${trigger}: ${replacement.name}: ${config.name}`,
        )
      }
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

// The peak RSS stays flat only when the allocator reuses, or hands back to the OS, the
// blocks that the closed encoders freed. libmalloc does for blocks this large (with its
// medium zone off under Rosetta, see below); other allocators are not checked, so the
// check runs on macOS only.
const rssTest = process.platform === 'darwin' ? test : test.skip

rssTest('VideoEncoder: an encoder an error closed frees its codec while it is still referenced', (t) => {
  // Each encoder fills an x264 1080p context, then a failing configure() closes it.
  // The encoders stay referenced, so whatever they still hold adds up in the peak RSS.
  const script = `
    const fs = require('node:fs');
    const { VideoEncoder, VideoFrame } = require('./index.js');
    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    // the peak RSS so far, in MB (maxRSS is in KB)
    const peak = () => process.resourceUsage().maxRSS / 1024;
    const settle = async () => { for (let i = 0; i < 5; i++) { global.gc(); await sleep(50); } };
    (async () => {
      const kept = [];
      const peaks = [];
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
        peaks.push(peak());
      }
      fs.writeSync(1, JSON.stringify({ peaks, states: kept.map((e) => e.state) }));
      process.exit(0);
    })();
  `
  const result = JSON.parse(
    execFileSync(process.execPath, ['--expose-gc', '-e', script], {
      cwd: process.cwd(),
      // x64 Node under Rosetta (as in CI) puts blocks of 32 KB to 8 MB in libmalloc's
      // medium zone. It keeps the ones the encoders free resident and does not reuse
      // them all, so RSS grew 12 to 28 MB per encoder with nothing leaked. With that
      // zone off they are freed as in arm64 Node, which has none.
      env: { ...childEnv(), MallocMediumZone: '0' },
      timeout: 120_000,
      encoding: 'utf8',
    }),
  ) as { peaks: number[]; states: string[] }
  t.deepEqual(result.states, ['closed', 'closed', 'closed'])
  // A 1080p x264 context is about 40 MB: one that a closed encoder keeps raises the peak by
  // that much for each next encoder, while a released one is reused or faulted in again. An
  // RSS sample would also depend on when the OS takes freed pages back: under load it took
  // them back early for the first encoder only, and RSS seemed to grow 22 MB per encoder.
  const growth = (result.peaks[2] - result.peaks[0]) / 2
  t.true(growth < 20, `peak RSS grows ${growth.toFixed(1)} MB per closed encoder (peaks ${result.peaks.join(', ')})`)
})
