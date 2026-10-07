---
title: "feat: enforce agent, model, effort and thinking restrictions"
type: feat
status: draft
date: 2026-10-07
---

# Agent restrictions — v1 task 1.5

Owner: release coordinator; implementation and independent review through
Rust Studio dev-task. This completes task 1.5 of the
[v1 plan](2026-10-05-002-feat-v1-release-plan.md); it does not retire other tasks.
Active release verification targets macOS. Implementation is authorized, but
this plan remains pre-code until independent plan review passes.

## Observable contract

The operator can disable an agent route or provider family, disable models,
and allow specific models, effort levels and independent thinking modes.
Forbidden assignments fail at flow admission with an actionable diagnostic.
Planners, retry ladders and quota fallback use the same policy. Actual ACP
defaults and acknowledged values must satisfy policy before a prompt; a
reported mid-session violation stops further host-authorized effects and
fails the stage. No provider-specific category or model name is guessed.

## Configuration and ownership

```toml
[restrictions]
disabled_agents = ["claude-acp"]
disabled_provider_families = ["anthropic"]

[restrictions.agents.codex-acp]
allowed_models = ["gpt-6-sol"]
disabled_models = ["gpt-6-luna"]
allowed_effort = ["high", "xhigh"]
allowed_thinking = ["enabled"]
model_option_id = "model"
effort_option_id = "reasoning_effort"
thinking_option_id = "_thinking_mode"
```

Pure policy belongs in `surge-core::agent_restrictions`, exposed through
`SurgeConfig.restrictions`. Use typed violations and dimensions; exact canonical
ACP value IDs, never translated labels. Optional ordered sets distinguish an
omitted allow list (unconstrained) from an empty list (deny all). Reject blank
IDs, unknown fields and contradictory allow/deny entries. Registry aliases
resolve to canonical route IDs before policy lookup; duplicate aliases must
not create a bypass.

Add explicit optional `provider_family` registry/config data, with builtins
declared as data and custom agents configurable. Do not infer family from
commands, runtime kind or tags. Family rules fail closed for unknown family;
family denial overrides per-agent permission. Disagreement with the explicit
capacity-route family is an error.

Thinking mode is a separate exact ACP option-ID selection. Generalize selection
targets to category or exact option ID; ambiguous/missing constrained selectors
fail. `thought_level` remains effort, never a substitute for thinking mode.

## Implementation closure

1. Core policy, validated configuration, runtime-field diagnostics and thinking
   accessor. Preserve operator constraints across deliberate runtime moves.
2. Propagate policy to every production engine constructor and standalone stage
   entry point. Freeze its provenance in the run; resume intersects the frozen
   permission with the current host policy, never expanding an accepted run's
   permission or ignoring a new prohibition. No hot reload is added.
3. Central effective-assignment resolution applies node/profile runtime choice,
   frozen candidate model and profile effort floor before restrictions. Validate
   main graphs, referenced nested subgraphs and dynamic splits before reservation
   or provider opening. Recommended model metadata alone is not proof of the
   actual selected session model.
4. Filter planner catalog, quota recipes, fallback rotation and verifier retry
   ladder; recheck restored and final assignments immediately before dispatch.
5. ACP negotiation ingests opening/resume/load state, resolves exact selections,
   applies model, replaces the catalog with the setter's complete response, then
   resolves effort/thinking against refreshed options. Check final current values,
   including defaults, and require restricted selections rather than best effort.
6. Retain authoritative session-option state. Process `ConfigOptionUpdate` even
   while ingress is historical/idle. A violation permanently invalidates the
   session, cancels its prompt, fences subsequent prompts/permissions/file,
   terminal and injected tool effects, and emits a public failure diagnostic.
   Reuse retained child settlement instead of adding a second cleanup owner.
7. Update example config, user documentation and acceptance evidence. Record
   executed checks and keep task open until independent spec and quality reviews.

Primary sites: core config/agent config; ACP registry, bridge session/worker/
session_inner/client/lifecycle; orchestrator engine config/validation/admission,
capacity_routes, run_task, stage/agent, feature_driver; CLI/daemon/UI wiring.
No dependency upgrade is needed for the currently pinned ACP schema.

## Acceptance evidence

- Denied primary, profile-derived/node assignment, nested subgraph and dynamic
  split fail before reservation/provider RPC; denial names node, route and rule.
- Omitted and empty allow lists differ. Malformed policy/runtime fields fail.
- Aliases cannot bypass route/family denial; unknown or conflicting family fails
  under a family constraint. Planner, quota, fallback and ladder exclude denials.
- Effective profile effort floor is checked; restricted values cannot become
  best-effort defaults. Restored candidates obey tightened host permissions.
- Uncorrected forbidden advertised defaults fail with zero prompt/host-effect dispatch.
  Display aliases cannot authorize prohibited canonical IDs.
- Explicit IDs support uncategorized options; ambiguous categories fail.
  Grouped options and a separate thinking selector work. Unsupported thinking
  under an active rule fails before prompt.
- Model setters can change effort/thinking choices; the refreshed catalog is
  used. A setter returning a different current value fails.
