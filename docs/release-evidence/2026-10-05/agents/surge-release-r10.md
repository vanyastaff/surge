# Role 10 — API contracts (read-only initial review)

Scope: CLI task command, stdio MCP tool surface, daemon JSON framing and typed task contracts. No cargo builds/tests were run, per coordinator instruction; findings below are source-supported, runtime verification pending. No published Git tags exist (`git tag --list` returned empty), so semver compatibility with a previous published release cannot be asserted.

## Findings

1. P2 — Invalid PR arguments cross the CLI/MCP transport boundary instead of being rejected locally. `crates/surge-cli/src/commands/task.rs:253` and `commands/mcp_serve.rs:943` construct `WorkItemPr` directly without `validate()`. Example: attach PR with repository `owner/repo`, number `0`, or repository `bad`. `WorkItemPr::validate` rejects these (`crates/surge-core/src/work_item.rs:272`) and its Deserialize uses `try_from = RawPr`, so daemon JSON decoding rejects the entire frame. `crates/surge-daemon/src/server.rs:362` closes that connection on parse failure instead of providing a correlated validation error. MCP converts any work_item error into `rejected` at mcp_serve.rs:964. This does not bypass validation or commit bad data, but presents a malformed client input as a daemon/transport failure. Recommended minimal fix: locally call `pr.validate()` in both adapters before audit/RPC, return actionable CLI error and MCP `invalid_argument`; add meaningful adapter tests proving invalid PR input never reaches daemon.

2. P2 — Task-read MCP limit/cursor validation violates documented error taxonomy and schema parity. `mcp_serve.rs:849-864` maps all query errors into `ToolError::Failed`; `work_items.rs:917-923` rejects limit 0 or >100 as `WorkItemError::Invalid`. Thus `surge_task_read({action:list,after:null,limit:0})` returns internal `failed` instead of invalid argument. TaskReadParams variants (mcp_serve.rs:345 onward) have plain u32 fields with no schema bounds, unlike existing read tools. `docs/mcp.md:158-161,193` promises bounded limits reflected in schema and invalid_argument for out-of-range limits, but task tools are missing from documented tool table entirely. Recommended fix: task-specific schema 1..100, typed error classification, document task cursor envelopes/limits.

3. P3 — MCP inventory/documentation drift: TOOLS contains 12 entries including surge_task_read and surge_task_control; docs/mcp.md:140-150 contains only the older ten. Module introductory comments say mutating tools are only steer/resolve/bootstrap, omitting task_control. Existing tests exercise task tool presence, read guard and daemon writes (mcp_serve/tests.rs:896 onward); no task limit/schema test appears in the existing limit suite (582,604).

## Positive source evidence

- IPC read/write size is capped at 8 MiB before allocation can grow beyond the cap (ipc.rs:671,702 onward), with oversize and framing tests at 1011,1116.
- WorkItemPr inbound deserialization routes through validation; accepted requirement types also have validated construction.
- MCP mutations share write guard and audit handle; tool-table/router synchronization and mutation refusal tests exist.
- Resolution enforces expected node, declared gate outcomes and human-only bootstrap gates; policy tests present.

## Release judgement

No P0/P1 release blocker established from this source-only inspection. Two P2 public contract defects should be fixed before declaring API-GATE complete. Full API compatibility, actual adapter reproductions and integration checks remain unverified.
