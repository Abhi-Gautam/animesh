//! Clock and jitter ports.
//!
//! Nothing in the engine reads the system clock directly. Plan 001 section 23
//! requires a fake wall clock that can jump independently of Tokio's monotonic
//! time, because schedule correctness depends on absolute instants while sleeps
//! depend on elapsed time, and tests must move those two independently.

use std::fmt;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::ids::UnixTimestamp;

/// Source of the current absolute UTC instant.
pub trait WallClock: fmt::Debug + Send + Sync {
    fn now(&self) -> UnixTimestamp;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl WallClock for SystemClock {
    fn now(&self) -> UnixTimestamp {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        // A clock before 1970 or beyond chrono's range is a broken machine, not
        // a condition worth propagating through every call site. Clamp to epoch
        // and let the health snapshot expose the resulting staleness.
        UnixTimestamp::new(seconds).unwrap_or(UnixTimestamp::EPOCH)
    }
}

/// A clock whose value is set explicitly by a test.
///
/// Available outside tests under `test-harness` so packaged end-to-end runs can
/// drive schedule transitions without waiting real hours.
#[derive(Debug)]
pub struct ManualClock {
    seconds: AtomicI64,
}

impl ManualClock {
    pub fn new(start: UnixTimestamp) -> Self {
        Self {
            seconds: AtomicI64::new(start.get()),
        }
    }

    pub fn advance(&self, seconds: i64) {
        self.seconds.fetch_add(seconds, Ordering::SeqCst);
    }

    pub fn set(&self, at: UnixTimestamp) {
        self.seconds.store(at.get(), Ordering::SeqCst);
    }
}

impl WallClock for ManualClock {
    fn now(&self) -> UnixTimestamp {
        let seconds = self.seconds.load(Ordering::SeqCst);
        UnixTimestamp::new(seconds).unwrap_or(UnixTimestamp::EPOCH)
    }
}

/// Source of bounded scheduling jitter.
///
/// Jitter is keyed and deterministic rather than random: a given media always
/// lands at the same offset within its refresh window, so a restart does not
/// reshuffle every deadline, but distinct media still spread across the window
/// instead of stampeding on the hour.
pub trait JitterSource: fmt::Debug + Send + Sync {
    /// Returns a value in `0..=max_seconds`, stable for a given `key`.
    fn jitter_secs(&self, key: i64, max_seconds: u32) -> u32;
}

/// Deterministic keyed jitter based on a SplitMix64 finalizer.
#[derive(Debug, Clone, Copy)]
pub struct KeyedJitter {
    seed: u64,
}

impl KeyedJitter {
    pub const fn new(seed: u64) -> Self {
        Self { seed }
    }
}

impl Default for KeyedJitter {
    fn default() -> Self {
        Self::new(0x9E37_79B9_7F4A_7C15)
    }
}

impl JitterSource for KeyedJitter {
    fn jitter_secs(&self, key: i64, max_seconds: u32) -> u32 {
        if max_seconds == 0 {
            return 0;
        }
        // SplitMix64 finalizer: cheap, dependency-free, and well-distributed
        // over the small consecutive integers that row identifiers actually are.
        let mut z = (key as u64).wrapping_add(self.seed);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z % (u64::from(max_seconds) + 1)) as u32
    }
}

/// Jitter source that always returns zero, for deterministic assertions.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoJitter;

impl JitterSource for NoJitter {
    fn jitter_secs(&self, _key: i64, _max_seconds: u32) -> u32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    #[test]
    fn system_clock_returns_a_plausible_instant() {
        // Sanity bound: after 2020-01-01 and before 2100-01-01.
        let now = SystemClock.now().get();
        assert!(now > 1_577_836_800, "clock reads {now}, before 2020");
        assert!(now < 4_102_444_800, "clock reads {now}, after 2100");
    }

    #[test]
    fn manual_clock_advances_only_when_told() {
        let clock = ManualClock::new(ts(1_000));
        assert_eq!(clock.now().get(), 1_000);
        clock.advance(500);
        assert_eq!(clock.now().get(), 1_500);
        clock.set(ts(42));
        assert_eq!(clock.now().get(), 42);
    }

    #[test]
    fn keyed_jitter_is_bounded() {
        let jitter = KeyedJitter::default();
        for key in 0..1_000 {
            let value = jitter.jitter_secs(key, 300);
            assert!(value <= 300, "key {key} produced {value}, above bound");
        }
    }

    #[test]
    fn keyed_jitter_is_stable_for_a_key() {
        // Stability across restarts is the whole point: a fresh process must not
        // reshuffle every refresh deadline.
        let a = KeyedJitter::default();
        let b = KeyedJitter::default();
        assert_eq!(a.jitter_secs(4242, 600), b.jitter_secs(4242, 600));
    }

    #[test]
    fn keyed_jitter_spreads_distinct_keys() {
        let jitter = KeyedJitter::default();
        let distinct: std::collections::HashSet<u32> =
            (0..200).map(|k| jitter.jitter_secs(k, 600)).collect();
        // Perfect uniformity is not required, but collapsing to a handful of
        // buckets would defeat the purpose.
        assert!(
            distinct.len() > 100,
            "200 keys produced only {} distinct offsets",
            distinct.len()
        );
    }

    #[test]
    fn zero_bound_yields_zero() {
        assert_eq!(KeyedJitter::default().jitter_secs(7, 0), 0);
        assert_eq!(NoJitter.jitter_secs(7, 600), 0);
    }
}
