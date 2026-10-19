---
name: sync-chrome-vendor
description: Use when moving the code that copies Chrome's image decoding to a newer Chrome stable (vendor/libjpeg-turbo, the Blink JPEG decoder port in src/ffi/jpeg_blink.c and the other ported Chrome rules), or to find which Chrome file and revision a ported piece follows.
---

# Sync the code that follows Chrome

Where Chrome and the spec differ, this project matches real Chrome. All the code in the
table below follows one Chrome stable version: the `chrome:` line of
`vendor/libjpeg-turbo/CHROMIUM_PIN`.

## Steps

1. Pick the Chrome stable to follow. Use the Linux release that is fully rolled out
   (`fraction: 1`); Mac and Windows often serve several versions at once, and
   `versions?pageSize=1` gives the highest one, which can still be a staged rollout:

   ```sh
   curl -s 'https://versionhistory.googleapis.com/v1/chrome/platforms/linux/channels/stable/versions/all/releases?filter=endtime=none'
   ```

   If no release is at `fraction: 1`, ask the maintainer which version to follow. The local
   Chrome that makes the digests in step 4 must be that same version.

2. Vendor the libjpeg-turbo that its DEPS pins (`src/third_party/libjpeg_turbo`):

   ```sh
   node tools/sync-libjpeg-turbo.mjs --chrome <version>
   ```

   It replaces `vendor/libjpeg-turbo` (with `README.chromium`, which lists Chromium's own
   changes to libjpeg-turbo, and Chromium's `LICENSE` as `CHROMIUM_LICENSE`) and the copies in
   `licenses/`, and prints every change. The source lists in `CHROMIUM_PIN`, the SIMD ones
   (`simd-x86_64`, `simd-aarch64`, `simd-arm`) too, come from that revision's `BUILD.gn`.
   Without `--chrome` it re-syncs the pinned revision and prints `No changes.`. Never edit
   vendored files: put any difference in the `build.rs` flags or in `src/ffi/jpeg_blink.c`.

3. Diff these files between the pinned tag and the new one. Gitiles serves a file as base64
   with `https://chromium.googlesource.com/chromium/src/+/refs/tags/<version>/<path>?format=TEXT`
   (its log pages need a sign-in, so fetch both versions and diff them):
   - `third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.cc`: the
     decode settings, YUV choice, CMYK rule and errors in `src/ffi/jpeg_blink.c`
   - `third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_decoder_factory.cc` and
     `skia/rusty_jpeg_feature.cc`: which decoder Chrome uses
   - `third_party/blink/renderer/platform/image-decoders/image_decoder.cc`: `SetSize` (the
     size check in `read_header`), `CalculateMaxDecodedBytes` (desired size) and
     `SetEmbeddedColorProfile` (when an ICC profile makes Chrome transform at decode time)
   - `third_party/blink/renderer/modules/webcodecs/image_decoder_core.cc`: no YUV after an
     RGB decode of partial data (`rgb_decode` and `jpeg_frame` in
     `src/webcodecs/image_decoder.rs`) and the color tags set at the end of `decode()`
   - `third_party/blink/renderer/modules/webcodecs/image_decoder_external.cc`: when a failure
     rejects `tracks.ready`, the pending decodes and `completed` (`SetFailed`, `OnMetadata`),
     followed by `jpeg_frame`, `apply_frame_failure`, `end_with_frame_failure`,
     `close_if_metadata_failed` and `closed_error`
   - `media/base/limits.h` `kMaxDimension`: Chrome rejects a decoded frame wider or taller
     than this
   - PNG and APNG: Skia's `src/codec/SkPngRustCodec.cpp` and `src/codec/SkCodec.cpp` (at the
     Skia revision in DEPS) and the png crate in
     `third_party/rust/chromium_crates_io/vendor/png-v0_18` (`src/decoder/stream.rs`,
     `src/decoder/read_decoder.rs`): when the header is ready, which chunks and CRCs fail the
     image or a frame, how frames are counted, the repetition count and how frames are drawn,
     all in `src/webcodecs/apng.rs`
   - WebP: libwebp's `src/demux/demux.c`, `src/dec/webp_dec.c`, `src/dec/vp8_dec.c` and
     `src/dec/vp8l_dec.c` (at the libwebp revision in DEPS), ported in
     `src/webcodecs/webp.rs`, and `third_party/blink/renderer/platform/image-decoders/webp/webp_image_decoder.cc`
     (`UpdateDemuxer`: when the header is ready, truncated files, a parse error after the header;
     frame blending), followed by `read_webp` in `src/webcodecs/image_decoder.rs` and `draw`
     in `webp.rs`
   - `third_party/blink/renderer/platform/image-decoders/skia/skia_image_decoder_base.cc`:
     which frame failures fail the decoder (`SetFailedFrameIndex`), followed by `read_png` and
     `layered_frames` in `src/webcodecs/image_decoder.rs`
   - the libjpeg-turbo `BUILD.gn` (in the vendored revision): the `defines` of `libjpeg`,
     `libjpeg12`, `libjpeg16`, `simd` and `libjpeg_simd_config`, and the `simd_asm` flags.
     `build.rs` copies them by hand: `NO_GETENV` and `NO_PUTENV` on the decoder files,
     `WITH_SIMD` with `NEON_INTRINSICS` (Arm) or `USE_CLZ_INTRINSIC` (x86) on the 8-bit ones,
     and `WITH_SIMD` (plus `NEON_INTRINSICS` on Arm) but not `NO_GETENV` on the SIMD files, so
     `JSIMD_FORCENONE` and the other `JSIMD_*` variables work as in Chrome
   - `third_party/nasm/nasm_assemble.gni` (`chromium/deps/nasm` at the revision in DEPS): the
     NASM flags that `assemble_libjpeg_turbo` in `build.rs` copies

   Port the decode changes to `src/ffi/jpeg_blink.c`, and size changes to
   `jpeg_scale_numerator`. If `kRustyJpegFeature` becomes enabled by default, or the factory
   picks the Rust decoder for the cases WebCodecs uses, Chrome no longer decodes JPEG with
   libjpeg-turbo: follow RustyJpeg instead. Also look
   for a RustyJpeg study in `testing/variations/fieldtrial_testing_config.json`: a field trial
   can switch some users to it, while the digests come from Chrome without field trials
   (Playwright starts Chrome with `--disable-field-trial-config`). There is none at
   155.0.8059.79. Check the other rows of the table the same way.

