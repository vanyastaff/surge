//! Shared host ownership safety primitives, without transport or persistence dependencies.
pub mod owner_panic;

/// Atomic native Windows child admission and retained job ownership.
#[cfg(windows)]
pub mod windows_job;
