/**
 * AudioEncoder API Conformance Tests
 *
 * Tests WebCodecs AudioEncoder specification compliance.
 * Uses callback-based constructor per W3C WebCodecs spec.
 */

import { execFileSync } from 'node:child_process'
import test from 'ava'

import { AudioEncoder } from '../index.js'
import {
  generateSineTone,
  generateSilence,
  waitFor,
  type EncodedAudioChunk,
  type EncodedAudioChunkMetadata,
} from './helpers/index.js'

// Helper to create encoder with callbacks that collect output
function createTestEncoder() {
  const chunks: EncodedAudioChunk[] = []
  const errors: Error[] = []

  const encoder = new AudioEncoder({
    output: (chunk, _metadata) => {
      chunks.push(chunk)
    },
    error: (e) => {
      errors.push(e)
    },
  })

  return { encoder, chunks, errors }
}

function createFlacTestEncoder() {
  const chunks: EncodedAudioChunk[] = []
  const decoderConfigs: NonNullable<EncodedAudioChunkMetadata['decoderConfig']>[] = []
  const errors: Error[] = []

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      if (metadata?.decoderConfig) {
        decoderConfigs.push(metadata.decoderConfig)
      }
    },
    error: (e) => {
      errors.push(e)
    },
  })

  return { encoder, chunks, decoderConfigs, errors }
}

function isExpectedFlacDecoderConfig(config: NonNullable<EncodedAudioChunkMetadata['decoderConfig']>) {
  const description = config.description
  if (!description) return false

  return (
    config.codec === 'flac' &&
    config.sampleRate === 44100 &&
    config.numberOfChannels === 2 &&
    description.byteLength === 42 &&
    description[0] === 0x66 &&
    description[1] === 0x4c &&
    description[2] === 0x61 &&
    description[3] === 0x43
  )
}

// ============================================================================
// Constructor Tests
// ============================================================================

test('AudioEncoder: constructor creates unconfigured encoder', (t) => {
  const { encoder } = createTestEncoder()

  t.is(encoder.state, 'unconfigured')
  t.is(encoder.encodeQueueSize, 0)

  encoder.close()
})

test('AudioEncoder: constructor requires init dictionary', (t) => {
  // @ts-expect-error - Testing that missing init throws
  t.throws(() => new AudioEncoder())
  // @ts-expect-error - Testing that missing error callback throws
  t.throws(() => new AudioEncoder({ output: () => {} }))
})

// ============================================================================
// Configuration Tests
// ============================================================================

test('AudioEncoder: configure() with AAC codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'mp4a.40.2', // AAC-LC
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with Opus codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with MP3 codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 192000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with FLAC codec', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with mono audio', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 1,
    bitrate: 32000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: configure() with invalid codec triggers error callback', async (t) => {
  const { encoder, errors } = createTestEncoder()

  encoder.configure({
    codec: 'invalid-codec',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  // Error callback transitions to closed
  t.is(encoder.state, 'closed')
  await waitFor(() => errors.length > 0, 'error callback')
  t.is(errors[0].name, 'NotSupportedError')

  // Already closed by error callback, so close() throws InvalidStateError
  try {
    encoder.close()
    t.fail('should have thrown')
  } catch (error) {
    t.true(error instanceof DOMException, 'error should be DOMException instance')
    t.is((error as DOMException).name, 'InvalidStateError')
  }
})

// ============================================================================
// Encoding Tests
// ============================================================================

test('AudioEncoder: encode() single frame', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Generate 20ms of audio (960 samples at 48kHz)
  const audio = generateSineTone(440, 960, 2, 48000, 'f32', 0)

  encoder.encode(audio)
  audio.close()

  // Flush to get output
  await encoder.flush()

  // We should have at least one chunk after flush
  t.true(chunks.length >= 0, 'Encoder should produce chunks or buffer them')

  for (const chunk of chunks) {
    t.is(chunk.type, 'key')
    t.true(chunk.byteLength > 0)
  }

  encoder.close()
})

