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
     followed by `jpeg_frame`, `close_if_failed` and `closed_error`
   - `media/base/limits.h` `kMaxDimension`: Chrome rejects a decoded frame wider or taller
     than this
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
  close, or at the next probe when the data has doubled, as for every format here. Chrome
  decodes again on each chunk and answers as soon as the frame is whole.
- reset() while a decode() waits: Chrome rejects that decode() with an AbortError; here it still
  resolves or fails later (all formats). The frames after the reset() match Chrome: a JPEG keeps
  the choice of RGB that a decode of part of the data made (`rgb_decode`).
- The data or the close in a 0 ms timer right after a decode(): Chrome's first decode (made
  before it read any data) is still on its decoder thread when the timer runs, so Chrome reads
  the close before it decodes again, and decodes YUV planes. Here that first decode is done at
  once, so the data is decoded before the close is read: BGRX. Both depend on timing; with
  30 ms between the decode() and the close, Chrome is BGRX too.
- Size limits: Chrome fails a frame wider or taller than `kMaxDimension` (32767); here FFmpeg
  allocates the frame, and fails it when (width + 128) × (height + 128) ≥ 2^28. Both fail the
  metadata above 2^29 - 1 pixels.

## Ported code that follows Chrome

Chrome 155.0.8059.79.
`src/ffi/jpeg_blink.c` is derived from Blink's `jpeg_image_decoder.cc` and keeps that file's
licence notice (LGPL 2.1 or later, MPL 1.1 or GPL 2.0): keep it when you port changes.
The files in `crates/pixel-simd/src/webp` port libwebp and keep its notice (BSD-3-Clause,
`licenses/libwebp-COPYING.txt` and `licenses/libwebp-PATENTS.txt`): keep it too.

| What                                   | Chromium file and revision                                                                                                                              | Where here                                                                                                                                                                     |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| libjpeg-turbo, decoder and SIMD files  | `src/third_party/libjpeg_turbo` = `chromium/deps/libjpeg_turbo` @ `640f254ad0fa`                                                                        | `vendor/libjpeg-turbo`, `tools/sync-libjpeg-turbo.mjs`                                                                                                                         |
| Blink JPEG decode settings             | `third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.cc` @ 155.0.8059.79                                                         | `src/ffi/jpeg_blink.c`, `src/codec/jpeg.rs`                                                                                                                                    |
| JPEG M/8 scale rule                    | same file, `DesiredScaleNumerator` @ 155.0.8059.79                                                                                                      | `jpeg_scale_numerator` in `src/webcodecs/image_decoder.rs`                                                                                                                     |
| WebCodecs YUV gate and color tags      | `third_party/blink/renderer/modules/webcodecs/image_decoder_core.cc` @ 155.0.8059.79                                                                    | `rgb_decode` and `jpeg_frame` in `src/webcodecs/image_decoder.rs`, the color tags in `src/ffi/jpeg_blink.c`                                                                    |
| WebCodecs failure schedule             | `third_party/blink/renderer/modules/webcodecs/image_decoder_external.cc` @ 155.0.8059.79                                                                | `jpeg_frame`, `close_if_failed` and `closed_error` in `src/webcodecs/image_decoder.rs`                                                                                         |
| WebCodecs decode schedule              | `image_decoder_external.cc` and `image_decoder_core.cc` (same directory as above) @ 155.0.8059.79                                                       | `FrameCall`, `JpegChunkReader` and `jpeg_frame` in `src/webcodecs/image_decoder.rs`                                                                                            |
| libwebp fancy upsampler and YUV to RGB | `src/third_party/libwebp/src` @ `b43b2caa710c`: `src/dsp/upsampling.c`, `src/dec/io_dec.c`, `src/dsp/yuv.h`                                             | `scalar`, `upsample`, `bgra` in `crates/pixel-simd/src/webp/mod.rs`; `fancy` in `crates/pixel-simd/src/webp/rows.rs`                                                           |
| libwebp SSE2 upsampler and YUV to RGB  | same @ `b43b2caa710c`: `src/dsp/upsampling_sse2.c`, `src/dsp/yuv_sse2.c`                                                                                | `crates/pixel-simd/src/webp/x86.rs`, `row_pair` in `webp/rows.rs`; `webp/wasm.rs` is the same code in simd128 and the AVX2 code is SSE2 widened (neither has an upstream file) |
| libwebp NEON upsampler and YUV to RGB  | same @ `b43b2caa710c`: `src/dsp/upsampling_neon.c`                                                                                                      | `crates/pixel-simd/src/webp/neon.rs` (16 lanes instead of 8)                                                                                                                   |
| Skia SkFrameHolder alpha rule (GIF)    | `src/third_party/skia` @ `29ed1e87a0a5`: `src/codec/SkCodec.cpp` `setAlphaAndRequiredFrame`                                                             | `frame_alpha` in `src/webcodecs/image_hints.rs`                                                                                                                                |
| image-rs BMP alpha rule                | `third_party/rust/chromium_crates_io/vendor/image-v0_25` (0.25.10) @ 155.0.8059.79: `src/codecs/bmp/decoder.rs`                                         | `bmp_header` in `src/webcodecs/image_hints.rs`                                                                                                                                 |
| image crate BMP bitfield scaling       | `third_party/rust/chromium_crates_io/vendor/image-v0_25` (0.25.10) @ 155.0.8059.79: `src/codecs/bmp/decoder.rs` `Bitfield::from_mask`, `Bitfield::read` | `Bitfield` in `crates/pixel-simd/src/lib.rs`                                                                                                                                   |