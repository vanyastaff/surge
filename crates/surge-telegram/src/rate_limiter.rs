//! Cockpit rate limiter — token-bucket per chat plus a time-refilled
//! global bucket.
//!
//! Decision 9 of the Telegram cockpit milestone plan pins the limits:
//!
//! - Per-chat: sustained `1 update/sec`, burst `5`.
//! - Global: `25 updates/sec` across all chats.
//!
//! Both limits use the same [`BucketState`] implementation —
//! capacity-bounded token bucket with continuous refill — so the
//! global ceiling is genuinely a *rate* (updates per second), not a
//! concurrency cap. Fast-completing calls cannot exceed the global
//! quota by recycling permits, because tokens only return at the
//! refill rate.
//!
//! [`CockpitRateLimiter::acquire`] parks until BOTH buckets have a
//! token, then returns a [`RateLimitToken`] marker. The token is
//! `#[must_use]` for API ergonomics but holds no resources — drop
//! semantics are a no-op.
//!
//! `RequestError::RetryAfter` handling lives in the emit code, not here —
//! the limiter only protects against self-inflicted bursts.

use std::collections::HashMap;

use tokio::sync::Mutex;
use tokio::time::{Duration, Instant};

/// Default per-chat burst capacity. Set to `5` so a small flurry of edits
/// for the same card (e.g. status updates + completion) lands without
/// throttling.
pub const DEFAULT_PER_CHAT_BURST: u32 = 5;

/// Default per-chat sustained refill, in tokens per second. Telegram's
/// documented soft cap is `1 msg/sec/chat`; we stay on that line.
pub const DEFAULT_PER_CHAT_REFILL_PER_SEC: u32 = 1;

/// Default global ceiling — updates per second across all chats.
/// Telegram's documented soft cap is `~30/sec`; we leave a safety
/// margin of 5. Used as both the bucket capacity (burst) and the
/// per-second refill rate (sustained), so the limiter genuinely caps
/// the rate (not just concurrent in-flight calls).
pub const DEFAULT_GLOBAL_LIMIT: u32 = 25;

/// Internal state of one chat's token bucket.
#[derive(Debug)]
struct BucketState {
    available: u32,
    capacity: u32,
    refill_per_sec: u32,
    last_refill: Instant,
}

impl BucketState {
    fn fresh(now: Instant, capacity: u32, refill_per_sec: u32) -> Self {
        Self {
            available: capacity,
            capacity,
            refill_per_sec,
            last_refill: now,
        }
    }

    /// Refill the bucket based on elapsed time since [`Self::last_refill`].
    ///
    /// Tokens are added as `elapsed_seconds * refill_per_sec`, then capped
    /// at [`Self::capacity`]. Sub-token elapsed time stays "owed" — we
    /// only advance `last_refill` when at least one token landed, so a
    /// caller that polls many times per second does not lose fractional
    /// progress.
    fn refill(&mut self, now: Instant) {
        if self.refill_per_sec == 0 {
            return;
        }
        let elapsed = now.duration_since(self.last_refill);
        let new_tokens = elapsed.as_secs_f64() * f64::from(self.refill_per_sec);
        let whole = new_tokens.floor() as u32;
        if whole > 0 {
            self.available = (self.available + whole).min(self.capacity);
            // Advance `last_refill` by exactly the whole-token portion so
            // the fractional remainder accrues toward the next token.
            let consumed_secs = f64::from(whole) / f64::from(self.refill_per_sec);
            let consumed = Duration::from_secs_f64(consumed_secs);
            self.last_refill += consumed;
        }
    }

    /// Try to consume one token. Returns `Ok(())` on success and
    /// `Err(wait_for_next_token)` when the bucket is empty.
    fn try_consume(&mut self, now: Instant) -> Result<(), Duration> {
        self.refill(now);
        if self.available > 0 {
            self.available -= 1;
            return Ok(());
        }
        // Bucket is empty — compute time until the next whole token.
        let elapsed_since_last = now.duration_since(self.last_refill);
        let period_per_token = Duration::from_secs_f64(1.0 / f64::from(self.refill_per_sec));
        let wait = period_per_token.saturating_sub(elapsed_since_last);
        // Never sleep zero — give the runtime at least 1ms to advance the
        // wall clock under tokio::time::advance.
        Err(wait.max(Duration::from_millis(1)))
    }
}

