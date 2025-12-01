/**
 * VideoDecoder API Conformance Tests
 *
 * Tests WebCodecs VideoDecoder specification compliance.
 */

import test from 'ava'

import {
  VideoEncoder,
  VideoDecoder,
  EncodedVideoChunk,
  EncodedVideoChunkType,
  CodecState,
} from '../index.js'
import { generateSolidColorI420Frame, generateFrameSequence, TestColors } from './helpers/index.js'
import { createEncoderConfig, createDecoderConfig } from './helpers/codec-matrix.js'

// ============================================================================
// Helper: Create encoded chunks for decoder tests
// ============================================================================

function createEncodedH264Chunks(
  width: number,
  height: number,
  frameCount: number,
): EncodedVideoChunk[] {
  const encoder = new VideoEncoder()
  encoder.configure(createEncoderConfig('h264', width, height))

  const frames = generateFrameSequence(width, height, frameCount)

  encoder.encode(frames[0], { keyFrame: true })
  for (let i = 1; i < frames.length; i++) {
    encoder.encode(frames[i])
  }

  for (const frame of frames) {
    frame.close()
  }

  encoder.flush()
  const chunks = encoder.takeEncodedChunks()
  encoder.close()

  return chunks
}

// ============================================================================
// Constructor and State Tests
// ============================================================================

test('VideoDecoder: constructor creates unconfigured decoder', (t) => {
  const decoder = new VideoDecoder()
  t.is(decoder.state, CodecState.Unconfigured)
  t.is(decoder.decodeQueueSize, 0)
  decoder.close()
})

test('VideoDecoder: state transitions correctly', (t) => {
  const decoder = new VideoDecoder()

  // Initial state
  t.is(decoder.state, CodecState.Unconfigured)

  // Configure
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))
  t.is(decoder.state, CodecState.Configured)

  // Close
  decoder.close()
  t.is(decoder.state, CodecState.Closed)
})

test('VideoDecoder: close() is idempotent', (t) => {
  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264'))

  decoder.close()
  t.is(decoder.state, CodecState.Closed)

  // Second close should not throw
  t.notThrows(() => decoder.close())
  t.is(decoder.state, CodecState.Closed)
})

// ============================================================================
// configure() Tests
// ============================================================================

test('VideoDecoder: configure() with H.264', (t) => {
  const decoder = new VideoDecoder()

  t.notThrows(() => {
    decoder.configure(createDecoderConfig('h264'))
  })

  t.is(decoder.state, CodecState.Configured)
  decoder.close()
})

test('VideoDecoder: configure() with VP8', (t) => {
  const decoder = new VideoDecoder()

  t.notThrows(() => {
    decoder.configure(createDecoderConfig('vp8'))
  })

  t.is(decoder.state, CodecState.Configured)
  decoder.close()
})

test('VideoDecoder: configure() with VP9', (t) => {
  const decoder = new VideoDecoder()

  t.notThrows(() => {
    decoder.configure(createDecoderConfig('vp9'))
  })

  t.is(decoder.state, CodecState.Configured)
  decoder.close()
})

test('VideoDecoder: configure() can be called multiple times', (t) => {
  const decoder = new VideoDecoder()

  // First configuration
  decoder.configure(createDecoderConfig('h264'))
  t.is(decoder.state, CodecState.Configured)

  // Reconfigure
  decoder.configure(createDecoderConfig('vp8'))
  t.is(decoder.state, CodecState.Configured)

  decoder.close()
})

// ============================================================================
// decode() Tests
// ============================================================================

test('VideoDecoder: decode() single chunk', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 1)
  t.true(chunks.length > 0, 'Should have encoded chunks')

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  t.notThrows(() => {
    decoder.decode(chunks[0])
  })

  decoder.flush()

  // Should have output
  t.true(decoder.hasOutput())

  const frames = decoder.takeDecodedFrames()
  t.true(frames.length > 0)

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: decode() multiple chunks', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 5)
  t.true(chunks.length > 0)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  decoder.flush()

  const frames = decoder.takeDecodedFrames()
  t.true(frames.length > 0, 'Should decode frames from multiple chunks')

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: decode() preserves frame properties', (t) => {
  const width = 320
  const height = 240

  const chunks = createEncodedH264Chunks(width, height, 1)
  t.true(chunks.length > 0)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: width, codedHeight: height }))

  decoder.decode(chunks[0])
  decoder.flush()

  const frames = decoder.takeDecodedFrames()
  t.true(frames.length > 0)

  const frame = frames[0]
  t.is(frame.codedWidth, width)
  t.is(frame.codedHeight, height)

  for (const f of frames) {
    f.close()
  }

  decoder.close()
})

// ============================================================================
// flush() Tests
// ============================================================================

test('VideoDecoder: flush() produces all pending output', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 3)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }

  // Flush to get all output
  decoder.flush()

  t.true(decoder.hasOutput(), 'Should have output after flush')

  const frames = decoder.takeDecodedFrames()
  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: flush() produces output once', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 1)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  decoder.decode(chunks[0])
  decoder.flush()

  const frames1 = decoder.takeDecodedFrames()
  t.true(frames1.length > 0)

  // Without new decodes, queue should be empty
  const frames2 = decoder.takeDecodedFrames()
  t.is(frames2.length, 0, 'No more output after taking frames')

  for (const frame of frames1) {
    frame.close()
  }

  decoder.close()
})

