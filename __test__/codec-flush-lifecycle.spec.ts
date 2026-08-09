import test from 'ava'

import {
  AudioDecoder,
  AudioEncoder,
  VideoDecoder,
  VideoEncoder,
  type CodecState,
} from '../index.js'

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
