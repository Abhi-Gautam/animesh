//! Injectable wall-clock.
//!
//! Every place that records `occurred_at`, `followed_at`, `created_at`
//! pulls the timestamp from a `Clock`. Production code uses
//! [`SystemClock`]; tests use [`FixedClock`] / [`AdvanceableClock`] so
//! assertions can fix the timeline without sleeping.
//!
//! Unix seconds — `i64` — to match the V0001+ SQLite schema. We do not
//! leak `chrono::DateTime` through this trait; conversion happens at
//! the call site that actually needs a human-readable form.

#[cfg(test)]
use std::sync::{Arc, Mutex};

/// Anything that can report the current Unix-epoch second.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

/// Real wall-clock. The only impl that calls `SystemTime::now()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            // System clocks pre-1970 are a category of broken we don't
            // try to recover from — just clamp to epoch.
            .unwrap_or(0)
    }
}

/// Test-only: a clock fixed at a single instant. Useful when an
/// assertion expects a specific `occurred_at` value.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}

/// Test-only: a clock that can be advanced manually between operations.
/// Used in tests that exercise sequencing (e.g. "follow then drop
/// later").
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct AdvanceableClock(Arc<Mutex<i64>>);

#[cfg(test)]
impl AdvanceableClock {
    pub fn new(start: i64) -> Self {
        Self(Arc::new(Mutex::new(start)))
    }

    /// Move the clock forward by `delta` seconds.
    pub fn advance(&self, delta: i64) {
        *self.0.lock().expect("clock lock poisoned") += delta;
    }

    /// Jump the clock to an absolute value.
    pub fn set(&self, t: i64) {
        *self.0.lock().expect("clock lock poisoned") = t;
    }
}

#[cfg(test)]
impl Clock for AdvanceableClock {
    fn now(&self) -> i64 {
        *self.0.lock().expect("clock lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_returns_a_positive_unix_second() {
        let t = SystemClock.now();
        // Anything after the year 2001 is fine; we just want a sanity
        // check that the clock isn't returning zero / negative.
        assert!(t > 1_000_000_000, "SystemClock returned {t}");
    }

    #[test]
    fn fixed_clock_returns_its_argument() {
        let c = FixedClock(42_000);
        assert_eq!(c.now(), 42_000);
        // Repeated calls are the same value.
        assert_eq!(c.now(), 42_000);
    }

    #[test]
    fn advanceable_clock_starts_at_init_value() {
        let c = AdvanceableClock::new(1000);
        assert_eq!(c.now(), 1000);
    }

    #[test]
    fn advanceable_clock_moves_forward_on_advance() {
        let c = AdvanceableClock::new(1000);
        c.advance(60);
        assert_eq!(c.now(), 1060);
        c.advance(60);
        assert_eq!(c.now(), 1120);
    }

    #[test]
    fn advanceable_clock_set_jumps_to_absolute() {
        let c = AdvanceableClock::new(1000);
        c.set(5000);
        assert_eq!(c.now(), 5000);
    }

    #[test]
    fn advanceable_clock_is_shared_via_clone() {
        // The handle is Arc-backed; cloning produces a second observer
        // of the same underlying timeline. Used by the sync loop where
        // both the engine and the verifier need to see the same `now`.
        let a = AdvanceableClock::new(0);
        let b = a.clone();
        a.advance(100);
        assert_eq!(b.now(), 100);
    }

    #[test]
    fn clock_is_object_safe() {
        // If this compiles, we can pass any clock through a trait
        // object — `&dyn Clock` is what every Library / engine call
        // takes so tests can swap in FixedClock without generics.
        let clocks: Vec<Box<dyn Clock>> = vec![
            Box::new(SystemClock),
            Box::new(FixedClock(0)),
            Box::new(AdvanceableClock::new(0)),
        ];
        for c in &clocks {
            let _ = c.now();
        }
    }
}
