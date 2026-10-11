# Agent restrictions: pure core stage — 2026-10-07

**COMPLETE for stage 1 only; task 1.5 and the release remain open.** This
adds a pure policy module and exports it from `surge-core`. It does not expose
user configuration or enforce runtime restrictions. The remaining closure is
in the [reviewed plan](../../../plans/2026-10-07-001-feat-agent-restrictions-plan.md).

[manifest.json](manifest.json) records the exact source bytes checked and SHA-256
of each raw receipt. Builds ran on local macOS, default Rust 1.98.1, without
environment or RUSTFLAGS overrides. This is core evidence, not a native release
candidate or Windows execution claim.

| Check | Actual command | Result | Receipt |
|---|---|---|---|
| Initial behavior RED | `cargo test -p surge-core --lib agent_restrictions` | exit 101; 2 passed, 5 failed before denial/contradiction guards | [RED](surge-restrictions-core-red.log) |
| Late duplicate-route regression | `cargo test -p surge-core --lib agent_restrictions::tests::duplicate_route_declarations_are_rejected` | exit 101; 1 failed with guard removed; this was calibration, not initial test-first evidence | [duplicate RED](surge-restrictions-core-duplicate-red.log) |
| Final core library tests | `cargo test -p surge-core --lib` | exit 0; 864 passed, including 9 new policy tests | [unit tests](surge-restrictions-core-final-green.log) |
| Project test runner | `cargo nextest run --locked -p surge-core --lib` | exit 0; 864 passed, 0 skipped, 3.009 seconds | [nextest](surge-restrictions-core-nextest.log) |
| Formatting | `rustfmt --edition 2024 --check crates/surge-core/src/agent_restrictions.rs crates/surge-core/src/lib.rs` | exit 0 | [format](surge-restrictions-core-fmt-final.log) |
| Strict core lint | `cargo clippy -p surge-core --all-targets -- -D warnings` | exit 0, 9.34 seconds | [Rust 1.98](surge-restrictions-core-clippy-final.log) |
| Current CI toolchain lint | `cargo +1.99.0 clippy -p surge-core --all-targets -- -D warnings` | exit 0, 23.05 seconds | [Rust 1.99](surge-restrictions-core-clippy-199.log) |
| MSRV | `cargo +1.96.0 check -p surge-core --locked` | exit 0, 16.92 seconds | [MSRV](surge-restrictions-core-msrv.log) |

Independent spec compliance (`restrictions_critic`), owning API lead
(`restrictions_plan`) and API/security (`restrictions_security_plan`) each returned
COMPLETE for this stage. No source repair dispatch was needed after review.
The duplicate-route case's late calibration is explicitly distinguished above.

Integration must resolve canonical registry identities and evaluate frozen and
current predicates independently. `constrains()` reports value constraints:
consumers must also check a declared `selector()` even when no value list exists.
Admission, frozen provenance, ACP negotiation/updates, effect fencing and all
production entry points remain unimplemented in this stage.
