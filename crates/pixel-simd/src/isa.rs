//! Which path a kernel runs.

/// An instruction set this CPU runs. Only this module makes one, after checking the CPU, so a
/// kernel may run the `#[target_feature]` code of any `Isa` it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Isa(Level);

/// The paths. Each SIMD level exists only on its arch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Level {
  Scalar,
  #[cfg(target_arch = "x86_64")]
  Sse2,
  #[cfg(target_arch = "x86_64")]
  Ssse3,
  /// AVX2 and SSSE3
  #[cfg(target_arch = "x86_64")]
  Avx2,
  #[cfg(target_arch = "aarch64")]
  Neon,
  #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
  Simd128,
}

impl Isa {
  pub(crate) fn level(self) -> Level {
    self.0
  }

  /// Every level this CPU runs, the best last. SSE2 and NEON are part of their targets, and
  /// simd128 of a wasm build made with it; std checks SSSE3 and AVX2 once and caches the result.
  pub(crate) fn supported() -> impl Iterator<Item = Isa> {
    #[cfg(target_arch = "x86_64")]
    let ssse3 = std::arch::is_x86_feature_detected!("ssse3");
    [
      Some(Level::Scalar),
      #[cfg(target_arch = "x86_64")]
      Some(Level::Sse2),
      #[cfg(target_arch = "x86_64")]
      ssse3.then_some(Level::Ssse3),
      #[cfg(target_arch = "x86_64")]
      (ssse3 && std::arch::is_x86_feature_detected!("avx2")).then_some(Level::Avx2),
      #[cfg(target_arch = "aarch64")]
      Some(Level::Neon),
      #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
      Some(Level::Simd128),
    ]
    .into_iter()
    .flatten()
    .map(Isa)
  }

  /// The best level this CPU runs.
  pub(crate) fn best() -> Isa {
    Isa::supported().last().unwrap_or(Isa(Level::Scalar))
  }
}
