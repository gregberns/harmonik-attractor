//! How long a rate-limited Claude attempt waits before its next spawn
//! (design §5), and the clock it waits on. [`wait_for`] is pure; the clock
//! is injected so tests never wait for real.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use crate::parse::RateLimit;

/// The shortest wait, so nothing can spin.
pub const MIN_WAIT: Duration = Duration::from_secs(1);

/// The first backoff when the agent gave no reset time; it doubles on each
/// further spawn.
pub const BACKOFF: Duration = Duration::from_secs(5);

/// How long to wait after spawn `spawn` (from 1) was rate limited, at
/// `now` (Unix seconds), with `remaining` left of the window; `None` when
/// the wait doesn't fit (stop and fail):
/// - `resets_at` in the future: until then;
/// - `resets_at` now or past (the limit has already reset): [`MIN_WAIT`];
/// - no `resets_at`: [`BACKOFF`] `* 2^(spawn - 1)`;
///
/// never less than [`MIN_WAIT`].
pub fn wait_for(
    rate_limit: &RateLimit,
    now: u64,
    spawn: u32,
    remaining: Duration,
) -> Option<Duration> {
    let wait = match rate_limit.resets_at {
        Some(resets_at) => Duration::from_secs(resets_at.saturating_sub(now)),
        None => {
            let doublings = spawn.saturating_sub(1).min(16);
            BACKOFF.saturating_mul(1 << doublings)
        }
    }
    .max(MIN_WAIT);
    (wait <= remaining).then_some(wait)
}

/// Wall-clock time and waiting, injected into the `claude-p` handler.
#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    async fn sleep(&self, duration: Duration);
}

/// The system clock and `tokio::time::sleep`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Seconds since the Unix epoch at `time` (0 before it).
pub fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(resets_at: Option<u64>) -> RateLimit {
        RateLimit {
            resets_at,
            text: "rate limited".into(),
        }
    }

    const WINDOW: Duration = Duration::from_secs(120);

    #[test]
    fn a_future_reset_is_waited_for() {
        assert_eq!(
            wait_for(&limit(Some(1030)), 1000, 1, WINDOW),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn a_reset_now_or_past_waits_the_minimum() {
        assert_eq!(
            wait_for(&limit(Some(1000)), 1000, 1, WINDOW),
            Some(MIN_WAIT)
        );
        assert_eq!(wait_for(&limit(Some(900)), 1000, 3, WINDOW), Some(MIN_WAIT));
    }

    #[test]
    fn no_reset_time_backs_off() {
        let wait = |spawn| wait_for(&limit(None), 1000, spawn, WINDOW);
        assert_eq!(wait(1), Some(Duration::from_secs(5)));
        assert_eq!(wait(2), Some(Duration::from_secs(10)));
        assert_eq!(wait(3), Some(Duration::from_secs(20)));
    }

    #[test]
    fn a_wait_that_does_not_fit_the_window_fails() {
        assert_eq!(
            wait_for(&limit(Some(1200)), 1000, 1, Duration::from_secs(199)),
            None
        );
        assert_eq!(
            wait_for(&limit(Some(1000)), 1000, 1, Duration::from_millis(999)),
            None
        );
        assert_eq!(wait_for(&limit(None), 1000, 1, Duration::ZERO), None);
    }

    #[test]
    fn a_huge_backoff_saturates() {
        assert_eq!(
            wait_for(&limit(None), 0, u32::MAX, Duration::MAX).map(|d| d >= BACKOFF),
            Some(true)
        );
    }
}
