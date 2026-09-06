+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-09-06"
+++

# ADR 0016 — Capacity-aware rate-limit classification: typed error taxonomy and the M0 measurement

## Status

Accepted, **M0 and M1**. Task 12 ("capacity scheduling") is six milestones
(M0–M5); this ADR is opened at M0 and is expected to gain sections for
persistence (M2), the run-task wiring (M3), the wake scheduler (M4), and the
inbox/doc surface (M5) as each lands — not to be rewritten wholesale once
they do. The M0 sections above use the field name current at the time they
were written (`account`); M1 renamed it — see the M1 section below, which
uses the post-rename name throughout.

## Context

The parking planner this task exists to build needs to answer "an account is
exhausted until time T" — and T has to come from somewhere. Before M0,
`classify_prompt_dispatch_error` (`surge-acp::bridge::worker`) had exactly two
arms: an auth-failure check, and a generic `Bridge` catch-all. No arm
recognized a rate limit at all.

The machinery to *use* a rate-limit signal already existed and looked
finished: `SurgeError::RateLimit { retry_after }` (`surge-core::error`), and
`surge_core::capacity::{looks_like_rate_limit, parse_retry_after_secs}`
(pure, well-tested classifiers). But their only producer on the *engine* run
path was nothing — `AgentPool::record_failure`, the sole call site that ever
constructed a rate-limit observation, lives in `surge-acp::pool`, which is
constructed only in `surge-cli` (`commands/agent.rs`, `main.rs`). Neither
`surge-orchestrator` nor `surge-daemon` ever executes that path. A capacity
policy built on top of this input would have shipped with its primary rule —
"exhausted with a known reset time → park" — structurally unreachable in
production, which is exactly the failure mode this task exists to replace
(R37 forbids "warn and dispatch anyway" as the sole behavior).

## Decision

1. **`SendMessageError::RateLimited { retry_after: Option<Duration>, details:
   String }`** (`surge-acp::bridge::error`) — the missing producer. Mirrors
   the existing `AgentAuthenticationFailed` precedent in the same enum,
   added for the identical reason: an operator/account condition deserves a
   dedicated variant instead of being buried in `Bridge` and stringified.

2. **Classification order is auth → rate-limit → generic, and is load-
   bearing.** `classify_prompt_dispatch_error`'s third arm calls
   `surge_core::capacity::looks_like_rate_limit` +
   `parse_retry_after_secs` — the same classifiers already used elsewhere in
   the workspace. **No new classifier was introduced**; a third classifier in
   this workspace was an explicit non-goal carried over from the prior task's
   review. Auth is checked first so a 401 whose text also happens to contain
   rate-limit vocabulary (e.g. "too many requests") still reports as an auth
   failure, not a rate limit — reversing the order would tell an operator to
   wait out a rate limit when the real problem is that the agent runtime
   isn't logged in. Pinned by test (`worker.rs`,
   `classify_prompt_error_401_wins_over_rate_limit_vocabulary`).

3. **`StageError::RateLimited { account: Option<String>, retry_after:
   Option<Duration>, details: String }`** (`surge-orchestrator::engine::
   stage`) — the bridge → stage boundary, matched directly off the
   `SendMessageError` variant at `agent.rs`'s `send_message` call site, never
   by re-parsing `Display` text.

4. **`#[non_exhaustive]` added to both `SendMessageError` and `StageError`**
   (neither had it before this task). Both enums are workspace-owned
   taxonomies expected to keep growing; this task already pays the one-time
   breaking-change cost of a new variant on each, so the marker is free here
   and saves a second such change later. The trade-off is explicit, not
   hidden: an external `match` is now forced to carry a wildcard arm, so a
   future variant lands quietly in that wildcard until the external caller
   deliberately updates it.

