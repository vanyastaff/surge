# Windows guardian pure binding contract (G1)

Five private-field types validate a strict version-one public envelope: canonical nonzero installation identity, canonical executable hash and non-nil occurrence, positive Windows PID/creation FILETIME, distinct guardian/child identities, exact derived local endpoint and permanently GroupOnly coverage. Nested deserialization passes constructors and rejects unknown fields, future/legacy shapes, aliases and coverage upgrades. Existing process/kernel-boot types and general hash/ULID parsing are unchanged. This data grants no authentication, executable provenance, native process ownership, receipt, boot or recovery authority.

Owning-lead, independent critic and security/API pre-code verdicts ACCEPTABLE. Implementation spec and API/security verdicts COMPLETE after one fixture repair. The initial legacy rejection fixture was malformed; adding a control old-decoder acceptance assertion exposed the fixture defect (1/1 RED). The corrected independent literal fixture succeeds as the existing WriterContainer and fails as WindowsGuardianContainer. Both receipts remain. Initial new-feature RED was 18 missing API compilation errors; it was not a native bug reproduction. Intermediate discriminator lifetime compilation failure is also retained.

Executed separately with CARGO_INCREMENTAL=0:

- `cargo +1.99.0 test --locked -p surge-core --lib execution_recovery::guardian` — 8/8 PASS after fixture repair.
- `cargo +1.99.0 test --locked -p surge-core --lib` — 872/872 PASS, zero ignored/filter, after fixture repair.
- `cargo +1.99.0 clippy --locked -p surge-core --all-targets -- -D warnings` — PASS before the test-only fixture repair.
- `cargo +1.96.0 check --locked -p surge-core --all-targets` — PASS before the test-only fixture repair.
- Both allowed source files pass rustfmt check; git diff --check passes.

Production scope is the new guardian module and its export only. Native helper, migrations, durable receipt consumers, adapter lifecycle and full Windows support are later stages under plan 004 / proposed ADR-0023. Source/raw-output hashes bind the retained compressed receipts.
