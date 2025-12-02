import { Bench } from 'tinybench'

import { VideoEncoder, VideoFrame } from '../index.js'

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
  const buffer = Buffer.alloc(640 * 480 * 1.5)
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
