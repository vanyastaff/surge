//! Vibe-flow ACP bridge.
//!
//! Pure-addition submodule introduced in M3. Coexists with the legacy
//! `AgentPool` / `SurgeClient` stack at the crate root; consumers pick the
//! style they need. See `docs/superpowers/specs/2026-05-03-surge-acp-bridge-m3-design.md`
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
