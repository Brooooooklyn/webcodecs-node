# @napi-rs/webcodecs - Project Context

## Overview

WebCodecs API implementation for Node.js using FFmpeg, built with napi-rs (Rust → Node.js native addon). Provides W3C WebCodecs spec-compliant video/audio encoding/decoding with FFmpeg as the backend.

**Package:** `@napi-rs/webcodecs` | **Version:** `0.0.0` | **License:** MIT

## Project Status

**Status:** Feature-complete, production-ready

| Component           | Status      | Notes                                              |
| ------------------- | ----------- | -------------------------------------------------- |
| VideoEncoder        | ✅ Complete | H.264, H.265, VP8, VP9, AV1 + EventTarget          |
| VideoDecoder        | ✅ Complete | All codecs + AV1 drain + EventTarget               |
| AudioEncoder        | ✅ Complete | AAC, Opus, MP3, FLAC + EventTarget                 |
| AudioDecoder        | ✅ Complete | All codecs with resampling + EventTarget           |
| VideoFrame          | ✅ Complete | All pixel formats, async copyTo, format conversion |
| AudioData           | ✅ Complete | All sample formats                                 |
| ImageDecoder        | ✅ Complete | JPEG, PNG, WebP, GIF, BMP, AVIF                    |
| Threading           | ✅ Complete | Non-blocking Drop, proper lifecycle                |
| W3C Spec Compliance | ✅ Complete | All APIs aligned                                   |
| Type Definitions    | ✅ Complete | ~1,100 lines in index.d.ts                         |
| Test Coverage       | ✅ Complete | 917 tests (34 files), all passing                  |
| Hardware Encoding   | ✅ Complete | Zero-copy GPU path, auto-tuning                    |

**Remaining Work:** None - All core features complete.

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
│   ├── hwdevice.rs    # Hardware device management
│   └── hwframes.rs    # Hardware frame context (GPU frame pools)
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
│   └── error.rs            # Native DOMException helpers
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

## Logging

Structured logging via Rust's `tracing` crate, configured through the `WEBCODECS_LOG` environment variable.

### Environment Variable

```bash
# Format: WEBCODECS_LOG=<target>=<level>,<target>=<level>,...
WEBCODECS_LOG=info                          # All targets at info
WEBCODECS_LOG=ffmpeg=debug                  # FFmpeg logs at debug
WEBCODECS_LOG=webcodecs=warn,ffmpeg=info    # Multiple targets
WEBCODECS_LOG=trace                         # Everything at trace
```

### Log Targets

| Target      | Description              | Example Locations                                              |
| ----------- | ------------------------ | -------------------------------------------------------------- |
| `ffmpeg`    | FFmpeg internal messages | Codec init, encode/decode operations                           |
| `webcodecs` | WebCodecs API messages   | Codec errors (`video_encoder.rs:1714`, `audio_decoder.rs:660`) |

### FFmpeg Log Level Mapping

| FFmpeg Level | Tracing Level | Constant                  |
| ------------ | ------------- | ------------------------- |
| FATAL/ERROR  | `error`       | `log_level::ERROR` (16)   |
| WARNING      | `warn`        | `log_level::WARNING` (24) |
| INFO         | `info`        | `log_level::INFO` (32)    |
| VERBOSE      | `debug`       | `log_level::VERBOSE` (40) |
| DEBUG/TRACE  | `trace`       | `log_level::DEBUG` (48)   |

### Implementation

- **Callback**: `ffmpeg_log_callback()` in `src/lib.rs:29-95` intercepts FFmpeg logs
- **Filter**: `Targets::from_str(&env_var)` parses `WEBCODECS_LOG` (lib.rs:118)
- **Subscriber**: `tracing_subscriber::fmt::layer()` outputs formatted logs (lib.rs:121)

Without `WEBCODECS_LOG` set, all logs are silently discarded.

## FFmpeg Linking

**Static linking only** (enforced in build.rs). Build will panic if static libs not found.

Required static libraries:

- `libavcodec.a`, `libavutil.a`, `libswscale.a`, `libswresample.a`
- Codec libs: `libx264.a`, `libx265.a`, `libvpx.a`
- AV1 libs (platform-specific):
  - **Windows x64 MSVC:** `rav1e.lib` (encoder) + `dav1d.lib` (decoder)
  - **Other platforms:** `libaom.a` (encoder + decoder)

Detection order:

1. `FFMPEG_DIR` environment variable
2. pkg-config
3. Common paths (`/opt/homebrew`, `/usr/local`, etc.)
4. Bundled in `ffmpeg/{platform}/`
5. Auto-download from GitHub Releases (Linux/Windows CI builds)

## Implemented WebCodecs API

### Video

