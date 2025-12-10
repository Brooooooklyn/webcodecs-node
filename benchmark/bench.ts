import { Bench } from 'tinybench'

import { VideoEncoder, VideoFrame } from '../index.js'

/**
 * I420 format buffer size multiplier: 1.5x (width * height)
 * - Y plane: width * height = 1.0
 * - U plane: (width/2) * (height/2) = 0.25
 * - V plane: (width/2) * (height/2) = 0.25
 * - Total: 1.0 + 0.25 + 0.25 = 1.5
 */
const I420_SIZE_MULTIPLIER = 1.5

const bench = new Bench()

// Create encoder for benchmarking
function createTestEncoder() {
  return new VideoEncoder({
    output: () => {},
    error: () => {},
  })
}

bench.add('Create and configure VideoEncoder', () => {
  const encoder = createTestEncoder()
  encoder.configure({
    codec: 'avc1.42001E',
    width: 640,
    height: 480,
    bitrate: 1_000_000,
  })
  encoder.close()
})

bench.add('Create VideoFrame (I420)', () => {
  const buffer = Buffer.alloc(640 * 480 * I420_SIZE_MULTIPLIER)
  const frame = new VideoFrame(buffer, {
    format: 'I420',
    codedWidth: 640,
    codedHeight: 480,
    timestamp: 0,
  })
  frame.close()
})

await bench.run()

console.table(bench.table())
