//! Audit-trail contract test for elevation events.
//!
//! Asserts that every event the elevation roundtrip appends carries the
//! fields downstream consumers need:
//!   - `SandboxElevationRequested` → `node`, `capability`
//!   - `SandboxElevationDecided` → `node`, `decision`, `remember`
//!   - `SandboxElevationTimedOut` → `node`, `capability`, `elapsed_seconds`
//!
//! Also asserts that **no PII-shaped field leaks into the payload**: the
//! ACP `request_permission` request carries the full tool call (which can
//! include file contents, shell commands, etc.) but surge only records a
//! short capability label (`"fs-write:./src"`) — the raw arguments must
//! not appear in the persisted audit row.
//!
//! Formalises the roadmap deliverable
//! "every elevation request + decision recorded with command summary"
//! without introducing a separate audit log file.

use surge_core::keys::NodeKey;
use surge_core::run_event::{ElevationDecision, EventPayload, VersionedEventPayload};

fn node() -> NodeKey {
    NodeKey::try_from("implementer_1").expect("valid node key")
}

#[test]
fn elevation_requested_payload_shape() {
    let payload = EventPayload::SandboxElevationRequested {
        node: node(),
        capability: "fs-write:./src/main.rs".into(),
    };
    let wrapper = VersionedEventPayload::new(payload);
    let json = serde_json::to_value(&wrapper).unwrap();

    let inner = json["payload"].as_object().expect("payload is object");
    assert_eq!(inner["type"], "sandbox_elevation_requested");
    assert_eq!(inner["node"], "implementer_1");
    assert_eq!(inner["capability"], "fs-write:./src/main.rs");

    // PII / leak check: only the three structural fields. Any extra field
    // would indicate a regression where the bridge or engine started
    // appending tool arguments / prompts / etc into the audit payload.
    let allowed: std::collections::BTreeSet<&str> =
        ["type", "node", "capability"].into_iter().collect();
    for key in inner.keys() {
        assert!(
            allowed.contains(key.as_str()),
            "unexpected field `{key}` in SandboxElevationRequested payload — \
             audit row must stay structural; tool arguments / prompts MUST NOT leak"
        );
    }
}

#[test]
fn elevation_decided_payload_shape() {
    let payload = EventPayload::SandboxElevationDecided {
        node: node(),
        decision: ElevationDecision::Allow,
        remember: false,
    };
    let wrapper = VersionedEventPayload::new(payload);
    let json = serde_json::to_value(&wrapper).unwrap();

    let inner = json["payload"].as_object().expect("payload is object");
    assert_eq!(inner["type"], "sandbox_elevation_decided");
    assert_eq!(inner["node"], "implementer_1");
    // `decision` serializes as the enum's wire form (snake_case via
    // `#[serde(rename_all = "snake_case")]` on the inner enum).
    assert_eq!(inner["decision"].as_str().unwrap(), "allow");
    assert_eq!(inner["remember"], false);

    let allowed: std::collections::BTreeSet<&str> =
        ["type", "node", "decision", "remember"].into_iter().collect();
    for key in inner.keys() {
        assert!(
            allowed.contains(key.as_str()),
            "unexpected field `{key}` in SandboxElevationDecided payload"
        );
    }
}

#[test]
fn elevation_timed_out_payload_shape() {
    let payload = EventPayload::SandboxElevationTimedOut {
        node: node(),
        capability: "network:api.example.com".into(),
        elapsed_seconds: 86_400,
    };
    let wrapper = VersionedEventPayload::new(payload);
    let json = serde_json::to_value(&wrapper).unwrap();

    let inner = json["payload"].as_object().expect("payload is object");
    assert_eq!(inner["type"], "sandbox_elevation_timed_out");
    assert_eq!(inner["node"], "implementer_1");
    assert_eq!(inner["capability"], "network:api.example.com");
    assert_eq!(inner["elapsed_seconds"], 86_400);

    let allowed: std::collections::BTreeSet<&str> =
        ["type", "node", "capability", "elapsed_seconds"].into_iter().collect();
    for key in inner.keys() {
        assert!(
            allowed.contains(key.as_str()),
            "unexpected field `{key}` in SandboxElevationTimedOut payload"
        );
    }
}

#[test]
fn schema_version_is_at_least_two() {
    let payload = EventPayload::SandboxElevationTimedOut {
        node: node(),
        capability: "shell:bash".into(),
        elapsed_seconds: 30,
    };
    let wrapper = VersionedEventPayload::new(payload);
    let json = serde_json::to_value(&wrapper).unwrap();
    // SandboxElevationTimedOut + RuntimeVersionWarning landed in schema v2.
    // Any persistence-layer test that expects v1 must update once the
    // migration chain advances; this guard catches an accidental downgrade.
    assert!(
        json["schema_version"].as_u64().unwrap() >= 2,
        "schema_version must be at least 2 — SandboxElevationTimedOut is a v2 variant"
    );
}

#[test]
fn no_prompt_field_in_any_elevation_payload() {
    // Defensive sweep across the three elevation payloads: none of them
    // should carry a `prompt`, `arguments`, `input`, or `args` field that
    // could leak raw agent / tool data into the audit trail.
    let sensitive_keys = ["prompt", "arguments", "input", "args", "raw"];
    let payloads = vec![
        EventPayload::SandboxElevationRequested {
            node: node(),
            capability: "fs-write:./x".into(),
        },
        EventPayload::SandboxElevationDecided {
            node: node(),
            decision: ElevationDecision::Deny,
            remember: false,
        },
        EventPayload::SandboxElevationTimedOut {
            node: node(),
            capability: "shell:bash".into(),
            elapsed_seconds: 10,
        },
    ];

    for payload in payloads {
        let wrapper = VersionedEventPayload::new(payload.clone());
        let json = serde_json::to_value(&wrapper).unwrap();
        let inner = json["payload"].as_object().unwrap();
        for sensitive in sensitive_keys {
            assert!(
                !inner.contains_key(sensitive),
                "{payload:?} payload must not contain sensitive field `{sensitive}`",
            );
        }
    }
}
