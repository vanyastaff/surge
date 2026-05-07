//! Disk + bundled profile resolution for the orchestrator.
//!
//! `surge-core::profile::registry` owns the pure inheritance and merge
//! semantics. This module is the I/O-touching half:
//!
//! - [`paths::surge_home`] / [`paths::profiles_dir`] honour `SURGE_HOME`.
//! - [`disk::DiskProfileSet::scan`] walks `*.toml` under `profiles_dir()`
//!   and warns on per-file parse failures.
//! - [`registry::ProfileRegistry`] is the public accessor: `load`,
//!   `resolve`, `list`. Resolution order is **versioned → latest →
//!   bundled**, with version match canonical against
//!   `Profile.role.version` (filename is just a hint).
//!
//! Plumbed into the engine via `EngineConfig::profile_registry` and
//! consumed by the agent stage to derive `AgentKind` from
//! `runtime.agent_id` instead of the M5 mock fast-path.

pub mod disk;
pub mod paths;
pub mod registry;

pub use disk::DiskProfileSet;
pub use paths::{profiles_dir, surge_home};
pub use registry::ProfileRegistry;