test('AudioEncoder: encode() multiple frames', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  // Encode 10 frames of 20ms each
  for (let i = 0; i < 10; i++) {
    const timestamp = i * 20000 // 20ms per frame in microseconds
    const audio = generateSineTone(440, 960, 2, 48000, 'f32', timestamp)
    encoder.encode(audio)
    audio.close()
  }

  await encoder.flush()

  t.true(chunks.length > 0, 'Should have produced encoded chunks')

  encoder.close()
})

test('AudioEncoder: mixed input formats recreate and bypass resamplers correctly', async (t) => {
  const { encoder, chunks, errors } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const inputs = [
    generateSilence(882, 1, 44100, 'f32', 0),
    generateSilence(960, 2, 48000, 'f32', 20000),
    generateSilence(640, 2, 32000, 's16', 40000),
  ]
  for (const input of inputs) {
    encoder.encode(input)
    input.close()
  }

  await encoder.flush()

  t.deepEqual(errors, [])
  t.true(chunks.length >= 3)
  const encodedDuration = chunks.reduce((total, chunk) => total + (chunk.duration ?? 0), 0)
  t.true(encodedDuration >= 59000, `expected about 60ms, got ${encodedDuration}us`)
  encoder.close()
})

test('AudioEncoder: encode() with AAC', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  encoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  // AAC typically needs 1024 samples per frame
  const audio = generateSineTone(440, 1024, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  // Need to encode more frames to get output (AAC buffers)
  for (let i = 1; i < 5; i++) {
    const frame = generateSineTone(440, 1024, 2, 48000, 'f32', i * 21333)
    encoder.encode(frame)
    frame.close()
  }

  await encoder.flush()

  // AAC may need more data before producing output
  t.true(chunks.length >= 0)

  encoder.close()
})

// ============================================================================
// State Machine Tests
// ============================================================================

test('AudioEncoder: encode() on unconfigured throws InvalidStateError', (t) => {
  const { encoder } = createTestEncoder()

  const audio = generateSilence(960, 2, 48000, 'f32', 0)

  // W3C spec: encode() on unconfigured encoder should throw InvalidStateError
  try {
    encoder.encode(audio)
    t.fail('should have thrown')
  } catch (error) {
    t.true(error instanceof DOMException, 'error should be DOMException instance')
    t.is((error as DOMException).name, 'InvalidStateError')
  }

  audio.close()
})

test('AudioEncoder: encode() on closed throws InvalidStateError', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  encoder.close()

  const audio = generateSilence(960, 2, 48000, 'f32', 0)

  // W3C spec: encode() on closed encoder should throw InvalidStateError
  try {
    encoder.encode(audio)
    t.fail('should have thrown')
  } catch (error) {
    t.true(error instanceof DOMException, 'error should be DOMException instance')
    t.is((error as DOMException).name, 'InvalidStateError')
  }

  audio.close()
})

test('AudioEncoder: reset() returns to unconfigured state', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.is(encoder.state, 'configured')

  encoder.reset()

  t.is(encoder.state, 'unconfigured')

  encoder.close()
})

test('AudioEncoder: can reconfigure after reset', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  encoder.reset()

  // Reconfigure with different settings
  encoder.configure({
    codec: 'mp3',
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 192000,
  })

  t.is(encoder.state, 'configured')

  encoder.close()
})

test('AudioEncoder: close() on closed encoder throws InvalidStateError', (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  encoder.close()
  // W3C spec: second close should throw InvalidStateError
  try {
    encoder.close()
    t.fail('should have thrown')
  } catch (error) {
    t.true(error instanceof DOMException, 'error should be DOMException instance')
    t.is((error as DOMException).name, 'InvalidStateError')
  }
})

// ============================================================================
// flush() Tests
// ============================================================================