4. Rebuild and check. x86_64 builds need NASM 2.15.05 or newer (`brew install nasm`,
   `apt-get install nasm`, or `NASM=<path>`); on macOS use NASM 3.00 or newer, older ones drop
   the `macho build_version` pragma and ld then warns.
   - `pnpm build`, then `cargo test --lib jpeg` (digests of Chrome's bytes for
     `__test__/fixtures/jpeg`) and the JPEG tests of `__test__/image-decoder-jpeg.spec.ts` and
     `__test__/decoded-frame-format.spec.ts` (Chrome's formats, bytes, stream and error cases
     through the JS API). The SIMD IDCTs give other bytes than the C one on corrupt data, and
     Neon others than x86, so `decodes_chrome_simd_bytes` has one digest per
     family: Neon (aarch64 and armv7) from Chrome arm64, x86 from Chrome x86_64 (SSE2 and
     AVX2 give the same bytes). Keep its crafted fixtures at full size and M = 4 and 2: at
     M = 3, 5, 6 and 7 the IDCTs are C code that computes in `long`, so on such data armv7 and
     Windows (32-bit `long`) give other bytes than Chrome on 64-bit macOS and Linux. On an
     Apple silicon Mac also run the tests for `x86_64-apple-darwin`, once as is (SSE2) and
     once with `ROSETTA_ADVERTISE_AVX=1` (AVX2)
   - `nm -g --defined-only` on `target/**/build/webcodecs-node-*/out/libjpeg_turbo.a`: every
     symbol starts with `webcodecs_` (`_webcodecs_` on macOS), the `jsimd_*` and `jconst_*`
     ones too
   - decode the fixtures in the new Chrome and compare with the digests in
     `src/codec/jpeg.rs`. Change a digest only when Chrome's bytes changed. Serve the fixtures
     from `http://127.0.0.1` (ImageDecoder needs a secure context) and run this in the page,
     for example with playwright-core's `chromium.launch({ executablePath })`:

     ```js
     // bytes: a fixture's Uint8Array. desired: {desiredWidth, desiredHeight} for the scaled
     // cases. stream: true for the allow_yuv=false cases.
     async function chromeDigest(bytes, desired = {}, stream = false) {
       const data = stream
         ? new ReadableStream({
             async start(c) {
               c.enqueue(bytes)
               await new Promise((r) => setTimeout(r, 30))
               c.close()
             },
           })
         : bytes
       const decoder = new ImageDecoder({ type: 'image/jpeg', data, ...desired })
       const { image } = await decoder.decode()
       // copyTo's default layout is tight: Y, U, V planes, or BGRX rows
       const buf = new Uint8Array(image.allocationSize())
       await image.copyTo(buf)
       const sha = new Uint8Array(await crypto.subtle.digest('SHA-256', buf))
       return {
         format: image.format,
         size: [image.codedWidth, image.codedHeight],
         colorSpace: image.colorSpace.toJSON(),
         sha256: [...sha].map((b) => b.toString(16).padStart(2, '0')).join(''),
       }
     }
     ```

     The scaled cases of `420-37x9.jpg` use `{desiredWidth: 74, desiredHeight: 2}` (M = 5)
     and `{desiredWidth: 5, desiredHeight: 2}` (M = 1), those of `idct-overflow-*-17x9.jpg`
     `{desiredWidth: 9, desiredHeight: 5}` (M = 4) and `{desiredWidth: 5, desiredHeight: 3}`
     (M = 2). For the x86 digests on an Apple silicon Mac, run the x86_64 half of the
     universal Google Chrome under Rosetta: point `executablePath` at a script that runs
     `exec /usr/bin/arch -x86_64 "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" "$@"`,
     once as is and once with `ROSETTA_ADVERTISE_AVX=1`; both must give the same bytes.

