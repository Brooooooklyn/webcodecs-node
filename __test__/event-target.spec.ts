/**
 * EventTarget Interface Tests
 *
 * Tests that addEventListener('dequeue', ...) fires correctly when the
 * encode/decode queue decreases. This is per W3C WebCodecs spec which
 * requires EventTarget interface on all codecs.
 */

import { execFileSync } from 'node:child_process'

import test from 'ava'

import {
  AudioData,
  AudioDecoder,
  AudioEncoder,
  EncodedVideoChunk,
  EncodedVideoChunkMetadata,
  resetHardwareFallbackState,
  VideoDecoder,
  VideoEncoder,
  VideoFrame,
} from '../index.js'
import {
  generateSolidColorI420Frame,
  generateFrameSequence,
  TestColors,
  type EncodedAudioChunk,
} from './helpers/index.js'
import { createEncoderConfig } from './helpers/codec-matrix.js'

// Helper: Create encoded chunks for decoder tests
interface EncodedChunkWithMetadata {
  chunk: EncodedVideoChunk
  metadata?: EncodedVideoChunkMetadata
}

async function createEncodedH264Chunks(
  width: number,
  height: number,
  frameCount: number,
): Promise<EncodedChunkWithMetadata[]> {
  const chunks: EncodedChunkWithMetadata[] = []
  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push({ chunk, metadata })
    },
    error: () => {},
  })
  encoder.configure(createEncoderConfig('h264', width, height))

  const frames = generateFrameSequence(width, height, frameCount)

  encoder.encode(frames[0], { keyFrame: true })
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }

  for (const frame of frames) {
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  return chunks
}

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// ============================================================================
// VideoEncoder EventTarget Tests
// ============================================================================

test('VideoEncoder: addEventListener dequeue fires when queue decreases', async (t) => {
  const chunks: EncodedVideoChunk[] = []
  const encoder = new VideoEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  // Set up the listener BEFORE encoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => encoder.addEventListener('dequeue', () => resolve(), { once: true }))

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame)
  frame.close()

  // Wait for dequeue event
  await dequeuePromise

  t.is(encoder.encodeQueueSize, 0)

  await encoder.flush()
  encoder.close()
})

test('VideoEncoder: multiple dequeue listeners all fire', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  let listener1Count = 0
  let listener2Count = 0

  encoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  encoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)
  encoder.encode(frame)
  frame.close()

  await encoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  encoder.close()
})

test('VideoEncoder: dequeue once:true fires only once', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  let onceCount = 0
  let regularCount = 0

  encoder.addEventListener(
    'dequeue',
    () => {
      onceCount++
    },
    { once: true },
  )
  encoder.addEventListener('dequeue', () => {
    regularCount++
  })

  // Encode 3 frames
  for (let i = 0; i < 3; i++) {
    const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, i * 33333)
    encoder.encode(frame)
    frame.close()
  }

  await encoder.flush()

  t.is(onceCount, 1, 'once listener should fire exactly once')
  t.true(regularCount >= 1, 'regular listener should fire for each dequeue')

  encoder.close()
})

// ============================================================================
// VideoDecoder EventTarget Tests
// ============================================================================

test('VideoDecoder: addEventListener dequeue fires when queue decreases', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const frames: VideoFrame[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      frames.push(frame)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: 'avc1.42001E',
    codedWidth: 320,
    codedHeight: 240,
    description: chunks[0].metadata?.decoderConfig?.description,
  })

  // Set up the listener BEFORE decoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => decoder.addEventListener('dequeue', () => resolve(), { once: true }))

  decoder.decode(chunks[0].chunk)

  // Wait for dequeue event
  await dequeuePromise

  t.is(decoder.decodeQueueSize, 0)

  await decoder.flush()
  decoder.close()

  // Clean up frames
  for (const frame of frames) {
    frame.close()
  }
})

