//! What PNG headers declare, read without FFmpeg (see `apng`, which reads them).
//!
//! Chrome 155 decodes PNG with Skia (the png crate) and labels each frame BGRA or BGRX by what
//! its header declares, not by its pixels. Some of it also changes bytes that FFmpeg decodes
//! differently (see `PixelHint`).

/// What a header says about pixels that FFmpeg decodes differently from Chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PixelHint {
  #[default]
  None,
  /// A 1, 2 or 4-bit gray PNG with a tRNS chunk: its transparent gray value scaled to 8 bits,
  /// or None when no sample can have it. FFmpeg drops it.
  GrayKey(Option<u8>),
}

/// A PNG header: whether Chrome decodes the image with alpha, and its `PixelHint`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ImageHeader {
  pub(crate) alpha: bool,
  pub(crate) pixels: PixelHint,
}
