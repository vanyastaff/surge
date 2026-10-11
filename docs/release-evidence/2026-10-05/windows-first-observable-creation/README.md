# First-observable Windows creation candidate

This candidate adds an exact ignored native probe to the existing standard-user launcher. A test-only hook observes successful native creation before production inspection, writes or flushes. An independent observer opens the literal home/database path, verifies actual owner and protected private ACL, checks the database is still zero bytes, compares file identities, and closes its handles before acknowledging. Exactly two observations are required. Producer errors and panics still join the observer before propagation and cleanup.

Frozen source hashes and exact filter/receipt are in `source-freeze.json`. Cross-target strict Clippy passed for the actual native modules and observation protocol; the harness does **not** compile the outer Storage integration test. The Rust 1.96 check covers the AtomicU64 method only. Initial compiler failures are build feedback, not a behavioral regression RED. Formatting and diff checks passed.

An independent security reviewer accepted the bounded pre-native source/spec preflight across these eight hashes, including hook timing, independent parent identity and observer settlement. Full acceptance and final code review remain pending actual Windows compilation and execution. No native behavior PASS is claimed.

The separate privileged fixture source is unwired and excluded. Stage 1, private-input support and the release remain open.
