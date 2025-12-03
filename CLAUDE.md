# @napi-rs/webcodec - Project Context

## Overview

WebCodecs API implementation for Node.js using FFmpeg, built with napi-rs (Rust → Node.js native addon). Provides W3C WebCodecs spec-compliant video/audio encoding/decoding with FFmpeg as the backend.

**Package:** `@napi-rs/webcodec` | **Version:** `0.0.0` | **License:** MIT

## Project Status

**Status:** Feature-complete, production-ready

| Component           | Status      | Notes                               |
| ------------------- | ----------- | ----------------------------------- |
| VideoEncoder        | ✅ Complete | H.264, H.265, VP8, VP9, AV1         |
| VideoDecoder        | ✅ Complete | All codecs + AV1 drain workaround   |
| AudioEncoder        | ✅ Complete | AAC, Opus, MP3, FLAC                |
| AudioDecoder        | ✅ Complete | All codecs with resampling          |
| VideoFrame          | ✅ Complete | All pixel formats, async copyTo     |
| AudioData           | ✅ Complete | All sample formats                  |
| ImageDecoder        | ✅ Complete | JPEG, PNG, WebP, GIF, BMP, AVIF     |
| Threading           | ✅ Complete | Non-blocking Drop, proper lifecycle |
| W3C Spec Compliance | ✅ Complete | All APIs aligned                    |
| Type Definitions    | ✅ Complete | ~930 lines in index.d.ts            |
| Test Coverage       | ✅ Complete | 282 tests, all passing              |

**Remaining Work:**

- Hardware-accelerated encoding integration (detection works)
- VideoFrame.visibleRect cropping (low priority)
- Temporal SVC layer application (low priority)

## Architecture

Three-layer design with clean separation of concerns:

```
src/
├── ffi/           # Low-level FFmpeg FFI bindings (hand-written, no bindgen)
│   ├── types.rs       # AVCodecID, AVPixelFormat, AVRational, etc.
│   ├── accessors.rs/c # Thin C library for FFmpeg struct field access
│   ├── avcodec.rs     # Video codec functions
│   ├── avutil.rs      # Utility functions (logging, options)
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
│   ├── codec_string.rs     # Codec string parsing
│   └── error.rs            # DOMException-style errors
└── lib.rs         # Crate root, module init, re-exports

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

- **VideoFrame**: All pixel formats (I420, I420A, I422, I444, NV12, NV21, RGBA, RGBX, BGRA, BGRX)
- **EncodedVideoChunk**: Key/Delta types with copyTo()
- **VideoEncoder**: Callback-based API, bitrateMode, latencyMode, scalabilityMode
- **VideoDecoder**: Full implementation with callback API
- **VideoColorSpace**: Class with constructor and toJSON()
- **DOMRectReadOnly**: For codedRect/visibleRect properties

### Audio

- **AudioData**: All sample formats (u8, s16, s32, f32, planar variants)
- **EncodedAudioChunk**: Key/Delta types with copyTo()
- **AudioEncoder**: Callback-based API, AAC/Opus/MP3/FLAC support
- **AudioDecoder**: Full implementation

### Image

- **ImageDecoder**: JPEG, PNG, WebP, GIF, BMP, AVIF → VideoFrame
  - Frame caching for efficient multi-frame access
  - `frame_index` support for animated images (GIF/WebP)
  - `frame_count` populated after first decode
  - ReadableStream and Uint8Array data sources
  - `closed` property (W3C spec compliant)
- **ImageTrackList**: W3C spec compliant track list
  - `ready` Promise property
  - `item(index)` method for indexed access
  - `selectedIndex` returns -1 when no track selected
- **ImageTrack**: W3C spec compliant track with writable `selected` property

### Hardware Acceleration

- `getHardwareAccelerators()` - List all known accelerators
- `getAvailableHardwareAccelerators()` - List available on system
- `getPreferredHardwareAccelerator()` - Platform-preferred
- `isHardwareAcceleratorAvailable(name)` - Check specific

### Supported Codecs

- **Video**: H.264 (avc1), H.265 (hev1/hvc1), VP8, VP9 (vp09), AV1 (av01)
- **Audio**: AAC (mp4a.40.2), Opus, MP3, FLAC, Vorbis, ALAC, PCM variants

## Key Files

| File                             | Purpose                                      |
| -------------------------------- | -------------------------------------------- |
| `src/webcodecs/video_encoder.rs` | VideoEncoder with callback API               |
| `src/webcodecs/video_decoder.rs` | VideoDecoder implementation                  |
| `src/webcodecs/audio_encoder.rs` | AudioEncoder with callback API               |
| `src/webcodecs/audio_decoder.rs` | AudioDecoder implementation                  |
| `src/webcodecs/video_frame.rs`   | VideoFrame with fromVideoFrame() factory     |
| `src/webcodecs/audio_data.rs`    | AudioData with sync copyTo()                 |
| `src/webcodecs/error.rs`         | DOMException-style error helpers             |
| `src/webcodecs/codec_string.rs`  | Codec string parser (avc1, vp09, av01, hev1) |
| `src/codec/context.rs`           | FFmpeg encoder/decoder context wrapper       |
| `src/lib.rs`                     | Module init with FFmpeg log suppression      |
| `build.rs`                       | FFmpeg detection and static linking          |
| `index.d.ts`                     | TypeScript type definitions (~1000 lines)    |

## Callback API Pattern (W3C Compliant)

All encoders/decoders use W3C-compliant init dictionary constructors:

```typescript
// VideoEncoder - init dictionary with output and error callbacks
const encoder = new VideoEncoder({
  output: (chunk: EncodedVideoChunk, metadata?: EncodedVideoChunkMetadata) => {
    // chunk is actual EncodedVideoChunk instance
  },
  error: (error: Error) => {
    /* error callback */
  },
})

