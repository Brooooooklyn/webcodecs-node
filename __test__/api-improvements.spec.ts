import test from 'ava'
import { VideoEncoder, VideoFrame, VideoPixelFormat } from '../index.js'

// ============================================================================
// Phase 2: bitrateMode, latencyMode Tests
// ============================================================================

test('VideoEncoderConfig: accepts bitrateMode constant', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure({
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 1_000_000,
    bitrateMode: 'constant',
  })
  t.is(encoder.state, 'Configured')
  encoder.close()
})

test('VideoEncoderConfig: accepts bitrateMode variable', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure({
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 1_000_000,
    bitrateMode: 'variable',
  })
  t.is(encoder.state, 'Configured')
  encoder.close()
})

test('VideoEncoderConfig: accepts latencyMode quality', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure({
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 1_000_000,
    latencyMode: 'quality',
  })
  t.is(encoder.state, 'Configured')
  encoder.close()
})

test('VideoEncoderConfig: accepts latencyMode realtime', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure({
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 1_000_000,
    latencyMode: 'realtime',
  })
  t.is(encoder.state, 'Configured')
  encoder.close()
})

test('VideoEncoderConfig: accepts scalabilityMode L1T1', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure({
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 1_000_000,
    scalabilityMode: 'L1T1',
  })
  t.is(encoder.state, 'Configured')
  encoder.close()
})

test('VideoEncoderConfig: combined bitrateMode and latencyMode', (t) => {
  const encoder = new VideoEncoder()
  encoder.configure({
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 1_000_000,
    bitrateMode: 'variable',
    latencyMode: 'realtime',
  })
  t.is(encoder.state, 'Configured')

  // Create and encode a frame to verify encoder works
  // I420 buffer: Y (640*480) + U (320*240) + V (320*240) = 460800 bytes
  const frameData = Buffer.alloc(640 * 480 + 320 * 240 * 2)
  const frame = new VideoFrame(frameData, {
    format: VideoPixelFormat.I420,
    codedWidth: 640,
    codedHeight: 480,
    timestamp: 0,
  })
  encoder.encode(frame)
  frame.close()

  encoder.flush()
  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length > 0, 'Should produce encoded output')

  encoder.close()
})

test('VideoEncoderConfig: queue mode works (no callbacks)', (t) => {
  const encoder = new VideoEncoder()

  encoder.configure({
    codec: 'avc1.42001E',
    width: 320,
    height: 240,
    bitrate: 500_000,
    framerate: 30,
  })

  // Create and encode some frames
  // I420 buffer: Y (320*240) + U (160*120) + V (160*120) = 115200 bytes
  const frameData = Buffer.alloc(320 * 240 + 160 * 120 * 2)
  for (let i = 0; i < 10; i++) {
    const frame = new VideoFrame(frameData, {
      format: VideoPixelFormat.I420,
      codedWidth: 320,
      codedHeight: 240,
      timestamp: i * 33333,
    })
    encoder.encode(frame)
    frame.close()
  }

  encoder.flush()

  // In queue mode, chunks should be available via takeEncodedChunks
  const chunks = encoder.takeEncodedChunks()
  t.true(chunks.length > 0, 'Should have encoded chunks in queue')

  encoder.close()
})
