# Windows pipe readiness and shell forwarding — 2026-10-07

Implementation independently reviewed; native Windows acceptance pending.
Real connection attempts replace socket-file polling at existing deadlines,
including awaited connection bounds and early-server error reporting. Stale
socket pathname cleanup is Unix-only. No mutation readiness RPC is sent.

Hooks and worktree shell_exec preserve explicit shell programs through Windows
cmd /D /S /C and outer-quoted raw_arg; structured ACP program/args are unchanged.
Surrounding ownership, cwd, environment, timeout and output contracts remain.
Separate native tests cover quoted executable paths with spaces, quoted argument,
redirection and compound exact output/effects. These new cases have no initial
native RED claim. Existing pipe/quoted-hook baseline failures are retained.

Local macOS, CARGO_INCREMENTAL=0: 122 scoped tests passed.

| Check | Actual command | Result | Receipt |
|---|---|---|---|
| Hook tests | `cargo test -p surge-orchestrator --lib engine::hooks::tests` | 15 passed | [hooks](hooks.log.gz) |
| Worktree tools | `cargo test -p surge-orchestrator --lib engine::tools::worktree::tests` | 9 passed | [tools](worktree.log.gz) |
| Four daemon targets | `cargo test -p surge-daemon --test quota_recovery_route_test --test owned_flow_framing_trace --test owned_flow_ipc --test work_item_route_test` | 98 passed | [daemon](daemon.log.gz) |
| Strict library lint | `cargo clippy -p surge-orchestrator --lib -- -D warnings` | passed | [library lint](orchestrator-clippy.log.gz) |
| Strict four-target lint | `cargo clippy -p surge-daemon --test quota_recovery_route_test --test owned_flow_framing_trace --test owned_flow_ipc --test work_item_route_test -- -D warnings` | passed | [daemon lint](daemon-clippy.log.gz) |

Scoped final rustfmt and diff checks passed. [Manifest](manifest.json) binds
source and original compressed receipts. Native execution is required; these
macOS checks cannot prove Windows shell escaping or named-pipe behavior.

Native eb5ed49 Clippy exposed `items_after_statements` in the Windows-only hook branch. The Windows CommandExt import is now before statements; behavior and shell assertions are unchanged, with no suppression. Native revalidation remains pending. Raw failing job output is retained in native-clippy-eb5ed49.log.gz.

Native eb5ed49 confirmed production quoted hook ownership, bootstrap materialization and all archetypes PASS. The two new fixture programs retained spaces around cmd redirection and compound operators, producing two trailing spaces while their oracle expected one. Independent lead/critic ACCEPTABLE: remove those incidental program spaces and retain exact quoted stdout/file bytes, redirection and compound assertions, with no trim/normalization or production change. Both files pass rustfmt/diff-check; native GREEN remains pending.
