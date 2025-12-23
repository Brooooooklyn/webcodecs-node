import { readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { createCanvas, LottieAnimation } from '@napi-rs/canvas'
import { VideoEncoder, VideoFrame, WebMMuxer, EncodedVideoChunk } from '../index.js'

const __dirname = new URL('.', import.meta.url).pathname
const lottieFile = './Christmas.json'

async function main() {
  const startTime = performance.now()

  const animation = LottieAnimation.loadFromData(readFileSync(join(__dirname, lottieFile), 'utf-8'))

  console.log('Lottie Info:')
  console.log(`  Duration: ${animation.duration.toFixed(2)}s`)
  console.log(`  FPS: ${animation.fps}`)
  console.log(`  Frames: ${animation.frames}`)
  console.log(`  Resolution: ${animation.width}x${animation.height}`)
  console.log(`  Version: ${animation.version}`)

  const encodedWidth = 512
  const encodedHeight = 512
  console.log(`\nVideo resolution: ${encodedWidth}x${encodedHeight}`)

  // Create the canvas for rendering
  const canvas = createCanvas(encodedWidth, encodedHeight)
  const ctx = canvas.getContext('2d')

  // Calculate frame duration in microseconds
  const fps = animation.fps
  const frameDurationUs = Math.round(1_000_000 / fps)
  const totalFrames = Math.round(animation.frames)

  // Collect all encoded chunks and metadata first (following webcodecs test pattern)
  const videoChunks = []
  const videoMetadatas = []

  // Create video encoder
  const encoder = new VideoEncoder({
    output: (chunk, meta) => {
      videoChunks.push(chunk)
      videoMetadatas.push(meta)
      console.log('✅ alphaSideData', meta.alphaSideData?.length)

      const count = videoChunks.length
      if (count % 30 === 0 || count === totalFrames) {
        console.log(`  Encoded ${count}/${totalFrames} frames`)
      }
    },
    error: (e) => {
      console.error('Encoder error:', e)
    },
  })

  // Configure encoder for VP9 with alpha channel support
  encoder.configure({
    codec: 'vp09.00.10.08', // VP9 Profile 0, Level 1.0, 8-bit
    width: encodedWidth,
    height: encodedHeight,
    bitrate: 5_000_000, // 5 Mbps
    framerate: fps,
    alpha: 'keep', // Keep alpha channel for transparency
  })

  console.log('\nEncoding frames...')

  // Render and encode each frame
  for (let frameIndex = 0; frameIndex < totalFrames; frameIndex++) {
    // Seek to exact frame for precise animation timing
    animation.seekFrame(frameIndex)

    // Clear the canvas with transparent background
    ctx.clearRect(0, 0, encodedWidth, encodedHeight)

    // Render the animation with destination rect for proper scaling
    // Note: ctx.scale() doesn't affect Skottie rendering - must use dst rect
    animation.render(ctx, { x: 0, y: 0, width: encodedWidth, height: encodedHeight })

    // Create a VideoFrame from the canvas with alpha preserved
    const timestamp = frameIndex * frameDurationUs
    const frame = new VideoFrame(canvas, {
      timestamp,
      duration: frameDurationUs,
      // Whether to preserve the alpha channel in the canvas.
      // discard: The alpha channel is discarded, and the frame is treated as fully opaque.
      // keep(default): The alpha channel is preserved.
      // alpha: 'discard',
    })

    // Encode the frame (request keyframe every 2 seconds)
    const isKeyFrame = frameIndex % Math.round(fps * 2) === 0
    encoder.encode(frame, { keyFrame: isKeyFrame })

    // Close the frame to release resources
    frame.close()
  }

  // Flush the encoder to ensure all frames are processed
  console.log('\nFlushing encoder...')
  await encoder.flush()
  encoder.close()

  console.log(`\nCollected ${videoChunks.length} chunks`)

  // Now create the muxer and add all chunks
  const muxer = new WebMMuxer()

  // Get codec description from the first keyframe's metadata
  const description = videoMetadatas[0]?.decoderConfig?.description

  // Add video track with VP9 codec and alpha support
  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width: encodedWidth,
    height: encodedHeight,
    description,
    alpha: true, // Enable alpha channel for transparency
  })

  console.log('Muxing chunks...')

  // Add all chunks to the muxer
  // WebM uses milliseconds for timestamps, so we need to convert from microseconds
  for (let i = 0; i < videoChunks.length; i++) {
    const chunk = videoChunks[i]
    // Create new chunk with timestamp converted to milliseconds
    const data = new Uint8Array(chunk.byteLength)
    chunk.copyTo(data)
    const convertedChunk = new EncodedVideoChunk({
      type: chunk.type,
      timestamp: Math.round(chunk.timestamp / 1000), // Convert µs to ms
      duration: chunk.duration ? Math.round(chunk.duration / 1000) : undefined,
      data,
    })
    muxer.addVideoChunk(convertedChunk, videoMetadatas[i])
  }

  // Flush and finalize the muxer
  console.log('Finalizing WebM...')
  muxer.flush()
  const webmData = muxer.finalize()
  muxer.close()

  // Write to file
  const outputPath = join(__dirname, 'lottie.webm')
  writeFileSync(outputPath, webmData)

  const endTime = performance.now()
  const duration = ((endTime - startTime) / 1000).toFixed(2)

  console.log(`\nVideo saved to: ${outputPath}`)
  console.log(`File size: ${(webmData.byteLength / 1024 / 1024).toFixed(2)} MB`)
  console.log(`Total time: ${duration}s`)
}

main().catch(console.error)
