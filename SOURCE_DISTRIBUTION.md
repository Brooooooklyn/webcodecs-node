# Corresponding Source: release work still required

Status: notice and packaging corrections only. No complete source bundle has
been reconstructed, verified, or published by this change. Do not treat the
presence of these files or a passing packaging check as release clearance.

## What was checked for 1.4.2

The `v1.4.2` tag resolves to
`3eb6525afb643baadcbe628f536e30265404d818`, also the root npm package's
`gitHead`. The registry tarballs for the root and all nine native packages were
downloaded and checked against their npm SHA-512 integrity values.

- The root tarball contains eight files, including the original MIT `LICENSE`.
- Each native tarball contains only `package.json`, `README.md`, and its `.node`
  binary. All ten manifests advertise `MIT`. The native packages contain no
  license, notice, or Corresponding Source files.
- x264-identifying strings are present in every native binary. This supports
  the build-recipe evidence but does not identify all linked revisions.
  The final addons did not retain a discoverable FFmpeg configure/license
  string in this inspection; no complete binary dependency attestation has
  been established.
- `build.rs` pins nine digests for the `ffmpeg-n9.0.2` static-library release.
  That release lists nine binary/install archives and no dedicated third-party
  source archive. Its automatic GitHub repository source snapshot contains the
  wrapper and build scripts, not the downloaded upstream source trees.
- The macOS arm64 and both Windows static-library archives were downloaded and
  their SHA-256 values verified against the release metadata. Their file lists
  contain headers/libraries but no license, copyright, notice, or source tree.
  Inspection of their `avutil` static libraries confirms `n9.0.2` and the
  `--enable-gpl`, `--enable-version3`, `--enable-libx264`, and
  `--enable-libx265` configure options. The other six static-library archives
  have not been inspected in this audit.

References: [npm root metadata](https://registry.npmjs.org/@napi-rs%2fwebcodecs/1.4.2),
[1.4.2 source](https://github.com/Brooooooklyn/webcodecs-node/tree/3eb6525afb643baadcbe628f536e30265404d818),
[FFmpeg static-library release](https://github.com/Brooooooklyn/webcodecs-node/releases/tag/ffmpeg-n9.0.2).

## Requirements to implement for the existing GPL build

[GPLv3](licenses/ffmpeg-COPYING.GPLv3.txt) sections 1, 4, 5, and 6 cover the
source and object-code distribution conditions. Section 1 covers the source
needed to generate, install, run, and modify the work, including the controlling
scripts; its System Library and general-purpose-tool exclusions require a
component-specific assessment. Preserve applicable notices and identify changes
and their dates. Distribution of the combined covered work must satisfy GPLv3
as a whole while retaining compatible upstream notices, including MIT.

For npm/network distribution, section 6(d) provides a route: equivalent access
to Corresponding Source at no extra charge, with clear directions beside the
object code. A different source server is permitted under that section, but
the distributor remains responsible for source availability. A generic link to
upstream or an unimplemented promise to provide source is insufficient. The
physical-product written-offer route in section 6(b) is not a generic substitute
for the npm download route. Installation Information may also be required when
the section 6 User Product conditions apply.

Before another release using this configuration:

1. Recover the actual source inputs for each old binary where available. Record
   package version, target, addon hash, FFmpeg archive hash, build run, source
   commits, tarball hashes, dependency lockfiles, and toolchain versions. Do not
   use today's dependency HEAD as evidence of the 1.4.2 inputs.
2. Retain all source trees required for the combined addon, including this
   project's Rust/C/JS sources, third-party code, relevant generated/interface
   inputs, and build/install scripts. Include transitive components unless a
   documented exclusion applies; a directory of static `.a`/`.lib` files and
   headers is not Corresponding Source.
3. Capture exact revisions and modifications. Unix x264 is unpinned; most
   dependency clones reuse an existing directory without revision validation.
   LAME's archive has no input checksum. The build script modifies x265 CMake
   policy settings and FFmpeg's generated `config.h` for Zig, emits compiler
   wrappers/toolchain files, and rewrites pkg-config files. Preserve those
   instructions and the actual patches/configuration, with change notices.
4. For Windows, retain the pinned vcpkg ports, resolved source archives and
   patches, triplets, feature selections, installed copyright files, and the
   source/build dependencies for the separately downloaded rav1e staticlib.
   Keep the x265 alpha/multilib build instructions as well.
5. Recover the release's resolved Cargo dependencies and toolchains where
   possible. For future releases, record lockfiles, immutable source identities,
   build environment and the actual linked library list alongside each binary.
   Byte-for-byte reproducibility is useful verification, not itself the GPL's
   definition of Corresponding Source.
6. Assemble the source materials and notices, test rebuilding each target from
   them, and resolve differences. Check upstream embedded subcomponent notices
   and runtime terms; the reference notice inventory is not complete evidence.
7. Choose and maintain a version-specific source distribution location. Put
   working links and checksums beside npm binaries and FFmpeg downloads and in
   every platform package; verify access without credentials. Do not invent
   source URLs or publish placeholders. Existing 1.4.2 tarballs cannot be fixed
   by editing the next version's package files alone.

## Maintainer decision

The original MIT license is preserved. Choose the future binary distribution
approach before publishing:

- Retain the GPL-enabled codecs and meet the combined-work GPLv3 distribution
  requirements, including complete corresponding-source delivery and notices.
- Design and rebuild a distribution without GPL components. This requires an
  actual codec/build/dependency review, removing GPL configure options and
  affected code, capability tests, and continued satisfaction of applicable
  LGPL and other dependency terms. It is not an MIT-only shortcut; static
  LGPL linking has its own requirements.

This patch makes neither choice and removes no codecs. It provides an
engineering inventory of the evidence and gaps, not a legal determination.