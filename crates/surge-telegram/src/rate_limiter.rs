//! Cockpit rate limiter — token-bucket per chat with a global ceiling.
//!
//! Decision 9 of the Telegram cockpit milestone plan pins the limits:
//!
//! - Per-chat: sustained `1 update/sec`, burst `5`.
//! - Global: `25 updates/sec` across all chats.
//!
//! [`CockpitRateLimiter::acquire`] returns a [`RateLimitToken`] that holds
//! a permit on the global semaphore for its lifetime; dropping the token
//! releases the permit. Per-chat throttling is enforced inside `acquire`
//! by parking until the chat's bucket has refilled to at least one
//! token.
//!
//! `RequestError::RetryAfter` handling lives in the emit code, not here —
//! the limiter only protects against self-inflicted bursts.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::{Duration, Instant};

/// Default per-chat burst capacity. Set to `5` so a small flurry of edits
/// for the same card (e.g. status updates + completion) lands without
/// throttling.
pub const DEFAULT_PER_CHAT_BURST: u32 = 5;

/// Default per-chat sustained refill, in tokens per second. Telegram's
/// documented soft cap is `1 msg/sec/chat`; we stay on that line.
pub const DEFAULT_PER_CHAT_REFILL_PER_SEC: u32 = 1;

/// Default global concurrent ceiling. Telegram's documented soft cap is
/// `~30/sec` across all chats; we leave a safety margin of 5.
pub const DEFAULT_GLOBAL_LIMIT: usize = 25;

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
    global: Arc<Semaphore>,
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

    /// Construct a limiter with custom limits. Used by tests and the
    /// daemon's config-driven path.
    #[must_use]
    pub fn with_limits(
        per_chat_burst: u32,
        per_chat_refill_per_sec: u32,
        global_limit: usize,
    ) -> Self {
        Self {
            per_chat: Mutex::new(HashMap::new()),
            global: Arc::new(Semaphore::new(global_limit)),
            capacity: per_chat_burst,
            refill_per_sec: per_chat_refill_per_sec,
        }
    }

    /// Acquire a rate-limit permit for `chat_id`. Blocks until both the
    /// per-chat bucket and the global ceiling allow the call.
    ///
    /// The returned [`RateLimitToken`] must outlive the Bot API call it
    /// gates. Dropping the token releases the global permit immediately;
    /// the per-chat token is consumed (not held) so concurrent calls for
    /// the same chat compete for the bucket, not for the token.
    pub async fn acquire(&self, chat_id: i64) -> RateLimitToken {
        // Hold the global permit before parking on the per-chat bucket so
        // the global ceiling is always observed.
        let permit = self
            .global
            .clone()
            .acquire_owned()
            .await
            .expect("global semaphore is never closed");

        loop {
            let wait = {
                let now = Instant::now();
                let mut map = self.per_chat.lock().await;
                let bucket = map
                    .entry(chat_id)
                    .or_insert_with(|| BucketState::fresh(now, self.capacity, self.refill_per_sec));
                match bucket.try_consume(now) {
                    Ok(()) => None,
                    Err(d) => Some(d),
                }
            };
            match wait {
                None => break,
                Some(d) => {
                    tracing::debug!(
                        target: "telegram::rate",
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

        RateLimitToken { _permit: permit }
    }

    /// Currently-available permits on the global semaphore. Useful for
    /// metrics; the test suite uses this to assert the global counter
    /// rebounds after a token drop.
    #[must_use]
    pub fn global_available(&self) -> usize {
        self.global.available_permits()
    }
}

/// RAII guard returned by [`CockpitRateLimiter::acquire`].
///
/// While alive, the holder owns one permit on the global semaphore. Drop
/// releases the permit. The per-chat token has been consumed at acquire
/// time; it is not held by this guard.
#[must_use = "the rate-limit token must outlive the API call it gates"]
pub struct RateLimitToken {
    _permit: OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    #[tokio::test(start_paused = true)]
    async fn fresh_limiter_admits_first_call_without_park() {
        let limiter = CockpitRateLimiter::new();
        // No park expected, so the future resolves immediately under
        // paused time.
        let _token = limiter.acquire(42).await;
        // Global permits dropped by one.
        assert_eq!(limiter.global_available(), DEFAULT_GLOBAL_LIMIT - 1);
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

        // Burn the burst of 5.
        let mut held = Vec::new();
        for _ in 0..5 {
            held.push(limiter.acquire(42).await);
        }

        // The 6th call must park. Run it concurrently and advance time.
        let limiter_clone = Arc::clone(&limiter);
        let park_task = tokio::spawn(async move { limiter_clone.acquire(42).await });

        // Yield once so the task starts and lands in the sleep.
        tokio::task::yield_now().await;
        assert!(
            !park_task.is_finished(),
            "6th acquire must park before the bucket refills"
        );

        // Advance virtual time by 1s. With refill_per_sec=1 the bucket
        // gains one token, unblocking the parked call.
        tokio::time::advance(StdDuration::from_secs(1)).await;
        tokio::task::yield_now().await;
        let _token6 = park_task.await.expect("park task finishes after refill");
        drop(held);
    }

    #[tokio::test(start_paused = true)]
    async fn per_chat_buckets_are_independent() {
        let limiter = CockpitRateLimiter::with_limits(1, 1, 25);
        let _a = limiter.acquire(42).await;
        // Chat 99 has its own bucket — second acquire is instant.
        let _b = limiter.acquire(99).await;
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_token_returns_permit_to_global_semaphore() {
        let limiter = CockpitRateLimiter::with_limits(1, 1, 2);
        let t1 = limiter.acquire(42).await;
        let t2 = limiter.acquire(99).await;
        assert_eq!(limiter.global_available(), 0);
        drop(t1);
        assert_eq!(limiter.global_available(), 1);
        drop(t2);
        assert_eq!(limiter.global_available(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn global_ceiling_blocks_when_all_permits_held() {
        let limiter = Arc::new(CockpitRateLimiter::with_limits(10, 10, 2));
        let _a = limiter.acquire(1).await;
        let _b = limiter.acquire(2).await;
        assert_eq!(limiter.global_available(), 0);

        let limiter_clone = Arc::clone(&limiter);
        let third = tokio::spawn(async move { limiter_clone.acquire(3).await });
        tokio::task::yield_now().await;
        assert!(
            !third.is_finished(),
            "third acquire must park while the global ceiling is saturated"
        );

        drop(_a);
        tokio::task::yield_now().await;
        let _t3 = third
            .await
            .expect("third acquire resolves once a permit frees up");
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
