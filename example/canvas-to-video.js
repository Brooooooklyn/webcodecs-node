import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { createCanvas } from '@napi-rs/canvas'
import { VideoEncoder, VideoFrame, WebMMuxer, EncodedVideoChunk } from '../index.js'

const __dirname = new URL('.', import.meta.url).pathname

/**
 * Draw a shape that morphs from square to circle
 * @param {CanvasRenderingContext2D} ctx - Canvas context
 * @param {number} centerX - Center X position
 * @param {number} centerY - Center Y position
 * @param {number} size - Size of the shape
 * @param {number} cornerRadius - Corner radius (0 = square, size/2 = circle)
 * @param {string} color - Fill color
 */
function drawRoundedSquare(ctx, centerX, centerY, size, cornerRadius, color) {
  const halfSize = size / 2
  const x = centerX - halfSize
  const y = centerY - halfSize

  // Clamp corner radius to valid range
  const radius = Math.min(cornerRadius, halfSize)

  ctx.beginPath()
  ctx.moveTo(x + radius, y)
  ctx.lineTo(x + size - radius, y)
  ctx.quadraticCurveTo(x + size, y, x + size, y + radius)
  ctx.lineTo(x + size, y + size - radius)
  ctx.quadraticCurveTo(x + size, y + size, x + size - radius, y + size)
  ctx.lineTo(x + radius, y + size)
  ctx.quadraticCurveTo(x, y + size, x, y + size - radius)
  ctx.lineTo(x, y + radius)
  ctx.quadraticCurveTo(x, y, x + radius, y)
  ctx.closePath()

  ctx.fillStyle = color
  ctx.fill()
}

/**
 * Easing function for smooth animation
 */
function easeInOutCubic(t) {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2
}

async function main() {
  const startTime = performance.now()

  // Animation parameters
  const width = 400
  const height = 400
  const fps = 30
  const durationSeconds = 3 // 3 second animation
  const totalFrames = fps * durationSeconds
  const frameDurationUs = Math.round(1_000_000 / fps)

  console.log('Canvas Animation Configuration:')
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

  // Configure encoder for VP9 with alpha channel support
  encoder.configure({
    codec: 'vp09.00.10.08', // VP9 Profile 0, Level 1.0, 8-bit
    width,
    height,
    bitrate: 2_000_000, // 2 Mbps
    framerate: fps,
    // https://w3c.github.io/webcodecs/#dom-videoencoderconfig-alpha
    // Whether the alpha component of the VideoFrame inputs SHOULD be kept or discarded prior to encoding. If alpha is equal to discard, alpha data is always discarded, regardless of a VideoFrame’s format.
    alpha: 'keep', // Keep alpha channel for transparency, default is 'discard'
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

    // Clear canvas with transparent background
    ctx.clearRect(0, 0, width, height)

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

    // Create a VideoFrame from the canvas with alpha preserved
    const timestamp = frameIndex * frameDurationUs
    // https://w3c.github.io/webcodecs/#videoframe-interface
    const frame = new VideoFrame(canvas, {
      timestamp,
      duration: frameDurationUs,
      // Whether to preserve the alpha channel in the canvas.
      // discard: The alpha channel is discarded, and the frame is treated as fully opaque.
      // keep(default): The alpha channel is preserved.
      // alpha: 'discard',
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

  // Now create the muxer and add all chunks
  const muxer = new WebMMuxer()

  // Get codec description from the first keyframe's metadata
  const description = videoMetadatas[0]?.decoderConfig?.description

  // Add video track with VP9 codec and alpha support
  muxer.addVideoTrack({
    codec: 'vp09.00.10.08',
    width,
    height,
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
  const outputPath = join(__dirname, 'canvas.webm')
  writeFileSync(outputPath, webmData)

  const endTime = performance.now()
  const duration = ((endTime - startTime) / 1000).toFixed(2)

  console.log(`\nVideo saved to: ${outputPath}`)
  console.log(`File size: ${(webmData.byteLength / 1024 / 1024).toFixed(2)} MB`)
  console.log(`Total time: ${duration}s`)

  // Verify alpha channel
  console.log('\nTo verify alpha channel, run:')
  console.log(`  ffprobe -v verbose ${outputPath} 2>&1 | grep alpha_mode`)
}

main().catch(console.error)
