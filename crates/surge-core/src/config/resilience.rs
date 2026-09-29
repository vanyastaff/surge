//! Timeouts, retries, backoff and reconnection policy.

use super::*;

/// Backoff strategy for retry delays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackoffStrategy {
    /// Fixed delay between retries.
    Linear,
    /// Exponentially increasing delay (delay *= 2 each retry).
    #[default]
    Exponential,
    /// Exponential backoff with random jitter to avoid thundering herd.
    #[serde(rename = "exponential_jitter")]
    ExponentialWithJitter,
}

/// Retry policy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Initial delay before first retry (milliseconds).
    #[serde(default = "default_initial_delay_ms")]
    pub initial_delay_ms: u64,
    /// Maximum delay between retries (milliseconds).
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
    /// Backoff strategy for calculating delays.
    #[serde(default)]
    pub backoff_strategy: BackoffStrategy,
    /// Jitter factor for randomizing retry delays (0.0 = no jitter, 1.0 = full jitter).
    #[serde(default = "default_jitter_factor")]
    pub jitter_factor: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            initial_delay_ms: default_initial_delay_ms(),
            max_delay_ms: default_max_delay_ms(),
            backoff_strategy: BackoffStrategy::default(),
            jitter_factor: default_jitter_factor(),
        }
    }
}

/// 3 retries with exponential backoff covers transient network issues
/// without hammering a struggling API endpoint.
fn default_max_retries() -> u32 {
    3
}
/// 1 second initial delay — long enough for rate limit windows to reset,
/// short enough for good UX.
fn default_initial_delay_ms() -> u64 {
    1000
}
/// 60 second cap prevents unreasonably long waits during extended outages.
fn default_max_delay_ms() -> u64 {
    60000
}
fn default_jitter_factor() -> f64 {
    0.1
}

/// Resilience configuration for agent connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceConfig {
    /// Timeout for spawning and ACP-initializing an agent process (seconds).
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// Timeout for a single `new_session` ACP call (seconds).
    #[serde(default = "default_session_timeout_secs")]
    pub session_timeout_secs: u64,
    /// Timeout for a single `prompt` ACP call (seconds).
    #[serde(default = "default_prompt_timeout_secs")]
    pub prompt_timeout_secs: u64,
    /// How many times to retry a failed prompt before giving up.
    #[serde(default = "default_prompt_retries")]
    pub prompt_retries: u32,
    /// Seconds to wait for a process to exit cleanly before SIGKILL.
    #[serde(default = "default_shutdown_grace_secs")]
    pub shutdown_grace_secs: u64,
    /// Retry policy configuration with backoff strategies.
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// Number of consecutive failures before circuit breaker trips.
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    /// If true, auth failures (401) fail immediately without retry.
    #[serde(default = "default_auth_failure_immediate_fail")]
    pub auth_failure_immediate_fail: bool,
    /// Interval for heartbeat checks when agent has active tasks (seconds).
    #[serde(default = "default_heartbeat_interval_active_secs")]
    pub heartbeat_interval_active_secs: u64,
    /// Interval for heartbeat checks when agent is idle (seconds).
    #[serde(default = "default_heartbeat_interval_idle_secs")]
    pub heartbeat_interval_idle_secs: u64,
    /// Maximum number of reconnection attempts before giving up.
    #[serde(default = "default_reconnect_max_attempts")]
    pub reconnect_max_attempts: u32,
    /// Initial delay for reconnection attempts (milliseconds).
    #[serde(default = "default_reconnect_initial_delay_ms")]
    pub reconnect_initial_delay_ms: u64,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            connect_timeout_secs: default_connect_timeout_secs(),
            session_timeout_secs: default_session_timeout_secs(),
            prompt_timeout_secs: default_prompt_timeout_secs(),
            prompt_retries: default_prompt_retries(),
            shutdown_grace_secs: default_shutdown_grace_secs(),
            retry_policy: RetryPolicy::default(),
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            auth_failure_immediate_fail: default_auth_failure_immediate_fail(),
            heartbeat_interval_active_secs: default_heartbeat_interval_active_secs(),
            heartbeat_interval_idle_secs: default_heartbeat_interval_idle_secs(),
            reconnect_max_attempts: default_reconnect_max_attempts(),
            reconnect_initial_delay_ms: default_reconnect_initial_delay_ms(),
        }
    }
}

/// 120s connect timeout — agent processes (especially Claude Code) can take
/// 30-60s to spawn + initialize ACP. 2x safety margin.
fn default_connect_timeout_secs() -> u64 {
    120
}
/// 10s session timeout — ACP `new_session` is lightweight; >10s means something
/// is fundamentally broken, not slow.
fn default_session_timeout_secs() -> u64 {
    10
}
/// 5 minutes per prompt — complex coding tasks (large refactors) can take 2-3 min.
/// 5 min covers worst case with margin.
fn default_prompt_timeout_secs() -> u64 {
    300
}
fn default_prompt_retries() -> u32 {
    1
}
fn default_shutdown_grace_secs() -> u64 {
    5
}
/// Trip circuit breaker after 5 consecutive failures — enough to filter transient
/// errors but fast enough to stop cascading damage.
fn default_circuit_breaker_threshold() -> u32 {
    5
}
fn default_auth_failure_immediate_fail() -> bool {
    true
}
fn default_heartbeat_interval_active_secs() -> u64 {
    30
}
fn default_heartbeat_interval_idle_secs() -> u64 {
    300
}
fn default_reconnect_max_attempts() -> u32 {
    5
}
fn default_reconnect_initial_delay_ms() -> u64 {
    1000
}