/// Rate limiter shared across every cockpit emit / Bot API call.
pub struct CockpitRateLimiter {
    per_chat: Mutex<HashMap<i64, BucketState>>,
    global: Mutex<BucketState>,
    capacity: u32,
    refill_per_sec: u32,
}

impl Default for CockpitRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl CockpitRateLimiter {
    /// Construct a limiter with the project defaults
    /// ([`DEFAULT_PER_CHAT_BURST`], [`DEFAULT_PER_CHAT_REFILL_PER_SEC`],
    /// [`DEFAULT_GLOBAL_LIMIT`]).
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_PER_CHAT_BURST,
            DEFAULT_PER_CHAT_REFILL_PER_SEC,
            DEFAULT_GLOBAL_LIMIT,
        )
    }

    /// Construct a limiter with custom limits.
    ///
    /// `global_per_sec` is the sustained rate AND the burst capacity —
    /// the global bucket starts full and refills at the same rate per
    /// second. This is genuinely a rate cap (not a concurrency cap):
    /// fast-completing calls cannot exceed the per-second budget by
    /// recycling permits.
    #[must_use]
    pub fn with_limits(
        per_chat_burst: u32,
        per_chat_refill_per_sec: u32,
        global_per_sec: u32,
    ) -> Self {
        let now = Instant::now();
        Self {
            per_chat: Mutex::new(HashMap::new()),
            global: Mutex::new(BucketState::fresh(now, global_per_sec, global_per_sec)),
            capacity: per_chat_burst,
            refill_per_sec: per_chat_refill_per_sec,
        }
    }

    /// Acquire a rate-limit permit for `chat_id`. Parks until BOTH the
    /// global bucket and the per-chat bucket have a token available.
    ///
    /// The returned [`RateLimitToken`] is a `#[must_use]` marker —
    /// dropping it is a no-op. Tokens are time-based, not
    /// connection-pool-based, so there is nothing to release.
    pub async fn acquire(&self, chat_id: i64) -> RateLimitToken {
        // Park until the global bucket has a token. Doing this first
        // means a saturated global rate cannot be bypassed by a chat
        // whose per-chat bucket is full.
        loop {
            let wait = {
                let now = Instant::now();
                let mut g = self.global.lock().await;
                g.try_consume(now)
            };
            match wait {
                Ok(()) => break,
                Err(d) => {
                    tracing::debug!(
                        target: "telegram::rate",
                        kind = "global",
                        wait_ms = %d.as_millis(),
                        "rate-limit park",
                    );
                    tokio::time::sleep(d).await;
                },
            }
        }

        loop {
            let wait = {
                let now = Instant::now();
                let mut map = self.per_chat.lock().await;
                let bucket = map
                    .entry(chat_id)
                    .or_insert_with(|| BucketState::fresh(now, self.capacity, self.refill_per_sec));
                bucket.try_consume(now)
            };
            match wait {
                Ok(()) => break,
                Err(d) => {
                    tracing::debug!(
                        target: "telegram::rate",
                        kind = "per_chat",
                        chat_id = %chat_id,
                        wait_ms = %d.as_millis(),
                        "rate-limit park",
                    );
                    tokio::time::sleep(d).await;
                },
            }
        }

        tracing::debug!(
            target: "telegram::rate",
            chat_id = %chat_id,
            "rate-limit acquired",
        );

        RateLimitToken {}
    }

    /// Currently-available tokens in the global bucket. Useful for
    /// telemetry; returns the snapshot value without refilling.
    pub async fn global_available(&self) -> u32 {
        self.global.lock().await.available
    }
}

/// Marker returned by [`CockpitRateLimiter::acquire`].
///
/// Both the global and per-chat tokens are consumed at acquire time
/// and refill on a clock — there is no permit to release, so dropping
/// this value is a no-op. The `#[must_use]` keeps callers honest about
/// the intent of binding the token for the lifetime of the Bot API
/// call.
#[must_use = "the rate-limit token must outlive the API call it gates"]
pub struct RateLimitToken {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration as StdDuration;

    #[tokio::test(start_paused = true)]
    async fn fresh_limiter_admits_first_call_without_park() {
        let limiter = CockpitRateLimiter::new();
        // No park expected, so the future resolves immediately under
        // paused time.
        let _token = limiter.acquire(42).await;
        // Global bucket consumed one of its DEFAULT_GLOBAL_LIMIT tokens.
        assert_eq!(limiter.global_available().await, DEFAULT_GLOBAL_LIMIT - 1,);
    }

