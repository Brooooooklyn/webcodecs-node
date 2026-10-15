---
name: sync-chrome-vendor
description: Use when moving the code that copies Chrome's image decoding to a newer Chrome stable (vendor/libjpeg-turbo, the Blink JPEG decoder port in src/ffi/jpeg_blink.c and the other ported Chrome rules), or to find which Chrome file and revision a ported piece follows.
---

# Sync the code that follows Chrome

Where Chrome and the spec differ, this project matches real Chrome. All the code in the
table below follows one Chrome stable version: the `chrome:` line of
`vendor/libjpeg-turbo/CHROMIUM_PIN`.

## Steps

1. Find the latest Chrome stable:

   ```sh
   curl -s 'https://versionhistory.googleapis.com/v1/chrome/platforms/mac/channels/stable/versions?pageSize=1'
   ```

2. Vendor the libjpeg-turbo that its DEPS pins (`src/third_party/libjpeg_turbo`):

   ```sh
   node tools/sync-libjpeg-turbo.mjs --chrome <version>
   ```

   It replaces `vendor/libjpeg-turbo` and the copies in `licenses/`, and prints every change.
   Without `--chrome` it re-syncs the pinned revision and prints `No changes.`. Never edit
   vendored files: put any difference in the `build.rs` flags or in `src/ffi/jpeg_blink.c`.

3. Diff these files between the pinned tag and the new one. Gitiles serves a file as base64
   with `https://chromium.googlesource.com/chromium/src/+/refs/tags/<version>/<path>?format=TEXT`
   (its log pages need a sign-in, so fetch both versions and diff them):
   - `third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.cc`
   - `third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_decoder_factory.cc`
   - `skia/rusty_jpeg_feature.cc`

   Port the changes to `src/ffi/jpeg_blink.c` (decode) and to `jpeg_scale_numerator` (size).
   If `kRustyJpegFeature` becomes enabled by default, or the factory picks the Rust decoder
   for the cases WebCodecs uses, Chrome no longer decodes JPEG with libjpeg-turbo: follow
   RustyJpeg instead. Check the other rows of the table the same way.

4. Rebuild and check:
   - `pnpm build`, then `cargo test --lib jpeg` (digests of Chrome's bytes for
     `__test__/fixtures/jpeg`)
   - `nm -g --defined-only` on `target/**/build/webcodecs-node-*/out/libjpeg_turbo.a`: every
     symbol starts with `webcodecs_` (`_webcodecs_` on macOS)
   - decode the fixtures in the new Chrome (`ImageDecoder.decode()`, then
     `VideoFrame.copyTo()`). Change a digest only when Chrome's bytes changed.

5. Update the versions in the table.

## Ported code that follows Chrome

Chrome 155.0.8059.79. Rows marked "labels branch" are on `fix/image-decoder-chrome-labels`.

| What                                   | Chromium file and revision                                                                                      | Where here                                                                 |
| -------------------------------------- | --------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| libjpeg-turbo, decoder files           | `src/third_party/libjpeg_turbo` = `chromium/deps/libjpeg_turbo` @ `640f254ad0fa`                                | `vendor/libjpeg-turbo`, `tools/sync-libjpeg-turbo.mjs`                     |
| Blink JPEG decode settings             | `third_party/blink/renderer/platform/image-decoders/jpeg/jpeg_image_decoder.cc` @ 155.0.8059.79                 | `src/ffi/jpeg_blink.c`, `src/codec/jpeg.rs`                                |
| JPEG M/8 scale rule                    | same file, `DesiredScaleNumerator` @ 155.0.8059.79                                                              | `jpeg_scale_numerator` in `src/webcodecs/image_decoder.rs` (labels branch) |
| libwebp fancy upsampler and YUV to RGB | `src/third_party/libwebp/src` @ `b43b2caa710c`: `src/dsp/upsampling.c`, `src/dec/io_dec.c`, `src/dsp/yuv.h`     | `webp_rgb` in `src/webcodecs/decoded_frame.rs` (labels branch)             |
| Skia SkFrameHolder alpha rule (GIF)    | `src/third_party/skia` @ `29ed1e87a0a5`: `src/codec/SkCodec.cpp` `setAlphaAndRequiredFrame`                     | `frame_alpha` in `src/webcodecs/image_hints.rs` (labels branch)            |
| image-rs BMP alpha rule                | `third_party/rust/chromium_crates_io/vendor/image-v0_25` (0.25.10) @ 155.0.8059.79: `src/codecs/bmp/decoder.rs` | `bmp_header` in `src/webcodecs/image_hints.rs` (labels branch)             |