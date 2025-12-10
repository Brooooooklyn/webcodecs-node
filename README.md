# @napi-rs/webcodec

[![CI](https://github.com/Brooooooklyn/webcodec-node/actions/workflows/CI.yml/badge.svg)](https://github.com/Brooooooklyn/webcodec-node/actions/workflows/CI.yml)

WebCodecs API implementation for Node.js using FFmpeg, built with [NAPI-RS](https://napi.rs).

## Features

- **W3C WebCodecs API compliant** - Full implementation of the WebCodecs specification
- **Video encoding/decoding** - H.264, H.265, VP8, VP9, AV1
- **Audio encoding/decoding** - AAC, Opus, MP3, FLAC, Vorbis, PCM variants
- **Image decoding** - JPEG, PNG, WebP, GIF, BMP, AVIF
- **Hardware acceleration** - Zero-copy GPU encoding with VideoToolbox (macOS), NVENC (NVIDIA), VAAPI (Linux), QSV (Intel)
- **Cross-platform** - macOS, Windows, Linux (glibc/musl, x64/arm64/armv7)
- **Structured logging** - FFmpeg logs redirected to Rust `tracing` crate for easy integration

## Installation

```bash
npm install @napi-rs/webcodec
# or
pnpm add @napi-rs/webcodec
# or
yarn add @napi-rs/webcodec
```

## Quick Start

### Video Encoding

```typescript
import { VideoEncoder, VideoFrame } from '@napi-rs/webcodec'

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

await encoder.flush()
encoder.close()
```

### Video Decoding

```typescript
import { VideoDecoder, EncodedVideoChunk } from '@napi-rs/webcodec'

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
import { AudioEncoder, AudioData } from '@napi-rs/webcodec'

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
import { ImageDecoder } from '@napi-rs/webcodec'
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

## Supported Codecs

### Video

| Codec | Codec String       | Encoding | Decoding |
| ----- | ------------------ | -------- | -------- |
| H.264 | `avc1.*`           | ✅       | ✅       |
| H.265 | `hev1.*`, `hvc1.*` | ✅       | ✅       |
| VP8   | `vp8`              | ✅       | ✅       |
| VP9   | `vp09.*`           | ✅       | ✅       |
| AV1   | `av01.*`           | ✅       | ✅       |

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

| Format | MIME Type    | Decoding |
| ------ | ------------ | -------- |
| JPEG   | `image/jpeg` | ✅       |
| PNG    | `image/png`  | ✅       |
| WebP   | `image/webp` | ✅       |
| GIF    | `image/gif`  | ✅       |
| BMP    | `image/bmp`  | ✅       |
| AVIF   | `image/avif` | ✅       |

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
| **Passing** | 781   | 96.2%      |
| **Skipped** | 31    | 3.8%       |
| **Failing** | 0     | 0%         |

**Skipped tests** are due to:

- High bit-depth pixel formats (10-bit/12-bit) not mapped to FFmpeg
- Temporal SVC layer metadata extraction not implemented

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

### Not Implemented

| Feature                | Status | Notes                                                                |
| ---------------------- | ------ | -------------------------------------------------------------------- |
| High bit-depth formats | ❌     | `I420P10`, `I422P10`, `I444P10`, `I420P12`, etc.                     |
| VideoFrame orientation | ❌     | `rotation` and `flip` properties                                     |
| Temporal SVC metadata  | ❌     | `scalabilityMode` parsed but `metadata.svc` not populated            |
| ImageDecoder options   | ⚠️     | `colorSpaceConversion`, `desiredWidth/Height` parsed but not applied |

### Platform-Specific Notes

- **ImageDecoder GIF animation**: FFmpeg may return only the first frame. Use `VideoDecoder` with GIF codec for full animation.
- **VideoFrame cloning**: Use `VideoFrame.fromVideoFrame()` factory method (NAPI-RS doesn't support constructor overloading).

## Logging

FFmpeg internal logs are redirected to the Rust `tracing` crate with the `ffmpeg` target. By default, logs are silently discarded unless you configure a tracing subscriber.

To enable FFmpeg logging in your application, you can use any `tracing`-compatible subscriber. For Node.js applications, you can expose this through the native module or use environment-based configuration.

Log level mapping:
| FFmpeg Level | Tracing Level |
| ------------ | ------------- |
| ERROR/FATAL | `error` |
| WARNING | `warn` |
| INFO | `info` |
| VERBOSE | `debug` |
| DEBUG/TRACE | `trace` |

## API Reference

This package implements the [W3C WebCodecs API](https://w3c.github.io/webcodecs/). Key classes:

- `VideoEncoder` / `VideoDecoder` - Video encoding and decoding
- `AudioEncoder` / `AudioDecoder` - Audio encoding and decoding
- `VideoFrame` - Raw video frame data
- `AudioData` - Raw audio sample data
- `EncodedVideoChunk` / `EncodedAudioChunk` - Encoded media data
- `ImageDecoder` - Static image decoding
- `VideoColorSpace` - Color space information

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
