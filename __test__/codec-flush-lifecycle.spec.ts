import { execFileSync, spawn } from 'node:child_process'

import test from 'ava'

import {
  AudioDecoder,
  AudioEncoder,
  VideoDecoder,
  VideoEncoder,
  type AudioDecoderConfig,
  type CodecState,
  type EncodedAudioChunk,
  type EncodedVideoChunk,
  type VideoDecoderConfig,
} from '../index.js'
import { delay, generateGradientI420Frame, generateSineTone, waitFor } from './helpers/index.js'

interface FlushableCodec {
  readonly state: CodecState
  flush(): Promise<void>
  reset(): void
  close(): void
}

function codecCases(t: { fail(message?: string): never }): Array<[string, () => FlushableCodec]> {
  return [
    [
      'VideoEncoder',
      () => {
        const codec = new VideoEncoder({ output() {}, error: (error) => t.fail(error.message) })
        codec.configure({
          codec: 'vp8',
          width: 64,
          height: 64,
          hardwareAcceleration: 'prefer-software',
        })
        return codec
      },
    ],
    [
      'VideoDecoder',
      () => {
        const codec = new VideoDecoder({
          output: (frame) => frame.close(),
          error: (error) => t.fail(error.message),
        })
        codec.configure({ codec: 'vp8' })
        return codec
      },
    ],
    [
      'AudioEncoder',
      () => {
        const codec = new AudioEncoder({ output() {}, error: (error) => t.fail(error.message) })
        codec.configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 })
        return codec
      },
    ],
    [
      'AudioDecoder',
      () => {
        const codec = new AudioDecoder({
          output: (data) => data.close(),
          error: (error) => t.fail(error.message),
        })
        codec.configure({ codec: 'opus', sampleRate: 48_000, numberOfChannels: 2 })
        return codec
      },
    ],
  ]
}

async function assertAbortError(
  t: {
    true(value: unknown, message?: string): void
    is(actual: unknown, expected: unknown, message?: string): void
  },
  promise: Promise<void>,
  codecName: string,
) {
  try {
    await promise
    t.true(false, `${codecName} flush should reject`)
  } catch (error) {
    t.true(error instanceof DOMException, `${codecName} should reject with DOMException`)
    t.is((error as DOMException).name, 'AbortError', `${codecName} should reject with AbortError`)
  }
}

test('close aborts pending flushes in every codec class', async (t) => {
  for (const [name, makeCodec] of codecCases(t)) {
    const codec = makeCodec()
    const flushPromise = codec.flush()
    codec.close()
    await assertAbortError(t, flushPromise, name)
  }
})

test('reset aborts pending flushes with native AbortError DOMExceptions', async (t) => {
  for (const [name, makeCodec] of codecCases(t)) {
    const codec = makeCodec()
    const flushPromise = codec.flush()
    codec.reset()
    await assertAbortError(t, flushPromise, name)
    codec.close()
  }
})

// Chrome: a flush issued before the error callback runs fails with that error.
test('errored encoders reject a flush issued before the error callback with that error', async (t) => {
  const encoders = [
    new AudioEncoder({ output() {}, error() {} }),
    new VideoEncoder({ output() {}, error() {} }),
  ] as const

  encoders[0].configure({ codec: 'unsupported-audio-codec', sampleRate: 48000, numberOfChannels: 2 })
  encoders[1].configure({ codec: 'unsupported-video-codec', width: 64, height: 64 })

  // Issue both flushes before either error callback can run.
  const flushes = encoders.map((encoder) => {
    t.is(encoder.state, 'closed')
    return encoder.flush()
  })
  for (const flush of flushes) {
    const error = await t.throwsAsync(flush)
    t.true(error instanceof DOMException)
    t.is((error as DOMException).name, 'NotSupportedError')
  }
})

const AUDIO_CONFIG = { codec: 'mp4a.40.2', sampleRate: 48_000, numberOfChannels: 2, bitrate: 128_000 }
const VIDEO_CONFIG = { codec: 'vp8', width: 64, height: 64, hardwareAcceleration: 'prefer-software' } as const
const AUDIO_FRAMES = 1500
const VIDEO_FRAMES = 3000

const audioInput = (i: number) =>
  generateSineTone(440, 1024, 2, 48_000, 'f32', Math.round((i * 1024 * 1_000_000) / 48_000))
const videoInput = (i: number) => generateGradientI420Frame(64, 64, i * 33_333, 33_333)