5. Update the versions in the table.

## Known differences from Chrome

`src/ffi/jpeg_blink.c` lists them at the top. Keep that list true after a sync:

- ICC profiles that Chrome applies at decode time with skcms (a CMYK profile on CMYK/YCCK,
  an RGB or gray profile Skia cannot hold exactly). Matching them needs skcms.

`src/webcodecs/image_decoder.rs` has these, outside the JPEG decoder:

- A stream that stays open after its last byte: a waiting decode() resolves (or fails) at the
  close, or at the next probe when the data has doubled (all formats but JPEG, PNG and WebP).
  Chrome decodes again on each chunk and answers as soon as the frame is whole. A JPEG does the
  same here: a decode() that waits tries the data read so far when it is made, when a chunk comes
  to an idle stream and when the stream goes idle (`post_jpeg_decode`). A PNG or WebP reads its
  layout again on each chunk (`apng`, `webp`) and decodes a frame once all of its data is here
  (a bare bitstream: see below). A decode() of it that waits is answered once a read has drawn
  its frame.
- tracks.ready of a PNG or WebP given much data at once: Chrome's decoder stops at the header.
  Here, from a buffer, the read that finds the header resolves it once it has parsed all the data
  (for a PNG, with every chunk's CRC checked) and copied the data of each whole frame, but before
  any frame is drawn. On a stream, a chunk that holds the header and whole frames resolves it only
  once the read of that chunk has drawn them (or a reset() stopped the draw: the read still stores
  the track); a chunk with the header and no whole frame resolves it at once.
- PNG and WebP data that is whole but bad, with the stream open: Chrome finds the error while it
  inflates or decodes the first bytes (a PNG whose first deflate block is bad fails at 95 of
  1442 bytes); here the frame fails once all of its data is here. A lossy (VP8) WebP frame that
  libwebp can not finish waits for more data in Chrome and fails at the stream's end; here it
  fails at once.
- Lossy (VP8) WebP frames that libwebp and FFmpeg read differently: libwebp rejects a frame whose
  first partition is too short for its macroblock modes, FFmpeg decodes it (here the frame
  decodes); and libwebp can finish a frame without the last byte of its data, so Chrome answers
  one byte sooner on an open stream, and decodes a file cut by that byte where it fails here.
  Matching them needs libwebp's VP8 decoder.
- APNG frame count while a frame is partly decoded: Skia counts no new frame while it has a
  frame partly decoded (`SkPngRustCodec::onGetFrameCount`), nor after a decode error. Here frames
  count as their headers come: a decode() that waits for a frame whose data does not inflate
  leaves Chrome at the frames counted then (2 of 6 in `apng-blend-corrupt1.png`), and 6 here.
- PNG and WebP chunks enqueued in the task of a decode(), or while a decode() waits: Chrome reads
  a chunk into its decoder when it is enqueued, so a decode() uses the chunks enqueued before it
  when no other decode() is still decoding ahead of it (with one still decoding, a later decode()
  also uses a chunk enqueued after it in its task). Here the collector reads the stream through
  napi's stream reader, which asks the JS thread for one chunk at a time, and a decode() looks at
  the frames that the reads done by then have drawn (a decode() made before tracks.ready waits for
  it, and so for the read of the chunk that holds the header). So its result depends on which
  thread gets there first. Posting a PNG or WebP decode() among the chunks in their order, as
  `queue_jpeg_post` does for a JPEG, would match Chrome in more of the cases below. Measured in
  fresh processes, 20 runs per case unless a count says otherwise, each case with one result in
  Chrome:
  - `anim-mux-bad1.webp` (frame 1 broken; frame 0 whole in the first 128 bytes) given as those
    bytes and the rest in one task, with a decode() of frame 0 between the two: Chrome gives frame
    0, here it fails (EncodingError), also when the close comes 300 ms later. With tracks.ready
    awaited after the first bytes and the rest enqueued just before the decode(): Chrome fails it,
    here it gives frame 0. A second decode() made in a later task, after the rest: Chrome gives
    frame 0 to the first and fails the second at the close; here both give frame 0, or both fail,
    or (in some runs, for some files) Chrome's result comes: how often each comes depends on the
    file, on the time between the two calls and on the load. The same with three 1920x1080 WebPs
    whose frame 1 is broken. With `apng-blend-corrupt1.png` (frame 1 does not inflate) the orders
    with one decode() give frame 0 in both.
  - `anim-lossy-bad.webp` and `anim-lossless-bad.webp` (frame 1 broken) given as a first part that
    holds frame 0 and the rest in one task, with a decode() between the two: Chrome gives frame 0,
    here it fails (it gave frame 0 in 2 and 1 of 160 runs). `anim-lossy.webp` and
    `anim-lossless.webp` given as a first part that holds frame 0 and ends inside frame 1, then a
    decode() and the close in one task: Chrome gives frame 0; here the decode() is answered after
    the close, where the cut frame 1 fails the decoder, so it fails.
  - `craft_vp8x_opaque_33x17.webp` and `vp8x-vp8l.webp` (VP8X still images) given as the bytes up
    to the image chunk header and the rest in one task: with a decode() between the two, Chrome
    fails it and here it gives the frame. With tracks.ready awaited after the first bytes, then the
    rest and the close enqueued before the decode(), Chrome gives the frame and here it usually
    fails (it gave the frame in 1 and 5 of 40 runs); with the decode() before the rest there, both
    fail (here it gave the frame in 7 and 4 of 160 runs).
  - A simple lossy WebP, YUV planes in Chrome and BGRX here: a decode() made in the tracks.ready
    reaction with the first 30 bytes and the rest enqueued in one task, or one made before the data
    or after the data and the close, with the data in two chunks and the close in one task (made
    between the two chunks, or between the data and the close, it is BGRX in both). The other way
    round, BGRX in Chrome and YUV here: a 4000x3000 one, with the decode() in the tracks.ready
    reaction, the whole file in one chunk and the close 50 ms later.
- A WebP that is a bare VP8 or VP8L bitstream (no RIFF header, no chunk header) on an open
  stream: it has no size, so a read decodes its frame from the bytes so far, and that frame or
  failure is kept (`webp::image_complete`). Chrome waits for the rest. The VP8 bitstream of
  `webp-valid.webp` cut at 500 of 1116 bytes, then the rest: Chrome gives the whole image, here
  only its first 7 of 48 rows are right. The VP8L bitstream of `wpt/four-colors.webp` cut at 20
  of 57 bytes: Chrome gives the frame (a decode() made at the cut fails it), here it fails.
- reset() or close() while decode() calls wait: like Chrome, each one rejects all of them in its
  own call, in call order, with one AbortError ("Aborted by reset.", and for close() Chrome's
  "Aborted by failure.": its two labels look swapped), in every format, whatever a call waits
  for: the tracks (reset() leaves `tracks.ready` pending), data, its frame work, or a decoded
  frame not given yet (`take_calls`). Unlike Chrome, close() resolves a `tracks.ready` that still
  waits, leaves `completed` to the stream and goes on reading it, where Chrome rejects both with
  that error and cancels the stream; and a failure that closes the decoder rejects the calls
  that wait one by one, on their turns, where Chrome rejects them all in one task. Like Chrome, a
  JPEG, GIF or BMP failure found by work that started before the reset() does not close the
  decoder: it is kept, and the next decode() fails and closes it.
  Chrome can also close on a kept JPEG failure when the next chunk comes (`completed` rejects)
  or the stream ends (`completed` resolves), but only when its decode ran before the reset(): it
  reads the metadata again then, and its decoder is still failed (OnMetadata). With reset() 3
  microtasks after decode(), that was 9 of 30 runs at the chunk and 4 of 30 at the end. Here
  only the next decode() closes it, and that decode() rejects with EncodingError where Chrome's
  is InvalidStateError. The frames after the reset() match Chrome: a JPEG keeps the choice of RGB
  that a decode of part of the data made (`rgb_decode`).
- The data or the close in a 0 ms timer right after a decode(): Chrome's first decode (made
  before it read any data) is still on its decoder thread when the timer runs, so Chrome reads
  the close before it decodes again, and decodes YUV planes. Here no decode is in flight: the
  data is tried as soon as it is read, before the close is read: BGRX. Both depend on timing;
  with 30 ms between the decode() and the close, Chrome is BGRX too.