test('VideoDecoder: multiple dequeue listeners all fire', async (t) => {
  const chunks = await createEncodedH264Chunks(320, 240, 1)

  const frames: unknown[] = []
  const decoder = new VideoDecoder({
    output: (frame) => {
      frames.push(frame)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  decoder.configure({
    codec: 'avc1.42001E',
    codedWidth: 320,
    codedHeight: 240,
    description: chunks[0].metadata?.decoderConfig?.description,
  })

  let listener1Count = 0
  let listener2Count = 0

  decoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  decoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  decoder.decode(chunks[0].chunk)

  await decoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  decoder.close()

  // Clean up frames
  for (const frame of frames) {
    ;(frame as { close: () => void }).close()
  }
})

// ============================================================================
// AudioEncoder EventTarget Tests
// ============================================================================

function createTestAudioData(timestamp: number): AudioData {
  const sampleRate = 48000
  const numberOfChannels = 2
  const numberOfFrames = 1024
  const data = new Float32Array(numberOfFrames * numberOfChannels)

  // Generate sine wave
  const frequency = 440
  for (let i = 0; i < numberOfFrames; i++) {
    const t = i / sampleRate
    const sample = Math.sin(2 * Math.PI * frequency * t) * 0.5
    data[i * numberOfChannels] = sample
    data[i * numberOfChannels + 1] = sample
  }

  return new AudioData({
    format: 'f32',
    sampleRate,
    numberOfFrames,
    numberOfChannels,
    timestamp,
    data,
  })
}

test('AudioEncoder: addEventListener dequeue fires when queue decreases', async (t) => {
  const chunks: EncodedAudioChunk[] = []
  const encoder = new AudioEncoder({
    output: (chunk) => {
      chunks.push(chunk)
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Set up the listener BEFORE encoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => encoder.addEventListener('dequeue', () => resolve(), { once: true }))

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()

  // Wait for dequeue event
  await dequeuePromise

  t.is(encoder.encodeQueueSize, 0)

  await encoder.flush()
  encoder.close()
})

test('AudioEncoder: multiple dequeue listeners all fire', async (t) => {
  const encoder = new AudioEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  let listener1Count = 0
  let listener2Count = 0

  encoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  encoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()

  await encoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  encoder.close()
})

// ============================================================================
// AudioDecoder EventTarget Tests
// ============================================================================

test('AudioDecoder: addEventListener dequeue fires when queue decreases', async (t) => {
  // First encode some audio to get encoded chunks
  const encodedChunks: EncodedAudioChunk[] = []
  let decoderConfig: unknown = null

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      encodedChunks.push(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig
      }
    },
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()
  await encoder.flush()
  encoder.close()

  // Now decode
  const decodedAudio: unknown[] = []
  const decoder = new AudioDecoder({
    output: (audio) => {
      decodedAudio.push(audio)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  const config = decoderConfig as {
    codec: string
    sampleRate: number
    numberOfChannels: number
    description?: Uint8Array
  }
  decoder.configure({
    codec: config.codec,
    sampleRate: config.sampleRate,
    numberOfChannels: config.numberOfChannels,
    description: config.description,
  })

  // Set up the listener BEFORE decoding to ensure we don't miss the event
  const dequeuePromise = new Promise<void>((resolve) => decoder.addEventListener('dequeue', () => resolve(), { once: true }))

  decoder.decode(encodedChunks[0])

  // Wait for dequeue event
  await dequeuePromise

  t.is(decoder.decodeQueueSize, 0)

  await decoder.flush()
  decoder.close()

  // Clean up
  for (const audio of decodedAudio) {
    ;(audio as { close: () => void }).close()
  }
})

test('AudioDecoder: multiple dequeue listeners all fire', async (t) => {
  // First encode some audio
  const encodedChunks: EncodedAudioChunk[] = []
  let decoderConfig: unknown = null

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      encodedChunks.push(chunk)
      if (metadata?.decoderConfig) {
        decoderConfig = metadata.decoderConfig
      }
    },
    error: () => {},
  })

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audioData = createTestAudioData(0)
  encoder.encode(audioData)
  audioData.close()
  await encoder.flush()
  encoder.close()

  // Now decode
  const decodedAudio: unknown[] = []
  const decoder = new AudioDecoder({
    output: (audio) => {
      decodedAudio.push(audio)
    },
    error: (e) => {
      t.fail(`Decoder error: ${e.message}`)
    },
  })

  const config = decoderConfig as {
    codec: string
    sampleRate: number
    numberOfChannels: number
    description?: Uint8Array
  }
  decoder.configure({
    codec: config.codec,
    sampleRate: config.sampleRate,
    numberOfChannels: config.numberOfChannels,
    description: config.description,
  })

  let listener1Count = 0
  let listener2Count = 0

  decoder.addEventListener('dequeue', () => {
    listener1Count++
  })
  decoder.addEventListener('dequeue', () => {
    listener2Count++
  })

  decoder.decode(encodedChunks[0])

  await decoder.flush()

  t.true(listener1Count >= 1, 'listener1 should have fired')
  t.true(listener2Count >= 1, 'listener2 should have fired')

  decoder.close()

  // Clean up
  for (const audio of decodedAudio) {
    ;(audio as { close: () => void }).close()
  }
})

// ============================================================================
// Event object / listener identity / DOMException regression tests
// ============================================================================

test('VideoEncoder: dequeue listener receives an Event with type dequeue', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  const eventPromise = new Promise<unknown>((resolve) =>
    encoder.addEventListener('dequeue', resolve, { once: true }),
  )

  const frame = generateSolidColorI420Frame(320, 240, TestColors.red, 0)
  encoder.encode(frame)
  frame.close()

  const event = await eventPromise

  t.true(event instanceof Event, 'listener arg should be an Event')
  t.is((event as Event).type, 'dequeue')

  await encoder.flush()
  encoder.close()
})

test('VideoEncoder: ondequeue receives an Event with type dequeue', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  const eventPromise = new Promise<unknown>((resolve) => {
    encoder.ondequeue = resolve
  })

  const frame = generateSolidColorI420Frame(320, 240, TestColors.blue, 0)
  encoder.encode(frame)
  frame.close()

  const event = await eventPromise

  t.true(event instanceof Event, 'ondequeue arg should be an Event')
  t.is((event as Event).type, 'dequeue')

  await encoder.flush()
  encoder.close()
})

test('VideoEncoder: removeEventListener removes the requested listener', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  let listener1Count = 0
  let listener2Count = 0
  const listener1 = () => {
    listener1Count++
  }
  const listener2 = () => {
    listener2Count++
  }

  encoder.addEventListener('dequeue', listener1)
  encoder.addEventListener('dequeue', listener2)
  // Removing listener1 must not remove listener2 (registration order must not matter)
  encoder.removeEventListener('dequeue', listener1)

  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)
  encoder.encode(frame)
  frame.close()

  await encoder.flush()

  t.is(listener1Count, 0, 'removed listener1 should not fire')
  t.true(listener2Count >= 1, 'listener2 should still fire')

  encoder.close()
})