async function encodeAudio(count = AUDIO_FRAMES) {
  const chunks: EncodedAudioChunk[] = []
  let config: AudioDecoderConfig | undefined
  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      config ??= metadata?.decoderConfig as AudioDecoderConfig | undefined
    },
    error() {},
  })
  encoder.configure(AUDIO_CONFIG)
  for (let i = 0; i < count; i++) {
    const data = audioInput(i)
    encoder.encode(data)
    data.close()
  }
  await encoder.flush()
  encoder.close()
  return { chunks, config: config! }
}

async function encodeVideo(count = VIDEO_FRAMES) {
  const chunks: EncodedVideoChunk[] = []
  let config: VideoDecoderConfig | undefined
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      config ??= metadata?.decoderConfig
    },
    error() {},
  })
  encoder.configure(VIDEO_CONFIG)
  for (let i = 0; i < count; i++) {
    const frame = videoInput(i)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }
  await encoder.flush()
  encoder.close()
  return { chunks, config: config! }
}

interface QueueingCodec extends FlushableCodec {
  addEventListener(type: 'dequeue', listener: () => void, options?: { once?: boolean }): void
}

// Feeds one input at a time and waits for dequeue only once the queue builds
// up, the way muxing libraries apply backpressure.
async function feedWithBackpressure(
  codec: QueueingCodec,
  queueSize: () => number,
  count: number,
  feed: (i: number) => void,
) {
  for (let i = 0; i < count; i++) {
    feed(i)
    if (queueSize() >= 4) {
      await new Promise<void>((resolve) => codec.addEventListener('dequeue', () => resolve(), { once: true }))
    }
    await Promise.resolve()
  }
}

test('flush resolves only after every output is delivered in every codec class', async (t) => {
  const audio = await encodeAudio()
  const video = await encodeVideo()
  const cases: Array<[string, number, (timestamps: number[]) => Promise<QueueingCodec>]> = [
    [
      'AudioEncoder',
      AUDIO_FRAMES + 1,
      async (timestamps) => {
        const codec = new AudioEncoder({
          output: (chunk) => timestamps.push(chunk.timestamp),
          error: (e) => t.fail(e.message),
        })
        codec.configure(AUDIO_CONFIG)
        await feedWithBackpressure(
          codec,
          () => codec.encodeQueueSize,
          AUDIO_FRAMES,
          (i) => {
            const data = audioInput(i)
            codec.encode(data)
            data.close()
          },
        )
        return codec
      },
    ],
    [
      'VideoEncoder',
      VIDEO_FRAMES,
      async (timestamps) => {
        const codec = new VideoEncoder({
          output: (chunk) => timestamps.push(chunk.timestamp),
          error: (e) => t.fail(e.message),
        })
        codec.configure(VIDEO_CONFIG)
        await feedWithBackpressure(
          codec,
          () => codec.encodeQueueSize,
          VIDEO_FRAMES,
          (i) => {
            const frame = videoInput(i)
            codec.encode(frame, { keyFrame: i === 0 })
            frame.close()
          },
        )
        return codec
      },
    ],
    [
      'AudioDecoder',
      audio.chunks.length,
      async (timestamps) => {
        const codec = new AudioDecoder({
          output: (data) => {
            timestamps.push(data.timestamp)
            data.close()
          },
          error: (e) => t.fail(e.message),
        })
        codec.configure(audio.config)
        await feedWithBackpressure(
          codec,
          () => codec.decodeQueueSize,
          audio.chunks.length,
          (i) => codec.decode(audio.chunks[i]),
        )
        return codec
      },
    ],
    [
      'VideoDecoder',
      VIDEO_FRAMES,
      async (timestamps) => {
        const codec = new VideoDecoder({
          output: (frame) => {
            timestamps.push(frame.timestamp)
            frame.close()
          },
          error: (e) => t.fail(e.message),
        })
        codec.configure(video.config)
        await feedWithBackpressure(
          codec,
          () => codec.decodeQueueSize,
          video.chunks.length,
          (i) => codec.decode(video.chunks[i]),
        )
        return codec
      },
    ],
  ]

  for (const [name, expected, run] of cases) {
    const timestamps: number[] = []
    const codec = await run(timestamps)
    await codec.flush()
    const deliveredAtResolve = timestamps.length
    await new Promise((resolve) => setTimeout(resolve, 100))

    t.is(deliveredAtResolve, expected, `${name} should deliver every output before flush() resolves`)
    t.is(timestamps.length, deliveredAtResolve, `${name} should deliver nothing after flush() resolves`)
    t.true(
      timestamps.every((timestamp, i) => i === 0 || timestamp > timestamps[i - 1]),
      `${name} should deliver outputs in timestamp order`,
    )
    codec.close()
  }
})

