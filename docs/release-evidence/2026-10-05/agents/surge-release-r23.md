# R23 Independent regression QA

## Evidence executed

- `rustc /tmp/surge-r23-before.rs -o /tmp/surge-r23-before`; executable exit 0: reproduced previous `redact_line` leaking first-token `SYNTHETIC_SECRET`.
- `rustc /tmp/surge-r23-stderr.rs -o /tmp/surge-r23-stderr`; executable exit 0: tested exact extracted production `StderrRecords::{byte,finish,complete}` source, without reimplementing parser.
- Fixed independent fixtures: bearer secret, JSON password, invalid UTF-8 secret, EOF without newline, ANSI secret; all outputs only opaque categories. CRLF/CR/LF count fixed at four; 16384/16385 byte boundary categories correct; 100000 newlines emit 500 categories and count 99500 suppressed; empty input emits no record.

## Findings

MCP raw-stderr confidentiality and bounded-memory parser: PASS for these fixtures. Parser uses 4096-byte read buffer, saturating record byte count, at most 500 categories plus terminal suppression category. Raw read errors no longer stringify child payload.

Telegram source review: `telegram_transport_error` calls `reqwest::Error::without_url()` and the synthetic connection-refused regression explicitly proves raw error contains marker before requiring sanitized output omits marker and full URL. Actual crate test execution remains pending build recovery; source review alone is not runtime proof.

Potential filesystem defect, escalated to R15 and coordinator: forwarder uses path-based create/truncate/chmod/write on child-writable capture paths and follows symlinks. A symlink can make this clobber another user file. This is separate from secret forwarding and not verified fixed in this report.

## Limits

No Cargo crate/workspace gates run here because build agent is repairing baseline; standalone parser checks do not exercise Tokio IO, tracing sink, permissions or Telegram/reqwest runtime. No real credentials used. No project files changed.

## Subsequent independent review

- R15 `stderr_capture.rs` and `connection.rs` proposed fix: ACCEPTABLE source review. Directory nofollow/openat descriptors and retained file writes remove path redirection. File type, owner and hardlink count checked before truncation. No executed Rust crate tests yet. Residual limit: same-UID hostile concurrent hardlink insertion is outside snapshot checking; synchronous filesystem writes can block async runtime on slow mounts.
- R26 pre-code plan: ACCEPTABLE. Verified server sole consumer of wait_changed drains all available queued slots. notify_one retained permit fixes lost pre-subscribe wake; both completion and rollback paths must switch. Same-dir NamedTempFile write/sync/persist correct for atomic replacement; no power-loss rename claim without parent fsync.
- R25 pre-code plan: ACCEPTABLE with unique durable MergeAttempted reservation before RPC, fail-closed DB errors, no automatic remerge on uncertain receipt, manual inspection escalation and independent failure/restart tests. Existing MergeProposed is terminal dedup and must retain that meaning; separate attempt enum avoids semantic collapse.

## Final source pass: outgoing admission and config

`AdmittedTelegramApi` card admission: ACCEPTABLE source review. Both send/edit call current durable admission before delegation and propagate persistence errors. Production emitter, recovery reconciliation and snooze use the wrapper. Revoked recovery returns error before closing card, permitting re-pair retry. Tests independently record no network delegation on unpaired/revoked/DB failure. Cargo results pending coordinator.

Found wider sensitive-reply gap: `ProductionRoutes::send_reply` remains raw for `/status` and `/runs` after awaited data lookup; revocation after initial command admit does not prevent these outgoing replies. Escalated R15 and coordinator. Pairing-only raw exemption should be explicit; ordinary sensitive command replies should recheck admission. Card-specific fix does satisfy narrow scope.

R26 final config source ACCEPTABLE: exclusive NamedTempFile descriptor, same-dir atomic persist, file sync, errors propagate, old predictable temp path unused. Explicit no power-loss directory-entry durability claim is correct. Reported red/green Cargo evidence belongs to R26/coordinator, not my independent execution.

Final R26 admission source PASS/ACCEPTABLE: both completion and rollback producers now notify_one; documented one consumer drains all free slots per coalesced retained wake. Independently inspected reported GREEN logs: admission 10 passed / 0 failed; config 4 passed / 0 failed. Commands were run by R26, logs read by R23.

R15 wider reply gap corrective plan ACCEPTABLE: normal send_reply durable allowlist check immediately before dispatch; private unchecked helper only /pair branches. In-flight dispatch is not atomically cancellable with revoke; current dispatch check guarantee only.

## Final R15 reply extension verdict

ACCEPTABLE: exact `reply_admission.rs` and production wiring reviewed. Normal command replies recheck durable admission directly before network request. Lookup failures propagate and prevent delegation. Private unchecked pairing helper is called only from normalized `/pair` success/error branches. Previous sensitive command-reply gap is resolved. Independently inspected `/tmp/surge-release-r15-reply-green-final.log`: 2 passed, 0 failed; wiremock assertions require no added requests after revoked/lookup-failed attempts. Test execution was performed by other agent, logs/source independently reviewed by R23.

Config privacy claim is limited to Unix mode 0600 verified by tests. Inherited macOS ACL behavior was not measured; no universal access/privacy guarantee asserted.