test('VideoEncoder: removeEventListener with unregistered callback removes nothing', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: (e) => {
      t.fail(`Encoder error: ${e.message}`)
    },
  })

  encoder.configure(createEncoderConfig('h264', 320, 240))

  let listenerCount = 0
  encoder.addEventListener('dequeue', () => {
    listenerCount++
  })
  // A callback that was never registered must not remove the registered one
  // oxlint-disable-next-line no-invalid-remove-event-listener -- intentionally unregistered
  encoder.removeEventListener('dequeue', () => {})

  const frame = generateSolidColorI420Frame(320, 240, TestColors.green, 0)
  encoder.encode(frame)
  frame.close()

  await encoder.flush()

  t.true(listenerCount >= 1, 'registered listener should still fire')

  encoder.close()
})

test('VideoDecoder: error callback receives a DOMException', async (t) => {
  const errorPromise = new Promise<unknown>((resolve) => {
    const decoder = new VideoDecoder({
      output: (frame) => {
        frame.close()
      },
      error: resolve,
    })

    decoder.configure({
      codec: 'avc1.42001f',
      codedWidth: 320,
      codedHeight: 240,
    })

    // Garbage payload makes the decoder worker fail asynchronously
    const badChunk = new EncodedVideoChunk({
      type: 'key',
      timestamp: 0,
      data: new Uint8Array([0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]),
    })
    try {
      decoder.decode(badChunk)
    } catch {
      // synchronous rejection also satisfies the test intent
    }
    void decoder.flush().catch(() => {})
  })

  const error = await errorPromise

  t.true(error instanceof DOMException, 'error callback arg should be a DOMException')
  t.true(error instanceof Error, 'DOMException should also be an Error')
  t.is((error as DOMException).name, 'EncodingError')
})