interface FlushOutputCase {
  name: string
  make(output: (timestamp: number) => void, error: (error: DOMException) => void): FlushableCodec
  queue(codec: FlushableCodec): void
  configureError(codec: FlushableCodec): void
}

async function flushOutputCases(): Promise<FlushOutputCase[]> {
  const audioInputCount = 3
  const videoInputCount = 4
  const audio = await encodeAudio(audioInputCount)
  const video = await encodeVideo(videoInputCount)
  return [
    {
      name: 'AudioEncoder',
      make: (output, error) => {
        const codec = new AudioEncoder({ output: (chunk) => output(chunk.timestamp), error })
        codec.configure(AUDIO_CONFIG)
        return codec
      },
      queue: (codec) => {
        for (let i = 0; i < audioInputCount; i++) {
          const data = audioInput(i)
          ;(codec as AudioEncoder).encode(data)
          data.close()
        }
      },
      configureError: (codec) => {
        ;(codec as AudioEncoder).configure({ ...AUDIO_CONFIG, codec: 'unsupported-audio-codec' })
      },
    },
    {
      name: 'AudioDecoder',
      make: (output, error) => {
        const codec = new AudioDecoder({
          output: (data) => {
            const timestamp = data.timestamp
            data.close()
            output(timestamp)
          },
          error,
        })
        codec.configure(audio.config)
        return codec
      },
      queue: (codec) => {
        for (const chunk of audio.chunks) (codec as AudioDecoder).decode(chunk)
      },
      configureError: (codec) => {
        ;(codec as AudioDecoder).configure({ ...audio.config, codec: 'unsupported-audio-codec' })
      },
    },
    {
      name: 'VideoEncoder',
      make: (output, error) => {
        const codec = new VideoEncoder({ output: (chunk) => output(chunk.timestamp), error })
        codec.configure(VIDEO_CONFIG)
        return codec
      },
      queue: (codec) => {
        for (let i = 0; i < videoInputCount; i++) {
          const frame = videoInput(i)
          ;(codec as VideoEncoder).encode(frame, { keyFrame: true })
          frame.close()
        }
      },
      configureError: (codec) => {
        ;(codec as VideoEncoder).configure({ ...VIDEO_CONFIG, codec: 'unsupported-video-codec' })
      },
    },
    {
      name: 'VideoDecoder',
      make: (output, error) => {
        const codec = new VideoDecoder({
          output: (frame) => {
            const timestamp = frame.timestamp
            frame.close()
            output(timestamp)
          },
          error,
        })
        codec.configure({ ...video.config, hardwareAcceleration: 'prefer-software' })
        return codec
      },
      queue: (codec) => {
        for (const chunk of video.chunks) (codec as VideoDecoder).decode(chunk)
      },
      configureError: (codec) => {
        ;(codec as VideoDecoder).configure({
          ...video.config,
          codec: 'unsupported-video-codec',
          hardwareAcceleration: 'prefer-software',
        })
      },
    },
  ]
}

async function flushOutputBaseline(codecCase: FlushOutputCase) {
  const timestamps: number[] = []
  const errors: DOMException[] = []
  const codec = codecCase.make(
    (timestamp) => timestamps.push(timestamp),
    (error) => errors.push(error),
  )
  codecCase.queue(codec)
  await codec.flush()
  if (codec.state !== 'closed') codec.close()
  return { timestamps, errors }
}