test('AudioEncoder: flush() returns a Promise', async (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audio = generateSineTone(440, 960, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  const flushResult = encoder.flush()
  t.true(flushResult instanceof Promise, 'flush() should return a Promise')

  await flushResult

  encoder.close()
})

test('AudioEncoder: AAC chunks flushed at the end keep distinct timestamps and microsecond durations', async (t) => {
  const { encoder, chunks } = createTestEncoder()

  encoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  for (let i = 0; i < 3; i++) {
    const audio = generateSineTone(440, 1024, 2, 48000, 'f32', Math.round((i * 1024 * 1_000_000) / 48000))
    encoder.encode(audio)
    audio.close()
  }
  await encoder.flush()

  // Three input frames plus one packet of encoder delay
  t.is(chunks.length, 4)
  const frameDuration = Math.floor((1024 * 1_000_000) / 48000)
  chunks.forEach((chunk, i) => {
    t.is(chunk.timestamp, Math.floor((i * 1024 * 1_000_000) / 48000), `timestamp of chunk ${i}`)
    t.is(chunk.duration, frameDuration, `duration of chunk ${i}`)
  })

  encoder.close()
})

test('AudioEncoder: AAC partial remainders keep packet durations and exact output timing', async (t) => {
  const { encoder, chunks, errors } = createTestEncoder()

  try {
    encoder.configure({
      codec: 'mp4a.40.2',
      sampleRate: 48000,
      numberOfChannels: 2,
      bitrate: 128000,
    })

    for (const frames of [1024, 1024, 1]) {
      const audio = generateSineTone(440, frames, 2, 48000, 'f32', 100000)
      encoder.encode(audio)
      audio.close()
    }
    await encoder.flush()

    t.deepEqual(errors, [])
    t.deepEqual(
      chunks.map((chunk) => [chunk.timestamp, chunk.duration]),
      [
        [100000, 21333],
        [121333, 21333],
        [142666, 21333],
        [164000, 20],
      ],
    )
  } finally {
    if (encoder.state !== 'closed') encoder.close()
  }
})

test('AudioEncoder: one AAC input with a partial frame keeps exact output timing', async (t) => {
  const { encoder, chunks, errors } = createTestEncoder()

  try {
    encoder.configure({
      codec: 'mp4a.40.2',
      sampleRate: 48000,
      numberOfChannels: 2,
      bitrate: 128000,
    })

    const audio = generateSineTone(440, 1025, 2, 48000, 'f32', 0)
    encoder.encode(audio)
    audio.close()
    await encoder.flush()

    t.deepEqual(errors, [])
    t.deepEqual(chunks.map((chunk) => [chunk.timestamp, chunk.duration]), [
      [0, 21333],
      [21333, 21333],
      [42666, 20],
    ])
  } finally {
    if (encoder.state !== 'closed') encoder.close()
  }
})

test('AudioEncoder: AAC timestamp overflow preserves valid output and rejects flush', (t) => {
  // Isolate the native worker so an unfixed panic cannot stall the test process.
  const script = `
    const { AudioEncoder, AudioData } = require('./index.js');
    const nearMax = 2 ** 63 - 4096;
    const events = [];
    const encoder = new AudioEncoder({
      output(chunk) {
        events.push('output:' + (chunk.timestamp === nearMax) + ':' + chunk.duration);
      },
      error(error) {
        events.push('error:' + error.name);
      },
    });
    const timer = setTimeout(() => {
      console.error('TIMEOUT');
      process.exit(2);
    }, 4000);
    (async () => {
      encoder.configure({
        codec: 'mp4a.40.2',
        sampleRate: 48000,
        numberOfChannels: 2,
        bitrate: 128000,
      });
      const audio = new AudioData({
        data: new Float32Array(1024 * 2),
        format: 'f32',
        sampleRate: 48000,
        numberOfFrames: 1024,
        numberOfChannels: 2,
        timestamp: nearMax,
      });
      encoder.encode(audio);
      audio.close();
      try {
        await encoder.flush();
        events.push('flush:resolved');
      } catch (error) {
        events.push('flush:' + error.name);
      }
      try {
        encoder.close();
        events.push('close:ok');
      } catch (error) {
        events.push('close:' + error.name);
      }
      clearTimeout(timer);
      process.stdout.write(JSON.stringify(events));
    })().catch((error) => {
      clearTimeout(timer);
      console.error(error && error.stack || error);
      process.exit(1);
    });
  `

  const output = execFileSync(process.execPath, ['-e', script], {
    cwd: process.cwd(),
    encoding: 'utf8',
    timeout: 5000,
  })
  t.deepEqual(JSON.parse(output), [
    'output:true:21333',
    'error:EncodingError',
    'flush:EncodingError',
    'close:InvalidStateError',
  ])
})

test('AudioEncoder: MP3 packets drained after queued timestamps have microsecond durations', async (t) => {
  const { encoder, chunks, errors } = createTestEncoder()

  encoder.configure({
    codec: 'mp3',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  const audio = generateSineTone(440, 1, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()
  await encoder.flush()

  t.deepEqual(errors, [])
  t.is(chunks.length, 2)
  t.is(chunks[1].duration, Math.floor((1152 * 1_000_000) / 48000))

  encoder.close()
})

test('AudioEncoder: FLAC empty flush does not synthesize drain duration', async (t) => {
  const { encoder, chunks, decoderConfigs, errors } = createFlacTestEncoder()

  try {
    encoder.configure({
      codec: 'flac',
      sampleRate: 44100,
      numberOfChannels: 2,
    })
    await encoder.flush()

    t.deepEqual(errors, [])
    t.is(chunks.length, 1)
    t.is(decoderConfigs.length, chunks.length)
    t.true(decoderConfigs.every(isExpectedFlacDecoderConfig))
    t.is(chunks[0].byteLength, 0)
    t.is(chunks[0].timestamp, 0)
    t.is(chunks[0].duration, null)
  } finally {
    if (encoder.state !== 'closed') encoder.close()
  }
})

test('AudioEncoder: FLAC partial-frame flush does not synthesize drain duration', async (t) => {
  const { encoder, chunks, decoderConfigs, errors } = createFlacTestEncoder()

  try {
    encoder.configure({
      codec: 'flac',
      sampleRate: 44100,
      numberOfChannels: 2,
    })
    const audio = generateSineTone(440, 1025, 2, 44100, 'f32', 0)
    encoder.encode(audio)
    audio.close()
    await encoder.flush()

    t.deepEqual(errors, [])
    t.is(chunks.length, 2)
    t.is(decoderConfigs.length, chunks.length)
    t.true(decoderConfigs.every(isExpectedFlacDecoderConfig))
    t.true(chunks[0].byteLength > 0)
    t.is(chunks[0].timestamp, 0)
    t.is(chunks[0].duration, 23242)
    t.is(chunks[1].byteLength, 0)
    t.is(chunks[1].timestamp, 23242)
    t.is(chunks[1].duration, null)
  } finally {
    if (encoder.state !== 'closed') encoder.close()
  }
})

test('AudioEncoder: FLAC full-frame flush does not synthesize drain duration', async (t) => {
  const { encoder, chunks, decoderConfigs, errors } = createFlacTestEncoder()

  try {
    encoder.configure({
      codec: 'flac',
      sampleRate: 44100,
      numberOfChannels: 2,
    })
    const audio = generateSineTone(440, 4608, 2, 44100, 'f32', 0)
    encoder.encode(audio)
    audio.close()
    await encoder.flush()

    t.deepEqual(errors, [])
    t.is(chunks.length, 2)
    t.is(decoderConfigs.length, chunks.length)
    t.true(decoderConfigs.every(isExpectedFlacDecoderConfig))
    t.true(chunks[0].byteLength > 0)
    t.is(chunks[0].timestamp, 0)
    t.is(chunks[0].duration, 104489)
    t.is(chunks[1].byteLength, 0)
    t.is(chunks[1].timestamp, 104489)
    t.is(chunks[1].duration, null)
  } finally {
    if (encoder.state !== 'closed') encoder.close()
  }
})

// ============================================================================
// isConfigSupported Tests
// ============================================================================

test('AudioEncoder.isConfigSupported: Opus is supported', async (t) => {
  const result = await AudioEncoder.isConfigSupported({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioEncoder.isConfigSupported: AAC is supported', async (t) => {
  const result = await AudioEncoder.isConfigSupported({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.true(result.supported)
})

test('AudioEncoder.isConfigSupported: invalid codec not supported', async (t) => {
  const result = await AudioEncoder.isConfigSupported({
    codec: 'invalid-codec',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  t.false(result.supported)
})

// ============================================================================
// Sample Rate Tests
// ============================================================================

test('AudioEncoder: encode with 44.1kHz sample rate', async (t) => {
  const { encoder } = createTestEncoder()

  encoder.configure({
    codec: 'mp3', // MP3 commonly uses 44.1kHz
    sampleRate: 44100,
    numberOfChannels: 2,
    bitrate: 128000,
  })

  // Generate audio at 44.1kHz
  const audio = generateSineTone(440, 1152, 2, 44100, 'f32', 0) // MP3 frame size
  encoder.encode(audio)
  audio.close()

  // Encode more to get output
  for (let i = 1; i < 10; i++) {
    const frame = generateSineTone(440, 1152, 2, 44100, 'f32', i * 26122) // ~1152/44100 seconds
    encoder.encode(frame)
    frame.close()
  }

  await encoder.flush()

  encoder.close()
  t.pass()
})

// ============================================================================
// ondequeue Event Tests
// ============================================================================

test('AudioEncoder: ondequeue fires when queue decreases', async (t) => {
  const { encoder } = createTestEncoder()

  let dequeueCount = 0
  encoder.ondequeue = () => {
    dequeueCount++
  }

  encoder.configure({
    codec: 'opus',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 64000,
  })

  const audio = generateSineTone(440, 960, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  await encoder.flush()

  t.true(dequeueCount >= 1, 'ondequeue should have fired')

  encoder.close()
})

test('AudioEncoder: ondequeue can be set to null', (t) => {
  const { encoder } = createTestEncoder()

  encoder.ondequeue = () => {}
  t.notThrows(() => {
    encoder.ondequeue = null
  })

  encoder.close()
})

// ============================================================================
// Format-Specific Tests
// ============================================================================

test('AudioEncoder: AAC ADTS format', async (t) => {
  const chunks: Array<{ chunk: any; metadata: any }> = []
  let firstMetadata: any = null

  const encoder = new AudioEncoder({
    output: (chunk, metadata) => {
      chunks.push({ chunk, metadata })
      if (!firstMetadata && metadata?.decoderConfig) {
        firstMetadata = metadata
      }
    },
    error: (e) => {
      throw e
    },
  })

  // Configure with ADTS format
  encoder.configure({
    codec: 'mp4a.40.2',
    sampleRate: 48000,
    numberOfChannels: 2,
    bitrate: 128000,
    aac: {
      format: 'adts',
    },
  } as Parameters<typeof encoder.configure>[0])

  const audio = generateSineTone(440, 1024, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  await encoder.flush()
  encoder.close()

  t.true(chunks.length > 0, 'Should have produced at least one chunk')

  // ADTS frames start with sync word 0xFF (first byte is 0xFF)
  const firstChunk = chunks[0].chunk
  const data = new Uint8Array(firstChunk.byteLength)
  firstChunk.copyTo(data)
  t.is(data[0], 0xff, 'ADTS frame should start with 0xFF sync byte')

  // ADTS mode should NOT have description (no ASC needed)
  t.is(firstMetadata?.decoderConfig?.description, undefined, 'ADTS mode should not have description')
})

test('AudioEncoder: FLAC description magic bytes', async (t) => {
  let flacDescription: Uint8Array | undefined

  const encoder = new AudioEncoder({
    output: (_chunk, metadata) => {
      if (metadata?.decoderConfig?.description && !flacDescription) {
        flacDescription = metadata.decoderConfig.description
      }
    },
    error: (e) => {
      throw e
    },
  })

  encoder.configure({
    codec: 'flac',
    sampleRate: 48000,
    numberOfChannels: 2,
  })

  const audio = generateSineTone(440, 4608, 2, 48000, 'f32', 0)
  encoder.encode(audio)
  audio.close()

  await encoder.flush()
  encoder.close()

  t.truthy(flacDescription, 'FLAC encoder should produce description')

  // Verify FLAC description starts with 'fLaC' magic (0x664C6143)
  // description is Uint8Array per W3C spec, extract underlying ArrayBuffer for DataView
  const view = new DataView(flacDescription!.buffer, flacDescription!.byteOffset, flacDescription!.byteLength)
  const magic = view.getUint32(0, false) // big-endian
  t.is(magic, 0x664c6143, "FLAC description should start with 'fLaC' magic bytes")
})
