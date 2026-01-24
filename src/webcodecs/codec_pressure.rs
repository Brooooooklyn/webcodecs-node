//! Codec Pressure Gauge - Global resource tracking for hardware encoders
//!
//! Tracks the number of active hardware encoder sessions to prevent resource
//! exhaustion. VideoToolbox on macOS has strict (undocumented) limits on
//! concurrent encoder sessions, causing failures when exceeded.
//!
//! This module provides automatic graceful degradation to software encoding
//! when hardware slots are exhausted, allowing tests to run in parallel safely.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

/// Global gauge for tracking codec resource pressure
pub struct CodecPressureGauge {
  /// Number of active hardware video encoder sessions
  hw_video_encoders: AtomicU32,
  /// Maximum allowed concurrent hardware encoders
  max_hw_encoders: u32,
}

impl CodecPressureGauge {
  /// Create a new pressure gauge with platform-specific limits
  const fn new() -> Self {
    // VideoToolbox on macOS is particularly restrictive (1-8 concurrent sessions)
    // Other platforms (NVENC, VAAPI, QSV) are generally more permissive
    #[cfg(target_os = "macos")]
    let max_hw_encoders = 2;
    #[cfg(not(target_os = "macos"))]
    let max_hw_encoders = 4;

    Self {
      hw_video_encoders: AtomicU32::new(0),
      max_hw_encoders,
    }
  }

  /// Try to acquire a hardware encoder slot
  ///
  /// Returns `true` if a slot was acquired, `false` if at capacity.
  /// The caller must call `release_hw_encoder()` when done if this returns `true`.
  pub fn try_acquire_hw_encoder(&self) -> bool {
    loop {
      let current = self.hw_video_encoders.load(Ordering::SeqCst);
      if current >= self.max_hw_encoders {
        return false;
      }
      if self
        .hw_video_encoders
        .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
      {
        tracing::debug!(
          target: "webcodecs",
          "Acquired hardware encoder slot ({}/{})",
          current + 1,
          self.max_hw_encoders
        );
        return true;
      }
      // CAS failed, another thread modified the counter - retry
    }
  }

  /// Release a hardware encoder slot
  ///
  /// Must be called exactly once for each successful `try_acquire_hw_encoder()` call.
  pub fn release_hw_encoder(&self) {
    let prev = self.hw_video_encoders.fetch_sub(1, Ordering::SeqCst);
    tracing::debug!(
      target: "webcodecs",
      "Released hardware encoder slot ({}/{})",
      prev - 1,
      self.max_hw_encoders
    );
  }
}

#[cfg(test)]
impl CodecPressureGauge {
  /// Get the current number of active hardware encoders (test-only)
  fn active_hw_encoders(&self) -> u32 {
    self.hw_video_encoders.load(Ordering::SeqCst)
  }

  /// Get the maximum number of hardware encoders allowed (test-only)
  fn max_hw_encoders(&self) -> u32 {
    self.max_hw_encoders
  }
}

/// Global pressure gauge instance
static GAUGE: OnceLock<CodecPressureGauge> = OnceLock::new();

/// Get the global codec pressure gauge
pub fn gauge() -> &'static CodecPressureGauge {
  GAUGE.get_or_init(CodecPressureGauge::new)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_acquire_release() {
    let gauge = CodecPressureGauge::new();
    assert_eq!(gauge.active_hw_encoders(), 0);

    // Acquire slots up to max
    for i in 0..gauge.max_hw_encoders() {
      assert!(gauge.try_acquire_hw_encoder());
      assert_eq!(gauge.active_hw_encoders(), i + 1);
    }

    // Should fail at capacity
    assert!(!gauge.try_acquire_hw_encoder());
    assert_eq!(gauge.active_hw_encoders(), gauge.max_hw_encoders());

    // Release one
    gauge.release_hw_encoder();
    assert_eq!(gauge.active_hw_encoders(), gauge.max_hw_encoders() - 1);

    // Can acquire again
    assert!(gauge.try_acquire_hw_encoder());
    assert_eq!(gauge.active_hw_encoders(), gauge.max_hw_encoders());

    // Release all
    for _ in 0..gauge.max_hw_encoders() {
      gauge.release_hw_encoder();
    }
    assert_eq!(gauge.active_hw_encoders(), 0);
  }
}