5. **`account` is a runtime-registry identity, not a credentialed-account
   one — named and documented as such, not disguised.** The value is the
   resolved profile's `RuntimeCfg::agent_id`, normalized through
   `surge_acp::Registry::normalize_agent_id` (so `"claude"` / `"claude-code"`
   / `"claude-acp"` — aliases of one registry entry — collapse to the same
   string instead of fragmenting one runtime's observations across three
   keys). It is `None` only on the legacy no-profile-registry path, where no
   id is known at all — never a placeholder string standing in for an
   identity nothing observed. Whether Surge can ever represent two distinct
   *credentialed* accounts sharing one runtime is a real, open question,
   deferred to M2's ledger design (which will make this field a primary key)
   — not settled here.

6. **A latent panic in `parse_retry_after_secs` was fixed, not routed
   around.** The function sliced the *original* error text at byte offsets
   computed against a *lowercased* copy; any character whose lowercasing
   changes UTF-8 byte length (e.g. `İ` U+0130 → 3-byte `i̇`) desynchronizes
   the two coordinate frames and can slice mid-character, panicking. Before
   this task nothing in `surge-acp` called this function, so the bug was
   latent; wiring the new classifier arm makes it reachable from **every**
   rejected prompt whose text matches `looks_like_rate_limit` — text that
   originates at an agent runtime the operator does not control, executed
   inline on `bridge_loop`, the single task serving *every* session. Fixed by
   slicing the lowercased copy (self-consistent by construction) instead of
   the original. Regression test in `surge-core::capacity` reproduces the
   panic pre-fix (`parse_retry_after_secs_does_not_panic_on_multi_byte_
   case_folding`).

## Measurement (M0's mandated deliverable)

The plan required measuring, before any policy is built on top: does the raw
ACP error text an agent runtime actually produces carry enough for
`parse_retry_after_secs` to recover a `retry_after`? **This table is
reconstructed, not captured** — every string below was authored for this
task; none is a captured response from a live provider or a real agent
runtime (Claude Code / Codex / Gemini CLI). The classifier operates on
`e.to_string()` of the ACP JSON-RPC error surfaced by *that runtime's own
adapter*, not on a provider's raw JSON — a layer this task had no live
instance of any agent runtime to observe. Treat the "reachable" conclusions
below as a plausibility argument pending real capture, not a measured fact
about any specific runtime.

| Provider-error shape (reconstructed) | `RateLimited`? | `retry_after` |
|---|---|---|
| `"429 Too Many Requests: Retry-After: 30"` (header-style, plain text) | yes | **Some(30s)** |
| `"429 Too Many Requests"` (no hint) | yes | None |
| Anthropic `rate_limit_error` (shape of the documented API error type) | yes | None |
| OpenAI `rate_limit_exceeded` (prose: *"Please try again in 20s"*) | yes | None — phrase not recognized |
| OpenAI `insufficient_quota` | yes | None |
| Google `RESOURCE_EXHAUSTED` | yes | None — a real `retryDelay` JSON field, if present, also would not be recognized (parser looks only for literal `"retry-after"`/`"retry after"` text) |
| Anthropic `overloaded_error` | yes | None |
| `"Claude AI usage limit reached\|<epoch>"` (subscription-style) | yes | None — epoch suffix not recognized |
| Surge's own `SurgeError::RateLimit` Display shape (*"Rate limit exceeded ... retry after 30s"*) | yes | **Some(30s)** |
| Bare *"please retry after 30s"* with no rate-limit keyword | **no** — never reaches the rate-limit arm | N/A |

Confirmed end-to-end (not just the pure classifier) for the one shape that
recovers a value, via a real `mock_acp_agent` subprocess round trip over the
actual ACP wire (`surge-acp/tests/bridge_rate_limit_classification.rs`,
`#[ignore]`d — see Consequences):
```
cargo nextest run -p surge-acp -j 2 --test bridge_rate_limit_classification --run-ignored ignored-only
   PASS real_429_with_retry_after_survives_the_acp_wire_as_rate_limited
```

