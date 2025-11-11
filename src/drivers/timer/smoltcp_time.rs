//! Time adapter for smoltcp

use smoltcp::time::{Duration, Instant};

use super::*;

/// Get current time as smoltcp Instant
pub fn now() -> Instant {
    let nanos = ticks_to_nanos(current_ticks());
    let millis = (nanos / 1_000_000) as i64;
    Instant::from_millis(millis)
}

/// Create a Duration from milliseconds
pub fn duration_from_millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}