- Resume/load defaults and idle/setup updates obey the same checks.
- Mid-prompt forbidden/missing options cancel the session; later host effects
  and prompts fail, and the stage cannot report verified success.
- Unrestricted sessions without config options retain existing behavior.

## Negotiation, identity and effect ordering

Opening/resume/load setup is an inactive negotiation state: prompts and host
effects are fenced. An initially forbidden default may be corrected by an
explicit permitted selection. Intermediate dependent option changes are allowed
only during this inactive negotiation. The complete final state must satisfy
every predicate before activation. Once active, a violating update permanently
invalidates that session; a later allowed response cannot reactivate it.

All complete option snapshots are ingested by the existing single-threaded
session owner with a monotonic generation. Setter responses and notifications
are ordered observations, not interchangeable cached values. Capture the
generation when sending a setter; a response received after a newer notification
cannot overwrite that newer state or authorize a prompt. Resolve dependent
selections against the newest state; bounded re-negotiation may obtain a fresh
acknowledgment, otherwise fail with a diagnostic. Check the latest generation
again at prompt admission without an intervening await. Once active, invalidation
is latched irrespective of response order or late prompt completion.

Freeze resolved canonical route, explicit family identity and selector bindings
alongside policy provenance. Frozen and current predicates are independent
conjunctions; never intersect only value sets or overwrite the frozen selector.
Different IDs must each be satisfied, or structural incompatibility must be
reported before dispatch. Changed family/route identity on resume is a conflict,
not an implicit expansion. Legacy runs without frozen provenance remain explicitly
legacy; current constraints still apply and cannot manufacture historical proof.

Effort ordering reuses the existing core vocabulary and ranking currently
implemented by the engine (`minimal`, `low`, `medium`, `high`, `xhigh`, `max`,
verified in `stage/agent.rs::effort_rank`). Extract one shared rank resolver,
then apply floor before canonical value-policy checks. Opaque advertised values
do not acquire a rank from labels. A required constrained floor that cannot be
established fails closed; unrestricted historical best-effort profile preferences
retain their existing behavior. Route changes re-evaluate floor compatibility.

Effect admission linearizes immediately before the actual host operation: an
effect already admitted may finish, but a pending approval, terminal acquisition
or injected/MCP effect must recheck session validity after its await and before
execution. Pending permission replies resolve as cancellation after invalidation.
Use the existing effect-fence boundary and retained settlement owner. A policy
failure survives cancellation/cleanup and dominates a late successful prompt
result or reported outcome.

Additional deterministic tests cover both response/update orders, update between
validation and prompt admission, late allowed response after invalidation,
forbidden default corrected by explicit choice, model-dependent intermediate
values, changed selector/family identity on resume, unknown ranked effort and
fallback vocabulary, and invalidation while approval/terminal acquisition waits.
Also reject duplicate option IDs/ambiguous bindings, test registry alias shadowing
between freeze and resume, and hold the terminal lock while invalidating the
session. An outcome already queued for the engine must carry/check the session's
validity generation at acceptance; invalidation must defeat queued success even
if its event was sent first and cleanup later finishes normally.

## Dependency-closed stages

1. Pure core policy/types and ranking/intersection tests. No user configuration
   advertises an enforceable setting until its runtime closure is implemented.
2. Configuration/provenance and assignment/planner/quota/ladder admission.
3. ACP negotiation, updates, effect fencing, full wiring, documentation and outer
   acceptance. Stages 2 and 3 may require a combined checkpoint because publishing
   partially enforced user configuration would misrepresent its behavior.
4. Independent spec review, then quality/security review and current gates.

Stage 1 completed on 2026-10-07 with independent spec/API/security review:
[core evidence](../release-evidence/2026-10-05/agent-restrictions-core/README.md).
Stages 2–3 remain open. `AgentRestrictions::constrains()` reports value rules;
consumers must evaluate declared selector bindings even without a value rule.

For behavior changes record a failing outer test before implementation, then
passing unit/integration/mock-ACP fixtures. Run format, strict Clippy in the
project's feature sets, relevant nextest suites and MSRV. Only one Cargo process
may build at once; check disk before broad tests (6.2 GiB available at discovery).
Final native candidate/provenance gates occur after all v1 source changes.

## Pre-code review and sources

Read-only scout mapped the sites and bypass paths. API-design lead returned
**ACCEPTABLE**, conditional on the complete admission and actual-session-state
closure above. Independent adversarial review returned **RESHAPE NEEDED** for
ordering, independent frozen predicates, effort semantics and effect admission.
The explicit contracts above address those findings. Re-review returned
**ACCEPTABLE** from both the adversarial and API/security reviewers. The plan is
approved for the authorized implementation. No implementation completion is claimed.

[Official ACP session config options](https://agentclientprotocol.com/protocol/v1/session-config-options)
was checked through Keenable and Firecrawl on 2026-10-07. Categories are optional
UX metadata; exact IDs identify controls. Setter responses and update messages
carry complete state, including dependent choices and provider-driven switches.
The existing worker uses stale opening options for every selection, discards
setter state, and ignores config updates; these are implementation sites.

ACP observations can stop further host-authorized effects after an announced
violation. They cannot prove absence of hidden vendor computation before the
announcement. This policy is configuration enforcement, not process containment.