test('flush recovers the callback suffix for overlapping flushes in every codec class', async (t) => {
  for (const codecCase of await flushOutputCases()) {
    const baseline = await flushOutputBaseline(codecCase)
    const expected = baseline.timestamps
    t.deepEqual(baseline.errors, [], `${codecCase.name}: baseline errors`)
    t.true(expected.length > 1, `${codecCase.name}: baseline should produce multiple outputs`)

    const sentinel = new Error(`${codecCase.name}: output sentinel`)
    const timestamps: number[] = []
    const errors: DOMException[] = []
    let shouldThrow = true
    let second: Promise<void> | undefined
    let deliveredAtSecondResolve = -1
    let codec: FlushableCodec
    codec = codecCase.make(
      (timestamp) => {
        timestamps.push(timestamp)
        if (shouldThrow) {
          shouldThrow = false
          // The first drain already owns the callback error when the overlap starts.
          second = codec.flush()
          void second.then(
            () => {
              deliveredAtSecondResolve = timestamps.length
            },
            () => {
              deliveredAtSecondResolve = timestamps.length
            },
          )
          throw sentinel
        }
      },
      (error) => errors.push(error),
    )
    codecCase.queue(codec)
    const [firstResult] = await Promise.allSettled([codec.flush()])
    if (second === undefined) {
      if (codec.state !== 'closed') codec.close()
      t.fail(`${codecCase.name}: second flush was not started`)
      continue
    }
    const [secondResult] = await Promise.allSettled([second])
    await delay(100)
    if (codec.state !== 'closed') codec.close()

    t.is(firstResult.status, 'rejected', `${codecCase.name}: first flush status`)
    if (firstResult.status === 'rejected') {
      t.true(firstResult.reason === sentinel, `${codecCase.name}: first flush must keep the sentinel`)
    }
    t.is(secondResult.status, 'fulfilled', `${codecCase.name}: second flush status`)
    t.deepEqual(errors, [], `${codecCase.name}: codec errors`)
    t.deepEqual(timestamps, expected, `${codecCase.name}: recovered timestamp order`)
    t.is(deliveredAtSecondResolve, expected.length, `${codecCase.name}: outputs at second resolution`)
    t.is(timestamps.length, deliveredAtSecondResolve, `${codecCase.name}: late outputs`)
  }
})

test('flush recovery precedes a reentrant configure error in every codec class', async (t) => {
  for (const codecCase of await flushOutputCases()) {
    const baseline = await flushOutputBaseline(codecCase)
    const expected = baseline.timestamps
    t.deepEqual(baseline.errors, [], `${codecCase.name}: baseline errors`)
    t.true(expected.length > 1, `${codecCase.name}: baseline should produce multiple outputs`)

    const sentinel = new Error(`${codecCase.name}: configure-error sentinel`)
    const events: string[] = []
    let shouldThrow = true
    let codec: FlushableCodec
    codec = codecCase.make(
      (timestamp) => {
        events.push(`output:${timestamp}`)
        if (shouldThrow) {
          shouldThrow = false
          codecCase.configureError(codec)
          throw sentinel
        }
      },
      (error) => events.push(`error:${error.name}`),
    )
    codecCase.queue(codec)
    const [flushResult] = await Promise.allSettled([codec.flush()])
    await waitFor(() => events.includes('error:NotSupportedError'), `${codecCase.name}: reentrant configure error`)
    await delay(50)

    t.is(flushResult.status, 'rejected', `${codecCase.name}: flush status`)
    if (flushResult.status === 'rejected') {
      t.true(flushResult.reason === sentinel, `${codecCase.name}: flush must keep the sentinel`)
    }
    t.deepEqual(
      events,
      [...expected.map((timestamp) => `output:${timestamp}`), 'error:NotSupportedError'],
      `${codecCase.name}: recovered suffix must precede the epoch error`,
    )
  }
})

test('flush recovery tracks a later flush without an initial overlap in every codec class', async (t) => {
  for (const codecCase of await flushOutputCases()) {
    const baseline = await flushOutputBaseline(codecCase)
    const expected = baseline.timestamps
    t.deepEqual(baseline.errors, [], `${codecCase.name}: baseline errors`)
    t.true(expected.length > 1, `${codecCase.name}: baseline should produce multiple outputs`)

    const sentinel = new Error(`${codecCase.name}: later-flush sentinel`)
    const timestamps: number[] = []
    const errors: DOMException[] = []
    let shouldThrow = true
    const codec = codecCase.make(
      (timestamp) => {
        timestamps.push(timestamp)
        if (shouldThrow) {
          shouldThrow = false
          throw sentinel
        }
      },
      (error) => errors.push(error),
    )
    codecCase.queue(codec)
    const [firstResult] = await Promise.allSettled([codec.flush()])
    const later = codec.flush()
    let deliveredAtLaterResolve = -1
    void later.then(
      () => {
        deliveredAtLaterResolve = timestamps.length
      },
      () => {
        deliveredAtLaterResolve = timestamps.length
      },
    )
    const [laterResult] = await Promise.allSettled([later])
    await delay(100)
    if (codec.state !== 'closed') codec.close()

    t.is(firstResult.status, 'rejected', `${codecCase.name}: first flush status`)
    if (firstResult.status === 'rejected') {
      t.true(firstResult.reason === sentinel, `${codecCase.name}: first flush must keep the sentinel`)
    }
    t.is(laterResult.status, 'fulfilled', `${codecCase.name}: later flush status`)
    t.deepEqual(errors, [], `${codecCase.name}: codec errors`)
    t.deepEqual(timestamps, expected, `${codecCase.name}: recovered timestamp order`)
    t.is(deliveredAtLaterResolve, expected.length, `${codecCase.name}: outputs at later resolution`)
    t.is(timestamps.length, deliveredAtLaterResolve, `${codecCase.name}: late outputs`)
  }
})