- Stream results and a JPEG decode() in one task: Chrome reads a chunk, the close or an error
  inside enqueue(), close() or error(), and starts its next read there too, so its decode() sees
  what came before it in the task and nothing after. Here the JS thread keeps one read waiting
  and starts the next read() only in the reaction to the last one (`onRead` calls
  `JpegStream::read_next`): after the first chunk of a task, the other chunks and the close of
  that task wait in the stream's queue until those reactions run, and the decode() can only wait
  for them (`queue_jpeg_post`). That gives the same order, except after two or more chunks in one
  task: a decode() made between them, or before the close that follows them, sees all of them,
  and the close. It then decodes YUV planes where Chrome gives BGRX (a frame that fails:
  `completed` resolves where Chrome's rejects).
- Two more results of the same cause, in every format (the other formats read through napi's
  `Reader`, which also has one read waiting and copies a chunk in the reaction), and on main too:
  a chunk whose buffer is detached right after enqueue() is read empty here, so the decode fails
  (or a JPEG gets other pixels), where Chrome copied it inside enqueue() and decodes it; and
  `desiredSize` after a second enqueue() in one task is -1 here and 0 in Chrome, whose pull() runs
  inside the first enqueue(). A decode() that takes a frame made from the first chunk alone does
  not show the first one: a GIF here, and a JPEG or a GIF on main.
- Fake timers that replace `setImmediate` on the main context (`@sinonjs/fake-timers`,
  Vitest's vi.useFakeTimers(), node:test's mock.timers; not Jest's, which live in its vm
  context) on Bun, Deno or WASI, or where Node lives in a DLL on Windows: there a JPEG stream
  checks whether it went idle from setImmediate (`after_task` finds no libuv check handle), so
  with setImmediate faked, a decode() made before the rest of the data comes waits for the close.
  Chrome uses no page global for this; in Node neither does this decoder (the try of a decode()
  is a promise reaction, the idle check a libuv check handle).
- Size limits: Chrome fails a frame wider or taller than `kMaxDimension` (32767); here FFmpeg
  allocates the frame, and fails it when (width + 128) × (height + 128) ≥ 2^28. Both fail the
  metadata above 2^29 - 1 pixels.

## Ported code that follows Chrome

Chrome 155.0.8059.79.
`src/ffi/jpeg_blink.c` is derived from Blink's `jpeg_image_decoder.cc` and keeps that file's
licence notice (LGPL 2.1 or later, MPL 1.1 or GPL 2.0): keep it when you port changes.
The files in `crates/pixel-simd/src/webp` and `src/webcodecs/webp.rs` port libwebp and keep its
notice (BSD-3-Clause, `licenses/libwebp-COPYING.txt` and `licenses/libwebp-PATENTS.txt`): keep it
too.

| What                                   | Chromium file and revision                                                                                                                              | Where here                                                                                                                                                                     |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| libjpeg-turbo, decoder and SIMD files  | `src/third_party/libjpeg_turbo` = `chromium/deps/libjpeg_turbo` @ `640f254ad0fa`                                                                        | `vendor/libjpeg-turbo`, `tools/sync-libjpeg-turbo.mjs`                                                                                                                         |
| Blink JPEG decode settings             | `third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.cc` @ 155.0.8059.79                                                         | `src/ffi/jpeg_blink.c`, `src/codec/jpeg.rs`                                                                                                                                    |
| JPEG M/8 scale rule                    | same file, `DesiredScaleNumerator` @ 155.0.8059.79                                                                                                      | `jpeg_scale_numerator` in `src/webcodecs/image_decoder.rs`                                                                                                                     |
| WebCodecs YUV gate and color tags      | `third_party/blink/renderer/modules/webcodecs/image_decoder_core.cc` @ 155.0.8059.79                                                                    | `rgb_decode` and `jpeg_frame` in `src/webcodecs/image_decoder.rs`, the color tags in `src/ffi/jpeg_blink.c`                                                                    |
| WebCodecs failure schedule             | `third_party/blink/renderer/modules/webcodecs/image_decoder_external.cc` @ 155.0.8059.79                                                                | `jpeg_frame`, `apply_frame_failure`, `close_if_metadata_failed` and `closed_error` in `src/webcodecs/image_decoder.rs`                                                         |
| WebCodecs decode schedule              | `image_decoder_external.cc` and `image_decoder_core.cc` (same directory as above) @ 155.0.8059.79                                                       | `FrameCall`, `JpegChunkReader` and `jpeg_frame` in `src/webcodecs/image_decoder.rs`                                                                                            |
| libwebp fancy upsampler and YUV to RGB | `src/third_party/libwebp/src` @ `b43b2caa710c`: `src/dsp/upsampling.c`, `src/dec/io_dec.c`, `src/dsp/yuv.h`                                             | `scalar`, `upsample`, `bgra` in `crates/pixel-simd/src/webp/mod.rs`; `fancy` in `crates/pixel-simd/src/webp/rows.rs`                                                           |
| libwebp SSE2 upsampler and YUV to RGB  | same @ `b43b2caa710c`: `src/dsp/upsampling_sse2.c`, `src/dsp/yuv_sse2.c`                                                                                | `crates/pixel-simd/src/webp/x86.rs`, `row_pair` in `webp/rows.rs`; `webp/wasm.rs` is the same code in simd128 and the AVX2 code is SSE2 widened (neither has an upstream file) |
| libwebp NEON upsampler and YUV to RGB  | same @ `b43b2caa710c`: `src/dsp/upsampling_neon.c`                                                                                                      | `crates/pixel-simd/src/webp/neon.rs` (16 lanes instead of 8)                                                                                                                   |
| Skia SkFrameHolder alpha rule (GIF)    | `src/third_party/skia` @ `29ed1e87a0a5`: `src/codec/SkCodec.cpp` `setAlphaAndRequiredFrame`                                                             | `frame_alpha` in `src/webcodecs/image_hints.rs`                                                                                                                                |
| image-rs BMP alpha rule                | `third_party/rust/chromium_crates_io/vendor/image-v0_25` (0.25.10) @ 155.0.8059.79: `src/codecs/bmp/decoder.rs`                                         | `bmp_header` in `src/webcodecs/image_hints.rs`                                                                                                                                 |
| image crate BMP bitfield scaling       | `third_party/rust/chromium_crates_io/vendor/image-v0_25` (0.25.10) @ 155.0.8059.79: `src/codecs/bmp/decoder.rs` `Bitfield::from_mask`, `Bitfield::read` | `Bitfield` in `crates/pixel-simd/src/lib.rs`                                                                                                                                   |
| png crate chunk and CRC rules          | `third_party/rust/chromium_crates_io/vendor/png-v0_18` (0.18.1) @ 155.0.8059.79: `src/decoder/stream.rs`, `src/decoder/read_decoder.rs`                 | `parse_with` in `src/webcodecs/apng.rs`                                                                                                                                        |
| Skia APNG frames                       | `src/third_party/skia` @ `29ed1e87a0a5`: `src/codec/SkPngRustCodec.cpp` (frame count, repetition count, `blendRow`), `src/codec/SkCodec.cpp`            | `make_layout`, `alpha_and_required_frame`, `blend_over` in `src/webcodecs/apng.rs`                                                                                             |
| libwebp demuxer and header parse       | `src/third_party/libwebp/src` @ `b43b2caa710c`: `src/demux/demux.c`, `src/dec/webp_dec.c`, `src/dec/vp8_dec.c`, `src/dec/vp8l_dec.c`                    | `demux`, `features`, `raw_still` in `src/webcodecs/webp.rs`                                                                                                                    |
| Blink WebP decoder                     | `third_party/blink/renderer/platform/image-decoders/webp/webp_image_decoder.cc` @ 155.0.8059.79, `image_decoder.cc` (`FindRequiredPreviousFrame`)       | `read_webp` in `src/webcodecs/image_decoder.rs`; `anim_frames`, `draw`, `blend_non_premultiplied` in `src/webcodecs/webp.rs`                                                   |
| Blink frame failures (PNG, WebP)       | `third_party/blink/renderer/platform/image-decoders/skia/skia_image_decoder_base.cc` @ 155.0.8059.79                                                    | `read_png`, `layered_frames` in `src/webcodecs/image_decoder.rs`                                                                                                               |