// AudioEncoder - same pattern
const audioEncoder = new AudioEncoder({
  output: (chunk: EncodedAudioChunk, metadata?: EncodedAudioChunkMetadata) => {
    // chunk is actual EncodedAudioChunk instance
  },
  error: (error: Error) => {
    /* error callback */
  },
})

// VideoDecoder
const decoder = new VideoDecoder({
  output: (frame: VideoFrame) => {
    /* handle decoded frame */
  },
  error: (error: Error) => {
    /* error callback */
  },
})

// AudioDecoder
const audioDecoder = new AudioDecoder({
  output: (data: AudioData) => {
    /* handle decoded audio */
  },
  error: (error: Error) => {
    /* error callback */
  },
})
```

## AudioData Constructor (W3C Compliant)

Data is INSIDE the init object per spec:

```typescript
const audioData = new AudioData({
  data: new Uint8Array(samples), // data inside init
  format: 'f32-planar',
  sampleRate: 48000,
  numberOfFrames: 1024,
  numberOfChannels: 2,
  timestamp: 0,
})
```

## VideoFrame Constructors

```typescript
// Buffer-based constructor
const frame = new VideoFrame(data, {
  format: 'I420',
  codedWidth: 1920,
  codedHeight: 1080,
  timestamp: 0,
})

// Clone from existing frame (factory method)
const cloned = VideoFrame.fromVideoFrame(sourceFrame, { timestamp: newTs })
```

## ImageDecoder Usage

```typescript
// Decode static image (PNG, JPEG, BMP)
const decoder = new ImageDecoder({
  data: imageBytes, // Uint8Array or ReadableStream
  type: 'image/png',
})

const result = await decoder.decode()
const frame = result.image // VideoFrame
console.log(frame.codedWidth, frame.codedHeight)
frame.close()
decoder.close()

// Decode specific frame from animated GIF
const gifDecoder = new ImageDecoder({
  data: gifBytes,
  type: 'image/gif',
})

// First decode populates frame_count
await gifDecoder.decode({ frameIndex: 0 })
const frameCount = gifDecoder.tracks.selectedTrack.frameCount

