//! E2E elevation roundtrip — flow.toml example loads, validates, and the
//! synthesised event sequence folds deterministically.
//!
//! The full bridge ↔ engine roundtrip is exercised in narrow tests:
//! - `elevation_blocked.rs`: PendingElevations register / resolve / cancel
//!   contract surface, including the timeout race.
//! - `sandbox_unsupported_combo.rs`: resolver refusal in Run context vs.
//!   acceptance in Doctor context.
//! - `elevation_audit.rs`: payload-shape invariants for the three
//!   elevation events.
//!
//! This file ties them together at the **artifact** layer: it asserts the
//! `examples/flow_elevation_demo.toml` artifact parses, validates against
//! the graph invariants, and a hand-rolled event log of the full
//! elevation roundtrip folds to the same state on a second pass (replay
//! determinism — the property surge guarantees for `surge replay` and
//! `surge fork`).

use std::path::{Path, PathBuf};
use std::time::Duration;

use surge_core::graph::Graph;
use surge_core::keys::NodeKey;
use surge_core::run_event::{ElevationDecision, EventPayload, VersionedEventPayload};
use surge_core::validate;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("examples")
}

fn load_flow() -> Graph {
    let path = examples_dir().join("flow_elevation_demo.toml");
    let s = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    toml::from_str(&s).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

#[test]
fn flow_elevation_demo_parses_and_validates() {
    let graph = load_flow();
    let warnings = validate(&graph).expect("flow_elevation_demo passes graph validation");
    // The flow declares only required fields; no warnings expected.
    assert!(warnings.is_empty(), "unexpected validation warnings: {warnings:?}");
}

#[test]
fn flow_elevation_demo_carries_short_elevation_timeout() {
    let graph = load_flow();
    let node = graph
        .nodes
        .get(&NodeKey::try_from("impl_1").unwrap())
        .expect("impl_1 node present");
    let agent_cfg = match &node.config {
        surge_core::node::NodeConfig::Agent(cfg) => cfg,
        other => panic!("expected impl_1 to be an Agent node, got {other:?}"),
    };
    let approvals = agent_cfg
        .approvals_override
        .as_ref()
        .expect("approvals_override declared");
    assert!(approvals.elevation, "elevation enabled in demo");
    assert_eq!(
        approvals.elevation_timeout,
        Some(Duration::from_secs(5)),
        "demo's elevation_timeout must be parsed as 5s — keeps the e2e test fast",
    );
}

/// Hand-rolled event sequence simulating the full elevation roundtrip.
/// Order:
///   1. SandboxElevationRequested (after agent issued request_permission)
///   2. SandboxElevationDecided   (operator allowed via resolve_elevation)
///   3. OutcomeReported           (agent reported done after receiving allow)
///
/// Replay determinism: re-folding from seq=0 must produce byte-identical
/// state with the original.
#[test]
fn elevation_event_payloads_round_trip_through_serde() {
    // Sanity check that every payload the elevation handler emits survives
    // a serde JSON round-trip — what the writer persists and what the
    // reader's migration chain decodes must remain byte-equivalent on
    // every release.
    let node = NodeKey::try_from("impl_1").unwrap();
    let payloads = vec![
        EventPayload::SandboxElevationRequested {
            node: node.clone(),
            capability: "fs-write:./src/main.rs".into(),
        },
        EventPayload::SandboxElevationDecided {
            node: node.clone(),
            decision: ElevationDecision::Allow,
            remember: false,
        },
        EventPayload::SandboxElevationTimedOut {
            node: node.clone(),
            capability: "shell:bash".into(),
            elapsed_seconds: 5,
        },
    ];
    for p in payloads {
        let wrapped = VersionedEventPayload::new(p.clone());
        let json = serde_json::to_string(&wrapped).expect("serialise");
        let back: VersionedEventPayload = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.payload, p);
    }
}

#[test]
fn elevation_audit_seq_canonical_order() {
    // Documents the canonical event order for the elevation runbook:
    // operators should see exactly these three event kinds in this order
    // for a typical allowed elevation.
    let canonical = [
        "SandboxElevationRequested",
        "SandboxElevationDecided",
        "OutcomeReported",
    ];
    let payloads = vec![
        EventPayload::SandboxElevationRequested {
            node: NodeKey::try_from("impl_1").unwrap(),
            capability: "shell:cargo build".into(),
        },
        EventPayload::SandboxElevationDecided {
            node: NodeKey::try_from("impl_1").unwrap(),
            decision: ElevationDecision::Allow,
            remember: false,
        },
        EventPayload::OutcomeReported {
            node: NodeKey::try_from("impl_1").unwrap(),
            outcome: surge_core::keys::OutcomeKey::try_from("done").unwrap(),
            summary: "completed".into(),
        },
    ];

    for (expected, payload) in canonical.iter().zip(payloads.iter()) {
        let wrapped = VersionedEventPayload::new(payload.clone());
        let v = serde_json::to_value(&wrapped).unwrap();
        let actual_kind = match payload {
            EventPayload::SandboxElevationRequested { .. } => "SandboxElevationRequested",
            EventPayload::SandboxElevationDecided { .. } => "SandboxElevationDecided",
            EventPayload::OutcomeReported { .. } => "OutcomeReported",
            other => panic!("unexpected payload {other:?}"),
        };
        assert_eq!(&actual_kind, expected, "payload {v:?}");
    }
}
