# Licenses and binary distribution

The original source code of `@napi-rs/webcodecs` is licensed under the MIT
license in [LICENSE](LICENSE). That license and its copyright notice remain
unchanged. It does not replace the licenses of bundled third-party code.

## Native packages

The `@napi-rs/webcodecs-*` platform packages contain native addons that statically
link FFmpeg and codec libraries. The checked-in build recipes enable
`--enable-gpl`, `--enable-version3`, `--enable-libx264`, and `--enable-libx265`.
FFmpeg describes this configuration as GPL version 3 or later; see its
[licensing explanation](licenses/ffmpeg-LICENSE.md.txt) and
[GPLv3 text](licenses/ffmpeg-COPYING.GPLv3.txt).

These addons cannot be described as MIT-only distributions. Distribution of
the combined GPL-covered work must satisfy GPLv3, including the applicable
Corresponding Source requirements. The original MIT grant for this project's
own source remains available; it does not provide an exception to those
requirements for the combined binaries. The root JavaScript package selects
these addons through optional dependencies, so installing it normally also
installs a native package with these obligations.

The npm `license` field points here to distinguish the original source license
from the conditions on the binary distribution. It is not a dual-license offer
allowing a recipient to select MIT for the bundled native addon.

## Third-party material and source status

[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) lists the configured dependencies
and reference license texts. Preserve their copyright, license, patent, and
other required notices when redistributing them. See
[SOURCE_DISTRIBUTION.md](SOURCE_DISTRIBUTION.md) for the source-material gaps
identified in the 1.4.2 distribution and the work required before the next
binary release. The repository URL and these notice files alone are not a
complete Corresponding Source distribution or a written source offer.

This notice correction does not certify compliance of existing releases,
change codec features, or relicense the project's original source. Custom
builds require an inventory of the libraries actually linked into them.