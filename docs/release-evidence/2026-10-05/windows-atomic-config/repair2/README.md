# Atomic publication repair 2

Native 77fff58 sharing failures remain preserved in the preceding CI receipt. The prior deny-DELETE reader oracle assumed replacement could override Windows sharing rules; that assumption was incorrect. The revised contract explicitly refuses a reader denying DELETE sharing with old bytes preserved and owned temporary cleanup; cooperative readers retain their old snapshot while replacement succeeds. All eight-writer and continuous complete-config assertions remain strict.

The publisher retains the actual create-new temporary handle with READ|DELETE sharing, denying competing content writes. TempPath cleanup is disabled immediately. Unpublished cleanup uses FileDispositionInfoEx on the retained object, never its replaceable pathname, latches the attempted state, exposes both failure diagnostics, and never retries or overrides readonly protection. Publication immediately disarms deletion. A native sentinel oracle moves the owned object and replaces its temporary name before failed publication; cleanup must remove the owned object and preserve the replacement sentinel.

Independent pre-code/specification/security/unsafe reviews are COMPLETE. The only post-review representation repair combines SDK typed flags using named constants' .0 values because windows 0.58 lacks BitOr for that flag wrapper. Executed with CARGO_INCREMENTAL=0:

- `cargo +1.99.0 clippy --locked -p surge-core --all-targets --target x86_64-pc-windows-msvc -- -D warnings` — PASS. The initial SDK flags compilation failure is preserved separately.
- `cargo +1.96.0 check --locked -p surge-core --all-targets --target x86_64-pc-windows-msvc` — PASS.
- `cargo +1.99.0 test --locked -p surge-core --lib config::io::` — 4/4 macOS PASS, 860 filtered. An initial incorrect filter selected zero tests; it is retained and is not counted as acceptance.
- `cargo +1.99.0 clippy --locked -p surge-core --all-targets -- -D warnings` — PASS on macOS.
- Both source files pass rustfmt check; git diff --check passes.

Cross checks compile the actual Windows implementation and native oracles. Native execution remains pending CI; this receipt is not Windows release GO.
