# @napi-rs/webcodec - Project Context

## Overview

WebCodecs API implementation for Node.js using FFmpeg, built with napi-rs (Rust → Node.js native addon).

## Architecture

```
src/
├── ffi/           # Low-level FFmpeg FFI bindings (avcodec, avutil, swscale, swresample, hwaccel)
├── codec/         # Mid-level codec abstractions (context, frame, packet, scaler, resampler)
├── webcodecs/     # High-level WebCodecs API classes
└── lib.rs         # Crate root, re-exports

lib/               # JavaScript wrappers (async iterators, Transform streams)
__test__/          # Test suite (ava)
```

## Build Commands

```bash
npm run build        # Release build
npm run build:debug  # Debug build
npm test             # Run tests (ava)
cargo clippy         # Lint Rust code
```

## Implemented WebCodecs API

### Video
- **VideoFrame**: All pixel formats (I420, I420A, I422, I444, NV12, RGBA, RGBX, BGRA, BGRX)
- **EncodedVideoChunk**: Key/Delta types
- **VideoEncoder**: Queue mode + callback mode, bitrateMode, latencyMode, scalabilityMode, codec-specific configs
- **VideoDecoder**: Full implementation

### Audio
- **AudioData**: All sample formats (U8, S16, S32, F32, planar variants)
- **EncodedAudioChunk**: Key/Delta types
- **AudioEncoder**: AAC/Opus with codec-specific options
- **AudioDecoder**: Full implementation

### Hardware Acceleration
- Detection utilities: `getHardwareAccelerators()`, `getAvailableHardwareAccelerators()`, etc.

### JavaScript Wrappers (lib/)
- Async iterator classes: `VideoEncoderAsync`, `VideoDecoderAsync`, `AudioEncoderAsync`, `AudioDecoderAsync`
- Transform streams: `VideoEncoderStream`, `VideoDecoderStream`, `AudioEncoderStream`, `AudioDecoderStream`

## Key Files

| File | Purpose |
|------|---------|
| `src/webcodecs/video_encoder.rs` | VideoEncoder with callback API, bitrateMode, latencyMode |
| `src/webcodecs/video_decoder.rs` | VideoDecoder implementation |
| `src/webcodecs/video_frame.rs` | VideoFrame with format conversion |
| `src/webcodecs/audio_encoder.rs` | AudioEncoder with AAC/Opus support |
| `src/webcodecs/codec_string.rs` | Codec string parser (avc1, vp09, av01, hev1) |
| `src/codec/context.rs` | FFmpeg encoder/decoder context wrapper |
| `lib/index.ts` | Async iterator wrappers |
| `lib/streams.ts` | Node.js Transform stream wrappers |

## Test Structure

- 244 tests across unit, integration, and performance categories
- Test helpers in `__test__/helpers/` for frame/audio generation

## Configuration Options

### VideoEncoderConfig
```typescript
{
  codec: string,           // "avc1.42001E", "vp09.00.10.08", "av01.0.04M.08", "hev1.1.6.L93.B0"
  width: number,
  height: number,
  bitrate?: number,
  framerate?: number,
  bitrateMode?: "constant" | "variable" | "quantizer",
  latencyMode?: "quality" | "realtime",
  scalabilityMode?: "L1T1" | "L1T2" | "L1T3",
  avc?: AvcEncoderConfig,
  vp9?: Vp9EncoderConfig,
  av1?: Av1EncoderConfig,
  hevc?: HevcEncoderConfig,
}
```

## Known Limitations / Future Work

1. **VideoFrame.copyTo() with rect** - Partial (rect parameter not implemented)
2. **Temporal SVC** - Parsing only, actual layer settings not applied
3. **Hardware-accelerated encoding** - Detection works, integration pending
4. **ImageDecoder** - Not implemented (static image decoding)
5. **Callback mode for Audio** - Only VideoEncoder has callback support

## Code TODOs

```
src/webcodecs/video_encoder.rs:238  # Apply temporal layer settings
src/codec/context.rs:309,332        # Set extradata for decoder config
```
