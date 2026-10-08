# Native integration fixture corrections — 2026-10-07

Implementation independently accepted; **native Windows execution pending**.
Cold hosts use unique endpoints and real connection readiness at the existing
one-second deadline. The dedicated child fixture keeps first endpoint cold.sock.
Git CLI alone receives structured Windows verbatim prefix normalization; recorded
checkpoint identity and independent historical commit/bytes are preserved.

SQLite fault injection appends exactly one unread malformed payload at the next
sequence inside an immediate transaction, copying an actual gate event metadata.
Both scoped tail and full inspection prove exact-sequence decode failure. Inode,
schema, append-only triggers and all original live-stream isolation/completion
assertions are preserved.

The original delivered-row corruption proposal failed locally: [old receipt](daemon.log.gz)
records 1 passed and 1 failed. The tail had already consumed that event. The
revised live-tail design received independent pre-code acceptance, then spec and
quality/security COMPLETE. It preserves the immediate stream-failure scenario.

All Cargo commands used CARGO_INCREMENTAL=0, local macOS.

| Command | Result | Receipt |
|---|---|---|
| `cargo test -p surge-orchestrator --test engine_snapshot_unit` | 8 passed | [snapshot](snapshot.log.gz) |
| `cargo test -p surge-daemon --test work_item_route_test` | 50 passed | [routes](route.log.gz) |
| `cargo test -p surge-daemon --test daemon_persisted_gate` | revised fixture: 2 passed | [gate](gate-green.log.gz) |
| `cargo clippy -p surge-daemon --test daemon_persisted_gate --test work_item_route_test -- -D warnings` | passed | [daemon lint](daemon-clippy.log.gz) |
| `cargo clippy -p surge-daemon --test daemon_persisted_gate -- -D warnings` | final revised gate: passed | [final gate lint](gate-clippy-green.log.gz) |
| `cargo clippy -p surge-orchestrator --test engine_snapshot_unit -- -D warnings` | passed | [snapshot lint](snapshot-clippy.log.gz) |

Final scoped rustfmt and git diff checks passed. [Manifest](manifest.json) binds
source and compressed original receipts. Full Windows CI remains open.
