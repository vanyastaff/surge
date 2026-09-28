# Live provider smoke — evidence and source-only diagnosis

Status: controlled daemon/ACP/MCP smoke and repaired oracle pass. One authorized
live Codex attempt failed; supported live-provider execution remains unverified.
This document records diagnosis, not an authorization to launch another provider.

## Observed runs

- Controlled report-only peer reached completion but the human/nonce oracle
  rejected it (genuine RED99847). Controlled human MCP roundtrip then passed.
- Final controlled run proved one SessionOpened, one HumanInputRequested, one
  exact HumanInputResolved, two authenticated StageToolReceipts, one
  OutcomeReported, and one RunCompleted. The caller-generated nonce was learned
  by the peer only from the MCP reply. No provider credentials were supplied.
- Oracle repair RED62910 rejected the previous acceptance of committed mutation
  and extra failure evidence. GREEN88918 passes both offline adversarial tests;
  the controlled daemon fixture and strict/fmt checks also pass. Both repository
  and separate worktree HEADs are pinned after deterministic preseed, and integrity
  is checked before propagating successful or failed run results. Duplicate and
  missing required events fail exact-count checks.
- Live Codex attempt13235 exited101 after14.87s. It returned RunOutcome::Failed
  with a timeout-class diagnostic, not a fixture timeout result. Raw provider
  output was withheld; credentials were not read. Temporary storage was deleted
  after explicit cleanup, so exact session/receipt counters from that attempt
  cannot now be recovered. No authentication conclusion is supported. Subsequent
  fixture revisions emit event-kind counts only for future diagnostics.
- No second live attempt has occurred. The source-only audit below launched no
  executable, provider, helper, or test and read no auth file/keychain/value.

## Local adapter and launch facts

Pinned cached native adapter:
`/Users/vanyastafford/.npm/_npx/e3854e347c184741/node_modules/@zed-industries/codex-acp-darwin-arm64/bin/codex-acp`.
Both platform-package and wrapper manifests report0.16.0. The wrapper contains
only README, package.json and bin/codex-acp.js. Its launcher resolves the native
platform binary and forwards argv with inherited stdio. The fixture invokes the
same native binary directly with no extra arguments. No npx download or wrapper
flag translation is involved.

Installed Codex CLI package is0.155.1. This is not the adapter version. The native
adapter contains embedded Codex Rust source paths at revision f221438, including
codex-rs/core/src/thread_manager.rs and mcp connection code. The local wrapper does
not execute the installed `codex` CLI. No local full Rust checkout or adapter
compatibility matrix is present; CLI0.155.1 incompatibility is therefore neither
established nor a justified diagnosis from these files. Binary strings are static
implementation hints, not executed-path evidence.

Cached adapter README explicitly advertises client MCP servers and ChatGPT,
CODEX_API_KEY and OPENAI_API_KEY authentication. Auth presence/validity was not
rechecked. No user configuration or auth store was opened for this audit.

## Fixture/path comparison

| Boundary | Source evidence | Conclusion |
|---|---|---|
| Launch | fixture prepare uses a custom registry entry with native path and empty args; worker.rs Custom arm forwards exactly those args | No accidental `codex acp` or `--acp` is added |
| CWD | fixture creates a real separate temporary Git worktree; worker canonicalizes it and uses it for ACP new_session | No missing/non-Git/relative CWD defect found |
| Model | profile recommended_model is `provider-default`; SessionConfig/open_session sends no model selection and no model CLI argument | This label is not sent as an invalid provider model; provider default remains in control |
| Protocol | worker.rs initializes explicit stable V1, then new_session with working directory and stage MCP server | Fits advertised ACP/client-MCP surface; live negotiation not proved |
| Helper | stage_tools.rs helper_path resolves the sibling built `surge`, with `internal-stage-mcp` argv | Controlled peer proves that local helper exists and speaks the protocol |
| Human tool visibility | fixture node explicitly enables escalation with a channel, and controlled catalog/human call succeeds | Earlier fixture visibility defect was fixed before the live attempt |
| Auth for helper | stage_tools.rs295–305 inserts SURGE_STAGE_MCP_AUTH and endpoint into SessionConfig.env, but MCP server descriptor.env is empty | Helper startup relies on provider child-environment inheritance |

