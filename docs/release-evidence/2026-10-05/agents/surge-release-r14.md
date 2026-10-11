# Role 14 — Authorization and isolation

Scope: read-only source audit of daemon IPC, stage MCP authority, Telegram controls, private owned-flow objects, and task project identities. No cargo build/test was run (coordinator requested source-only first wave). Commands used: rg, sed, cat. No repository changes.

## Release finding

P1 authorization fail-open — crates/surge-daemon/src/server.rs:199-211. The source explicitly identifies the control socket as the sole authorization boundary with no per-verb checks. Metadata errors are silently ignored; chmod failures only log a warning and the daemon proceeds to accept privileged requests. Unlike stage/local.rs:31-39, which tears down and returns an error on permissions failure, this path can run without having established its advertised boundary. pidfile.rs:119 creates the directory with default umask and does not establish 0700; thus there is no guaranteed private parent boundary to compensate. The actual exploitability depends on filesystem/platform permissions and parent accessibility; no dynamic exploit was executed.

Required remediation: fail startup on any inability to establish the socket authorization boundary; establish a user-private socket directory before publishing the listener or validate peer credentials to prevent pre-chmod queued connections. Keep platform-specific guarantees explicit (Windows DACL claim still needs native evidence). Acceptance: default and permissive umask tests observe secure directory/socket modes; injected metadata/chmod error prevents accepting requests; verify no other-user connection under supported Linux/macOS security semantics.

## Inspected protections (source evidence, not runtime pass)

- Stage MCP: local.rs:17-39 creates unpredictable 0700 directory and 0600 socket, aborts permissions failure. transport.rs:142-180 checks session secret before catalog/calls and ensures call name is in exact catalog; owner receives host-held context rather than caller-selected run/node/session. Existing tests `invalid_auth_and_hidden_tools_never_reach_owner` and `authenticated_call_waits_for_owner_reply_and_close_revokes_connections` are appropriate runtime checks to schedule.
- Telegram: production.rs:753 requires current admission for commands except /pair; production_actions.rs:122 requires admission for edit replies. callback.rs:255 checks pairing and :283 binds card chat before engine call. production.rs:558 maps pairing-store failure to false (fail closed). Existing callback `unpaired_chat_short_circuits_at_admission` asserts no engine call.
- Owned Flow private objects: private_files.rs:34-73 uses descriptor-relative O_NOFOLLOW opens, validates uid, mode, ACL, hardlinks and bounds; :143-166 revalidates namespace identity before operations. private_inputs.rs:272-297 authenticates full envelope, canonical object bytes and public projection. Existing tests cover symlink/hardlink/replacement and cross-envelope object transplant.
- Project tasks: daemon/work_items.rs:142-152 creates workspace plan from explicit project; :159-162 validates attached PR repository against task workspace. Persistence work_items.rs:503-529 keys project registry by repository, uses typed project identity. This is single-user OS authority, not multi-tenant per-project ACL; project filtering should not be marketed as security isolation.

## Non-blocking policy/test clarification

P2 policy ambiguity: Telegram uses chat admission, not individual actor identity (cockpit/run.rs:338-346). Group chat commands are explicitly supported by production.rs:745; any member of a paired group can trigger privileged commands or click its card. That may be intended shared cockpit authority; document group authorization clearly or restrict pairing to private chats if owner-only control is intended. Do not declare this an authentication bypass without establishing the product policy.

P2 test gap: no named cross-chat callback regression was found. Add admitted chat B + chat A card test asserting AdmissionDenied and zero engine calls; also revoked pairing edit-reply test. Current source guard is correct.

## Suggested targeted checks

- cargo nextest run -p surge-mcp -E 'test(stage::transport::tests)'
- cargo nextest run -p surge-telegram -E 'test(cockpit::callback::tests)'
- cargo nextest run -p surge-persistence -E 'test(private_inputs) | test(private_files)'
- OS-isolated socket smoke using disposable SURGE_HOME and restrictive/permissive umask; avoid live home.

Verdict: NEEDS WORK for sole authz boundary fail-open; runtime verification remains unverified. Coordinator should assign server.rs ownership before fixes.
