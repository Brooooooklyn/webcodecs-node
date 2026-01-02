import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { createCanvas } from '@napi-rs/canvas'
import { VideoEncoder, VideoFrame, Mp4Muxer, EncodedVideoChunk } from '../index.js'
import { drawRoundedSquare, easeInOutCubic } from './canvas-utils.js'

const __dirname = new URL('.', import.meta.url).pathname

async function main() {
  const startTime = performance.now()

  // Animation parameters
  const width = 400
  const height = 400
  const fps = 30
  const durationSeconds = 3 // 3 second animation
  const totalFrames = fps * durationSeconds
  const frameDurationUs = Math.round(1_000_000 / fps)

  console.log('Canvas Animation Configuration (HEVC/H.265):')
  console.log(`  Size: ${width}x${height}`)
  console.log(`  FPS: ${fps}`)
  console.log(`  Duration: ${durationSeconds}s`)
  console.log(`  Total Frames: ${totalFrames}`)

  // Create the canvas for rendering
  const canvas = createCanvas(width, height)
  const ctx = canvas.getContext('2d')

  // Collect all encoded chunks and metadata
  const videoChunks = []
  const videoMetadatas = []

  // Create video encoder
  const encoder = new VideoEncoder({
    output: (chunk, meta) => {
      videoChunks.push(chunk)
      videoMetadatas.push(meta)

      const count = videoChunks.length
      if (count % 30 === 0 || count === totalFrames) {
        console.log(`  Encoded ${count}/${totalFrames} frames`)
      }
    },
    error: (e) => {
      console.error('Encoder error:', e)
    },
  })

  // Configure encoder for HEVC (H.265)
  // HEVC codec string format: hev1.P.T.LL.CC or hvc1.P.T.LL.CC
  // - hev1/hvc1: HEVC codec identifier
  // - P: Profile (1=Main, 2=Main10)
  // - T: Tier and constraints (6=Main tier)
  // - LL: Level (L93=Level 3.1, L120=Level 4.0, L150=Level 5.0)
  // - CC: Constraint flags (B0=no constraints)
  encoder.configure({
    // codec: 'hev1.1.6.L93.B0', // Safari not support
    codec: 'hvc1.1.6.L93.B0', // Safari compatible hvc1
    width,
    height,
    bitrate: 2_000_000, // 2 Mbps
    framerate: fps,
    // latencyMode: 'realtime', // When set realtime, has_b_frames = 0
  })

  console.log('\nRendering and encoding frames...')

  // Animation: Square → Circle with scaling
  // Phase 1 (0-50%): Scale up while morphing to circle
  // Phase 2 (50-100%): Scale down while morphing back to square

  const baseSize = 150
  const maxSize = 250
  const centerX = width / 2
  const centerY = height / 2

  for (let frameIndex = 0; frameIndex < totalFrames; frameIndex++) {
    // Calculate animation progress (0 to 1)
    const progress = frameIndex / (totalFrames - 1)

    // Create a looping animation: 0→1→0
    const loopProgress =
      progress < 0.5
        ? progress * 2 // 0 to 1 in first half
        : 2 - progress * 2 // 1 to 0 in second half

    const easedProgress = easeInOutCubic(loopProgress)

    // Calculate current size (scale from base to max and back)
    const currentSize = baseSize + (maxSize - baseSize) * easedProgress

    // Calculate corner radius (0 = square, currentSize/2 = circle)
    const cornerRadius = (currentSize / 2) * easedProgress

    // Color gradient from blue to purple based on progress
    const hue = 220 + 60 * easedProgress // Blue (220) to Purple (280)
    const saturation = 70 + 20 * easedProgress
    const lightness = 50 + 10 * easedProgress
    const color = `hsl(${hue}, ${saturation}%, ${lightness}%)`

    // Clear canvas with solid background (HEVC doesn't support alpha in most cases)
    ctx.fillStyle = '#1a1a2e'
    ctx.fillRect(0, 0, width, height)

    // Add subtle rotation for extra visual interest
    const rotation = (easedProgress * Math.PI) / 6 // Rotate up to 30 degrees

    ctx.save()
    ctx.translate(centerX, centerY)
    ctx.rotate(rotation)
    ctx.translate(-centerX, -centerY)

    // Draw the morphing shape
    drawRoundedSquare(ctx, centerX, centerY, currentSize, cornerRadius, color)

    // Add a shadow/glow effect
    ctx.shadowColor = color
    ctx.shadowBlur = 20 * easedProgress
    drawRoundedSquare(ctx, centerX, centerY, currentSize * 0.95, cornerRadius * 0.95, color)
    ctx.shadowBlur = 0

    ctx.restore()

    // Create a VideoFrame from the canvas
    const timestamp = frameIndex * frameDurationUs
    const frame = new VideoFrame(canvas, {
      timestamp,
      duration: frameDurationUs,
    })

    // Encode the frame (request keyframe every 1 second)
    const isKeyFrame = frameIndex % fps === 0
    encoder.encode(frame, { keyFrame: isKeyFrame })

    // Close the frame to release resources
    frame.close()
  }

  // Flush the encoder to ensure all frames are processed
  console.log('\nFlushing encoder...')
  await encoder.flush()
  encoder.close()

  console.log(`\nCollected ${videoChunks.length} chunks`)

  // Now create the MP4 muxer (MP4 container supports HEVC)
  const muxer = new Mp4Muxer()
  // const muxer = new Mp4Muxer({ fastStart: true }) // segmentation fault on HEVC

  // Get codec description from the first keyframe's metadata
  const description = videoMetadatas[0]?.decoderConfig?.description

  // Add video track with HEVC codec
  muxer.addVideoTrack({
    // codec: 'hev1.1.6.L93.B0', // Safari not support
    codec: 'hvc1.1.6.L93.B0', // Safari compatible hvc1
    width,
    height,
    description,
    framerateNum: fps,
    framerateDen: 1,
  })

  console.log('Muxing chunks...')

  // Add all chunks to the muxer
  for (let i = 0; i < videoChunks.length; i++) {
    const chunk = videoChunks[i]
    muxer.addVideoChunk(chunk, videoMetadatas[i])
  }

  // Flush and finalize the muxer
  console.log('Finalizing MP4...')
  muxer.flush()
  const mp4Data = muxer.finalize()
  muxer.close()

  // Write to file
  const outputPath = join(__dirname, 'canvas-hevc.mp4')
  writeFileSync(outputPath, mp4Data)

  const endTime = performance.now()
  const duration = ((endTime - startTime) / 1000).toFixed(2)

  console.log(`\nVideo saved to: ${outputPath}`)
  console.log(`File size: ${(mp4Data.byteLength / 1024 / 1024).toFixed(2)} MB`)
  console.log(`Total time: ${duration}s`)

  // Verify HEVC codec
  console.log('\nTo verify HEVC codec, run:')
  console.log(`  ffprobe -v quiet -show_streams ${outputPath} | grep codec_name`)
}

main().catch(console.error)