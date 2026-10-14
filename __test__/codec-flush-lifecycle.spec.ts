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
import { generateGradientI420Frame, generateSineTone } from './helpers/index.js'

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

test('errored encoders reject flush with native EncodingError DOMExceptions', async (t) => {
  const encoders = [
    new AudioEncoder({ output() {}, error() {} }),
    new VideoEncoder({ output() {}, error() {} }),
  ] as const

  encoders[0].configure({ codec: 'unsupported-audio-codec', sampleRate: 48000, numberOfChannels: 2 })
  encoders[1].configure({ codec: 'unsupported-video-codec', width: 64, height: 64 })

  for (const encoder of encoders) {
    t.is(encoder.state, 'closed')
    const error = await t.throwsAsync(encoder.flush())
    t.true(error instanceof DOMException)
    t.is((error as DOMException).name, 'EncodingError')
  }
})

const AUDIO_CONFIG = { codec: 'mp4a.40.2', sampleRate: 48_000, numberOfChannels: 2, bitrate: 128_000 }
const VIDEO_CONFIG = { codec: 'vp8', width: 64, height: 64, hardwareAcceleration: 'prefer-software' } as const
const AUDIO_FRAMES = 1500
const VIDEO_FRAMES = 3000

const audioInput = (i: number) =>
  generateSineTone(440, 1024, 2, 48_000, 'f32', Math.round((i * 1024 * 1_000_000) / 48_000))
const videoInput = (i: number) => generateGradientI420Frame(64, 64, i * 33_333, 33_333)

async function encodeAudio() {
  const chunks: EncodedAudioChunk[] = []
  let config: AudioDecoderConfig | undefined
  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      config ??= metadata?.decoderConfig
    },
    error() {},
  })
  encoder.configure(AUDIO_CONFIG)
  for (let i = 0; i < AUDIO_FRAMES; i++) {
    const data = audioInput(i)
    encoder.encode(data)
    data.close()
  }
  await encoder.flush()
  encoder.close()
  return { chunks, config: config! }
}

async function encodeVideo() {
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
  for (let i = 0; i < VIDEO_FRAMES; i++) {
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
async function feedWithBackpressure(codec: QueueingCodec, queueSize: () => number, count: number, feed: (i: number) => void) {
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
        const codec = new AudioEncoder({ output: (chunk) => timestamps.push(chunk.timestamp), error: (e) => t.fail(e.message) })
        codec.configure(AUDIO_CONFIG)
        await feedWithBackpressure(codec, () => codec.encodeQueueSize, AUDIO_FRAMES, (i) => {
          const data = audioInput(i)
          codec.encode(data)
          data.close()
        })
        return codec
      },
    ],
    [
      'VideoEncoder',
      VIDEO_FRAMES,
      async (timestamps) => {
        const codec = new VideoEncoder({ output: (chunk) => timestamps.push(chunk.timestamp), error: (e) => t.fail(e.message) })
        codec.configure(VIDEO_CONFIG)
        await feedWithBackpressure(codec, () => codec.encodeQueueSize, VIDEO_FRAMES, (i) => {
          const frame = videoInput(i)
          codec.encode(frame, { keyFrame: i === 0 })
          frame.close()
        })
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
        await feedWithBackpressure(codec, () => codec.decodeQueueSize, audio.chunks.length, (i) => codec.decode(audio.chunks[i]))
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
        await feedWithBackpressure(codec, () => codec.decodeQueueSize, video.chunks.length, (i) => codec.decode(video.chunks[i]))
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
