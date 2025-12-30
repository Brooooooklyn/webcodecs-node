/**
 * Benchmark: @napi-rs/webcodecs with real video file
 */

import * as fs from 'node:fs'
import * as path from 'node:path'
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

// Demux the video
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

// Decode all frames
const frames: NapiWebCodecs.VideoFrame[] = []

const decoder = new NapiWebCodecs.VideoDecoder({
  output: (frame) => {
    frames.push(frame)
  },
  error: (e) => console.error('Decode error:', e),
})
decoder.configure(videoConfig)

const decodeStart = performance.now()
for (const chunk of chunks) {
  decoder.decode(
    new NapiWebCodecs.EncodedVideoChunk({
      type: chunk.type,
      timestamp: chunk.timestamp,
      data: chunk.data,
    }),
  )
}
await decoder.flush()
const decodeEnd = performance.now()
decoder.close()

results.decodeFps = (frames.length / (decodeEnd - decodeStart)) * 1000

// Re-encode all frames
let encodedCount = 0
let encodedBytes = 0
const encoder = new NapiWebCodecs.VideoEncoder({
  output: (chunk) => {
    encodedCount++
    encodedBytes += chunk.byteLength
  },
  error: (e) => console.error('Encode error:', e),
})
encoder.configure({
  codec: 'avc1.42001E',
  width: videoConfig.codedWidth,
  height: videoConfig.codedHeight,
  bitrate: 2_000_000,
  framerate: 30,
  latencyMode: 'realtime',
  hardwareAcceleration: 'prefer-software',
})

const encodeStart = performance.now()
for (let i = 0; i < frames.length; i++) {
  encoder.encode(frames[i]!, { keyFrame: i % 30 === 0 })
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