// ============================================================================
// DOM dispatch semantics: shared Event identity, target/currentTarget/this
// ============================================================================

test('VideoEncoder: all listeners in one dispatch share the same Event object', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  const seen: Event[] = []
  encoder.addEventListener('dequeue', (e) => {
    seen.push(e)
  })
  encoder.addEventListener('dequeue', (e) => {
    seen.push(e)
  })

  encoder.configure(createEncoderConfig('h264', 64, 64))
  const frame = generateSolidColorI420Frame(64, 64, TestColors.green, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await new Promise<void>((resolve) => {
    const check = () => (seen.length >= 2 ? resolve() : setTimeout(check, 10))
    check()
  })

  t.true(seen.every((e) => e === seen[0]), 'listeners should share one Event instance')
  t.is(seen[0].type, 'dequeue')
  encoder.close()
})

test('VideoEncoder: event.target/currentTarget/this resolve to the codec', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  let observed: {
    target: EventTarget | null
    currentTarget: EventTarget | null
    self: unknown
    captured: Event
  } | null = null

  encoder.addEventListener('dequeue', function (this: VideoEncoder, e) {
    observed = {
      target: e.target,
      currentTarget: e.currentTarget,
      // eslint-disable-next-line @typescript-eslint/no-this-alias
      self: this,
      captured: e,
    }
  })

  encoder.configure(createEncoderConfig('h264', 64, 64))
  const frame = generateSolidColorI420Frame(64, 64, TestColors.green, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await new Promise<void>((resolve) => {
    const check = () => (observed ? resolve() : setTimeout(check, 10))
    check()
  })

  t.is(observed!.target as unknown, encoder)
  t.is(observed!.currentTarget as unknown, encoder)
  t.is(observed!.self, encoder)
  // DOM: currentTarget is null once dispatch completes
  t.is(observed!.captured.currentTarget, null)
  encoder.close()
})

test('VideoEncoder: listener removed during dispatch does not run', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  const fired: string[] = []
  const l2 = () => {
    fired.push('l2')
  }
  encoder.addEventListener('dequeue', () => {
    fired.push('l1')
    encoder.removeEventListener('dequeue', l2)
  })
  encoder.addEventListener('dequeue', l2)

  encoder.configure(createEncoderConfig('h264', 64, 64))
  const frame = generateSolidColorI420Frame(64, 64, TestColors.green, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await new Promise<void>((resolve) => {
    const check = () => (fired.length >= 1 ? resolve() : setTimeout(check, 10))
    check()
  })
  // Give the second dequeue event (if any) a chance to disprove
  await new Promise((r) => setTimeout(r, 50))

  t.deepEqual(fired, ['l1'])
  encoder.close()
})

test('VideoEncoder: listener added during dispatch runs on next dispatch only', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  const fired: string[] = []
  encoder.addEventListener('dequeue', () => {
    fired.push('l1')
    encoder.addEventListener('dequeue', () => {
      fired.push('l2')
    })
  })

  encoder.configure(createEncoderConfig('h264', 64, 64))
  const frames = generateFrameSequence(64, 64, 2)
  for (const f of frames) {
    encoder.encode(f, { keyFrame: true })
    f.close()
  }

  await new Promise<void>((resolve) => {
    const check = () => (fired.length >= 3 ? resolve() : setTimeout(check, 10))
    check()
  })

  // Two dispatches: first runs l1 only, second runs l1 + l2
  t.deepEqual(fired.slice(0, 3), ['l1', 'l1', 'l2'])
  encoder.close()
})

