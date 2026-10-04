//! Host admission carried by queued ACP work and retained by actual sessions.

use std::sync::Arc;

/// The current host owner refuses a new provider effect.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("host execution control refuses provider effect")]
pub struct HostEffectRefused;

/// Runtime-only final admission, injected without a storage dependency.
///
/// A successful check admits only the immediately following request or spawn.
/// Already admitted work may complete while a later Stop is being settled.
/// Implementations retain the real owner's capability for the session lifetime.
pub trait HostEffectFence: Send + Sync {
    /// Inspect current ownership and control before one effect is initiated.
    fn check(&self) -> Result<(), HostEffectRefused>;
}

pub(crate) fn check(fence: Option<&Arc<dyn HostEffectFence>>) -> Result<(), HostEffectRefused> {
    fence.map_or(Ok(()), |fence| fence.check())
}

pub(crate) async fn admitted<T>(
    fence: Option<&Arc<dyn HostEffectFence>>,
    future: impl std::future::Future<Output = T>,
) -> Result<T, HostEffectRefused> {
    // This body is lazy. Validation and the SDK future's first poll occur in
    // the same poll, after any caller/worker admission queues have completed.
    check(fence)?;
    Ok(future.await)
}
