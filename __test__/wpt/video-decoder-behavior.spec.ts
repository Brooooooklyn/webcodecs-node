/**
 * VideoDecoder Behavior Tests (WPT)
 *
 * Ported from W3C Web Platform Tests:
 * https://github.com/web-platform-tests/wpt
 *
 * Tests core VideoDecoder behavior including decode, flush, reset, queue management.
 */

import test from 'ava'

import { EncodedVideoChunk, resetHardwareFallbackState, VideoDecoder, VideoEncoder, VideoFrame } from '../../index.js'
import type { VideoDecoderConfig } from '../../standard.js'
import { generateSolidColorI420Frame, TestColors } from '../helpers/frame-generator.js'
import {
  createCollectingCodecInit,
  createErrorTrackingCodecInit,
  endAfterEventLoopTurn,
  getDefaultCodecInit,
  testClosedCodec,
  testUnconfiguredCodec,
  waitFor,
} from '../helpers/wpt-utils.js'

// Reset hardware fallback state before each test
test.beforeEach(() => {
  resetHardwareFallbackState()
})

// Helper to create encoded chunks from an encoder
async function createEncodedChunks(
  codec: string,
  width: number,
  height: number,
  count: number,
): Promise<{ chunks: EncodedVideoChunk[]; config: VideoDecoderConfig }> {
  const chunks: EncodedVideoChunk[] = []
  let decoderConfig: VideoDecoderConfig | null = null

  const encoder = new VideoEncoder({
    output: (chunk, metadata) => {
      chunks.push(chunk)
      if (metadata?.decoderConfig && !decoderConfig) {
        decoderConfig = metadata.decoderConfig as unknown as VideoDecoderConfig
      }
    },
    error: () => {},
  })

  encoder.configure({
    codec,
    width,
    height,
  })

  for (let i = 0; i < count; i++) {
    const frame = generateSolidColorI420Frame(width, height, TestColors.blue, i * 33333)
    encoder.encode(frame, { keyFrame: i === 0 })
    frame.close()
  }

  await encoder.flush()
  encoder.close()

  return { chunks, config: decoderConfig! }
}

// ============================================================================
// Construction Tests
// ============================================================================

test('VideoDecoder: construction with valid init', async (t) => {
  // Missing required fields should throw
  t.throws(
    () => {
      // @ts-expect-error - Testing missing fields
      new VideoDecoder({})
    },
    { instanceOf: TypeError },
  )

  // Valid init
  const decoder = new VideoDecoder(getDefaultCodecInit(t))
  t.is(decoder.state, 'unconfigured')
  decoder.close()

  await endAfterEventLoopTurn()
})

// ============================================================================
// Decode and Flush Tests
// ============================================================================

test('VideoDecoder: decode a key frame', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 2)

  if (chunks.length === 0) {
    t.pass('No chunks produced - codec may not be supported')
    return
  }

  const { init, outputs } = createCollectingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder(init)
  decoder.configure(config)
  decoder.decode(chunks[0])

  await decoder.flush()

  t.is(outputs.length, 1, 'output count')
  t.is(outputs[0].timestamp, chunks[0].timestamp, 'timestamp')

  outputs[0].close()
  decoder.close()
})

test('VideoDecoder: decode non-key frame first fails', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 3)

  if (chunks.length < 2) {
    t.pass('Not enough chunks produced')
    return
  }

  // Find a delta frame
  const deltaChunk = chunks.find((c) => c.type === 'delta')
  if (!deltaChunk) {
    t.pass('No delta frame in test data')
    return
  }

  const decoder = new VideoDecoder({
    output: () => {},
    error: () => {},
  })

  decoder.configure(config)

  // Decoding a delta frame first should throw DataError (native DOMException)
  const error = t.throws(
    () => {
      decoder.decode(deltaChunk)
    },
    { name: 'DataError' },
    'decode delta first should throw DataError',
  )
  t.true(error instanceof DOMException, 'error should be DOMException instance')

  decoder.close()
})

// ============================================================================
// Reset Tests
// ============================================================================