test('reset or close before a callback throw drops the flush suffix in every codec class', async (t) => {
  for (const teardown of ['reset', 'close'] as const) {
    for (const codecCase of await flushOutputCases()) {
      const baseline = await flushOutputBaseline(codecCase)
      const expected = baseline.timestamps
      t.deepEqual(baseline.errors, [], `${codecCase.name}: baseline errors`)
      t.true(expected.length > 1, `${codecCase.name}: baseline should produce multiple outputs`)

      const sentinel = new Error(`${codecCase.name}: ${teardown} sentinel`)
      const timestamps: number[] = []
      const errors: DOMException[] = []
      let shouldThrow = true
      let second: Promise<void> | undefined
      let codec: FlushableCodec
      codec = codecCase.make(
        (timestamp) => {
          timestamps.push(timestamp)
          if (shouldThrow) {
            shouldThrow = false
            // The first drain already owns the callback error when the overlap starts.
            second = codec.flush()
            codec[teardown]()
            throw sentinel
          }
        },
        (error) => errors.push(error),
      )
      codecCase.queue(codec)
      const [firstResult] = await Promise.allSettled([codec.flush()])
      if (second === undefined) {
        if (codec.state !== 'closed') codec.close()
        t.fail(`${codecCase.name}: ${teardown}: second flush was not started`)
        continue
      }
      const [secondResult] = await Promise.allSettled([second])
      await delay(50)
      const stateAfter = codec.state
      if (codec.state !== 'closed') codec.close()

      t.is(firstResult.status, 'rejected', `${codecCase.name}: ${teardown}: first flush status`)
      if (firstResult.status === 'rejected') {
        t.true(
          firstResult.reason === sentinel,
          `${codecCase.name}: ${teardown}: callback error identity, got ${String(firstResult.reason)}`,
        )
      }
      t.is(secondResult.status, 'rejected', `${codecCase.name}: ${teardown}: second flush status`)
      if (secondResult.status === 'rejected') {
        t.true(secondResult.reason instanceof DOMException, `${codecCase.name}: ${teardown}: abort type`)
        t.is(
          (secondResult.reason as DOMException).name,
          'AbortError',
          `${codecCase.name}: ${teardown}: second flush error`,
        )
      }
      t.deepEqual(errors, [], `${codecCase.name}: ${teardown}: codec errors`)
      t.deepEqual(timestamps, expected.slice(0, 1), `${codecCase.name}: ${teardown}: dropped suffix`)
      t.is(stateAfter, teardown === 'reset' ? 'unconfigured' : 'closed', `${codecCase.name}: final state`)
    }
  }
})

test('flush waits for outputs migrated by a configure error after its worker finishes', async (t) => {
  const inputCount = 2501
  let outputs = 0
  const errors: Error[] = []
  const codec = new AudioEncoder({
    output: () => {
      outputs++
    },
    error: (error) => errors.push(error),
  })
  codec.configure(AUDIO_CONFIG)
  for (let i = 0; i < inputCount; i++) {
    const data = audioInput(i)
    codec.encode(data)
    data.close()
  }
  await Promise.resolve()
  await Promise.resolve()

  const flushing = codec.flush()
  await Promise.resolve()
  await Promise.resolve()

  // Keep JavaScript busy while the worker answers flush, then migrate its
  // buffered outputs before the flush resolver can run.
  const sleeper = new Int32Array(new SharedArrayBuffer(4))
  const deadline = Date.now() + 20_000
  while (codec.encodeQueueSize > 0 && Date.now() < deadline) {
    Atomics.wait(sleeper, 0, 0, 1)
  }
  t.is(codec.encodeQueueSize, 0)
  Atomics.wait(sleeper, 0, 0, 100)
  codec.configure({ ...AUDIO_CONFIG, codec: 'unsupported-audio-codec' })

  await flushing.catch(() => {})
  t.is(outputs, inputCount + 1)
  await new Promise((resolve) => setTimeout(resolve, 100))
  t.is(outputs, inputCount + 1)
  t.is(errors.length, 1)
})