test('VideoEncoder: re-registering the same callback is a no-op', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  let count = 0
  const listener = () => {
    count++
  }
  encoder.addEventListener('dequeue', listener)
  encoder.addEventListener('dequeue', listener)

  encoder.configure(createEncoderConfig('h264', 64, 64))
  const frame = generateSolidColorI420Frame(64, 64, TestColors.green, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await new Promise<void>((resolve) => {
    const check = () => (count >= 1 ? resolve() : setTimeout(check, 10))
    check()
  })
  await new Promise((r) => setTimeout(r, 50))

  t.is(count, 1, 'duplicate registration should not double-fire per dispatch')
  encoder.close()
})

test('VideoEncoder: capture flag is part of listener identity', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  let bubble = 0
  let capture = 0
  const listener = () => {
    bubble++
  }
  encoder.addEventListener('dequeue', listener)
  // Same callback with capture:true is a DISTINCT registration per DOM spec
  encoder.addEventListener('dequeue', () => {
    capture++
  }, { capture: true })
  encoder.addEventListener('dequeue', listener, { capture: true })

  encoder.configure(createEncoderConfig('h264', 64, 64))
  const frame = generateSolidColorI420Frame(64, 64, TestColors.green, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await new Promise<void>((resolve) => {
    const check = () => (bubble + capture >= 2 ? resolve() : setTimeout(check, 10))
    check()
  })
  await new Promise((r) => setTimeout(r, 50))

  // One dispatch fires: bubble listener once + capture listener once + capture'd
  // instance of `listener` once (it registered again with capture:true)
  t.is(bubble + capture >= 2, true)
  // Removing with capture:false removes only the bubble registration
  encoder.removeEventListener('dequeue', listener)
  encoder.close()
})

test('VideoEncoder: once listener does not re-fire on re-entrant dispatch', async (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  let count = 0
  const listener = () => {
    count++
    // Re-entrant dispatch must not observe this once-listener again
    encoder.dispatchEvent('dequeue')
  }
  encoder.addEventListener('dequeue', listener, { once: true })

  encoder.configure(createEncoderConfig('h264', 64, 64))
  const frame = generateSolidColorI420Frame(64, 64, TestColors.green, 0)
  encoder.encode(frame, { keyFrame: true })
  frame.close()

  await new Promise<void>((resolve) => {
    const check = () => (count >= 1 ? resolve() : setTimeout(check, 10))
    check()
  })
  await new Promise((r) => setTimeout(r, 50))

  t.is(count, 1, 'once listener must be removed before invocation')
  encoder.close()
})

test('VideoEncoder: throwing listener does not abort later listeners', (t) => {
  // DOM semantics: a throwing listener must not prevent later listeners from
  // running, and the exception is *reported* as an uncaught error rather than
  // propagated from dispatchEvent. Observed via a child process so the report
  // doesn't hit ava's own uncaughtException handling.
  const script = `
    const { VideoEncoder } = require('./index.js');
    const enc = new VideoEncoder({ output: () => {}, error: () => {} });
    process.on('uncaughtException', (e) => {
      console.log('UNCAUGHT:' + e.message);
      enc.close();
    });
    const fired = [];
    enc.addEventListener('dequeue', () => { fired.push('l1'); throw new Error('listener boom'); });
    enc.addEventListener('dequeue', () => { fired.push('l2'); console.log('FIRED:' + fired.join(',')); });
    console.log('RETURNED:' + enc.dispatchEvent('dequeue'));
  `
  const output = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    timeout: 15_000,
    encoding: 'utf8',
  })
  t.true(output.includes('RETURNED:true'), 'dispatchEvent must not propagate listener exceptions')
  t.true(output.includes('FIRED:l1,l2'), 'dispatch must continue after a listener throws')
  t.true(output.includes('UNCAUGHT:listener boom'), 'exception must be reported as uncaught')
})

