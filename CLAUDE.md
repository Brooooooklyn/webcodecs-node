# @napi-rs/webcodec - Project Context

## Overview

WebCodecs API implementation for Node.js using FFmpeg, built with napi-rs (Rust → Node.js native addon). Provides W3C WebCodecs spec-compliant video/audio encoding/decoding with FFmpeg as the backend.

**Package:** `@napi-rs/webcodec` | **Version:** `0.0.0` | **License:** MIT

## Architecture

Three-layer design with clean separation of concerns:

```
src/
├── ffi/           # Low-level FFmpeg FFI bindings (hand-written, no bindgen)
│   ├── types.rs       # AVCodecID, AVPixelFormat, AVRational, etc.
│   ├── accessors.rs/c # Thin C library for FFmpeg struct field access
│   ├── avcodec.rs     # Video codec functions
│   ├── avutil.rs      # Utility functions
│   ├── swscale.rs     # Video scaling/format conversion
│   ├── swresample.rs  # Audio resampling
│   └── hwaccel.rs     # Hardware acceleration
├── codec/         # Mid-level RAII wrappers around FFmpeg
│   ├── context.rs     # CodecContext (encoder/decoder)
│   ├── frame.rs       # Frame handling
│   ├── packet.rs      # Packet handling
│   ├── audio_buffer.rs# Audio sample buffers
│   ├── scaler.rs      # Video scaling
│   ├── resampler.rs   # Audio resampling
│   └── hwdevice.rs    # Hardware device management
├── webcodecs/     # High-level WebCodecs API classes (NAPI)
│   ├── video_encoder.rs    # VideoEncoder class
│   ├── video_decoder.rs    # VideoDecoder class
│   ├── audio_encoder.rs    # AudioEncoder class
│   ├── audio_decoder.rs    # AudioDecoder class
│   ├── video_frame.rs      # VideoFrame class
│   ├── audio_data.rs       # AudioData class
│   ├── encoded_video_chunk.rs
│   ├── encoded_audio_chunk.rs
│   ├── image_decoder.rs    # ImageDecoder class
│   └── codec_string.rs     # Codec string parsing
└── lib.rs         # Crate root, re-exports

__test__/          # Test suite (ava)
├── helpers/       # Test utilities (frame/audio generators, codec matrix)
└── integration/   # Integration tests (roundtrip, lifecycle, performance)
```

## Build Commands

```bash
pnpm build         # Release build
pnpm build:debug   # Debug build
pnpm test          # Run tests (ava, 2 min timeout)
pnpm bench         # Run benchmarks
pnpm typecheck     # TypeScript type checking
pnpm lint          # Run oxlint
pnpm format        # Format code (prettier, rustfmt, taplo)
cargo clippy       # Lint Rust code
```

## FFmpeg Linking

**Static linking only** (enforced in build.rs). Build will panic if static libs not found.

Required static libraries:
- `libavcodec.a`, `libavutil.a`, `libswscale.a`, `libswresample.a`
- Codec libs: `libx264.a`, `libx265.a`, `libvpx.a`, `libaom.a`

Detection order:
1. `FFMPEG_DIR` environment variable
2. pkg-config
3. Common paths (`/opt/homebrew`, `/usr/local`, etc.)
4. Bundled in `ffmpeg/{platform}/`

## Implemented WebCodecs API

### Video
- **VideoFrame**: All pixel formats (I420, I420A, I422, I444, NV12, RGBA, RGBX, BGRA, BGRX)
- **EncodedVideoChunk**: Key/Delta types
- **VideoEncoder**: Callback-based API, bitrateMode, latencyMode, scalabilityMode
- **VideoDecoder**: Full implementation with callback API

### Audio
- **AudioData**: All sample formats (U8, S16, S32, F32, planar variants)
- **EncodedAudioChunk**: Key/Delta types
- **AudioEncoder**: Callback-based API, AAC/Opus/MP3/FLAC support
- **AudioDecoder**: Full implementation

### Image
- **ImageDecoder**: JPEG, PNG, WebP, GIF, BMP → VideoFrame

