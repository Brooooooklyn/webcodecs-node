# @napi-rs/webcodecs

[![CI](https://github.com/Brooooooklyn/webcodecs-node/actions/workflows/CI.yml/badge.svg)](https://github.com/Brooooooklyn/webcodecs-node/actions/workflows/CI.yml)

WebCodecs API implementation for Node.js using FFmpeg, built with [NAPI-RS](https://napi.rs).

## Features

- **W3C WebCodecs API compliant** - Full implementation of the WebCodecs specification with native `DOMException` errors
- **Video encoding/decoding** - H.264, H.265, VP8, VP9, AV1
- **Audio encoding/decoding** - AAC, Opus, MP3, FLAC, Vorbis, PCM variants
- **Container muxing/demuxing** - MP4, WebM, MKV containers with seeking support
- **Image decoding** - JPEG, PNG, WebP, GIF, BMP, AVIF, JPEG XL
- **Canvas integration** - Create VideoFrames from `@napi-rs/canvas` for graphics and text rendering
- **Hardware acceleration** - Zero-copy GPU encoding with VideoToolbox (macOS), NVENC (NVIDIA), VAAPI (Linux), QSV (Intel)
- **Cross-platform** - macOS, Windows, Linux (glibc/musl, x64/arm64/armv7)
- **Structured logging** - FFmpeg logs redirected to Rust `tracing` crate for easy integration

## Installation

```bash
bun add @napi-rs/webcodecs
# or
pnpm add @napi-rs/webcodecs
# or
yarn add @napi-rs/webcodecs
```

### Optional: Canvas Integration

For creating VideoFrames from canvas content, install `@napi-rs/canvas`:

```bash
npm install @napi-rs/canvas
```

## Quick Start

### Video Encoding

```typescript
import { VideoEncoder, VideoFrame } from '@napi-rs/webcodecs'

const encoder = new VideoEncoder({
  output: (chunk, metadata) => {
    console.log(`Encoded ${chunk.type} chunk: ${chunk.byteLength} bytes`)
  },
  error: (e) => console.error(e),
})

encoder.configure({
  codec: 'avc1.42001E', // H.264 Baseline
  width: 1920,
  height: 1080,
  bitrate: 5_000_000,
  hardwareAcceleration: 'prefer-hardware', // Use GPU when available
  latencyMode: 'realtime', // Optimize for low latency
})

// Create and encode frames
const frameData = new Uint8Array(1920 * 1080 * 4) // RGBA
const frame = new VideoFrame(frameData, {
  format: 'RGBA',
  codedWidth: 1920,
  codedHeight: 1080,
  timestamp: 0,
})

encoder.encode(frame)
frame.close()

// Force a keyframe for seeking/streaming
const frame2 = new VideoFrame(frameData, {
  format: 'RGBA',
  codedWidth: 1920,
  codedHeight: 1080,
  timestamp: 33333, // 30fps
})
encoder.encode(frame2, { keyFrame: true }) // Force I-frame
frame2.close()

await encoder.flush()
encoder.close()
```

### Video Decoding

```typescript
import { VideoDecoder, EncodedVideoChunk } from '@napi-rs/webcodecs'

const decoder = new VideoDecoder({
  output: (frame) => {
    console.log(`Decoded frame: ${frame.codedWidth}x${frame.codedHeight}`)
    frame.close()
  },
  error: (e) => console.error(e),
})

decoder.configure({
  codec: 'avc1.42001E',
  codedWidth: 1920,
  codedHeight: 1080,
})

// Decode chunks
const chunk = new EncodedVideoChunk({
  type: 'key',
  timestamp: 0,
  data: encodedData,
})

decoder.decode(chunk)
await decoder.flush()
decoder.close()
```

### Audio Encoding

```typescript
import { AudioEncoder, AudioData } from '@napi-rs/webcodecs'

const encoder = new AudioEncoder({
  output: (chunk, metadata) => {
    console.log(`Encoded audio: ${chunk.byteLength} bytes`)
  },
  error: (e) => console.error(e),
})

encoder.configure({
  codec: 'opus',
  sampleRate: 48000,
  numberOfChannels: 2,
  bitrate: 128000,
})

const audioData = new AudioData({
  format: 'f32-planar',
  sampleRate: 48000,
  numberOfFrames: 1024,
  numberOfChannels: 2,
  timestamp: 0,
  data: new Float32Array(1024 * 2),
})

encoder.encode(audioData)
audioData.close()

await encoder.flush()
encoder.close()
```

### Image Decoding

```typescript
import { ImageDecoder } from '@napi-rs/webcodecs'
import { readFileSync } from 'fs'

const imageData = readFileSync('image.png')
const decoder = new ImageDecoder({
  data: imageData,
  type: 'image/png',
})

const result = await decoder.decode()
console.log(`Image: ${result.image.codedWidth}x${result.image.codedHeight}`)
result.image.close()
decoder.close()
```

### Container Demuxing

Read encoded video/audio from MP4, WebM, or MKV containers:

```typescript
import { Mp4Demuxer, VideoDecoder, AudioDecoder } from '@napi-rs/webcodecs'

// Create decoder instances
const videoDecoder = new VideoDecoder({
  output: (frame) => {
    console.log(`Decoded frame: ${frame.timestamp}`)
    frame.close()
  },
  error: (e) => console.error(e),
})

const audioDecoder = new AudioDecoder({
  output: (data) => {
    console.log(`Decoded audio: ${data.timestamp}`)
    data.close()
  },
  error: (e) => console.error(e),
})

// Create demuxer with callbacks
const demuxer = new Mp4Demuxer({
  videoOutput: (chunk) => videoDecoder.decode(chunk),
  audioOutput: (chunk) => audioDecoder.decode(chunk),
  error: (e) => console.error(e),
})

// Load from file or buffer
await demuxer.load('./video.mp4')
// or: await demuxer.loadBuffer(uint8Array)

// Configure decoders with extracted configs
videoDecoder.configure(demuxer.videoDecoderConfig)
audioDecoder.configure(demuxer.audioDecoderConfig)

// Get track info
console.log(demuxer.tracks) // Array of track info
console.log(demuxer.duration) // Duration in microseconds

// Demux all packets (calls callbacks)
demuxer.demux()

// Or demux in batches
demuxer.demux(100) // Demux up to 100 packets

// Seek to timestamp (microseconds)
demuxer.seek(5_000_000) // Seek to 5 seconds

demuxer.close()
```

### Container Muxing

Write encoded video/audio to MP4, WebM, or MKV containers:

```typescript
import { Mp4Muxer, VideoEncoder, AudioEncoder } from '@napi-rs/webcodecs'
import { writeFileSync } from 'fs'

// Create muxer
const muxer = new Mp4Muxer({ fastStart: true })

// Track description will be set from encoder metadata
let videoDescription: Uint8Array | undefined

// Create encoder that feeds into muxer
const videoEncoder = new VideoEncoder({
  output: (chunk, metadata) => {
    // Capture codec description from first keyframe
    if (metadata?.decoderConfig?.description) {
      videoDescription = metadata.decoderConfig.description
    }
    muxer.addVideoChunk(chunk, metadata)
  },
  error: (e) => console.error(e),
})

videoEncoder.configure({
  codec: 'avc1.42001E',
  width: 1920,
  height: 1080,
  bitrate: 5_000_000,
})

// Add tracks before adding chunks
muxer.addVideoTrack({
  codec: 'avc1.42001E',
  width: 1920,
  height: 1080,
})

// Encode frames...
// videoEncoder.encode(frame)

await videoEncoder.flush()

// Finalize and write output
const mp4Data = muxer.finalize()
writeFileSync('output.mp4', mp4Data)
muxer.close()
```

#### Streaming Muxer Mode

For live streaming or large files, use streaming mode:

```typescript
import { Mp4Muxer } from '@napi-rs/webcodecs'

const muxer = new Mp4Muxer({
  fragmented: true, // Required for MP4 streaming
  streaming: { bufferCapacity: 256 * 1024 },
})

muxer.addVideoTrack({ codec: 'avc1.42001E', width: 1920, height: 1080 })

// Add chunks as they arrive
muxer.addVideoChunk(chunk, metadata)

// Read available data incrementally
const data = muxer.read()
if (data) {
  stream.write(data)
}

// Check when finished
if (muxer.isFinished) {
  stream.end()
}
```

### VideoFrame from Canvas

Create VideoFrames from `@napi-rs/canvas` for graphics, text rendering, or image compositing:

```typescript
import { VideoFrame } from '@napi-rs/webcodecs'
import { createCanvas } from '@napi-rs/canvas'

const canvas = createCanvas(1920, 1080)
const ctx = canvas.getContext('2d')

// Draw graphics
ctx.fillStyle = '#FF0000'
ctx.fillRect(0, 0, 1920, 1080)
ctx.fillStyle = '#FFFFFF'
ctx.font = '48px sans-serif'
ctx.fillText('Hello WebCodecs!', 100, 100)

// Create VideoFrame from canvas (timestamp required per W3C spec)
const frame = new VideoFrame(canvas, {
  timestamp: 0,
  duration: 33333, // optional: frame duration in microseconds
})

console.log(frame.format) // 'RGBA'
console.log(frame.codedWidth, frame.codedHeight) // 1920, 1080

// Use with VideoEncoder (see Video Encoding section)
frame.close()
```

**Note:** Canvas pixel data is copied as RGBA format with sRGB color space.

## Supported Codecs

### Video

| Codec | Codec String            | Encoding | Decoding |
| ----- | ----------------------- | -------- | -------- |
| H.264 | `avc1.*`                | ✅       | ✅       |
| H.265 | `hev1.*`, `hvc1.*`      | ✅       | ✅       |
| VP8   | `vp8`                   | ✅       | ✅       |
| VP9   | `vp09.*`, `vp9`         | ✅       | ✅       |
| AV1   | `av01.*`, `av01`, `av1` | ✅       | ✅       |

**Note:** Short form codec strings (`vp9`, `av01`, `av1`) are accepted for compatibility with browser implementations.

### Audio

| Codec  | Codec String | Encoding | Decoding |
| ------ | ------------ | -------- | -------- |
| AAC    | `mp4a.40.2`  | ✅       | ✅       |
| Opus   | `opus`       | ✅       | ✅       |
| MP3    | `mp3`        | ✅       | ✅       |
| FLAC   | `flac`       | ✅       | ✅       |
| Vorbis | `vorbis`     | ❌       | ✅       |
| PCM    | `pcm-*`      | ❌       | ✅       |

### Image

| Format  | MIME Type    | Decoding |
| ------- | ------------ | -------- |
| JPEG    | `image/jpeg` | ✅       |
| PNG     | `image/png`  | ✅       |
| WebP    | `image/webp` | ✅       |
| GIF     | `image/gif`  | ✅       |
| BMP     | `image/bmp`  | ✅       |
| AVIF    | `image/avif` | ✅       |
| JPEG XL | `image/jxl`  | ✅       |

### Containers

| Container | Video Codecs                | Audio Codecs                 | Demuxer       | Muxer       |
| --------- | --------------------------- | ---------------------------- | ------------- | ----------- |
| MP4       | H.264, H.265, AV1           | AAC, Opus, MP3, FLAC         | `Mp4Demuxer`  | `Mp4Muxer`  |
| WebM      | VP8, VP9, AV1               | Opus, Vorbis                 | `WebMDemuxer` | `WebMMuxer` |
| MKV       | H.264, H.265, VP8, VP9, AV1 | AAC, Opus, Vorbis, FLAC, MP3 | `MkvDemuxer`  | `MkvMuxer`  |

## Platform Support

Pre-built binaries are available for:

| Platform                 | Architecture |
| ------------------------ | ------------ |
| macOS                    | x64, arm64   |
| Windows                  | x64, arm64   |
| Linux (glibc)            | x64, arm64   |
| Linux (musl)             | x64, arm64   |
| Linux (glibc, gnueabihf) | armv7        |

## W3C Web Platform Tests Compliance

This implementation is validated against the [W3C Web Platform Tests](https://github.com/web-platform-tests/wpt) for WebCodecs.

### Ported Tests Status

| Status      | Count | Percentage |
| ----------- | ----- | ---------- |
| **Passing** | 573   | 99.1%      |
| **Skipped** | 5     | 0.9%       |
| **Failing** | 0     | 0%         |

**Skipped tests** are due to platform-specific features or edge cases.

### Tests Not Ported (Browser-Only)

19 WPT test files require browser APIs unavailable in Node.js:

| Category               | Tests | APIs Required                          |
| ---------------------- | ----- | -------------------------------------- |
| Serialization/Transfer | 5     | MessageChannel, structured clone       |
| WebGL/Canvas           | 5     | WebGL textures, ImageBitmap, Canvas 2D |
| Cross-Origin Isolation | 8     | COOP/COEP headers                      |
| WebIDL                 | 1     | IDL interface validation               |

See [`__test__/wpt/README.md`](./__test__/wpt/README.md) for detailed test status.

## Hardware Acceleration

Hardware encoding is fully supported with automatic GPU selection and fallback:

| Platform | Encoders     | Features                                               |
| -------- | ------------ | ------------------------------------------------------ |
| macOS    | VideoToolbox | H.264, HEVC; realtime mode, allow_sw control           |
| NVIDIA   | NVENC        | H.264, HEVC, AV1; presets p1-p7, spatial-aq, lookahead |
| Linux    | VAAPI        | H.264, HEVC, VP9, AV1; quality 0-8                     |
| Intel    | QSV          | H.264, HEVC, VP9, AV1; presets, lookahead              |

### Configuration

```typescript
encoder.configure({
  codec: 'avc1.42001E',
  width: 1920,
  height: 1080,
  // Hardware acceleration preference
  hardwareAcceleration: 'prefer-hardware', // 'no-preference' | 'prefer-hardware' | 'prefer-software'
  // Latency mode affects encoder tuning
  latencyMode: 'realtime', // 'quality' | 'realtime'
})
```

- `latencyMode: 'realtime'` - Enables low-latency encoder options (smaller GOP, no B-frames, fast presets)
- `latencyMode: 'quality'` - Enables quality-focused options (larger GOP, B-frames, lookahead)

The encoder automatically applies optimal settings for each hardware encoder based on the latency mode.

## Limitations

### Scalable Video Coding (SVC)

All scalability modes (L1Tx, L2Tx, L3Tx, S2Tx, S3Tx, and variants) are accepted and populate `metadata.svc.temporalLayerId` when temporal layers >= 2.

The W3C WebCodecs spec only defines `temporalLayerId` in `SvcOutputMetadata` - there is no `spatialLayerId` field in the spec. See [W3C WebCodecs §6.7](https://w3c.github.io/webcodecs/#encoded-video-chunk-metadata).

Note: This implementation computes temporal layer IDs algorithmically from frame index per W3C spec. FFmpeg is not configured for actual SVC encoding, so base layer frames are not independently decodable.

### Error Handling

Synchronous errors (e.g., calling `encode()` on a closed encoder) throw native `DOMException` instances that pass `instanceof DOMException` checks per W3C spec:

```typescript
try {
  encoder.encode(frame) // on closed encoder
} catch (e) {
  console.log(e instanceof DOMException) // true
  console.log(e.name) // "InvalidStateError"
}
```

Asynchronous error callbacks receive standard `Error` objects with the DOMException name in the message:

```typescript
const encoder = new VideoEncoder({
  output: (chunk) => {},
  error: (e) => {
    console.log(e.message) // "EncodingError: ..."
  },
})
```

### VideoFrame Format Conversion

`VideoFrame.copyTo()` and `VideoFrame.allocationSize()` support format conversion per W3C WebCodecs spec:

```typescript
const frame = new VideoFrame(i420Data, {
  format: 'I420',
  codedWidth: 1920,
  codedHeight: 1080,
  timestamp: 0,
})

// Get allocation size for RGBA output
const rgbaSize = frame.allocationSize({ format: 'RGBA' })

// Copy with format conversion (I420 → RGBA)
const rgbaBuffer = new Uint8Array(rgbaSize)
const layout = await frame.copyTo(rgbaBuffer, { format: 'RGBA' })

frame.close()
```

**Supported conversions:**

| Source Format                | Target Format          | Status               |
| ---------------------------- | ---------------------- | -------------------- |
| I420, I422, I444, NV12, NV21 | RGBA, RGBX, BGRA, BGRX | ✅                   |
| RGBA, RGBX, BGRA, BGRX       | RGBA, RGBX, BGRA, BGRX | ✅                   |
| RGBA, RGBX, BGRA, BGRX       | I420, I422, I444, NV12 | ❌ NotSupportedError |

Per WPT `videoFrame-copyTo-rgb.any.js`, RGB-to-YUV conversion throws `NotSupportedError`.

Custom layouts with overflow-inducing values (e.g., `offset: 2³²-2`) throw `TypeError` via checked arithmetic. Rect alignment is validated against the source format during conversion.

### ImageDecoder Options

ImageDecoder supports all W3C spec options:

| Option                 | Status | Notes                                                                             |
| ---------------------- | ------ | --------------------------------------------------------------------------------- |
| `desiredWidth/Height`  | ✅     | Scales decoded frames to specified dimensions                                     |
| `preferAnimation`      | ✅     | When `false`, only decodes first frame for animated formats                       |
| `colorSpaceConversion` | ✅     | `"default"` extracts color space metadata, `"none"` ignores it (Chromium-aligned) |

**Note:** Per W3C spec, `desiredWidth` and `desiredHeight` must both be specified or both omitted.

### Platform-Specific Notes

- **ImageDecoder GIF animation**: FFmpeg may return only the first frame. Use `VideoDecoder` with GIF codec for full animation.

## Logging

This library uses Rust's `tracing` crate for structured logging. Enable logging via the `WEBCODECS_LOG` environment variable:

```bash
# Enable all logs at info level
WEBCODECS_LOG=info node your-app.js

# Enable FFmpeg logs at debug level
WEBCODECS_LOG=ffmpeg=debug node your-app.js

# Enable WebCodecs codec errors at warn, FFmpeg at info
WEBCODECS_LOG=webcodecs=warn,ffmpeg=info node your-app.js

# Enable trace-level logging for everything
WEBCODECS_LOG=trace node your-app.js
```

### Log Targets

| Target      | Description                                                            |
| ----------- | ---------------------------------------------------------------------- |
| `ffmpeg`    | FFmpeg internal logs (codec initialization, encoding/decoding details) |
| `webcodecs` | WebCodecs API logs (codec errors, state transitions)                   |

### FFmpeg Log Level Mapping

| FFmpeg Level | Tracing Level |
| ------------ | ------------- |
| ERROR/FATAL  | `error`       |
| WARNING      | `warn`        |
| INFO         | `info`        |
| VERBOSE      | `debug`       |
| DEBUG/TRACE  | `trace`       |

Without `WEBCODECS_LOG` set, all logs are silently discarded.

## API Reference

This package implements the [W3C WebCodecs API](https://w3c.github.io/webcodecs/). Key classes:

- `VideoEncoder` / `VideoDecoder` - Video encoding and decoding with EventTarget support
- `AudioEncoder` / `AudioDecoder` - Audio encoding and decoding with EventTarget support
- `VideoFrame` - Raw video frame data (supports buffer data, existing VideoFrame, or `@napi-rs/canvas` Canvas)
- `AudioData` - Raw audio sample data
- `EncodedVideoChunk` / `EncodedAudioChunk` - Encoded media data
- `ImageDecoder` - Static image decoding
- `VideoColorSpace` - Color space information
- `Mp4Demuxer` / `WebMDemuxer` / `MkvDemuxer` - Container demuxing with seeking
- `Mp4Muxer` / `WebMMuxer` / `MkvMuxer` - Container muxing with streaming support

All encoders and decoders implement the `EventTarget` interface with `addEventListener()`, `removeEventListener()`, and `dispatchEvent()`.

For full API documentation, see the [W3C WebCodecs specification](https://w3c.github.io/webcodecs/).

## Development

### Requirements

- Rust (latest stable)
- Node.js 18+
- pnpm

### Build

```bash
pnpm install
pnpm build
```

### Test

```bash
pnpm test
```

### Lint

```bash
pnpm lint
cargo clippy
```

## License

MIT