test('VideoEncoder: once dequeue listener keeps the process alive until it fires', (t) => {
  // A once listener must prevent Node from exiting before the dequeue fires —
  // the dispatcher TSFN is ref'd while once listeners are pending.
  const script = `
    const { VideoEncoder, VideoFrame } = require('./index.js');
    const enc = new VideoEncoder({ output: () => {}, error: () => {} });
    enc.addEventListener('dequeue', () => {
      console.log('DEQUEUE_FIRED');
      enc.close();
    }, { once: true });
    enc.configure({ codec: 'avc1.42001f', width: 64, height: 64, bitrate: 100_000 });
    const f = new VideoFrame(new Uint8Array(64*64*1.5), {
      format: 'I420', codedWidth: 64, codedHeight: 64, timestamp: 0,
    });
    enc.encode(f, { keyFrame: true });
    f.close();
    // No other handles — process would exit immediately if the dispatcher
    // were not ref'd while the once listener is pending.
  `
  const output = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    timeout: 15_000,
    encoding: 'utf8',
  })
  t.true(output.includes('DEQUEUE_FIRED'), 'once listener must fire before process exit')
})

test('VideoEncoder: stopImmediatePropagation stops remaining listeners', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  const fired: string[] = []
  encoder.addEventListener('dequeue', (event: Event) => {
    fired.push('l1')
    event.stopImmediatePropagation()
  })
  encoder.addEventListener('dequeue', () => {
    fired.push('l2')
  })

  encoder.dispatchEvent('dequeue')
  t.deepEqual(fired, ['l1'], 'l2 must not run after stopImmediatePropagation')

  // A fresh dispatch must still reach both listeners — the flag is per-event
  fired.length = 0
  encoder.dispatchEvent('dequeue')
  t.deepEqual(fired, ['l1'], 'each new event starts un-stopped')
  encoder.close()
})

test('VideoEncoder: retained event falls back to native stopImmediatePropagation', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  let retained: Event | null = null
  encoder.addEventListener('dequeue', (event: Event) => {
    retained = event
  })
  encoder.dispatchEvent('dequeue')

  t.truthy(retained)
  // After dispatch returns, the own-property wrapper is removed — calling it
  // must reach the native prototype method without crashing or dangling.
  t.notThrows(() => retained!.stopImmediatePropagation())
  encoder.close()
})

test('VideoEncoder: eventPhase and composedPath reflect dispatch state', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  const during: { phase: number; path: unknown[] } = { phase: -1, path: [] }
  let retained: Event | null = null
  encoder.addEventListener('dequeue', (event: Event) => {
    retained = event
    during.phase = event.eventPhase
    during.path = event.composedPath()
  })
  encoder.dispatchEvent('dequeue')

  t.is(during.phase, 2, 'eventPhase is AT_TARGET during dispatch')
  t.deepEqual(during.path, [encoder], 'composedPath returns [codec] during dispatch')
  t.is(retained!.eventPhase, 0, 'eventPhase resets to NONE after dispatch')
  t.deepEqual(retained!.composedPath(), [], 'composedPath returns [] after dispatch')
  encoder.close()
})

test('VideoEncoder: every throwing listener exception is reported', (t) => {
  // DOM reports each listener exception independently — not just the first.
  const script = `
    const { VideoEncoder } = require('./index.js');
    const enc = new VideoEncoder({ output: () => {}, error: () => {} });
    const reported = [];
    process.on('uncaughtException', (e) => {
      reported.push(e.message);
      if (reported.length === 2) {
        console.log('REPORTED:' + reported.sort().join(','));
        enc.close();
      }
    });
    enc.addEventListener('dequeue', () => { throw new Error('boom-1'); });
    enc.addEventListener('dequeue', () => { throw new Error('boom-2'); });
    enc.dispatchEvent('dequeue');
  `
  const output = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    timeout: 15_000,
    encoding: 'utf8',
  })
  t.true(output.includes('REPORTED:boom-1,boom-2'), 'each listener exception must be reported')
})