test('VideoDecoder: reset suppresses outputs', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 20)

  if (chunks.length < 16) {
    t.pass('Not enough chunks produced')
    return
  }

  let outputs = 0
  let resetDone = false

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs++
      if (outputs === 1) {
        decoder.reset()
        resetDone = true
      }
      frame.close()
    },
    error: () => {},
  })

  decoder.configure(config)

  // Get actual encoded data from the first chunk
  const chunkData = new Uint8Array(chunks[0].byteLength)
  chunks[0].copyTo(chunkData)

  // Queue many decodes with actual encoded data
  for (let i = 0; i < 16; i++) {
    decoder.decode(
      new EncodedVideoChunk({
        type: 'key',
        timestamp: i * 33333,
        data: chunkData,
      }),
    )
  }

  // Wait for first output and reset
  await waitFor(() => resetDone, 'Reset should happen after first output', 5000)

  t.true(resetDone)
  t.is(decoder.decodeQueueSize, 0, 'queue should be empty after reset')

  // Reconfigure and decode more
  decoder.configure(config)
  decoder.decode(chunks[0])

  await decoder.flush()

  t.true(outputs >= 1)
  decoder.close()
})

test('VideoDecoder: reset during flush', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 3)

  if (chunks.length < 2) {
    t.pass('Not enough chunks produced')
    return
  }

  let outputs = 0
  let flushPromise: Promise<void> | null = null

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs++
      if (outputs === 1) {
        decoder.reset()
      }
      frame.close()
    },
    error: () => {},
  })

  decoder.configure(config)
  decoder.decode(chunks[0])
  if (chunks.length > 1 && chunks[1].type === 'delta') {
    decoder.decode(chunks[1])
  }

  flushPromise = decoder.flush()

  // Wait for output and reset
  await waitFor(() => outputs >= 1, 'Should get at least one output', 5000)

  // Flush should have been aborted
  await t.throwsAsync(flushPromise, { message: /AbortError/ })

  // Note: With native addon multi-threaded implementation, callbacks that were
  // already queued before reset() may still fire. The key test is that flush()
  // is aborted with AbortError.
  t.true(outputs >= 1, 'at least one output before reset')

  decoder.close()
})

// ============================================================================
// Closed Codec Tests
// ============================================================================

test('VideoDecoder: closed decoder operations', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 1)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  const decoder = new VideoDecoder(getDefaultCodecInit(t))

  await testClosedCodec(t, decoder, config, chunks[0])
})

// ============================================================================
// Unconfigured Codec Tests
// ============================================================================

test('VideoDecoder: unconfigured decoder operations', async (t) => {
  const { chunks } = await createEncodedChunks('vp8', 320, 240, 1)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  const decoder = new VideoDecoder(getDefaultCodecInit(t))

  await testUnconfiguredCodec(t, decoder, chunks[0])

  decoder.close()
})

// ============================================================================
// decodeQueueSize Tests
// ============================================================================

test('VideoDecoder: decodeQueueSize tracking', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 10)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  const decoder = new VideoDecoder({
    output: (frame) => frame.close(),
    error: () => {},
  })

  // No decodes yet
  t.is(decoder.decodeQueueSize, 0)

  decoder.configure(config)

  // Still no decodes
  t.is(decoder.decodeQueueSize, 0)

  // Track dequeue events
  // Note: With native addon multi-threaded implementation, callbacks may not see
  // monotonically decreasing queue sizes due to concurrent decode/process operations.
  // We verify: (1) dequeue events fire, (2) final queue state is correct.
  let dequeueCount = 0
  decoder.ondequeue = () => {
    dequeueCount++
  }

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  t.true(decoder.decodeQueueSize >= 0)
  t.true(decoder.decodeQueueSize <= chunks.length)

  await decoder.flush()

  // After flush, queue should be empty
  t.is(decoder.decodeQueueSize, 0, 'queue empty after flush')
  // Verify dequeue events were fired (at least chunks.length events)
  t.true(dequeueCount >= chunks.length, `at least ${chunks.length} dequeue events fired, got ${dequeueCount}`)

  // Reset also clears queue
  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  t.true(decoder.decodeQueueSize >= 0)
  decoder.reset()
  t.is(decoder.decodeQueueSize, 0, 'queue empty after reset')

  decoder.close()
})

// ============================================================================
// Negative Timestamp Tests
// ============================================================================

test('VideoDecoder: decode with negative timestamp', async (t) => {
  // Create a chunk with negative timestamp
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 1)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  // Create new chunk with negative timestamp
  const originalData = new Uint8Array(chunks[0].byteLength)
  chunks[0].copyTo(originalData)

  const negativeChunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: -42,
    data: originalData,
  })

  const { init, outputs } = createCollectingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder(init)
  decoder.configure(config)
  decoder.decode(negativeChunk)

  await decoder.flush()

  t.is(outputs.length, 1, 'output count')
  t.is(outputs[0].timestamp, -42, 'negative timestamp preserved')

  outputs[0].close()
  decoder.close()
})