// ============================================================================
// reset() Tests
// ============================================================================

test('VideoDecoder: reset() returns to unconfigured state', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 1)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  decoder.decode(chunks[0])
  decoder.reset()

  // Reset returns to unconfigured state (implementation detail)
  t.is(decoder.state, CodecState.Unconfigured)

  // Output should be cleared
  t.false(decoder.hasOutput())

  decoder.close()
})

test('VideoDecoder: reset() then reconfigure allows new decoding', (t) => {
  // Create two separate encoded streams (each starting with a keyframe)
  const chunks1 = createEncodedH264Chunks(320, 240, 1)
  const chunks2 = createEncodedH264Chunks(320, 240, 1)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  // First decode
  decoder.decode(chunks1[0])
  decoder.flush()
  const frames1 = decoder.takeDecodedFrames()
  t.true(frames1.length > 0, 'First decode should produce frames')
  for (const f of frames1) {
    f.close()
  }

  // Reset (goes to unconfigured)
  decoder.reset()
  t.is(decoder.state, CodecState.Unconfigured)

  // Reconfigure
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))
  t.is(decoder.state, CodecState.Configured)

  // Second decode should work (with fresh keyframe)
  t.notThrows(() => {
    decoder.decode(chunks2[0])
  })

  decoder.flush()
  t.true(decoder.hasOutput(), 'Second decode should produce output')

  const frames2 = decoder.takeDecodedFrames()
  for (const f of frames2) {
    f.close()
  }

  decoder.close()
})

// ============================================================================
// Output Queue Tests
// ============================================================================

test('VideoDecoder: hasOutput() reflects queue state', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 1)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  // Initially empty
  t.false(decoder.hasOutput())

  decoder.decode(chunks[0])
  decoder.flush()

  // After flush, should have output
  t.true(decoder.hasOutput())

  // Take all frames
  const frames = decoder.takeDecodedFrames()
  for (const frame of frames) {
    frame.close()
  }

  // Should be empty again
  t.false(decoder.hasOutput())

  decoder.close()
})

test('VideoDecoder: takeDecodedFrames() empties queue', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 1)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  decoder.decode(chunks[0])
  decoder.flush()

  const frames1 = decoder.takeDecodedFrames()
  t.true(frames1.length > 0)

  // Second call should return empty array
  const frames2 = decoder.takeDecodedFrames()
  t.is(frames2.length, 0)

  for (const frame of frames1) {
    frame.close()
  }

  decoder.close()
})

test('VideoDecoder: takeNextFrame() returns frames one at a time', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 3)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264', { codedWidth: 320, codedHeight: 240 }))

  for (const chunk of chunks) {
    decoder.decode(chunk)
  }
  decoder.flush()

  // Take frames one at a time
  const frames: any[] = []
  while (decoder.hasOutput()) {
    const frame = decoder.takeNextFrame()
    if (frame) {
      frames.push(frame)
    }
    if (frames.length > 100) break // Safety limit
  }

  t.true(frames.length > 0, 'Should have taken at least one frame')

  // No more frames
  const finalFrame = decoder.takeNextFrame()
  t.is(finalFrame, null)

  for (const frame of frames) {
    frame.close()
  }

  decoder.close()
})

// ============================================================================
// isConfigSupported() Tests
// ============================================================================

test('VideoDecoder: isConfigSupported() for H.264', (t) => {
  const config = createDecoderConfig('h264')
  const result = VideoDecoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
  t.is(typeof result.codec, 'string')
})

test('VideoDecoder: isConfigSupported() for VP8', (t) => {
  const config = createDecoderConfig('vp8')
  const result = VideoDecoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
})

test('VideoDecoder: isConfigSupported() for VP9', (t) => {
  const config = createDecoderConfig('vp9')
  const result = VideoDecoder.isConfigSupported(config)

  t.is(typeof result.supported, 'boolean')
})

test('VideoDecoder: isConfigSupported() returns false for unknown codec', (t) => {
  const result = VideoDecoder.isConfigSupported({
    codec: 'unknown-codec',
  })

  t.false(result.supported)
})

// ============================================================================
// Error Handling Tests
// ============================================================================

test('VideoDecoder: decode() on unconfigured decoder throws', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 1)

  const decoder = new VideoDecoder()

  t.throws(() => {
    decoder.decode(chunks[0])
  })

  decoder.close()
})

test('VideoDecoder: decode() on closed decoder throws', (t) => {
  const chunks = createEncodedH264Chunks(320, 240, 1)

  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264'))
  decoder.close()

  t.throws(() => {
    decoder.decode(chunks[0])
  })
})

test('VideoDecoder: flush() on unconfigured decoder throws', (t) => {
  const decoder = new VideoDecoder()

  t.throws(() => {
    decoder.flush()
  })

  decoder.close()
})

test('VideoDecoder: flush() on closed decoder throws', (t) => {
  const decoder = new VideoDecoder()
  decoder.configure(createDecoderConfig('h264'))
  decoder.close()

  t.throws(() => {
    decoder.flush()
  })
})
