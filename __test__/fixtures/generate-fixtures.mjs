// Script to generate minimal test images for ImageDecoder tests
import { writeFileSync } from 'fs'
import { deflateSync } from 'zlib'

// Generate a minimal 8x8 solid color PNG
function generatePNG(width, height, r, g, b) {
  // PNG signature
  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])

  // IHDR chunk
  const ihdrData = Buffer.alloc(13)
  ihdrData.writeUInt32BE(width, 0)
  ihdrData.writeUInt32BE(height, 4)
  ihdrData[8] = 8 // bit depth
  ihdrData[9] = 2 // color type (RGB)
  ihdrData[10] = 0 // compression method
  ihdrData[11] = 0 // filter method
  ihdrData[12] = 0 // interlace method

  const ihdrChunk = createChunk('IHDR', ihdrData)

  // IDAT chunk - raw pixel data
  const rawData = Buffer.alloc(height * (1 + width * 3)) // filter byte + RGB per pixel per row
  for (let y = 0; y < height; y++) {
    const offset = y * (1 + width * 3)
    rawData[offset] = 0 // filter: none
    for (let x = 0; x < width; x++) {
      rawData[offset + 1 + x * 3] = r
      rawData[offset + 1 + x * 3 + 1] = g
      rawData[offset + 1 + x * 3 + 2] = b
    }
  }

  const compressedData = deflateSync(rawData)
  const idatChunk = createChunk('IDAT', compressedData)

  // IEND chunk
  const iendChunk = createChunk('IEND', Buffer.alloc(0))

  return Buffer.concat([signature, ihdrChunk, idatChunk, iendChunk])
}

function createChunk(type, data) {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length, 0)

  const typeBuffer = Buffer.from(type)
  const crc = crc32(Buffer.concat([typeBuffer, data]))

  const crcBuffer = Buffer.alloc(4)
  crcBuffer.writeUInt32BE(crc >>> 0, 0)

  return Buffer.concat([length, typeBuffer, data, crcBuffer])
}

// CRC32 implementation for PNG
function crc32(data) {
  let crc = 0xffffffff
  const table = makeCRCTable()

  for (let i = 0; i < data.length; i++) {
    crc = (crc >>> 8) ^ table[(crc ^ data[i]) & 0xff]
  }

  return crc ^ 0xffffffff
}

function makeCRCTable() {
  const table = Array.from({ length: 256 })
  for (let n = 0; n < 256; n++) {
    let c = n
    for (let k = 0; k < 8; k++) {
      if (c & 1) {
        c = 0xedb88320 ^ (c >>> 1)
      } else {
        c = c >>> 1
      }
    }
    table[n] = c
  }
  return table
}

