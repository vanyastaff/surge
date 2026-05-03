//! Engine — drives a frozen `Graph` through ACP sessions and persistence.
//!
//! See `docs/superpowers/specs/2026-05-03-surge-orchestrator-engine-m5-design.md`
//! for the full design contract. M5 ships sequential-pipeline-only support;
//! parallel/loops/subgraphs are M6 scope and rejected at run-start.

// Submodules added incrementally as later phases land. Currently empty.