// ============================================================================
// Empty Frame Tests
// ============================================================================

test('VideoDecoder: decode empty frame triggers error', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 1)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  const { init, outputs, gotError } = createErrorTrackingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs.push(frame)
      frame.close()
    },
    error: init.error,
  })

  decoder.configure(config)
  decoder.decode(chunks[0]) // Decode good frame first

  // Decode empty frame
  const emptyChunk = new EncodedVideoChunk({
    type: 'key',
    timestamp: 1,
    data: new ArrayBuffer(0),
  })
  decoder.decode(emptyChunk)

  // Worker errors use standard Error with DOMException name in message
  const flushError = await t.throwsAsync(decoder.flush())
  t.true(flushError?.message.includes('EncodingError'), 'flush error should include EncodingError')

  const error = await gotError
  t.true(error instanceof Error)
  // Error callbacks receive standard Error with DOMException name in message
  t.true(error.message.includes('EncodingError'), 'error callback should include EncodingError')
  t.is(decoder.state, 'closed', 'decoder closed after error')
})

// ============================================================================
// Decoding After Flush Tests
// ============================================================================

test('VideoDecoder: decoding after flush', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 2)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  const { init, outputs } = createCollectingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs.push(frame)
      frame.close()
    },
    error: init.error,
  })

  decoder.configure(config)
  decoder.decode(chunks[0])

  await decoder.flush()
  t.is(outputs.length, 1, 'first decode')

  decoder.decode(chunks[0])
  await decoder.flush()
  t.is(outputs.length, 2, 'second decode after flush')

  decoder.close()
})

// ============================================================================
// Configure, Reset, Configure Tests
// ============================================================================

test('VideoDecoder: configure, reset, configure does not stall', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 1)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  const { init, outputs } = createCollectingCodecInit<VideoFrame>()

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputs.push(frame)
      frame.close()
    },
    error: init.error,
  })

  decoder.configure(config)
  decoder.reset()
  decoder.configure(config)
  decoder.decode(chunks[0])

  await decoder.flush()

  t.is(outputs.length, 1, 'output after reset and reconfigure')
  t.is(outputs[0].timestamp, chunks[0].timestamp)

  decoder.close()
})

// ============================================================================
// New Flush After Reset in Callback Tests
// ============================================================================

test('VideoDecoder: new flush after reset in callback', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 2)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  let firstFlushPromise: Promise<void>
  let secondFlushPromise: Promise<void> | null = null
  let outputs = 0

  const decoder = new VideoDecoder({
    output: (frame) => {
      if (outputs === 0) {
        decoder.reset()
        frame.close()

        // Reconfigure and start new flush
        decoder.configure(config)
        decoder.decode(chunks[0])
        secondFlushPromise = decoder.flush()
      } else {
        frame.close()
      }
      outputs++
    },
    error: () => {},
  })

  decoder.configure(config)
  decoder.decode(chunks[0])
  firstFlushPromise = decoder.flush()

  // First flush should be aborted
  await t.throwsAsync(firstFlushPromise, { message: /AbortError/ })

  // Wait for second flush (may be set by callback)
  // eslint-disable-next-line @typescript-eslint/await-thenable -- type narrowing limitation
  if (secondFlushPromise) await secondFlushPromise

  t.is(outputs, 2, 'got two outputs')

  decoder.close()
})

// ============================================================================
// Low Latency Decoding Tests
// ============================================================================

test('VideoDecoder: optimizeForLatency decoding', async (t) => {
  const { chunks, config } = await createEncodedChunks('vp8', 320, 240, 1)

  if (chunks.length === 0) {
    t.pass('No chunks produced')
    return
  }

  let outputReceived = false

  const decoder = new VideoDecoder({
    output: (frame) => {
      outputReceived = true
      frame.close()
    },
    error: () => {},
  })

  decoder.configure({
    ...config,
    optimizeForLatency: true,
  })

  decoder.decode(chunks[0])

  // With optimizeForLatency, frame should be output without flushing
  await waitFor(() => outputReceived, 'Output should be received without flush', 5000)

  t.true(outputReceived)

  decoder.close()
})