- **VideoFrame**: All pixel formats (I420, I420A, I422, I444, NV12, NV21, RGBA, RGBX, BGRA, BGRX)
  - RGBA/RGBX/BGRA/BGRX formats default to sRGB colorSpace (BT.709 primaries, IEC 61966-2-1 transfer)
- **EncodedVideoChunk**: Key/Delta types with copyTo()
- **VideoEncoder**: Callback-based API, bitrateMode, latencyMode, scalabilityMode, keyFrame forcing, EventTarget
- **VideoDecoder**: Full implementation with callback API, EventTarget interface
- **VideoColorSpace**: Class with constructor and toJSON()
- **DOMRectReadOnly**: For codedRect/visibleRect properties

### Audio

- **AudioData**: All sample formats (u8, s16, s32, f32, planar variants)
- **EncodedAudioChunk**: Key/Delta types with copyTo()
- **AudioEncoder**: Callback-based API, AAC/Opus/MP3/FLAC support, EventTarget interface
- **AudioDecoder**: Full implementation with EventTarget interface

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

**Detection API:**

- `getHardwareAccelerators()` - List all known accelerators
- `getAvailableHardwareAccelerators()` - List available on system
- `getPreferredHardwareAccelerator()` - Platform-preferred
- `isHardwareAcceleratorAvailable(name)` - Check specific

**Hardware Encoding (Zero-Copy GPU Path):**

- Automatic hardware encoder selection based on `hardwareAcceleration` config
- Zero-copy GPU frame upload via `HwFrameContext` (when supported)
- Automatic I420→NV12 conversion for hardware encoders
- Platform-specific encoder options auto-applied based on `latencyMode`:
  - **VideoToolbox (macOS):** `realtime` mode, `allow_sw` disabled
  - **NVENC (NVIDIA):** Presets p1-p7, tune hq/ll, spatial-aq, rc-lookahead
  - **VAAPI (Linux):** Quality 0-8 scale
  - **QSV (Intel):** Presets veryfast-veryslow, look-ahead
- Graceful fallback: GPU upload failure → CPU frames → software encoder

### Supported Codecs

- **Video**: H.264 (avc1), H.265 (hev1/hvc1), VP8, VP9 (vp09, vp9), AV1 (av01, av1)
- **Audio**: AAC (mp4a.40.2), Opus, MP3, FLAC, Vorbis, ALAC, PCM variants

**Note:** Short form codec strings `vp9` and `av01`/`av1` are accepted for compatibility with browser implementations, though W3C WPT considers them ambiguous.

## Key Files

| File                             | Purpose                                      |
| -------------------------------- | -------------------------------------------- |
| `src/webcodecs/video_encoder.rs` | VideoEncoder with callback API               |
| `src/webcodecs/video_decoder.rs` | VideoDecoder implementation                  |
| `src/webcodecs/audio_encoder.rs` | AudioEncoder with callback API               |
| `src/webcodecs/audio_decoder.rs` | AudioDecoder implementation                  |
| `src/webcodecs/video_frame.rs`   | VideoFrame with constructor overloading      |
| `src/webcodecs/audio_data.rs`    | AudioData with sync copyTo()                 |
| `src/webcodecs/error.rs`         | Native DOMException helpers                  |
| `src/webcodecs/codec_string.rs`  | Codec string parser (avc1, vp09, av01, hev1) |
| `src/codec/context.rs`           | FFmpeg encoder/decoder context wrapper       |
| `src/codec/hwframes.rs`          | HwFrameContext for GPU frame pools           |
| `src/codec/hwdevice.rs`          | HwDeviceContext for hardware devices         |
| `src/lib.rs`                     | Module init, FFmpeg→tracing log redirect     |
| `build.rs`                       | FFmpeg detection and static linking          |
| `index.d.ts`                     | TypeScript type definitions (~1,100 lines)   |

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
// From buffer data (VideoFrameBufferInit - format, codedWidth, codedHeight, timestamp required)
const frame = new VideoFrame(data, {
  format: 'I420',
  codedWidth: 1920,
  codedHeight: 1080,
  timestamp: 0,
})

// From existing VideoFrame (clone with optional overrides)
const cloned = new VideoFrame(sourceFrame, { timestamp: newTs })
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

- **34 test files** (~15,000+ lines)
- **864 tests** all passing (14 skipped)
- Test helpers in `__test__/helpers/` for frame/audio generation
- Integration tests for roundtrip, lifecycle, multi-codec, performance
- W3C WPT tests in `__test__/wpt/` for spec compliance verification
- Test fixtures in `__test__/fixtures/` for ImageDecoder and WPT (video/audio samples)

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

### AV1 libaom Issues

**Location:** Native code in libaom library
**Symptom:** Occasional segmentation fault during AV1 encoder/decoder cleanup (CVE-2025-8879)
**Workaround:**

