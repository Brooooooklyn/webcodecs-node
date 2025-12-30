/**
 * Benchmark: webcodecs-polyfill with real video file
 * Note: Uses napi-rs Mp4Demuxer since polyfill doesn't have one
 */

import * as fs from 'node:fs'
import * as path from 'node:path'

import * as Polyfill from 'webcodecs-polyfill'

import * as NapiWebCodecs from '../index.js'

const VIDEO_PATH = path.join(import.meta.dirname, '../__test__/fixtures/small_buck_bunny.mp4')

interface Results {
  decodeFps: number
  encodeFps: number
  encodeBytesPerFrame: number
  totalFrames: number
  resolution: string
}

const results: Results = {
  decodeFps: 0,
  encodeFps: 0,
  encodeBytesPerFrame: 0,
  totalFrames: 0,
  resolution: '',
}

// Read video file
const videoData = fs.readFileSync(VIDEO_PATH)

// Demux the video using napi-rs (polyfill doesn't have demuxer)
const chunks: Array<{ data: Uint8Array; type: 'key' | 'delta'; timestamp: number }> = []
let videoConfig: any = null

const demuxer = new NapiWebCodecs.Mp4Demuxer({
  videoOutput: (chunk) => {
    const data = new Uint8Array(chunk.byteLength)
    chunk.copyTo(data)
    chunks.push({ data, type: chunk.type, timestamp: Number(chunk.timestamp) })
  },
  audioOutput: () => {},
  error: (e) => console.error('Demux error:', e),
})

await demuxer.loadBuffer(videoData)

videoConfig = demuxer.videoDecoderConfig
results.resolution = `${videoConfig.codedWidth}x${videoConfig.codedHeight}`

// Demux all packets using async API
await demuxer.demuxAsync()
demuxer.close()

results.totalFrames = chunks.length

// Decode all frames with polyfill
const frames: any[] = []
let decodeCount = 0

const decoder = new Polyfill.VideoDecoderPolyfill({
  output: (frame: any) => {
    decodeCount++
    frames.push(frame)
  },
  error: (e: any) => console.error('Decode error:', e),
})
decoder.configure({
  codec: videoConfig.codec,
  codedWidth: videoConfig.codedWidth,
  codedHeight: videoConfig.codedHeight,
  description: videoConfig.description,
})

const decodeStart = performance.now()
for (const chunk of chunks) {
  decoder.decode(
    new Polyfill.EncodedVideoChunkPolyfill({
      type: chunk.type,
      timestamp: chunk.timestamp,
      data: chunk.data,
    }),
  )
}
await decoder.flush()
const decodeEnd = performance.now()
decoder.close()

results.decodeFps = (decodeCount / (decodeEnd - decodeStart)) * 1000

// Re-encode all frames with polyfill
let encodedCount = 0
let encodedBytes = 0
const encoder = new Polyfill.VideoEncoderPolyfill({
  output: (chunk: any) => {
    encodedCount++
    encodedBytes += chunk.byteLength
  },
  error: (e: any) => console.error('Encode error:', e),
})
encoder.configure({
  codec: 'avc1.42001E',
  width: videoConfig.codedWidth,
  height: videoConfig.codedHeight,
  bitrate: 2_000_000,
  framerate: 30,
  latencyMode: 'realtime',
})

const encodeStart = performance.now()
for (let i = 0; i < frames.length; i++) {
  encoder.encode(frames[i], { keyFrame: i % 30 === 0 })
}
await encoder.flush()
const encodeEnd = performance.now()
encoder.close()

results.encodeFps = (encodedCount / (encodeEnd - encodeStart)) * 1000
results.encodeBytesPerFrame = encodedBytes / encodedCount

// Close all frames
for (const frame of frames) {
  frame.close()
}

// Output as JSON
console.log(JSON.stringify(results))