**Honest reading:** the coarser rung — "exhausted, no reset time known" →
`Decision::Dispatch{degraded}` or park-without-a-wake-time (M1's design) — is
reachable for every shape tested, real-provider-JSON shapes included. The
finer rung — "exhausted, park at a known wake time" — is reachable in
principle (2/9 shapes) but conditioned entirely on an agent runtime's own
adapter happening to leave a literal `Retry-After:`/`retry after` substring
in the text it hands back; none of the four realistic *provider* JSON bodies
reconstructed here do. This is a reduction of the risk the pre-code review
flagged ("structurally unreachable" → "reachable, but usage-dependent"), not
its elimination.

## Alternatives Rejected

- **Hand-tune a classifier against these reconstructed shapes specifically**
  (e.g. add JSON-field-aware parsing for `retryDelay`, `rate_limit_error`,
  etc.): rejected for this task. Tuning a parser against strings this task's
  own author wrote would fabricate confidence a live capture hasn't earned.
  If real agent-runtime error samples are captured later, that is new
  evidence to design against (see Revisit Triggers), not something to
  simulate now.
- **Leave `parse_retry_after_secs`'s panic as pre-existing debt, out of
  scope**: rejected. The bug was latent only because nothing called the
  function from `surge-acp`; this task is what makes it live, on the single
  task serving every bridge session. Shipping M0 without the fix would ship
  a crash, not a capacity signal.
- **Route `account` through the *un*normalized `RuntimeCfg::agent_id`
  string**: rejected. Three spellings of one registry entry would silently
  fragment one runtime's rate-limit history across three keys the moment two
  profiles disagreed on which alias to use.

## Consequences

- `surge-acp` and `surge-orchestrator` gained one breaking change each
  (`SendMessageError`, `StageError` both non-exhaustive now) — paid once,
  intentionally, rather than twice.
- `resolve_stage_error`'s persisted `StageFailed.reason` — which
  `surge-cli`'s inbox capacity scan (`scan_capacity_signal`) re-classifies —
  still round-trips correctly through `RateLimited`'s new `Display`, but
  only because `details` is spliced in verbatim and the field debug-dumps
  (`retry_after`, `account` — underscore/space, not hyphen) never shadow the
  real marker ahead of it. Pinned by
  `stage::tests::rate_limited_display_still_classifies_through_the_full_
  stage_failed_reason` — if that test goes red, the fix belongs in the
  `#[error(...)]` format string, not in the test.
- The `surge-acp/tests/bridge_rate_limit_classification.rs` real-wire proof
  is `#[ignore]`d and must be added to both `just test-ignored`
  (`justfile`) and its CI job (`ci.yml`) alongside the existing
  `-p surge-orchestrator` scope — tracked as a same-round follow-up, not
  deferred.
- M1's `CapacityPolicy`/`Decision` design should treat "park at a known wake
  time" as the exceptional case it measures as, not the common case the
  ticket's original framing assumed.

## M1: Model and Policy (`surge-core`)

**A1 — the key is renamed `account` → `runtime` throughout, ratified after
three rounds finding the same bug.** Task 11 already moved this field once
(`ProfileKey` → `agent_id`) because the name misdescribed the content;
leaving `account` on data that is, by every constructor this crate has,
purely a runtime identity would guarantee a fourth round. Surge never
stores provider credentials, and `builtin_registry.json` carries exactly
one launch configuration per registered runtime — there is no second axis
a distinct login could be keyed on today, so runtime and login coincide for
every installation this crate can express. `CapacityWindow.account` (field,
`account()` accessor, `observed_429`/`from_observed_error` parameters),
`StageError::RateLimited.account`, and the module doc's "per-agent-account"
framing are all renamed to `runtime`; `SendMessageError::RateLimited` is
untouched (the bridge layer never carried an identity to rename). Where a
future installation *can* express two logins on one runtime, this key errs
toward the safe side (over-parking both together) rather than the unsafe
one (under-parking either) — see A2 below for the follow-up that would
actually distinguish them. `SendMessageError`/`StageError`/`Decision`
marker questions from the API-gate pass are settled in-line on each type
(see `surge_core::capacity`'s doc comments); not re-litigated here.

**A2 (deferred, not this task):** introduce `Profile.runtime.login: Option<String>`
naming a specific `SurgeConfig.agents` entry, once the engine path actually
resolves through `agents` rather than solely through `Registry` — a real
cost concentrated in the launch-resolution path, not the scheduler, and
worth its own review rather than folding into a parking-policy change.

**Normalization decision: fix the write site, not every reader.**
`StageError::RateLimited.runtime` was already normalized at construction
(M0), but `EventPayload::SessionOpened.agent_id` — the field `surge-cli`'s
inbox capacity scan actually keys off today — was written raw, un-normalized,
in `engine/stage/agent.rs`. One fact (which runtime a session belongs to)
had two different values depending on which event you read, silently
reopening the exact `claude`/`claude-code`/`claude-acp` fragmentation this
ADR's M0 section already claimed was closed. Fixed at the write site
(`SessionOpened`'s construction, `agent.rs`), not by asking every reader to
normalize on its own: this is the only place the fact is ever produced,
while it already has one real reader and will gain another (M2's ledger);
"remember to normalize" is a contract this codebase's own standards single
out as the wrong shape for exactly this reason. Cost accepted knowingly:
the run event log is append-only, so any `SessionOpened` written *before*
this fix keeps its raw `agent_id` forever — this closes the gap for every
session opened from here on, not retroactively. A future reader spanning
both eras (a `surge-cli` inbox scan against a run started before this
change) still sees the un-normalized string for that older run; that reader
owns its own historical-data decision, this write-site fix cannot undo it.

**Rule order (`CapacityPolicy::decide`, pure, no I/O):**
1. exhausted, usable reset time known → `Park{ObservedReset, wake_at =
   resets_at}`. "Usable" is `seconds_until_reset(now).is_some()`, never
   `resets_at.is_some()` alone — a stale past `resets_at` is not evidence of
   anything.
2. exhausted, no usable reset time, `blind_backoff` configured →
   `Park{PolicyBackoff, wake_at = now + blind_backoff}`.
3. exhausted, no usable reset time, `blind_backoff` removed by the operator
   → `Dispatch{degraded: ExhaustedNoResetTime | ExhaustedResetElapsed}` —
   the two `Degraded` reasons distinguish "never had a reset time" from "had
   one, it went stale," which the fold and the log can both tell apart.
4. no `WorkEstimate` → `Dispatch{degraded: None}`, unconditionally — the one
   claim this milestone can actually measure (R35.1): absence of an
   estimate is never itself a reason to refuse. Pinned by a test that fails
   red under the exact defect this rule forbids (verified during review by
   deliberately reintroducing it).
5. otherwise (estimate present, not exhausted) — attempt a real comparison
   against the runtime's remaining capacity. **Structurally unreachable
   with any `CapacityWindow` this crate's own constructors can produce
   today**: `remaining` is only ever `None` or exactly `EXHAUSTED` (the sole
   live constructor is `observed_429`), so a genuine fraction never occurs
   in production, and `window` (a learned duration) is populated only by
   the rare `with_learned_window` path. Implemented as real code anyway (not
   a stub) so the arm is pluggable the day a producer exists; exercised in
   tests via direct construction bypassing the public API, which no
   production caller can do.

`Decision` carries **no** `#[non_exhaustive]` (each variant demands
categorically different caller behavior — a silently-absorbed future
variant would run the wrong one under the new variant's name) and **is**
`#[must_use]`. `WakeBasis` carries no `#[non_exhaustive]` either (a
consecutive-blind-park counter must match it exhaustively). `Degraded`
does carry it (its only consumer is a log line; over-inclusion costs
nothing).

**Rotation (R41) — shape only, not live.** `RotationPolicy` and
`Decision::Rotate` exist so the seam is ready, but `decide` never emits
`Rotate` in this delivery regardless of `RotationPolicy`'s value: verifying
a candidate targets a genuinely different `RuntimeKind` needs
`surge_acp::Registry`, which `surge-core` does not and should not depend
on. Shipping a live `Rotate` today — "next profile of the same runtime" —
would silently repeat rule 2/3's already-exhausted dispatch with extra
steps, worse than refusing outright. R41 remains deferred, against a
verified target, not yet scheduled as a numbered task.

**`RemainingShare` validation moved onto the type.** The derived
`Deserialize` bypassed `RemainingShare::new`'s `[0.0, 1.0]` range check
entirely — a `NaN` or out-of-range `f64` off the wire produced a
`RemainingShare` nothing had validated, and `NaN <= 0.0` is `false`, so
`is_exhausted()` would have read it as "capacity available." Fixed via
`#[serde(try_from = "f64")]`, routing every deserialization through `new`.
A future SQLite `remaining REAL` reader (M2) must do the same: a value that
fails `new` there is a corrupt/unreadable row, which is
`CapacityStatus::Unclassified`, never `NeverObserved` — those answer
different questions.

**Schema bump 6→7, unconditional.** `RunParked`/`RunWokeFromPark` are two
new `EventPayload` variants under one bump — `docs/schema-versioning.md`'s
field-composition exception (which covered a *field* added to the
already-v6 `SkillBound`) does not extend to a new variant, and "no tagged
release has shipped v6 yet" is a risk-radius observation about today's
blast radius, not a version-policy exemption. A v6-max reader has no
representation to decode either new variant into at all and must fail
closed with `SurgeError::SchemaTooNew`, exactly like every prior variant
bump (v2/v4/v5/v6).

**Parser widening (`parse_reset_hint`, additive next to
`parse_retry_after_secs`).** Three new, narrowly-anchored rules — OpenAI's
`"try again in Ns"` prose, Google's `retryDelay` protobuf-`Duration` field,
and Anthropic's `"usage limit reached|<epoch>"` suffix (accepted only when
the epoch is strictly future and within a 30-day horizon) — move 3 of the
4 real-provider-shaped bodies from M0's measurement table out of "no reset
time recognized." `parse_retry_after_secs` stays `pub`, signature and
contract untouched: M0 gave it a second cross-crate caller
(`surge_acp::bridge::worker`), so privatizing it would have broken the
build in the same commit that added that caller. Deliberately does **not**
copy `surge_acp::pool::parse_retry_after`'s unanchored numeric fallback or
its invented 60-second default — that fallback is exactly the fabricated
signal this module exists to refuse. All three parsers (`parse_retry_after_secs`,
`parse_reset_hint`, `pool::parse_retry_after`) carry a doc comment stating
they are intentionally separate (observation vs. policy) and must not be
unified — a regression test cannot catch its own premise disappearing, so
the comment is the guard a test can't be.

## Revisit Triggers

- Real agent-runtime error text is captured (from Claude Code / Codex /
  Gemini CLI actually rate-limited) — replace the reconstructed table with
  captured samples and update the achievability reading.
- ~~M2 designs the capacity ledger's key and resolves whether two
  credentialed accounts can share one runtime identity~~ — resolved in M1
  (A1): the key is the canonical runtime id (`runtime`, renamed from
  `account`); M2's ledger key follows directly. A2 (a real
  `Profile.runtime.login` distinguishing two logins on one runtime) remains
  open and not yet scheduled as a numbered task — see the M1 section above.
- A provider's `Retry-After` **HTTP header** turns out to be observable
  through some agent runtime's ACP adapter (as opposed to only its JSON
  error body) — worth a dedicated classifier note if so.