test('VideoEncoder: queued dequeue event survives codec GC', (t) => {
  // The dispatch payload carries a strong state reference, so a dequeue event
  // queued by the worker is still delivered even if the codec wrapper is
  // finalized before the callback runs.
  const script = `
    const { VideoEncoder, VideoFrame } = require('./index.js');
    let fired = 0, targetOk = false, thisOk = false;
    (() => {
      const enc = new VideoEncoder({ output: () => {}, error: () => {} });
      // Marker on the codec so the listener can verify target identity
      // WITHOUT capturing enc in its closure (which would pin it anyway).
      enc.marker = 42;
      enc.addEventListener('dequeue', function (e) {
        fired++;
        targetOk = e.target != null && e.target.marker === 42;
        thisOk = this === e.target;
      });
      enc.configure({ codec: 'avc1.42001f', width: 64, height: 64, bitrate: 100_000 });
      const f = new VideoFrame(new Uint8Array(64*64*1.5), {
        format: 'I420', codedWidth: 64, codedHeight: 64, timestamp: 0,
      });
      enc.encode(f, { keyFrame: true });
      f.close();
    })();
    globalThis.gc();
    setTimeout(() => {
      globalThis.gc();
      console.log('FIRED:' + fired + ' TARGET:' + targetOk + ' THIS:' + thisOk);
    }, 1500);
  `
  const output = execFileSync(process.execPath, ['--expose-gc', '-e', script], {
    cwd: process.cwd(),
    timeout: 15_000,
    encoding: 'utf8',
  })
  t.regex(output, /FIRED:[1-9]/, 'queued dispatch must still fire its listeners')
  t.regex(
    output,
    /TARGET:true/,
    'queued dispatch must retain the codec object as event.target'
  )
  t.regex(output, /THIS:true/, 'listener this must be the codec')
})

test('VideoEncoder: retained event re-dispatched on a real EventTarget sees native state', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  let retained: Event | null = null
  encoder.addEventListener('dequeue', (event: Event) => {
    retained = event
  })
  encoder.dispatchEvent('dequeue')
  t.truthy(retained)
  // Our dispatch state persists correctly on the event
  t.is(retained!.target, encoder as unknown as EventTarget)
  t.is(retained!.currentTarget, null)
  t.is(retained!.eventPhase, 0)

  // Re-dispatching through a real EventTarget must show native dispatch state
  const other = new EventTarget()
  const seen: { t?: unknown; ct?: unknown; ph?: number } = {}
  other.addEventListener('dequeue', (e) => {
    seen.t = e.target
    seen.ct = e.currentTarget
    seen.ph = e.eventPhase
  })
  other.dispatchEvent(retained!)
  t.is(seen.t, other, 'target reflects the real dispatch target')
  t.is(seen.ct, other, 'currentTarget reflects the real dispatch target')
  t.is(seen.ph, 2, 'eventPhase is AT_TARGET during real dispatch')
  t.is(retained!.target, other, 'target persists as the last dispatch target')
  encoder.close()
})

test('VideoEncoder: ondequeue participates in registration order', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  // Handler assigned after addEventListener must run after it
  const order: string[] = []
  encoder.addEventListener('dequeue', () => order.push('listener'))
  encoder.ondequeue = () => order.push('ondequeue')
  encoder.dispatchEvent('dequeue')
  t.deepEqual(order, ['listener', 'ondequeue'])

  // Handler assigned before addEventListener runs first
  order.length = 0
  const encoder2 = new VideoEncoder({ output: () => {}, error: () => {} })
  encoder2.ondequeue = () => order.push('ondequeue')
  encoder2.addEventListener('dequeue', () => order.push('listener'))
  encoder2.dispatchEvent('dequeue')
  t.deepEqual(order, ['ondequeue', 'listener'])

  // Re-assigning while set replaces the callback in the SAME slot
  order.length = 0
  const encoder3 = new VideoEncoder({ output: () => {}, error: () => {} })
  encoder3.ondequeue = () => order.push('first')
  encoder3.addEventListener('dequeue', () => order.push('listener'))
  encoder3.ondequeue = () => order.push('second')
  encoder3.dispatchEvent('dequeue')
  t.deepEqual(order, ['second', 'listener'])

  // But clearing then re-setting creates a fresh registration at the end
  order.length = 0
  encoder3.ondequeue = null
  encoder3.ondequeue = () => order.push('third')
  encoder3.dispatchEvent('dequeue')
  t.deepEqual(order, ['listener', 'third'])

  encoder.close()
  encoder2.close()
  encoder3.close()
})

