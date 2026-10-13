# Third-party notices

The native addon uses FFmpeg and the dependencies below. Version entries are
build-recipe references, not a verified bill of materials for each released
binary. All platforms receive this combined notice bundle; inclusion of a
license text does not mean that component is present on every platform.

Full reference texts, copyright notices, and patent notices are in `licenses/`.
[licenses/manifest.json](licenses/manifest.json) records where each text was
obtained and its SHA-256 digest. A source-file header excerpt is explicitly
marked there. Those digests identify notice files, not source archives or
binaries. Original project source remains under [MIT](LICENSE); see
[LICENSES.md](LICENSES.md) for the combined binary distribution.

## Configured dependencies

The Unix versions come from `tools/build-ffmpeg/src/main.rs`. Windows uses
vcpkg commit `74e6536215718009aae747d86d84b78376bf9e09` plus separately built
x265 and, on x64, a downloaded rav1e static library. The vcpkg baseline is pinned;
its port patches and actual resolved build inputs must also be retained.

| Component                  | Build reference                                                                                  | Reference terms and files in `licenses/`                                                                               |
| -------------------------- | ------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| FFmpeg                     | `n9.0.2`, all targets                                                                            | GPL-3.0-or-later for the configured build; `ffmpeg-LICENSE.md.txt`, `ffmpeg-COPYING.*.txt`                             |
| x264                       | Unix: unpinned shallow clone; Windows: `0.164.3108` / `31e19f92f00c7003fa115047ce50978bc98c3a0d` | GPL-2.0-or-later; `x264-COPYING.txt`, `x264-x264.h.txt`                                                                |
| x265                       | `Release_4.1`, multilib with alpha                                                               | GPL-2.0-or-later; `x265-COPYING.txt`, `x265-source-x265.h.txt`                                                         |
| libvpx                     | `v1.15.2`                                                                                        | BSD-3-Clause and patent grant; `libvpx-LICENSE.txt`, `libvpx-PATENTS.txt`                                              |
| libaom                     | `v3.13.1`; excluded from Windows x64 recipe                                                      | BSD-2-Clause and AOM patent license; `aom-LICENSE.txt`, `aom-PATENTS.txt`                                              |
| rav1e                      | `v0.8.1`, Windows x64 prebuilt archive                                                           | BSD-2-Clause and patent grant; `rav1e-LICENSE.txt`, `rav1e-PATENTS.txt`; transitive dependencies require inventory     |
| dav1d                      | Windows vcpkg `1.5.1`                                                                            | BSD-2-Clause; `dav1d-COPYING.txt`                                                                                      |
| Opus                       | `v1.5.2`                                                                                         | BSD-style notice and patent references; `opus-COPYING.txt`                                                             |
| LAME / libmp3lame / mpghip | `3.100`                                                                                          | GNU Library GPL v2 text and upstream guidance; `lame-COPYING.txt`, `lame-LICENSE.txt`; retain component-level headers  |
| libogg                     | Unix `v1.3.5`; Windows `v1.3.6`                                                                  | BSD-3-Clause; `ogg-COPYING.txt`, `ogg-windows-COPYING.txt`                                                             |
| libvorbis                  | `v1.3.7`                                                                                         | BSD-3-Clause; `vorbis-COPYING.txt`                                                                                     |
| libwebp / libsharpyuv      | `v1.6.0`                                                                                         | BSD-3-Clause and patent grant; `libwebp-COPYING.txt`, `libwebp-PATENTS.txt`                                            |
| zlib                       | `v1.3.1`                                                                                         | zlib license; `zlib-LICENSE.txt`                                                                                       |
| JPEG XL / libjxl           | `v0.11.1`                                                                                        | BSD-3-Clause and patent grant; `libjxl-LICENSE.txt`, `libjxl-PATENTS.txt`                                              |
| Highway                    | `1.3.0`                                                                                          | Apache-2.0 / BSD-3-Clause reference texts; `highway-LICENSE.txt`, `highway-LICENSE-BSD3.txt`; inspect file-level terms |
| Brotli                     | `v1.1.0`                                                                                         | MIT; `brotli-LICENSE.txt`                                                                                              |
| Little CMS 2               | Unix `lcms2.16`; Windows `lcms2.17`                                                              | MIT; `lcms2-LICENSE.txt`, `lcms2-windows-LICENSE.txt`                                                                  |
| NVIDIA codec headers       | Linux recipe `n13.0.19.0`, except armv7                                                          | MIT; `nv-codec-headers-nvEncodeAPI.h.txt`                                                                              |

This software is based in part on the work of the Independent JPEG Group.
FFmpeg's licensing explanation identifies IJG-derived codec files requiring
this attribution. The checked-in build recipe does not modify those files;
other builds must document any changes to them.

## Inventory still required for release

These reference texts are an initial notice inventory. They do not establish
that every upstream subcomponent notice or actual linked dependency has been
captured. In particular:

- Capture the exact x264 revision for every Unix build; the reference header in
  this bundle is identified by its own commit, not asserted to be the binary's.
- Preserve vcpkg port patches and installed `share/*/copyright` files, checking
  their contents against the exact source trees and transitive dependencies.
- Inventory Rust dependencies (including rav1e's), the Rust standard library,
  statically linked C/C++/compiler runtimes, and any platform-specific notices.
  `Cargo.lock` is currently ignored and release toolchains are not all pinned.
- `build.rs` searches local library paths and may link additional optional
  libraries. These custom builds need their own notice and source inventory.
- Resolve and publish the matching source materials described in
  [SOURCE_DISTRIBUTION.md](SOURCE_DISTRIBUTION.md).

Upstream entry points: [FFmpeg](https://ffmpeg.org/legal.html),
[x264](https://code.videolan.org/videolan/x264),
[x265](https://bitbucket.org/multicoreware/x265_git), and
[vcpkg baseline](https://github.com/microsoft/vcpkg/tree/74e6536215718009aae747d86d84b78376bf9e09/ports).