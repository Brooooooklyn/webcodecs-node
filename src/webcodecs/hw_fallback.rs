//! Hardware acceleration fallback tracking (Chromium-aligned)
//!
//! Implements global tracking of hardware encoder failures with
//! automatic fallback to software codecs after repeated failures.
//!
//! Behavior aligned with Chromium:
//! - After GLOBAL_FAILURE_THRESHOLD (3) failures, hardware encoding is disabled
//! - After FORGIVENESS_INTERVAL (60s), hardware encoding is re-enabled
//! - Success resets the failure count
//!
//! Note: Decoder always uses software by default for `no-preference` mode
//! due to FFmpeg hardware decoding reliability issues. Hardware decoding
//! is only used when explicitly requested via `prefer-hardware`.
//!
//! The state can be reset via `reset_hardware_fallback_state()` for testing
//! or error recovery scenarios.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use napi_derive::napi;

const GLOBAL_FAILURE_THRESHOLD: u32 = 3;
const FORGIVENESS_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
struct HwFallbackState {
  // Encoding state only - decoder uses software by default
  encoding_disabled: bool,
  encoding_failure_count: u32,
  encoding_disabled_at: Option<Instant>,
}

static HW_STATE: Mutex<HwFallbackState> = Mutex::new(HwFallbackState {
  encoding_disabled: false,
  encoding_failure_count: 0,
  encoding_disabled_at: None,
});

/// Reset all hardware fallback state.
///
/// This clears all failure counts and re-enables hardware acceleration.
/// Useful for:
/// - Test isolation (call in beforeEach)
/// - Error recovery after fixing hardware issues
/// - Manual reset by users
#[napi]
pub fn reset_hardware_fallback_state() {
  if let Ok(mut state) = HW_STATE.lock() {
    state.encoding_disabled = false;
    state.encoding_failure_count = 0;
    state.encoding_disabled_at = None;
  }
}

/// Check if hardware encoding is currently disabled due to failures.
/// Also handles time-based forgiveness.
pub fn is_hw_encoding_disabled() -> bool {
  if let Ok(mut state) = HW_STATE.lock() {
    if !state.encoding_disabled {
      return false;
    }

    // Check for forgiveness interval - measured from when encoding was disabled
    if let Some(disabled_at) = state.encoding_disabled_at
      && disabled_at.elapsed() >= FORGIVENESS_INTERVAL
    {
      // Re-enable hardware after forgiveness period
      state.encoding_disabled = false;
      state.encoding_failure_count = 0;
      state.encoding_disabled_at = None;
      return false;
    }

    true
  } else {
    // If mutex is poisoned, allow hardware (conservative default)
    false
  }
}

/// Record a hardware encoding failure.
/// After GLOBAL_FAILURE_THRESHOLD failures, hardware encoding is disabled.
pub fn record_hw_encoding_failure() {
  if let Ok(mut state) = HW_STATE.lock() {
    state.encoding_failure_count = state.encoding_failure_count.saturating_add(1);

    // Only set disabled_at when FIRST becoming disabled (fixes forgiveness timer)
    if state.encoding_failure_count >= GLOBAL_FAILURE_THRESHOLD && !state.encoding_disabled {
      state.encoding_disabled = true;
      state.encoding_disabled_at = Some(Instant::now());
    }
  }
}

/// Record a successful hardware encoding operation.
/// Resets the failure count.
pub fn record_hw_encoding_success() {
  if let Ok(mut state) = HW_STATE.lock() {
    state.encoding_failure_count = 0;
    // Don't clear disabled_at - only used when disabled
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn reset_state() {
    reset_hardware_fallback_state();
  }

  #[test]
  fn test_encoding_failure_threshold() {
    reset_state();

    assert!(!is_hw_encoding_disabled());

    // Record failures up to threshold
    for _ in 0..GLOBAL_FAILURE_THRESHOLD {
      record_hw_encoding_failure();
    }

    assert!(is_hw_encoding_disabled());
  }

  #[test]
  fn test_encoding_success_resets_count() {
    reset_state();

    // Record some failures (but not enough to disable)
    record_hw_encoding_failure();
    record_hw_encoding_failure();

    // Success should reset
    record_hw_encoding_success();

    // More failures needed to disable
    record_hw_encoding_failure();
    record_hw_encoding_failure();

    assert!(!is_hw_encoding_disabled());
  }

  #[test]
  fn test_reset_clears_state() {
    reset_state();

    // Disable encoding
    for _ in 0..GLOBAL_FAILURE_THRESHOLD {
      record_hw_encoding_failure();
    }

    assert!(is_hw_encoding_disabled());

    // Reset should clear everything
    reset_hardware_fallback_state();

    assert!(!is_hw_encoding_disabled());
  }

  #[test]
  fn test_additional_failures_dont_restart_timer() {
    reset_state();

    // Record failures to hit threshold
    for _ in 0..GLOBAL_FAILURE_THRESHOLD {
      record_hw_encoding_failure();
    }

    assert!(is_hw_encoding_disabled());

    // Get the disabled_at time
    let disabled_at = HW_STATE.lock().unwrap().encoding_disabled_at;
    assert!(disabled_at.is_some());

    // Additional failures should NOT update disabled_at
    record_hw_encoding_failure();
    record_hw_encoding_failure();

    let disabled_at_after = HW_STATE.lock().unwrap().encoding_disabled_at;
    assert_eq!(disabled_at, disabled_at_after);
  }

  /// Get encoding failure count (for testing only)
  #[allow(dead_code)]
  pub fn get_hw_encoding_failure_count() -> u32 {
    if let Ok(state) = HW_STATE.lock() {
      state.encoding_failure_count
    } else {
      0
    }
  }
}
