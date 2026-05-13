#![warn(missing_docs)]
//! Surge ACP bridge.
//!
//! Pure-addition submodule introduced in M3. Coexists with the legacy
//! `AgentPool` / `SurgeClient` stack at the crate root; consumers pick the
//! style they need. See `docs/ARCHITECTURE.md`
//! for the design contract.
//!
//! Public API surface (all under `bridge::`; **not** re-exported at the
//! `surge_acp` crate root, to avoid collision with the legacy
//! `connection::SessionState` / `pool::SessionHandle` types that already
//! occupy that namespace):
//!
//! - [`AcpBridge`] — owned by the engine, owns its own LocalSet thread.
//! - [`session::SessionConfig`], [`session::MessageContent`],
//!   [`session::SessionState`] — open-session inputs and read-back state.
//!   Distinct from legacy `surge_acp::SessionState`; access as `bridge::SessionState`.
//! - [`BridgeEvent`] — broadcast of everything observable about a session.
//! - [`Sandbox`] / [`SandboxDecision`] / [`AlwaysAllowSandbox`] / [`DenyListSandbox`]
//!   — interim M3 sandbox surface; M4 will add OS-level impls additively.
//! - [`ToolDef`] — shape of an injectable tool. `tools::build_report_stage_outcome_tool`
//!   and `tools::build_request_human_input_tool` produce the engine-injected pair.
//! - Errors: [`BridgeError`], [`OpenSessionError`], [`SendMessageError`],
//!   [`CloseSessionError`], [`AcpError`].
//!
//! Subscribers are warned in [`AcpBridge::subscribe`] that broadcast is
//! best-effort observability; durable consumers must add their own backpressure
//! (see spec §11.8).

// Submodules are wired in subsequent tasks.
pub mod error;
pub use error::{
    AcpError, BridgeError, CloseSessionError, OpenSessionError, ReplyToPermissionError,
    ReplyToToolError, SendMessageError,
};

pub mod event;
pub use event::{AgentMessageMeta, BridgeEvent, SessionEndReason, ToolCallMeta, ToolResultPayload};

// Forward stubs replaced in Phase 3 / Phase 4. Kept minimal so Phase 2 tests
// can exercise SessionConfig in isolation.

pub mod sandbox;
pub use sandbox::{AlwaysAllowSandbox, DenyListSandbox, Sandbox, SandboxDecision};

pub mod sandbox_resolver;
pub use sandbox_resolver::{ResolveContext, SandboxResolveError, resolve_launch_flags};

// Re-exports from the ACP SDK that downstream callers (engine, doctor,
// tests) need to construct `reply_to_permission` arguments.
pub use agent_client_protocol::{
    PermissionOptionId, RequestPermissionOutcome, RequestPermissionResponse,
    SelectedPermissionOutcome,
};

pub mod tools;
pub use tools::{ToolCategory, ToolDef};

pub mod session;
pub use session::{AgentKind, MessageContent, SessionConfig, SessionState, SessionStatus};

pub mod command;
pub use command::BridgeCommand;

pub(crate) mod worker;

pub(crate) mod session_inner;

pub(crate) mod client;

pub(crate) mod tokens;

pub mod acp_bridge;
pub use acp_bridge::AcpBridge;

pub mod facade;
pub use facade::BridgeFacade;

#[cfg(test)]
mod facade_smoke {
    use super::*;

    /// Compile-time assertion that `AcpBridge` implements `BridgeFacade`.
    /// Existence of the trait + impl is what we're locking in here; behavior
    /// is exercised in tests/facade_contract.rs.
    #[test]
    fn acp_bridge_impls_facade() {
        fn assert_impl<T: facade::BridgeFacade>() {}
        assert_impl::<AcpBridge>();
    }
}