// Access any frame by index (uses cache)
const frame2 = await gifDecoder.decode({ frameIndex: 1 })
frame2.image.close()

// Reset clears cache (re-decode on next call)
gifDecoder.reset()
gifDecoder.close()
```

## Test Structure

- **16 test files** (~5600+ lines)
- **282 tests** all passing
- Test helpers in `__test__/helpers/` for frame/audio generation
- Integration tests for roundtrip, lifecycle, multi-codec, performance
- Test fixtures in `__test__/fixtures/` for ImageDecoder (PNG, GIF)

## Encoder/Decoder Threading Architecture

All encoders and decoders use a **crossbeam channel-based worker thread pattern** for non-blocking FFmpeg operations:

### Components Using Worker Thread Pattern

| Component    | Worker Commands | Notes                                            |
| ------------ | --------------- | ------------------------------------------------ |
| VideoDecoder | Decode, Flush   | EncodedVideoChunkInner uses Arc<RwLock<...>>     |
| VideoEncoder | Encode, Flush   | Frame cloned on main thread, encoded on worker   |
| AudioEncoder | Encode, Flush   | Resampling on main thread, encoding on worker    |
| AudioDecoder | Decode, Flush   | Data extracted on main thread, decoded on worker |

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ Encoder/Decoder                                                 │
├─────────────────────────────────────────────────────────────────┤
│ inner: Arc<Mutex<Inner>>              ← shared state            │
│ command_sender: Option<Sender<Command>>                         │
│ worker_handle: Option<JoinHandle<()>>                           │
└─────────────────────────────────────────────────────────────────┘
         │                                    ▲
         │ Command::Encode/Decode             │ output callback
         │ Command::Flush                     │ (ThreadsafeFunction)
         ▼                                    │
┌─────────────────────────────────────────────────────────────────┐
│ Worker Thread (worker_loop)                                     │
├─────────────────────────────────────────────────────────────────┤
│ - Receives commands via crossbeam::channel                      │
│ - Processes encode/decode/flush sequentially                    │
│ - Holds mutex during FFmpeg operations                          │
│ - Exits when channel disconnected (sender dropped)              │
└─────────────────────────────────────────────────────────────────┘
```

### Critical Implementation Details

```rust
// In reset(): MUST drop sender BEFORE joining worker thread
drop(self.command_sender.take());  // Signal worker to stop
if let Some(handle) = self.worker_handle.take() {
    let _ = handle.join();  // Now safe to join - channel disconnected
}
```

**Why?** If sender isn't dropped before join, channel stays connected and worker blocks on `recv()` forever → deadlock.

### Thread Safety

- `encode()/decode()`: Increments queue size under lock, sends command (non-blocking)
- `flush()`: Sends Flush command with response channel, waits via `spawn_blocking`
- Worker: Holds mutex during FFmpeg operations, uses `saturating_sub` for queue decrement
- AV1 special case: Drains encoder/decoder before context drop (libaom thread safety)

### Drop Behavior (Non-Blocking)

```rust
impl Drop for VideoEncoder {
    fn drop(&mut self) {
        self.command_sender = None;  // Signal worker to stop
        // Don't join - let thread become detached
    }
}
```

**Why no join in Drop?**

- `Drop` is called during JavaScript garbage collection on the main thread
- Joining would block the Node.js event loop until FFmpeg finishes current operation
- FFmpeg operations can take tens to hundreds of milliseconds → causes app freezes

**Why this is safe:**

- `Arc<Mutex<Inner>>` reference counting keeps inner state alive
- Worker holds a clone of the Arc, so Inner won't be freed until worker exits
- Worker sees channel disconnect → `recv()` returns `Err` → exits loop → drops its Arc
- ThreadsafeFunction callbacks remain valid while worker holds Arc reference

**Explicit close() still joins** - for users who need synchronous cleanup:

```rust
pub fn close(&mut self) {
    self.command_sender = None;
    if let Some(handle) = self.worker_handle.take() {
        let _ = handle.join();  // Blocking is acceptable here - explicit user action
    }
}
```

### VideoFrame.copyTo

Uses `spawn_blocking` to offload frame data copy to a blocking thread, preventing main thread blocking for large frames (4K+).

### Pending TODOs

```
src/codec/context.rs:339,362  # Set extradata if provided (non-critical)
```

## Known Issues

### AV1 SIGSEGV (libaom)

**Location:** Native code in libaom library
**Symptom:** Occasional segmentation fault during AV1 encoder/decoder cleanup
**Workaround:** AV1 encoder/decoder implementations drain all frames before dropping context
**Status:** Mitigated with drain workaround in `video_encoder.rs:755-759` and `video_decoder.rs:600-604`

## Known Limitations

1. **VideoFrame.visibleRect cropping** - Parameter not implemented, returns error
2. **Temporal SVC** - Parsing only, layer settings not applied to encoder
3. **Hardware-accelerated encoding** - Detection works, integration pending
4. **Duration type** - Using i64 instead of u64 due to NAPI-RS constraints
5. **ImageDecoder parameters** - `colorSpaceConversion`, `desiredWidth/Height`, `preferAnimation` parsed but not applied
6. **ImageDecoder GIF animation** - FFmpeg may return only first frame; for full animation use VideoDecoder with GIF codec
7. **ImageDecoder ReadableStream blocking** - Constructor uses `rt.block_on()` for ReadableStream data collection, which blocks the Node.js event loop during initialization. **Workaround:** Use Uint8Array data source instead of ReadableStream for large images. Location: `src/webcodecs/image_decoder.rs` lines 86-107

## NAPI-RS Limitations

These are fundamental limitations that cannot be resolved without upstream NAPI-RS changes:

| Limitation                     | Impact                                             | Workaround                                                                                 |
| ------------------------------ | -------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| **No constructor overloading** | VideoFrame cannot have two constructor signatures  | Use `VideoFrame.fromVideoFrame()` factory for cloning                                      |
| **Duration as i64 not u64**    | Microsecond timestamps use signed integer          | No practical impact for typical usage                                                      |
| **No native DOMException**     | Errors use standard `Error` with formatted message | Error message includes DOMException name (e.g., "InvalidStateError: VideoFrame is closed") |

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
  sampleRate: number,      // REQUIRED
  numberOfChannels: number, // REQUIRED
  bitrate?: number,
}
```

## Platform Support

- macOS (x86_64, aarch64)
- Linux (x86_64, aarch64)
- Windows (x86_64, aarch64)

## Spec Compliance Notes

| Feature                      | Spec                   | Implementation            |
| ---------------------------- | ---------------------- | ------------------------- |
| VideoFrame.copyTo()          | Promise<PlaneLayout[]> | ✅ Async                  |
| AudioData.copyTo()           | void (sync)            | ✅ Sync                   |
| AudioData.allocationSize()   | options required       | ✅ Required               |
| Encoder callbacks            | (chunk, metadata?)     | ✅ Spread args            |
| AudioData constructor        | data in init           | ✅ Inside init            |
| Enum casing                  | lowercase              | ✅ "key", "unconfigured"  |
| VideoColorSpace.toJSON()     | toJSON() method        | ✅ Correct capitalization |
| DOMRectReadOnly.toJSON()     | toJSON() method        | ✅ Correct capitalization |
| VideoFrame.codedRect         | throws on closed       | ✅ InvalidStateError      |
| VideoFrame.visibleRect       | throws on closed       | ✅ InvalidStateError      |
| ImageDecoder.closed          | readonly boolean       | ✅ Implemented            |
| ImageTrackList.ready         | Promise                | ✅ Implemented            |
| ImageTrackList.item()        | indexed access         | ✅ Implemented            |
| ImageTrackList.selectedIndex | returns -1 if none     | ✅ Implemented            |
| ImageTrack.selected          | writable property      | ✅ Getter/setter          |