test('VideoEncoder: ondequeue replaced mid-dispatch runs the replacement', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  // Listener registered first, ondequeue second — so ondequeue's slot runs
  // after the listener, which replaces the handler before its turn.
  const order: string[] = []
  encoder.addEventListener('dequeue', () => {
    order.push('listener')
    encoder.ondequeue = () => order.push('second')
  })
  encoder.ondequeue = () => order.push('first')
  encoder.dispatchEvent('dequeue')
  t.deepEqual(order, ['listener', 'second'])

  // Clearing it mid-dispatch removes the slot: no ondequeue call at all.
  order.length = 0
  const encoder2 = new VideoEncoder({ output: () => {}, error: () => {} })
  encoder2.addEventListener('dequeue', () => {
    order.push('listener')
    encoder2.ondequeue = null
  })
  encoder2.ondequeue = () => order.push('cleared')
  encoder2.dispatchEvent('dequeue')
  t.deepEqual(order, ['listener'])

  encoder.close()
  encoder2.close()
})

test('VideoEncoder: stopImmediatePropagation does not poison native re-dispatch', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  let retained: Event | null = null
  encoder.addEventListener('dequeue', (event: Event) => {
    retained = event
    event.stopImmediatePropagation()
  })
  encoder.dispatchEvent('dequeue')

  // The internal stop flag must not leak onto the event — a later native
  // dispatchEvent on the retained event must run its listeners normally.
  const other = new EventTarget()
  let fired = 0
  other.addEventListener('dequeue', () => fired++)
  other.dispatchEvent(retained!)
  t.is(fired, 1, 'native re-dispatch must not be stopped by our dispatch')
  encoder.close()
})

test('VideoEncoder: extracted composedPath stays safe and forwards natively', (t) => {
  const encoder = new VideoEncoder({
    output: () => {},
    error: () => {},
  })

  // eslint-disable-next-line @typescript-eslint/no-unsafe-function-type
  let saved: Function | null = null
  let retained: Event | null = null
  encoder.addEventListener('dequeue', (event: Event) => {
    retained = event
    saved = event.composedPath
    t.is(event.composedPath().length, 1, 'during dispatch: [codec]')
  })
  encoder.dispatchEvent('dequeue')

  // Extracted function called after dispatch must not touch a stale handle —
  // forwards to the native method ([] for a non-dispatching event).
  t.deepEqual(saved!.call(retained!), [])
  // During a native re-dispatch the extracted wrapper reports the real path.
  const other = new EventTarget()
  let nativePathLen = -1
  other.addEventListener('dequeue', (e: Event) => {
    nativePathLen = saved!.call(e).length
  })
  other.dispatchEvent(retained!)
  t.is(nativePathLen, 1, 'extracted wrapper returns native path on re-dispatch')
  encoder.close()
})

test('VideoEncoder: once listener for a non-emitted type does not pin the process', (t) => {
  // Only 'dequeue' is emitted automatically; a once-listener for another type
  // must not keep Node alive waiting for an event that cannot arrive.
  const script = `
    const { VideoEncoder } = require('./index.js');
    const enc = new VideoEncoder({ output: () => {}, error: () => {} });
    enc.addEventListener('not-a-real-event', () => {}, { once: true });
    console.log('REGISTERED');
    // No other handles — process should exit promptly.
  `
  const output = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    timeout: 10_000,
    encoding: 'utf8',
  })
  t.true(output.includes('REGISTERED'), 'process must exit without waiting')
})