    #[tokio::test(start_paused = true)]
    async fn burst_admits_capacity_calls_back_to_back() {
        let limiter = CockpitRateLimiter::with_limits(5, 1, 25);
        let mut tokens = Vec::new();
        for _ in 0..5 {
            tokens.push(limiter.acquire(42).await);
        }
        assert_eq!(tokens.len(), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn sixth_call_parks_until_bucket_refills() {
        let limiter = Arc::new(CockpitRateLimiter::with_limits(5, 1, 25));

        // Burn the per-chat burst of 5.
        let mut held = Vec::new();
        for _ in 0..5 {
            held.push(limiter.acquire(42).await);
        }

        // The 6th call must park on the per-chat bucket. Run it
        // concurrently and advance time.
        let limiter_clone = Arc::clone(&limiter);
        let park_task = tokio::spawn(async move { limiter_clone.acquire(42).await });

        // Yield once so the task starts and lands in the sleep.
        tokio::task::yield_now().await;
        assert!(
            !park_task.is_finished(),
            "6th acquire must park before the per-chat bucket refills"
        );

        // Advance virtual time by 1s. With per-chat refill_per_sec=1
        // the bucket gains one token, unblocking the parked call.
        tokio::time::advance(StdDuration::from_secs(1)).await;
        tokio::task::yield_now().await;
        let _token6 = park_task.await.expect("park task finishes after refill");
        drop(held);
    }

    #[tokio::test(start_paused = true)]
    async fn per_chat_buckets_are_independent() {
        // Per-chat bucket of 1 with refill 1/sec; global of 25/sec.
        // Two calls to two different chats both land instantly.
        let limiter = CockpitRateLimiter::with_limits(1, 1, 25);
        let _a = limiter.acquire(42).await;
        let _b = limiter.acquire(99).await;
    }

    #[tokio::test(start_paused = true)]
    async fn global_rate_throttles_burst_across_chats() {
        // Global of 2/sec, per-chat of 10/sec — so the global is the
        // binding constraint. Two calls land instantly (drain the
        // bucket); the third must park until the bucket refills.
        let limiter = Arc::new(CockpitRateLimiter::with_limits(10, 10, 2));
        let _a = limiter.acquire(1).await;
        let _b = limiter.acquire(2).await;
        assert_eq!(limiter.global_available().await, 0);

        let limiter_clone = Arc::clone(&limiter);
        let third = tokio::spawn(async move { limiter_clone.acquire(3).await });
        tokio::task::yield_now().await;
        assert!(
            !third.is_finished(),
            "third acquire must park while the global bucket is drained"
        );

        // Advance virtual time by 1s — global refill of 2/sec adds
        // two tokens, unblocking the parked call.
        tokio::time::advance(StdDuration::from_secs(1)).await;
        tokio::task::yield_now().await;
        let _t3 = third
            .await
            .expect("third acquire resolves once the global bucket refills");
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_token_does_not_return_capacity() {
        // Time-based limiter: dropping a token is a no-op. The
        // bucket only refills on the clock. This is intentional —
        // see the module docstring.
        let limiter = CockpitRateLimiter::with_limits(1, 1, 2);
        let t1 = limiter.acquire(42).await;
        let t2 = limiter.acquire(99).await;
        assert_eq!(limiter.global_available().await, 0);
        drop(t1);
        drop(t2);
        // Still zero — the bucket only refills on the clock, not on
        // permit drop.
        assert_eq!(limiter.global_available().await, 0);
    }

    #[test]
    fn bucket_refill_caps_at_capacity() {
        let start = Instant::now();
        let mut bucket = BucketState::fresh(start, 5, 1);
        bucket.available = 0;
        // Advance by an enormous interval — refill must still cap at 5.
        let later = start + Duration::from_secs(3600);
        bucket.refill(later);
        assert_eq!(bucket.available, 5);
    }

    #[test]
    fn bucket_refill_with_zero_rate_is_a_noop() {
        let start = Instant::now();
        let mut bucket = BucketState::fresh(start, 5, 0);
        bucket.available = 0;
        let later = start + Duration::from_secs(3600);
        bucket.refill(later);
        assert_eq!(bucket.available, 0);
    }
}