- **Windows x64 MSVC:** Uses rav1e (encoder) + dav1d (decoder) instead of libaom
- **Other platforms:** AV1 encoder/decoder implementations drain all frames before dropping context
  **Status:** Fully resolved on Windows x64; mitigated with drain workaround elsewhere (`video_encoder.rs`, `video_decoder.rs`)

## Known Limitations

1. **Temporal SVC** - All modes with >= 2 temporal layers (L1Tx, L2Tx, L3Tx, SxTx) populate `metadata.svc.temporalLayerId`; W3C spec does not define `spatialLayerId`
2. **Duration type** - Using i64 instead of u64 due to NAPI-RS constraints
3. **ImageDecoder GIF animation** - FFmpeg may return only first frame; for full animation use VideoDecoder with GIF codec

## NAPI-RS Limitations

These are fundamental limitations that cannot be resolved without upstream NAPI-RS changes:

| Limitation                  | Impact                                    | Workaround                            |
| --------------------------- | ----------------------------------------- | ------------------------------------- |
| **Duration as i64 not u64** | Microsecond timestamps use signed integer | No practical impact for typical usage |

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

**9 build targets** with full CI coverage:

| Target                        | OS      | AV1 Encoder | AV1 Decoder |
| ----------------------------- | ------- | ----------- | ----------- |
| x86_64-apple-darwin           | macOS   | libaom      | libaom      |
| aarch64-apple-darwin          | macOS   | libaom      | libaom      |
| x86_64-pc-windows-msvc        | Windows | rav1e       | dav1d       |
| aarch64-pc-windows-msvc       | Windows | libaom      | libaom      |
| x86_64-unknown-linux-gnu      | Linux   | libaom      | libaom      |
| aarch64-unknown-linux-gnu     | Linux   | libaom      | libaom      |
| x86_64-unknown-linux-musl     | Linux   | libaom      | libaom      |
| aarch64-unknown-linux-musl    | Linux   | libaom      | libaom      |
| armv7-unknown-linux-gnueabihf | Linux   | libaom      | libaom      |

**Note:** Windows x64 MSVC uses rav1e + dav1d due to libaom crash issues (CVE-2025-8879).

## Spec Compliance Notes

| Feature                      | Spec                    | Implementation                |
| ---------------------------- | ----------------------- | ----------------------------- |
| VideoFrame.copyTo()          | Promise<PlaneLayout[]>  | ✅ Async                      |
| AudioData.copyTo()           | void (sync)             | ✅ Sync                       |
| AudioData.allocationSize()   | options required        | ✅ Required                   |
| Encoder callbacks            | (chunk, metadata?)      | ✅ Spread args                |
| AudioData constructor        | data in init            | ✅ Inside init                |
| Enum casing                  | lowercase               | ✅ "key", "unconfigured"      |
| VideoColorSpace.toJSON()     | toJSON() method         | ✅ Correct capitalization     |
| DOMRectReadOnly.toJSON()     | toJSON() method         | ✅ Correct capitalization     |
| VideoFrame.codedRect         | throws on closed        | ✅ InvalidStateError          |
| VideoFrame.visibleRect       | throws on closed        | ✅ InvalidStateError          |
| VideoFrame visibleRect param | cropping support        | ✅ Full W3C compliance        |
| VideoFrame.copyTo rect       | subregion copy          | ✅ Full W3C compliance        |
| VideoFrame.copyTo format     | format conversion       | ✅ YUV→RGB, RGB→RGB supported |
| VideoFrame layout overflow   | TypeError on overflow   | ✅ Checked arithmetic (u64)   |
| VideoFrame rect validation   | source format alignment | ✅ Validates against source   |
| ImageDecoder.closed          | readonly boolean        | ✅ Implemented                |
| ImageTrackList.ready         | Promise                 | ✅ Implemented                |
| ImageTrackList.item()        | indexed access          | ✅ Implemented                |
| ImageTrackList.selectedIndex | returns -1 if none      | ✅ Implemented                |
| ImageTrack.selected          | writable property       | ✅ Getter/setter              |
| SvcOutputMetadata            | temporalLayerId only    | ✅ All multi-temporal modes   |
| DOMException errors          | instanceof DOMException | ✅ Native DOMException        |
| VideoEncoder.encode options  | keyFrame forcing        | ✅ Forces I-frame             |
| EventTarget interface        | addEventListener, etc   | ✅ All codecs                 |
| RGBA colorSpace default      | sRGB for RGB formats    | ✅ BT.709/sRGB                |
| VP9 short form codec         | "vp9" accepted          | ✅ For compatibility          |
| AV1 short form codec         | "av01"/"av1" accepted   | ✅ For compatibility          |
