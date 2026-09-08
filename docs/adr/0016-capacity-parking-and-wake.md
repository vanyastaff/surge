+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-09-06"
+++

# ADR 0016 — Capacity-aware rate-limit parking: classification, policy, persistence, and wake

## Status

Accepted, **M0–M5 (complete)**. Task 12 ("capacity scheduling") was six
milestones; this ADR was opened at M0 and grew a section per milestone as it
landed — M0/M1 (typed classification and the pure policy), M2 (durable
capacity + `wake_at`), M3 (the run-task wiring that actually calls the
policy), M4 (the daemon wake scheduler, jitter, and blind-park-limit
escalation), M5 (the inbox surface and this document). Sections are not
rewritten wholesale once landed — where a later milestone changed an earlier
decision (there is one: A1's key-is-the-runtime decision from M1 is final,
not provisional — see the M5 section below), the later section says so
explicitly rather than silently editing the earlier text. The M0 sections
above use the field name current at the time they were written (`account`);
M1 renamed it — see the M1 section below, which uses the post-rename name
throughout.

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
Agent Client Protocol (ACP) error text an agent runtime actually produces carry enough for
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
- `resolve_stage_error`'s persisted `StageFailed.reason` round-trips
  correctly through `RateLimited`'s new `Display` — `details` is spliced in
  verbatim and the field debug-dumps (`retry_after`, `account` —
  underscore/space, not hyphen) never shadow the real marker ahead of it.
  **Superseded by M5, not merely stale:** this only mattered because
  `surge-cli`'s inbox capacity scan (`scan_capacity_signal`) used to
  re-classify this exact persisted string, pinned by
  `stage::tests::rate_limited_display_still_classifies_through_the_full_
  stage_failed_reason`. M5 deletes both the scan and that test — the inbox
  capacity column no longer reads `StageFailed.reason` (or any run's own
  event log) at all, so this round-trip has no reader left depending on it.
  Left here as the historical record of why the format was ever
  load-bearing for `surge-cli`, not as a claim that it still is — see the
  M5 section below.
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
inbox capacity scan keyed off at the time this decision was made — was
written raw, un-normalized, in `engine/stage/agent.rs`. One fact (which
runtime a session belongs to) had two different values depending on which
event you read, silently reopening the exact `claude`/`claude-code`/
`claude-acp` fragmentation this ADR's M0 section already claimed was
closed. Fixed at the write site (`SessionOpened`'s construction,
`agent.rs`), not by asking every reader to normalize on its own: this is
the only place the fact is ever produced, while it already had one real
reader and would gain another (M2's ledger); "remember to normalize" is a
contract this codebase's own standards single out as the wrong shape for
exactly this reason. Cost accepted knowingly: the run event log is
append-only, so any `SessionOpened` written *before* this fix keeps its raw
`agent_id` forever — this closes the gap for every session opened from here
on, not retroactively.

**M5 update: the reader this cost was accepted for no longer exists, and no
other one replaced it.** At the time the paragraph above was written, "a
future reader spanning both eras" meant `surge-cli`'s inbox capacity scan
reading an old run's un-normalized `agent_id`. M5 deletes that scan
entirely — the inbox capacity column now reads `EventPayload::RunParked
.runtime` (canonical since that variant's introduction in this same
milestone, M1, with no un-normalized era to span at all — see the M5
section below) through a registry point-lookup, never `SessionOpened
.agent_id` in any form. The append-only cost accepted above is therefore
inert for capacity purposes today: nothing in the capacity path reads
`agent_id` at all, normalized or not. This does not retroactively normalize
old `SessionOpened` events (the write-site fix still cannot undo that), it
just means the one reader that cared about the gap is gone.

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

## M2: Durable observation (`surge-persistence`)

**Table:** `migrations/registry/0015_runtime_capacity.sql` —
`runtime_capacity(runtime TEXT PRIMARY KEY, remaining REAL, resets_at_ms
INTEGER, window_secs INTEGER, source TEXT NOT NULL)`. One row per runtime
(a capacity window is a point sample of "the last thing Surge saw", not a
history — `runs::capacity::observe` upserts in place), columns nullable
exactly where `CapacityWindow`'s own fields are `Option` and nowhere else.
`migrations/registry/0016_runs_wake_at.sql` adds `runs.wake_at INTEGER`
(additive, `NULL` for every pre-existing row), written by
`registry::set_run_parked` alongside the status transition to `Parked` in
the same statement, and read by `registry::due_parked(now)` (M4's poll
target) and `registry::clear_parked` (the resume path).

**No `observed_at_ms` column, deliberately.** The original schema draft
carried one; it was cut before landing because nothing in the workspace
ever reads it — `runs::capacity::status` doesn't project it into
`CapacityStatus`, and no query selects it — while every `observe` call
would have had to invent a value to write. A column with a mandatory
writer and zero readers is exactly the kind of debt this module's own
"observe only what has an actual consumer" philosophy argues against.
Staleness is already representable one layer up
(`CapacityWindow::seconds_until_reset` returns `None` once `resets_at` has
passed), so this schema does not need a second mechanism for the same
fact. Adding it back later is one additive migration, if a real reader
ever shows up.

**Fails open on an unnormalized key, by design, at this layer.**
`runs::capacity::observe`/`status` do not call
`Registry::normalize_agent_id` themselves — `surge-persistence` does not
depend on `surge-acp`, and `surge-core`'s capacity model is deliberately
registry-free — so an unnormalized `window.runtime()` (e.g. `"claude"`
when every prior row was written under `"claude-acp"`) does not error
here, it silently writes or reads a *second* row. This is exactly the
"insufficient refusal" direction Task 12's own risk table rules out, and
M2 does not close it: closing it is M3's job, in the type system, not
prose at this layer (see the M3 section's `CanonicalRuntimeId` below).

## M3: The run-task wiring (`surge-orchestrator`)

**`CanonicalRuntimeId` — the acceptance criterion M2 deferred, closed at
the ledger port.** `engine::capacity::CanonicalRuntimeId` is constructible
only through `CanonicalRuntimeId::resolve(registry, raw_agent_id)`, which
performs the one normalization this crate already trusts
(`surge_acp::Registry::normalize_agent_id`, with the same raw-id fallback
`StageError::RateLimited.runtime` and `SessionOpened.agent_id` already
use). `CapacityLedger::observe`/`status` both take `&CanonicalRuntimeId`,
never a bare `&str` — every real call site in this crate goes through
`resolve` before it can reach the table. **Precisely, not absolutely**:
`surge-persistence`'s own `Storage::capacity_status`/`observe_capacity`
remain `pub` and raw-`&str`-keyed (M2's fail-open behavior above still
exists at that layer), so the guarantee is "every call this crate actually
makes is normalized," not "a raw string can never reach the table by any
path" — a future caller bypassing this port entirely is not stopped by
the type system, only discouraged by there being no reason to.

**`dispatch_agent_node_with_capacity_gate` — the two places `decide`
actually runs.**
1. **Before dispatch** (the precheck): resolve the node's runtime, read
   `CapacityLedger::status`, call `CapacityPolicy::decide`. `Decision::Park`
   returns `StageDispatch::Park` without ever calling the agent —
   dispatch simply does not happen, which is what R37 asks for literally
   ("refuse to start work"), not a warning next to a dispatch that
   proceeds anyway.
2. **After a `StageError::RateLimited`**: the fresh 429 is `observe`d into
   the ledger and `decide` runs again on that freshly-built window
   *before* the error reaches the `on_error` hook chain. This is the
   mechanism that keeps parking ahead of the retry cycle — without it, a
   hook that routes the failure back to the same node (an ordinary "retry
   on failure" pipeline shape) would re-dispatch straight into the same
   exhausted window, burning an attempt against a wall `wake_at` says will
   not move. Pinned by
   `engine_capacity_park_test.rs::
   rate_limited_agent_parks_instead_of_dispatching_a_second_node_on_the_same_runtime`
   ("zero retries after a 429: exactly one SendMessage may have reached
   the bridge").

**Three mechanisms keep this from livelocking**, each answering a
different way "keep re-parking forever" could happen:
1. **The precheck bypass on resume**
   (`RunTaskParams::capacity_precheck_bypass_once`, consumed — flipped to
   `false` — on read). A runtime whose reset time was *never learned*
   (`resets_at: None`) has no way to refresh that fact except a real
   dispatch attempt, and the precheck exists precisely to block dispatch
   attempts on an exhausted runtime — without a bypass, those two facts
   deadlock each other forever. The bypass fires for exactly the first
   agent dispatch after a resume, so a genuine outage still burns exactly
   one real attempt per wake before re-parking (bounded), while a resolved
   one clears the row via mechanism 2 below.
2. **Clearing the ledger row on a non-rate-limited outcome.** Any dispatch
   result that is not `StageError::RateLimited` clears
   `CapacityLedger::clear(&runtime)` for that node's runtime. This is what
   lets a row with no learned reset time stop haunting every future
   dispatch after a single successful bypass attempt, rather than only the
   one dispatch right after it.
3. **Rule 2's unconditional dispatch on a stale reset** (see the M1
   section above) — a *known, elapsed* `resets_at` dispatches regardless
   of `blind_backoff`, so a one-time observation whose reset time has long
   since passed cannot re-park on a policy guess forever; it gets a real
   attempt on every single dispatch once elapsed, not just the first one
   after a resume.

(A fourth mechanism — the blind-park-limit escalation cap — bounds a
*human-visible* consequence of repeated guessing rather than preventing
the parking itself; see the M4 section below. It does not stop the run
from parking again, it stops the run from parking silently forever.)

**Rotation (R41) is verified and still always refused, per the M1
section's design — `stage/agent.rs::verify_rotation_target` /
`RotationRefusal` are real code, not a stub.** Every one of
`RotationRefusal`'s four arms is a refusal:
`CurrentRuntimeUnresolved`/`CandidateRuntimeUnresolved` (nothing to rotate
from/to), `DifferentRuntimes` (rotation, as specified, targets "the next
profile of the *same* runtime," not a different agent), and — the case
the original R41 design called "allowed" —
`SameRuntimeIsSameAccount` (both profiles resolve to the identical
runtime, which today has exactly one launch configuration, so "rotating"
to it is a no-op that would silently repeat the already-exhausted
dispatch). `CapacityPolicy::decide` structurally cannot emit
`Decision::Rotate` in this delivery (`RotationPolicy` is always
`Disabled` — `From<&CapacityConfig>`'s only output), and even if it could,
`verify_rotation_target` would refuse every candidate anyway. R41 remains
not deliverable until a real second-account model exists (A2, below) and
is not scheduled as a numbered task.

**`CapacityConfig` gained the representability check the M1-ticket review
asked for.** `validate()` now checks two layers, not one:
`chrono::Duration::try_seconds` (whether `blind_backoff` fits that type's
own ~292-billion-year range) and, separately,
`Utc::now().checked_add_signed(delta).is_some()` (whether *adding* it to
an actual instant stays inside `DateTime<Utc>`'s much narrower
~262,000-year range). A value like `9_000_000_000_000s` (~285,000 years)
clears the first layer easily and only the second one catches it — before
this fix such a value loaded cleanly and was silently clamped to
`DateTime::<Utc>::MAX_UTC` three layers down inside `decide`, exactly the
outcome `validate`'s own doc says it exists to prevent. An operator now
gets a config error at load time, naming the field, instead.

## M4: Wake scheduler (`surge-daemon`)

**`WakeScheduler`** polls `Storage::due_parked(now)` on a
`DEFAULT_POLL_INTERVAL` of 30s and resumes every due row through
`server::resume_run_tracked` — the *same* seam
`recovery::DaemonRecoveryEffects::resume` calls for a startup-recovered
run, so admission, the broadcast registry, and the frozen-budget re-arm
(`Engine::resume_run`, the mechanism from commit `1ed5caa`) all apply
identically regardless of which of the two paths woke the run. Pinned by
`engine_capacity_park_test.rs::resuming_a_parked_run_re_arms_its_frozen_token_budget`.
Not folded into `inbox::snooze_scheduler::SnoozeScheduler` despite the
near-identical ~15-line "poll on an interval, act on what's due" shape —
the two operate on unrelated domains
(`ticket_index`/`TicketState::Snoozed` for the snoozer;
`runs`/`RunStatus::Parked`/the run registry here) and rule-of-three says
two occurrences do not earn an abstraction. The clock is injected
(`Arc<dyn Clock>`, the same precedent `RecoveryOptions::now_ms` set), not
read inline, so wake timing is deterministic under test.

**Recovery integration — `SkipParked` sits at decision-order position 5,
deliberately between the worktree check (4) and the idle-stuck check
(6).** Before the worktree check: a parked run's worktree is expected to
exist (parking never deletes it), so this ordering is not load-bearing the
way the terminal-log-vs-worktree ordering is, but placing the parked check
early keeps a parked-and-not-due run from being evaluated against later
rules at all. **After it, and specifically before the stuck-idle check, is
load-bearing** (Task 12 M3 review, BLOCKING #2): a parked run's silence
for however long the provider window (or blind backoff) takes is
*intentional*, not evidence of a wedged run — without this ordering, a run
parked longer than `stuck_threshold` would be flagged `FlagStuck` instead
of quietly waiting out its own park. A **due** parked run does not get a
special "due" action; it falls through the same idle check and default
`Resume` every other healthy run does — `Engine::resume_run` itself clears
`Parked`/`wake_at` and arms the one-shot capacity-precheck bypass (M3),
so recovery does not need to special-case "due" separately from an
ordinary resume.

**Jitter (`CapacityPolicy::apply_park_jitter`) is the herd-avoidance
mechanism, separate from the three livelock mechanisms in the M3
section.** Several runs parked on the same exhausted runtime compute the
identical `wake_at` from the identical `CapacityWindow`; without jitter
they would all wake in the same scheduler tick and re-hit the
still-exhausted (or freshly-reset) window together — a stampede, not a
livelock, but wasteful and bursty. `apply_park_jitter` adds a
deterministic `[0, jitter_max)` offset derived from `run_id` alone (SHA-256
of the ULID bytes, not `DefaultHasher` — that hasher's per-process
`RandomState` seed would make replay disagree with the original run about
its own `wake_at`), applied once at park time and persisted, never
re-derived. Default `jitter_max` is 30s against a default `blind_backoff`
of 5 minutes (10%) — enough to break up a herd, not enough to meaningfully
delay any single run past what the policy already decided.

**Blind-park-limit escalation is a human-visible-consequence bound, not a
livelock fix.** `CapacityConfig::blind_park_limit` (default 5) caps
*consecutive* `WakeBasis::PolicyBackoff` parks with no successful dispatch
between them — folded from the run's own event log by
`surge_persistence::runs::query::aggregate_status`:
`RunParked{basis: PolicyBackoff}` increments
`RunStatusSnapshot::consecutive_blind_parks`; `StageCompleted` resets it
(and clears `blind_park_limit_escalated`) — a real dispatch succeeding is
proof the runtime is not blindly stuck; `EscalationRequested{cause:
CapacityBlindParkLimitExceeded}` sets the escalated flag so the scheduler
raises it only once per streak, not once per tick. `WakeScheduler` reads
this same snapshot (already paid for per due run) and, once the limit is
crossed and not yet escalated, appends
`EventPayload::EscalationRequested` (schema v8 — see below) and sends a
desktop notification via the same `NotifyDeliverer` `recovery.rs`'s
stuck-run path uses. The run keeps parking and waking unchanged — this
does not stop the guessing, it makes a long unproductive streak visible to
a human, the same role `engine/stage/agent.rs`'s loop guard and
`engine/bootstrap.rs`'s edit-loop cap play for their own repeat conditions.

**Schema v7 vs. v8 — the distinction is "new top-level variant" vs. "new
variant of a nested enum," not two instances of the same thing.** v7
(`MAX_SUPPORTED_VERSION` 6→7, M1) added
`EventPayload::RunParked`/`EventPayload::RunWokeFromPark` — two entirely
new top-level variants of `EventPayload` itself. v8 (7→8, M4) added
`EscalationCause::CapacityBlindParkLimitExceeded` — `EventPayload::
EscalationRequested` was already a v7-and-earlier variant, unchanged; what
is new is one variant of `cause`'s own nested enum type. Both still force
a version bump for the identical underlying reason: a reader with no
representation for a tag cannot decode it as a missing-field default, it
is an unknown-tag error, whether the tag lives on `EventPayload` directly
or on an enum reachable from inside one of its variants.
`EscalationRequested.cause` is itself `#[serde(default)]` (a v7-max reader
tolerates the *key being absent* on an old payload), but
`EscalationCause`'s `#[serde(rename_all = "snake_case")]` has no
`#[serde(other)]` catch-all, so a payload that actually *carries* the new
tag fails exactly like an unrecognized top-level variant would — the same
failure mode one level deeper, not a different one. `docs/schema-versioning.md`
documents this distinction generally ("the same logic governs any nested
closed enum reachable from a persisted payload," not just
`EventPayload`'s own variants); this ADR names the concrete instance.

## M5: Surface and documents (`surge-cli`, this document)

**`surge inbox`'s WAITING group now carries the wake time and its basis,
typed, not just the "waiting" label.** `Attention::Waiting` (M1) grew a
second field: `basis: WakeBasis`, forwarded from `RunState::Pipeline
.parked.basis` alongside the pre-existing `until`, so an operator (and a
`--format json` consumer) sees not just *when* a parked run resumes but
*why* — an actually-observed provider reset vs. a policy-backoff guess —
without re-deriving it from the run's event log. `InboxEntry` carries the
same two facts as `wake_at: Option<DateTime<Utc>>` /
`wake_basis: Option<WakeBasis>` (both `Some` exactly when `attention ==
"waiting"`, `None` otherwise, `#[serde(skip_serializing_if =
"Option::is_none")]` so every non-parked entry's JSON is unchanged).

**The WAITING group's presence is now pinned by a test that can actually
fail.** Before this milestone, every test that exercised a parked run
asserted on `InboxEntry::attention` — a classification fact — never on
what `print_inbox` actually printed; deleting the entire `if
!waiting.is_empty() { .. }` block from `print_inbox` left all of
`surge-cli`'s tests green (a review-caught mutation). `print_inbox` is
split into a thin `stdout`-locking wrapper and a testable
`print_inbox_to(writer, entries, show_done)` that writes through a generic
`impl std::io::Write`; a new test constructs a `"waiting"` entry directly,
captures the printed bytes into a `Vec<u8>`, and asserts the WAITING
heading, the rendered wake time, and the basis label are all present.
Verified directly (not just argued) by temporarily deleting the
`if !waiting.is_empty()` block and confirming that test — and only that
test — goes red; restored before landing.

**The capacity column moved to a single home: the registry, not the
journal.** Before this fix, `scan_capacity_signal` derived `surge inbox`'s
capacity column by folding *this run's own* event log — the most recent
`StageFailed` reason, attributed to the runtime from the preceding
`SessionOpened.agent_id`. That conflated two different facts under one
name: "what happened to this specific run" (a run fact, correctly a
journal question) and "is this runtime currently rate-limited" (a
runtime/account fact, which the M2 ledger already exists to answer). Two
homes for the same fact diverge on the first opportunity: run A's own
journal knows nothing of the 429 run B observed on the *same* runtime, so
run A's column stayed silent while `CapacityLedger`/`decide` had already
parked on that exact exhaustion — the scheduler and the operator-facing
display could disagree about the state of the same runtime. The fix keeps
the split clean going forward:
- **"What is known about this runtime"** → the registry's
  `runtime_capacity` table, one canonical-keyed point lookup
  (`CanonicalRuntimeId::resolve` + `Storage::capacity_status`) — the same
  table `CapacityLedger` itself reads before every dispatch, so the inbox
  and the scheduler can no longer disagree about one runtime's state.
- **"Is this run parked, and until when"** → still the run's own journal,
  still a fold (`RunParked`/`RunWokeFromPark` via `Attention::Waiting`) —
  a run fact, untouched by this change.
`scan_capacity_signal`, `scan_run_capacity`, and the per-node
`CapacityStatus` merge they implemented are deleted outright, not
refactored — `commands/inbox.rs`'s module doc and `classify`'s doc carry
the full accounting.

**Deliberate narrowing for terminal runs — this walks back part of what
Task 11's review fixed, for a different reason, and that has to be said
plainly.** Task 11's own review caught `classify` short-circuiting on
`is_terminal()` before a capacity scan ever ran, so a run killed by a 429
(`Failed`, or `Aborted`/`Crashed` if the daemon died mid-retry) showed no
signal at all — the fix back then was to scan those terminal runs too, not
skip them. **This milestone re-introduces exactly that gap, on purpose,
for a different reason than the one Task 11 fixed.** Task 11's gap was a
short-circuit bug: the scan was cheap enough and correct enough to run, it
was just skipped. Today's gap is different: it is *impossible to source
cheaply and honestly* under the new, correct model. The registry lookup
needs a specific canonical runtime to key on; a currently-parked run has
one for free (`RunParked.runtime`, a fact its own journal already carries
for the wake-time display). A terminal run has no such fact without a full
journal fold — the exact per-run read this milestone removes as accepted
Task 11 debt (a fresh `RunReader` plus `read_events(0..MAX)` per terminal
run, on every `surge inbox` invocation). Re-adding that read would undo the
performance fix to restore a display line, and worse, *guessing* a
terminal run's runtime from whichever registry row happens to exist right
now would attribute the registry's current, possibly unrelated state to a
dead run's old failure — the same kind of unearned inference the scan
itself is being deleted for. Net effect, named without softening: a
Failed/Aborted/Crashed run's capacity column is `NeverObserved` again,
unconditionally, same surface symptom as the bug Task 11 fixed, opposite
cause, and an operator diagnosing a dead run's own rate-limit history from
`surge inbox` alone has lost that display — real information a future
change (a real per-run runtime record on `RunSummary` itself, a registry
migration and its own design decision, not a guess) would have to restore
deliberately, not stumble back into.

**`agent_id` risk (M1, above) is inert for this column, not resolved.** The
new capacity lookup never reads `SessionOpened.agent_id` in any form —
only `EventPayload::RunParked.runtime`, canonical since that variant's
introduction in this same task's M1 (both write sites in
`engine::run_task` build it via `CanonicalRuntimeId::resolve` before
constructing the event, so there is no un-normalized era for `RunParked`
to span the way `agent_id` has one). The historical-data caveat the M1
section raised for `agent_id` still holds for whatever *does* still read
that field; it simply does not apply here, because nothing here reads it.

**A second, duplicate instance of the same stale rationale existed in
`surge-acp::health.rs`, not `surge-cli`.** The ticket named lines
336–344, whose comment justified `RATE_LIMIT_PATTERNS`' breadth by saying
over-inclusion "only changes what an operator is shown, not a routing
decision." That was true when written — the classifier's only consumer
was `HealthTracker`'s own `self.capacity` map, read by `surge doctor`, a
display path — but Task 12 gave `looks_like_rate_limit` a second, load-
bearing caller: `surge_acp::bridge::worker::classify_prompt_dispatch_error`
uses it to decide `SendMessageError::RateLimited` vs. a generic `Bridge`
error on the live engine dispatch path, and M3 wires a `RateLimited`
result into `CapacityLedger::observe` → `CapacityPolicy::decide`, which
can return `Decision::Park` — actually stopping dispatch. A pattern this
list over-matches on (`overloaded_error` — Anthropic's 529-style
transient-overload signal, not quota exhaustion, is the concrete case;
see the M1 measurement table) now parks a run that could have retried or
failed normally, not just mislabeled a `surge doctor` line. Both
`health.rs` doc comments making this claim (the one at the `record_failure`
call site and the near-identical one on `is_rate_limited_for_routing`, a
few lines above it) are rewritten to say so; the classifiers themselves
are unchanged — this is a documentation correction, not a behavior change,
and it does not by itself widen or narrow `RATE_LIMIT_PATTERNS`.

**A1 (M1's "key is the runtime, not an account") is confirmed final by
everything M2–M4 built on it, not merely unrevisited.** M2's table key,
M3's `CanonicalRuntimeId`, and M4's per-runtime wake scheduling all assume
one row/one capacity fact per canonical runtime id; nothing in M2–M5
introduced a second identity axis or found A1 insufficient in practice.
The over-parking cost A1 accepted (two credentialed logins sharing one
runtime share one capacity window) remains real and unmeasured — no
installation in this codebase's test suite or documentation exercises two
logins on one runtime — and is still deferred to A2, unscheduled.

**Deliberately not built in this delivery, named so an operator does not
assume otherwise:**
- **Rotation across accounts is not shipped.** Not "not wired yet" —
  structurally unrepresentable under today's one-launch-configuration-per-
  runtime model (see the M3 section's `RotationRefusal` above). `[capacity]
  .rotation` is not a `surge.toml` field; there is nothing to enable.
- **`WorkEstimate`/rules 5–6 of `decide` do not affect any real dispatch
  today.** Every `CapacityWindow` this crate's own constructors can
  produce is either not exhausted (estimate is irrelevant — rule 5 always
  dispatches) or exhausted with `remaining` exactly `EXHAUSTED` (handled by
  rules 1–4, never reaching the comparison). The comparison arm (rule 6) is
  real code, not a stub, so a future estimate producer can plug in without
  a `decide` change, but nothing wires one in this delivery.
- **The capacity window learns only from observed 429s
  (`CapacityWindow::observed_429`/`from_observed_error`), never from an ACP
  `session/usage` signal.** `CapacitySource::AcpUsage` exists on the enum
  for forward compatibility, but `agent-client-protocol` 0.10.2's
  `unstable_session_usage` feature does not expose a provider rate-limit
  window, remaining share, or reset time at all (checked against the crate
  source, not assumed — see the M1 section's module-doc citation). There is
  nothing to wire, not "not wired yet."

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
