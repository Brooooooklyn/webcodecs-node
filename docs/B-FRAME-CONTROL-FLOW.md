# B-Frame Encoding/Decoding Control Flow

This document describes the complete control flow for B-frame handling in `@napi-rs/webcodecs`, including interactions with FFmpeg and compatibility with Chromium.

## Table of Contents

1. [Overview](#overview)
2. [Encoder B-Frame Handling](#encoder-b-frame-handling)
3. [EncodedVideoChunk Timestamp Storage](#encodedvideochunk-timestamp-storage)
4. [Muxer B-Frame Handling](#muxer-b-frame-handling)
5. [Decoder B-Frame Handling](#decoder-b-frame-handling)
6. [FFmpeg Internals](#ffmpeg-internals)
7. [Chromium Compatibility](#chromium-compatibility)
8. [Timestamp Flow Diagrams](#timestamp-flow-diagrams)

---

## Overview

### What are B-Frames?

B-frames (Bidirectional Predictive frames) are compressed video frames that reference both past and future frames for prediction. They provide better compression but introduce complexity:

- **Decode Order (DTS)**: The order frames must be decoded (need reference frames first)
- **Display Order (PTS)**: The order frames should be displayed to the viewer

For a typical GOP with B-frames:

```
Display Order (PTS):  I(0) B(1) B(2) P(3) B(4) B(5) P(6)
Decode Order (DTS):   I(0) P(3) B(1) B(2) P(6) B(4) B(5)
```

### Key Constraint

**FFmpeg requires `PTS >= DTS` at the API level.** This is enforced because a frame cannot be displayed before it's decoded. However, B-frames naturally have `PTS < DTS` in terms of relative ordering, which requires special handling.

---

## Encoder B-Frame Handling

### Configuration

**File:** `src/webcodecs/video_encoder.rs`

#### latencyMode Effect on B-Frames

```rust
// video_encoder.rs:458-463
fn get_default_gop_settings(realtime: bool) -> (Option<u32>, Option<u32>) {
  if realtime {
    (Some(10), Some(0)) // Low latency: small GOP, NO B-frames
  } else {
    (None, None) // Quality mode: let encoder use defaults (with B-frames)
  }
}
```

| latencyMode           | max_b_frames    | Effect                            |
| --------------------- | --------------- | --------------------------------- |
| `"realtime"`          | 0               | B-frames disabled for low latency |
| `"quality"` (default) | Encoder default | B-frames enabled (typically 2-4)  |

#### Encoder-Specific Defaults

| Encoder      | Default max_b_frames | Notes               |
| ------------ | -------------------- | ------------------- |
| libx264      | 3                    | Software H.264      |
| libx265      | 4                    | Software HEVC       |
| VideoToolbox | 0 (overridden to 2)  | Hardware, see below |
| NVENC        | Hardware default     | NVIDIA GPU          |

#### VideoToolbox Special Case

**File:** `src/codec/context.rs:479-485`

```rust
// VideoToolbox defaults to 0 B-frames, so we explicitly enable them in quality mode
if !realtime {
  ffctx_set_max_b_frames(self.ptr.as_ptr(), 2);
}
```

### Timestamp Queue Mechanism

**File:** `src/webcodecs/video_encoder.rs`

The encoder maintains a timestamp queue to correlate input frame timestamps with output packets, since FFmpeg may buffer and reorder frames:

```rust
// video_encoder.rs:354-356
struct VideoEncoderInner {
  /// Queue of input timestamps for correlation with output packets
  timestamp_queue: std::collections::VecDeque<i64>,
}
```

**Flow:**

1. **On encode** (`video_encoder.rs:885`):

   ```rust
   guard.timestamp_queue.push_back(timestamp);
   ```

2. **On packet output** (`video_encoder.rs:1246`):

   ```rust
   let output_timestamp = guard.timestamp_queue.pop_front();
   let chunk = EncodedVideoChunk::from_packet_with_format(
     packet,
     Some(buffered_ts),  // Original input timestamp
     ...
   );
   ```

3. **On reset/reconfigure** (`video_encoder.rs:1615, 1723, 3065`):
   ```rust
   guard.timestamp_queue.clear();
   ```

### Silent Failure Detection

**File:** `src/webcodecs/video_encoder.rs:272-277`

```rust
/// Threshold for detecting silent encoder failure (no output after N frames)
/// This value should be larger than max_b_frames + 1 because B-frame encoders
/// need to buffer frames before producing output (not a real failure).
/// For HEVC with B-pyramid (has_b_frames=2), we need at least 4 frames.
const SILENT_FAILURE_THRESHOLD: u32 = 5;
```

---

## EncodedVideoChunk Timestamp Storage

### Internal Structure

**File:** `src/webcodecs/encoded_video_chunk.rs:211-225`

```rust
pub(crate) struct EncodedVideoChunkInner {
  pub(crate) data: Either<Vec<u8>, Packet>,
  pub(crate) chunk_type: EncodedVideoChunkType,
  pub(crate) timestamp_us: i64,        // Public WebCodecs timestamp
  pub(crate) duration_us: Option<i64>,

  // B-frame support fields (internal only, not exposed to JS API):
  pub(crate) dts_us: Option<i64>,       // Decode timestamp from encoder
  pub(crate) original_pts: Option<i64>, // Presentation timestamp from encoder
}
```

### Population from Encoder Packet

**File:** `src/webcodecs/encoded_video_chunk.rs:276-345`

```rust
pub fn from_packet_with_format(
  packet: Packet,
  explicit_timestamp: Option<i64>,  // Original input frame timestamp
  ...
) -> Self {
  // Extract DTS from encoder packet (in microseconds)
  let dts_us = if packet_dts != AV_NOPTS_VALUE {
    Some(unsafe { av_rescale_q(packet_dts, encoder_time_base, dst_tb) })
  } else {
    None
  };

  // Extract original PTS from encoder packet (in microseconds)
  let original_pts = if packet_pts != AV_NOPTS_VALUE {
    Some(unsafe { av_rescale_q(packet_pts, encoder_time_base, dst_tb) })
  } else {
    None
  };

  // Public timestamp uses explicit (input) timestamp, falls back to original_pts
  let timestamp_us = explicit_timestamp.or(original_pts).unwrap_or(0);
}
```

### JavaScript API vs Internal Creation

| Source                         | `dts_us`    | `original_pts` | Notes                   |
| ------------------------------ | ----------- | -------------- | ----------------------- |
| `new EncodedVideoChunk({...})` | `None`      | `None`         | No B-frame info from JS |
| `from_packet_with_format()`    | Encoder DTS | Encoder PTS    | Full B-frame support    |

---

## Muxer B-Frame Handling

### B-Frame Detection

**File:** `src/webcodecs/muxer_base.rs:547-548`

```rust
// B-frames detected when chunk has both original_pts and dts
let has_b_frames = chunk_original_pts.is_some() && chunk_dts.is_some();
```

### Container-Specific Handling

**File:** `src/webcodecs/muxer_base.rs:620-691`

#### MP4 Container

```rust
if F::FORMAT == ContainerFormat::Mp4 {
  // Apply DTS shift to ensure pts >= dts (required by FFmpeg)
  let mut shifted_dts = dts + self.video_dts_shift;

  // Ensure DTS is monotonically increasing
  let min_dts = self.last_video_dts + 1;
  if shifted_dts < min_dts {
    shifted_dts = min_dts;
  }

  // If pts < shifted_dts, increase global shift for future packets
  if pts < shifted_dts {
    let needed_shift = shifted_dts - pts;
    self.video_dts_shift -= needed_shift;
    shifted_dts = pts; // For this packet, DTS = PTS
  }

  // Final monotonicity check with PTS adjustment if needed
  if shifted_dts <= self.last_video_dts {
    shifted_dts = self.last_video_dts + 1;
    if pts < shifted_dts {
      packet.set_pts(shifted_dts); // Adjust PTS to maintain pts >= dts
    }
  }

  self.last_video_dts = shifted_dts;
  packet.set_dts(shifted_dts);
}
```

#### MKV/WebM Container

```rust
else if has_b_frames {
  // MKV/WebM requires monotonic DTS AND PTS >= DTS
  // Solution: Use sequential timestamps, losing B-frame display order
  let frame_idx = self.video_frame_count - 1;
  let sequential_ts = (frame_idx as i64) * (ticks_per_frame as i64);

  packet.set_pts(sequential_ts);
  packet.set_dts(sequential_ts);  // PTS == DTS
}
```

### FFmpeg movflags for B-Frame Support

**File:** `src/codec/muxer.rs:338-365`

```rust
// negative_cts_offsets: Use CTTS version 1 with signed composition offsets.
// This allows proper B-frame timing without destroying PTS/DTS relationship.
let movflags = if opts.fragmented {
  "frag_keyframe+empty_moov+default_base_moof+negative_cts_offsets"
} else if opts.fast_start {
  "faststart+negative_cts_offsets"
} else {
  "negative_cts_offsets"
};
```

### Container Comparison

| Feature        | MP4                    | MKV/WebM                     |
| -------------- | ---------------------- | ---------------------------- |
| B-frame timing | Preserved via CTTS     | Lost (sequential timestamps) |
| DTS handling   | Dynamic shift          | DTS = PTS                    |
| FFmpeg flags   | `negative_cts_offsets` | None                         |
| CTTS version   | 1 (signed offsets)     | N/A                          |

---

## Decoder B-Frame Handling

### Context Configuration

**File:** `src/codec/context.rs:799-806`

```rust
// For H.264 and HEVC, set has_b_frames BEFORE opening the codec
// This tells the decoder to allocate a proper reorder buffer for B-frames.
// Without this, the decoder may drop frames when reordering is needed.
if matches!(config.codec_id, AVCodecID::H264 | AVCodecID::Hevc) {
  ffctx_set_has_b_frames(ctx, 2);
}
```

### Decoder Flags

**File:** `src/codec/context.rs:786-797`

```rust
// OUTPUT_CORRUPT: Output even potentially corrupted frames
flags |= ffi::accessors::codec_flag::OUTPUT_CORRUPT;

// SHOW_ALL: Ensures all frames are output including B-frames
// that might otherwise be held back due to recovery state
let flags2 = ffi::accessors::codec_flag2::SHOW_ALL;
```

### Timestamp Queue (Decoder)

**File:** `src/webcodecs/video_decoder.rs:219-221`

```rust
/// Queue of input timestamps for correlation with output frames
timestamp_queue: std::collections::VecDeque<(i64, Option<i64>)>,
```

**Important Limitation:** The decoder uses FIFO timestamp queue, which assumes frames are output in decode order. For B-frame content where FFmpeg outputs in presentation order, this can cause timestamp misassignment.

### Flush for B-Frame Content

**File:** `src/codec/context.rs:987-1051`

```rust
/// Flush the decoder - drains all buffered frames
///
/// For decoders with B-frames, this ensures all reordered frames are output.
/// CRITICAL: Only exit on AVERROR_EOF, not on EAGAIN.
```

---

## FFmpeg Internals

### CTTS (Composition Time to Sample) Box

**File:** `ffmpeg-src/FFmpeg/libavformat/movenc.c:3090-3128`

```c
static int mov_write_ctts_tag(AVFormatContext *s, AVIOContext *pb, MOVTrack *track)
{
  // CTS offset per sample
  ctts_entries[0].offset = track->cluster[0].cts;  // cts = pts - dts

  // Write CTTS atom
  ffio_wfourcc(pb, "ctts");

  // Version determines signed vs unsigned offsets
  if (mov->flags & FF_MOV_FLAG_NEGATIVE_CTS_OFFSETS)
    avio_w8(pb, 1);  // Version 1: signed 32-bit offsets
  else
    avio_w8(pb, 0);  // Version 0: unsigned 32-bit offsets
}
```

### DTS Shift Mechanism

**File:** `ffmpeg-src/FFmpeg/libavformat/movenc.c:7108-7111`

```c
if (mov->flags & FF_MOV_FLAG_NEGATIVE_CTS_OFFSETS) {
  if (trk->dts_shift == AV_NOPTS_VALUE)
    trk->dts_shift = pkt->pts - pkt->dts;  // Capture first packet's offset
  pkt->dts += trk->dts_shift;  // Shift all DTS values forward
}
```

### x264 B-Frame Output

**File:** `ffmpeg-src/FFmpeg/libavcodec/libx264.c`

```c
// B-frame configuration (lines 1153-1154)
if (avctx->max_b_frames >= 0)
  x4->params.i_bframe = avctx->max_b_frames;

// has_b_frames computation (lines 1399-1402)
avctx->has_b_frames = x4->params.i_bframe ?
  x4->params.i_bframe_pyramid ? 2 : 1 : 0;

// Timestamp output (lines 663-664)
pkt->pts = pic_out.i_pts;
pkt->dts = pic_out.i_dts;
```

---

## Chromium Compatibility

### CTTS Parsing

**File:** `chromium/media/formats/mp4/box_definitions.cc:2251-2252`

```cpp
// Chromium reads composition time offsets as SIGNED 32-bit integers
if (sample_composition_time_offsets_present)
  RCHECK(reader->Read4s(&sample_composition_time_offsets[i]));
```

**Storage:** `std::vector<int32_t>` - supports negative values (CTTS version 1)

### Edit List Processing

**File:** `chromium/media/formats/mp4/track_run_iterator.cc:326-341`

```cpp
// Process edit list to remove CTS offset introduced in the presence of B-frames
int64_t edit_list_offset = 0;
const std::vector<EditListEntry>& edits = trak->edit.list.edits;
if (!edits.empty()) {
  if (edits.size() > 1)
    DVLOG(1) << "Multi-entry edit box detected; some components ignored.";
  if (edits[0].media_time >= 0) {
    edit_list_offset = edits[0].media_time;
  }
}
```

### PTS Calculation

**File:** `chromium/media/formats/mp4/track_run_iterator.cc:171-177`

```cpp
auto cts_offset = -base::CheckedNumeric<int64_t>(edit_list_offset);
if (i < trun.sample_composition_time_offsets.size())
  cts_offset += trun.sample_composition_time_offsets[i];
```

**Formula:**

```
PTS = DTS + CTS_offset
    = DTS + (sample_composition_time_offset - edit_list_offset)
```

### Frame Emission Order

**File:** `chromium/media/formats/mp4/mp4_stream_parser.cc:1168-1180`

Chromium emits frames in **decode order** (DTS), not presentation order:

```cpp
stream_buf->set_timestamp(runs_->cts());  // PTS for display
stream_buf->SetDecodeTimestamp(runs_->dts());  // DTS for decode order
(*buffers)[runs_->track_id()].push_back(stream_buf);
```

Frame reordering is done by the video decoder, not the demuxer.

---

## Timestamp Flow Diagrams

### Encoding Flow

```
Input VideoFrame                 Encoder                    EncodedVideoChunk
┌──────────────────┐            ┌──────────────────┐       ┌──────────────────┐
│ timestamp: 33333 │ ────────── │ FFmpeg Encoder   │ ───── │ timestamp_us:    │
│ (1st frame)      │  (push to  │ (buffers for     │ (pop  │   33333          │
└──────────────────┘  ts_queue) │  B-frame reorder)│  from │ dts_us: -16667   │
                                └──────────────────┘  queue)│ original_pts: 0  │
                                                           └──────────────────┘

Input VideoFrame                                           EncodedVideoChunk
┌──────────────────┐                                       ┌──────────────────┐
│ timestamp: 66666 │ ──────────────────────────────────── │ timestamp_us:    │
│ (2nd frame)      │                                       │   66666          │
└──────────────────┘                                       │ dts_us: 0        │
                                                           │ original_pts:    │
                                                           │   66666          │
                                                           └──────────────────┘
```

### Muxing Flow (MP4)

```
EncodedVideoChunk              Muxer Processing              Written Packet
┌──────────────────┐          ┌──────────────────┐          ┌──────────────────┐
│ pts: 512         │          │ has_b_frames: ✓  │          │ pts: 512         │
│ dts: 1024        │  ──────  │ pts < dts?       │  ──────  │ dts: 512         │
│ (B-frame)        │          │ YES: shift DTS   │          │ (shift applied)  │
└──────────────────┘          │ dts_shift: -512  │          └──────────────────┘
                              └──────────────────┘
                                     │
                                     ▼
                              ┌──────────────────┐
                              │ Future packets   │
                              │ get -512 shift   │
                              │ applied to DTS   │
                              └──────────────────┘
```

### CTTS Storage (MP4 Container)

```
┌─────────────────────────────────────────────────────────────┐
│ CTTS Box (Composition Time to Sample)                       │
├─────────────────────────────────────────────────────────────┤
│ Version: 1 (signed offsets, enabled by negative_cts_offsets)│
│ Flags: 0                                                    │
├─────────────────────────────────────────────────────────────┤
│ Entry Count: N                                              │
├─────────────────────────────────────────────────────────────┤
│ Sample 0: count=1, offset=512   (I-frame: pts > dts)       │
│ Sample 1: count=1, offset=2048  (P-frame: pts > dts)       │
│ Sample 2: count=1, offset=512   (P-frame: pts > dts)       │
│ Sample 3: count=1, offset=-512  (B-frame: pts < dts)       │
│ ...                                                         │
└─────────────────────────────────────────────────────────────┘
```

### Decoding Flow

```
EncodedVideoChunk              Decoder                       VideoFrame
┌──────────────────┐          ┌──────────────────┐          ┌──────────────────┐
│ timestamp: 33333 │          │ has_b_frames: 2  │          │ timestamp: 33333 │
│ (in DTS order)   │  ──────  │ Reorder buffer   │  ──────  │ (PTS order)      │
└──────────────────┘          │ active           │          └──────────────────┘
                              └──────────────────┘
```

---

## Summary

### Key Points

1. **Encoder**: B-frames controlled by `latencyMode` - `"quality"` enables B-frames, `"realtime"` disables them.

2. **EncodedVideoChunk**: Stores both `original_pts` and `dts_us` internally for B-frame support (not exposed to JS API).

3. **MP4 Muxer**: Uses dynamic DTS shift + `negative_cts_offsets` to satisfy FFmpeg's `pts >= dts` constraint while preserving B-frame timing in CTTS.

4. **MKV/WebM Muxer**: Cannot preserve B-frame display order; uses sequential timestamps.

5. **Decoder**: Pre-configures `has_b_frames=2` for H.264/HEVC to enable reorder buffer.

6. **Chromium**: Reads CTTS as signed int32, supports negative composition offsets, emits frames in DTS order.

### Constraints

| Constraint    | Enforced By  | Handling                    |
| ------------- | ------------ | --------------------------- |
| `PTS >= DTS`  | FFmpeg API   | Dynamic DTS shift in muxer  |
| Monotonic DTS | FFmpeg muxer | `last_video_dts` tracking   |
| Signed CTTS   | CTTS version | `negative_cts_offsets` flag |