## Concrete environment portability gap

`stage_tools::prepare` gives the two stage transport variables to the provider
process. `worker.rs` forwards the descriptor in new_session without filling its
env array. The local ACP1.9.1 schema documents McpServerStdio.env as the environment
variables to set when launching the MCP server; its constructor defaults to an
empty array. `surge-mcp/src/stage/helper.rs:92–95` requires both variables.

The controlled peer's `mock_acp_agent/stage_mcp.rs:33–36` calls Command::new and
then envs(descriptor.env), without env_clear. It therefore silently supplies the
missing variables through inherited provider environment. That green fixture does
not prove operation with a provider that constructs a filtered MCP environment.
A filtered child environment would deterministically omit the helper locator/auth
under the current descriptor. This is a concrete untested interoperability
contract, not yet proof that the cached Codex adapter took that path.

The cached native adapter exposes startup_timeout_sec/startup_timeout_ms,
shell_environment_policy and MCP env/env_vars strings, but has no shipped Rust
source that establishes their values or child-environment policy. Do not infer an
exact10s/15s default from these strings. Do not change global Codex settings or
increase timeouts on this evidence alone.

## Deadline audit

- Agent node:60s, zero retries; StageExecutionConfig override absent.
- Human input default:300s, but outer fixture stream wait is90s.
- Bridge initialize + new_session share a30s startup deadline; shutdown8s.
- Fixture completion join10s; daemon cleanup join10s; outer process watchdog150s.
- Dedicated helper first authenticated frame has a5s deadline. That timer cannot
  establish a15s end-to-end provider stage limit; missing helper environment can
  instead fail before connection.

No deterministic15s fixture deadline was found. The initial broad string
classification loses whether the provider error arose at session startup, MCP
startup, prompt dispatch, or provider networking. That remains unknown.

## Next single safe diagnostic (proposed, not run)

Add one controlled-provider mode that launches the real advertised MCP helper
with a deliberately empty inherited environment, adding only descriptor.env
(and platform-required nonsecret launcher essentials if needed). Use actual
new_session and the existing bounded helper cleanup/oracle. It must never invoke
Codex, access provider login, or introduce a token into argv/logs.

Expected current failure: helper has no transport variables and cannot initialize.
A genuine RED would prove the portability gap independently of a paid provider.
Then choose an explicit helper-environment contract with maintainer review before
implementation. Passing the two required variables in the standard transient
MCP descriptor is the smallest interoperable option, but changes the earlier
"credentials never in descriptors" design promise and requires an explicit
security/logging decision. Keep credentials out of durable capture, argv and
Surge-owned logs; upstream TRACE remains a known raw-payload exposure boundary.
Do not silently add a secret-bearing wrapper file or mutate user's Codex config.

Only after that controlled boundary is resolved should another separately
authorized live diagnostic capture structural milestones (initialize/new_session,
SessionOpened, human request/response, receipts, terminal), typed failure phase
and elapsed time. No raw payload capture or auth-file inspection is needed.


## Descriptor portability repair (2026-09-28, validation in progress)

The authorized repair removes inherited endpoint/auth reliance. The controlled ACP
peer now explicitly clears both inherited variables before applying descriptor env.
That real daemon smoke failed in 0.65s with the old empty descriptor (RED6787).
The independent serialized Debug diagnostic test also failed (RED3349). Production
now advertises the endpoint and per-generation capability through the standard ACP
stdio environment; provider process env no longer receives them from Surge.
StageMcpConfig Debug is redacted and provider output redaction obtains the literal
from the descriptor. This changes the earlier no-credential-descriptor design:
the provider is the trusted recipient, while argv/model/durable events remain free
of these values. Upstream SDK raw TRACE remains an explicit limitation. No live
provider retry was run; the original Codex timeout cause remains unconfirmed.

Validation: controlled GREEN10099, production descriptor unit GREEN8063, ACP261
unit +2 redaction tests, Engine7 MCP journeys, MCP41 (+3 ignored), combined strict
clippy49227 PASS. Owned fmt/diff PASS; independent review pending.
