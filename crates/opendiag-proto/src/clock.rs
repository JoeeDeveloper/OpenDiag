//! Injectable time source so protocol timing is testable without sleeping.

use std::cell::Cell;
use std::time::{Duration, Instant};

/// Monotonic time as a `Duration` since an arbitrary origin.
pub trait Clock {
    fn now(&self) -> Duration;
    /// Block until `t`. Real clocks sleep; the fake clock jumps forward.
    fn wait_until(&self, t: Duration);
}

/// Wall-clock time for real hardware.
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn wait_until(&self, t: Duration) {
        let remaining = t.saturating_sub(self.now());
        if !remaining.is_zero() {
            std::thread::sleep(remaining);
        }
    }
}

/// Manually advanced clock for deterministic tests.
#[derive(Debug, Default)]
pub struct FakeClock {
    now: Cell<Duration>,
}

impl FakeClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance(&self, by: Duration) {
        self.now.set(self.now.get() + by);
    }

    /// Jump forward to `t`. Never moves backwards.
    pub fn advance_to(&self, t: Duration) {
        if t > self.now.get() {
            self.now.set(t);
        }
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Duration {
        self.now.get()
    }

    fn wait_until(&self, t: Duration) {
        self.advance_to(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_advances_and_never_goes_back() {
        let c = FakeClock::new();
        c.advance(Duration::from_millis(5));
        c.advance_to(Duration::from_millis(20));
        c.advance_to(Duration::from_millis(10));
        assert_eq!(c.now(), Duration::from_millis(20));
    }
}