// Generate a minimal animated GIF (3 frames)
function generateAnimatedGIF(width, height) {
  const colors = [
    [255, 0, 0], // Red
    [0, 255, 0], // Green
    [0, 0, 255], // Blue
  ]

  const parts = []

  // GIF Header
  parts.push(Buffer.from('GIF89a'))

  // Logical Screen Descriptor
  const lsd = Buffer.alloc(7)
  lsd.writeUInt16LE(width, 0)
  lsd.writeUInt16LE(height, 2)
  lsd[4] = 0xf0 // Global color table, 2 colors
  lsd[5] = 0 // Background color index
  lsd[6] = 0 // Pixel aspect ratio
  parts.push(lsd)

  // Global Color Table (2 colors: black and white as placeholder)
  const gct = Buffer.alloc(6)
  gct[0] = 0
  gct[1] = 0
  gct[2] = 0 // black
  gct[3] = 255
  gct[4] = 255
  gct[5] = 255 // white
  parts.push(gct)

  // Application Extension for animation (NETSCAPE2.0)
  parts.push(
    Buffer.from([
      0x21,
      0xff,
      0x0b, // Extension introducer, Application extension, block size
      0x4e,
      0x45,
      0x54,
      0x53,
      0x43,
      0x41,
      0x50,
      0x45, // NETSCAPE
      0x32,
      0x2e,
      0x30, // 2.0
      0x03,
      0x01,
      0x00,
      0x00, // Sub-block size, index 1, loop count (0 = infinite)
      0x00, // Block terminator
    ]),
  )

  // Add frames
  for (let i = 0; i < colors.length; i++) {
    const [r, g, b] = colors[i]

    // Graphic Control Extension (for frame delay)
    parts.push(
      Buffer.from([
        0x21,
        0xf9,
        0x04, // Extension introducer, Graphic control, block size
        0x04, // Disposal method: don't dispose
        0x64,
        0x00, // Delay time (100ms = 10 in 1/100 sec units)
        0x00, // Transparent color index
        0x00, // Block terminator
      ]),
    )

    // Image Descriptor
    const imageDesc = Buffer.alloc(10)
    imageDesc[0] = 0x2c // Image separator
    imageDesc.writeUInt16LE(0, 1) // Left
    imageDesc.writeUInt16LE(0, 3) // Top
    imageDesc.writeUInt16LE(width, 5)
    imageDesc.writeUInt16LE(height, 7)
    imageDesc[9] = 0x87 // Local color table, 256 colors
    parts.push(imageDesc)

    // Local Color Table (256 colors, all set to current frame color)
    const lct = Buffer.alloc(256 * 3)
    for (let c = 0; c < 256; c++) {
      lct[c * 3] = r
      lct[c * 3 + 1] = g
      lct[c * 3 + 2] = b
    }
    parts.push(lct)

    // LZW compressed image data
    // Minimum code size
    parts.push(Buffer.from([0x08])) // 8 bits

    // LZW data - simple clear + data + end
    // For solid color, we just need to output color index 0 for all pixels
    const lzwData = generateLZWData(width * height)
    parts.push(lzwData)
  }

  // GIF Trailer
  parts.push(Buffer.from([0x3b]))

  return Buffer.concat(parts)
}

// Generate simple LZW compressed data for solid color (all index 0)
function generateLZWData(pixelCount) {
  // For 8-bit LZW: clear code = 256, end code = 257
  // We'll use a simple approach: clear + literal 0s + end

  const minCodeSize = 8
  const clearCode = 1 << minCodeSize // 256
  const endCode = clearCode + 1 // 257

  const output = []
  let currentByte = 0
  let bitPos = 0
  let codeSize = minCodeSize + 1 // 9 bits

  function writeCode(code) {
    currentByte |= code << bitPos
    bitPos += codeSize
    while (bitPos >= 8) {
      output.push(currentByte & 0xff)
      currentByte >>= 8
      bitPos -= 8
    }
  }

  // Write clear code
  writeCode(clearCode)

  // Write pixel data (all 0s)
  for (let i = 0; i < pixelCount; i++) {
    writeCode(0)
  }

  // Write end code
  writeCode(endCode)

  // Flush remaining bits
  if (bitPos > 0) {
    output.push(currentByte & 0xff)
  }

  // Split into sub-blocks (max 255 bytes each)
  const blocks = []
  for (let i = 0; i < output.length; i += 255) {
    const blockSize = Math.min(255, output.length - i)
    blocks.push(Buffer.from([blockSize]))
    blocks.push(Buffer.from(output.slice(i, i + blockSize)))
  }
  blocks.push(Buffer.from([0x00])) // Block terminator

  return Buffer.concat(blocks)
}

// Generate fixtures
console.log('Generating test PNG (8x8 red)...')
const png = generatePNG(8, 8, 255, 0, 0)
writeFileSync(new URL('./test.png', import.meta.url), png)
console.log(`Created test.png (${png.length} bytes)`)

console.log('Generating animated GIF (8x8, 3 frames)...')
const gif = generateAnimatedGIF(8, 8)
writeFileSync(new URL('./animated.gif', import.meta.url), gif)
console.log(`Created animated.gif (${gif.length} bytes)`)

console.log('Done!')