interface FlushAtomicitySummary {
  iterations: number
  completed: number
  expected: number
  races: number
  early: number
  incomplete: number
  badErrorCounts: number
  badOrdering: number
  timeouts: number
  firstFailures: unknown[]
  elapsedMs: number
}

interface FlushAtomicityResult {
  code: number | null
  signal: NodeJS.Signals | null
  summary: FlushAtomicitySummary
}

function runFlushAtomicityProbe(): Promise<FlushAtomicityResult> {
  const marker = 'FLUSH_ATOMICITY_SUMMARY '
  const maxOutputLength = 32 * 1024 * 1024
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ['__test__/helpers/flush-atomicity-probe.cjs'], {
      env: { ...process.env, WEBCODECS_LOG: 'trace', FLUSH_ATOMICITY_ITERATIONS: '2000' },
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    let pending = ''
    let outputLength = 0
    let summary: FlushAtomicitySummary | undefined
    let settled = false

    const fail = (error: Error) => {
      if (settled) return
      settled = true
      clearTimeout(timeout)
      child.kill('SIGKILL')
      reject(error)
    }
    const parseLine = (line: string) => {
      if (!line.startsWith(marker)) return
      try {
        summary = JSON.parse(line.slice(marker.length)) as FlushAtomicitySummary
      } catch (error) {
        fail(new Error(`Invalid flush atomicity summary: ${String(error)}`))
      }
    }
    const consume = (chunk: string) => {
      outputLength += chunk.length
      if (outputLength > maxOutputLength) {
        fail(new Error(`Flush atomicity probe exceeded ${maxOutputLength} output characters`))
        return
      }
      pending += chunk
      let newline = pending.indexOf('\n')
      while (newline >= 0) {
        parseLine(pending.slice(0, newline))
        pending = pending.slice(newline + 1)
        newline = pending.indexOf('\n')
      }
      // Native tracing is line-oriented; never retain an unbounded partial line.
      if (pending.length > 65_536) pending = ''
    }
    const timeout = setTimeout(() => {
      fail(new Error('Flush atomicity probe timed out'))
    }, 60_000)

    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', consume)
    child.stderr.on('data', consume)
    child.on('error', fail)
    child.on('close', (code, signal) => {
      if (settled) return
      parseLine(pending)
      settled = true
      clearTimeout(timeout)
      if (!summary) {
        reject(new Error(`Flush atomicity probe produced no summary (code=${code}, signal=${signal})`))
        return
      }
      resolve({ code, signal, summary })
    })
  })
}

// This is a bounded stress regression, not a deterministic schedule. Native
// tracing widens the locked reconfigure window that used to let flush settle
// between its optimistic delivery check and the pending-output drain.
test('flush rechecks delivery under the codec lock before worker-error migration drains outputs', async (t) => {
  const { code, signal, summary } = await runFlushAtomicityProbe()
  const details = JSON.stringify(summary)
  t.is(signal, null, details)
  t.is(summary.completed, summary.iterations, details)
  t.is(summary.expected, 5, details)
  t.is(summary.races, 0, details)
  t.is(summary.early, 0, details)
  t.is(summary.incomplete, 0, details)
  t.is(summary.badErrorCounts, 0, details)
  t.is(summary.badOrdering, 0, details)
  t.is(summary.timeouts, 0, details)
  t.is(code, 0, details)
})

interface FlushShutdownSummary {
  initialThreads: number
  finalThreads: number
  rows: Array<{ delta: number; exitCode: number }>
}

test.serial('Worker.terminate releases delivery waiters during environment cleanup', (t) => {
  const marker = 'FLUSH_SHUTDOWN_SUMMARY '
  const output = execFileSync(process.execPath, ['__test__/helpers/flush-shutdown-probe.cjs'], {
    cwd: process.cwd(),
    timeout: 180_000,
    encoding: 'utf8',
    maxBuffer: 4 * 1024 * 1024,
  })
  const line = output
    .trim()
    .split('\n')
    .find((entry) => entry.startsWith(marker))
  t.truthy(line)
  if (!line) return
  const summary = JSON.parse(line.slice(marker.length)) as FlushShutdownSummary
  const details = JSON.stringify(summary)
  t.is(summary.rows.length, 6, details)
  t.true(summary.rows.every((row) => row.exitCode === 1), details)
  t.true(
    summary.finalThreads <= summary.initialThreads + 2,
    `delivery waiters accumulated during worker teardown: ${details}`,
  )
})