### Hardware Acceleration
- `getHardwareAccelerators()` - List all known accelerators
- `getAvailableHardwareAccelerators()` - List available on system
- `getPreferredHardwareAccelerator()` - Platform-preferred
- `isHardwareAcceleratorAvailable(name)` - Check specific

### Supported Codecs
- **Video**: H.264 (avc1), H.265 (hev1/hvc1), VP8, VP9 (vp09), AV1 (av01), ProRes
- **Audio**: AAC (mp4a.40.2), Opus, MP3, FLAC, Vorbis, ALAC, PCM variants

## Key Files

| File | Purpose |
|------|---------|
| `src/webcodecs/video_encoder.rs` | VideoEncoder with callback API |
| `src/webcodecs/video_decoder.rs` | VideoDecoder implementation |
| `src/webcodecs/audio_encoder.rs` | AudioEncoder with callback API |
| `src/webcodecs/audio_decoder.rs` | AudioDecoder implementation |
| `src/webcodecs/codec_string.rs` | Codec string parser (avc1, vp09, av01, hev1) |
| `src/codec/context.rs` | FFmpeg encoder/decoder context wrapper |
| `build.rs` | FFmpeg detection and static linking |
| `index.d.ts` | TypeScript type definitions (~900 lines) |

## Callback API Pattern

All encoders/decoders use W3C-compliant callback-based constructors:

```typescript
// VideoEncoder callback receives tuple: [EncodedVideoChunkOutput, metadata]
const encoder = new VideoEncoder(
  (result: [EncodedVideoChunkOutput, EncodedVideoChunkMetadata]) => {
    const [chunk, metadata] = result;
    // chunk has: type, timestamp, duration, data
  },
  (error: Error) => { /* error callback */ }
);

// AudioEncoder callback receives tuple: (EncodedAudioChunk, metadata)
const audioEncoder = new AudioEncoder(
  (chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata) => {
    // Note: callback signature differs from VideoEncoder
  },
  (error: Error) => { /* error callback */ }
);
```

## Test Structure

- **15 test files** (~3000+ lines)
- Test helpers in `__test__/helpers/` for frame/audio generation
- Integration tests for roundtrip, lifecycle, multi-codec, performance

## Known Issues

### Test Failure: `AudioEncoder: encode() single frame`
**Location:** `__test__/audio-encoder.spec.ts:170`
**Issue:** Test callback handler doesn't properly destructure the tuple result
**Expected:** `chunk.type === 'Key'`
**Actual:** `chunk.type === undefined`
**Root cause:** AudioEncoder callback passes `(chunk, metadata)` but test treats first arg as chunk directly

### Pending TODOs

```
src/codec/context.rs:317,340  # Set extradata if provided
```

## Known Limitations

1. **VideoFrame.copyTo() with rect** - rect parameter not implemented
2. **Temporal SVC** - Parsing only, layer settings not applied to encoder
3. **Hardware-accelerated encoding** - Detection works, integration pending
4. **Codec extradata** - Not fully implemented in encoder context setup

## Configuration Options

### VideoEncoderConfig
```typescript
{
  codec: string,           // "avc1.42001E", "vp09.00.10.08", "av01.0.04M.08"
  width: number,
  height: number,
  bitrate?: number,
  framerate?: number,
  bitrateMode?: "constant" | "variable" | "quantizer",
  latencyMode?: "quality" | "realtime",
  scalabilityMode?: "L1T1" | "L1T2" | "L1T3",
  hardwareAcceleration?: "no-preference" | "prefer-hardware" | "prefer-software",
}
```

### AudioEncoderConfig
```typescript
{
  codec: string,           // "opus", "mp4a.40.2", "mp3", "flac"
  sampleRate?: number,
  numberOfChannels?: number,
  bitrate?: number,
  // Opus-specific options
  complexity?: number,
  opus_application?: "voip" | "audio" | "lowdelay",
}
```

## Platform Support

- macOS (x86_64, aarch64)
- Linux (x86_64, aarch64)
- Windows (x86_64, aarch64)

## Feature Flags

- `hwaccel` - Hardware acceleration support
- `ffmpeg_5_1` - FFmpeg 5.1+ API (AVChannelLayout)
