# Project journey resumption — 2026-10-16

The full project-lifecycle goal remains active. A usable generated application,
an existing-project improvement, retained follow-up history and native acceptance
are still required. Local infrastructure checks do not satisfy those outcomes.

## Current verified publication state

GitHub was queried on resumption. PR #150 is merged at
`6722b4952adcff12a532665863606a40093dee25`, mergedAt
`2026-10-16T08:19:53Z`. Its tested head was
`88603ffadf1138c17f3dfd7f58c2f5e765dbfbcf`. All eleven jobs in
[CI run 38869138216](https://github.com/vanyastaff/surge/actions/runs/38869138216)
completed successfully, including Windows tests and strict Clippy. The authorized
fifth repair is closed; no sixth repair is inferred.

The independent `codex/results-planning-history` worktree retains local commits
`c3a3ab3a`, `9709f437`, and `fe685c37`. It is clean before this checkpoint.
These commits have not been published or accepted by Windows CI. Prior local
verification is recorded in `.rust-studio/tasks/recovery-observability.md`;
the referenced temporary logs are currently unavailable and are not fresh proof.

## Native acceptance interruption

The prior turn recorded cold retry of the original habit-tracker operation:
planning completed without another author split/session, the generated Flow
retained `codex-acp` / `gpt-6.1-sol`, approval launched the reserved implementation
run, and `milestone_spec` failed with `Authentication required`. The application
was not implemented. These are historical observations from conversation state;
the original journal and captures cannot currently be re-inspected.

On resumption, the following exact directories no longer exist:

- `/private/tmp/surge-sol61-native-9tkig5g4/habit-tracker`
- `/private/tmp/surge-home-native-oy5u6wlw/runtime`
- `/private/tmp/surge-habit-pre-B-snapshot-aaer05mk`
- `/private/tmp/surge-recovery-native-k_badqxm`

The old native processes are absent. Free disk space is currently 31 GiB; no
cleanup was performed in this resumption. The cause of the missing directories
has not been established. A name search under Develop, Codex worktrees, the
ordinary Surge home, Desktop, Documents and the per-user temporary directory
found no habit-tracker or named snapshot copy. This is a scoped search, not proof
that no backup exists anywhere.

Do not recreate a journal, manufacture missing receipts, treat a new fixture as
recovery of the original, or infer expired credentials from the error text.
The user was asked for any retained copy. A read-only source audit is tracing
project registry/environment propagation from planning to implementation while
that question is pending. No provider retry or speculative production fix has
been performed. Credentials and runtime configuration values must remain private.

Future long-lived acceptance fixtures and their complete backups must live in
durable, private storage outside OS temporary directories. Preserve the original
fixture if recovered; a new acceptance attempt must have a distinct identity and
must not be reported as recovered history.

The cancelled sandbox/verification-receipts work remains cancelled. Stage C
TaskPlanning implementation remains deferred until the real application outcome.

## Source audit result

Read-only audit of unchanged Rust source at `fe685c37` found no demonstrated
registry/environment loss at handoff. `bootstrap_supervisor.rs::launch_inputs`
sets `agent_registry: Some(runtime.agents())` before selecting planning versus
implementation, and engine admission prefers that per-run registry. Custom
registry entries retain command/arguments/env; the bridge applies the resolved
environment at spawn. Missing required injection fails before spawning. This
does not prove that original resolved values or provider authentication matched.

Planning adaptation changes only untouched bundled authoring profiles. The
implementation graph retains its approved explicit selections. Successful
planning alone therefore does not establish identical runtime/recipe selection;
the missing graph and SessionOpened evidence are needed for that comparison.

A separate diagnostic defect is established directly by current source:
`surge-acp/src/pool.rs::is_auth_failure` omits the literal provider message
`Authentication required`. Consequently `bridge/worker.rs` falls back to
`Bridge(CommandSendFailed(...))`. That wrapper does not prove a broken command
channel; provider prompt errors pass through the same classifier.

The next bounded code regression is to assert that this actual message reaches
the existing `AgentAuthenticationFailed` classification, with auth precedence,
rate-limit and generic-error behavior retained. First execute a failing test
against unchanged production code, then fix the shared classifier and verify
its dependent error paths. No such test or fix has yet been executed. This would
repair truthful problem reporting, not prove or repair the historical login
failure. Restoring the original fixture remains necessary for its continuation.
