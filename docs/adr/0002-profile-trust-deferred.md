---
status: accepted
deciders: vanyastaff
date: 2026-05-07
supersedes: none
---

# ADR 0002 — Defer profile trust and signature verification to post-v0.1

## Context

The roadmap entry for `Profile registry & bundled roles` lists, among other items:

> Trust / signature story for shared profiles — decide or explicitly defer to post-v0.1.

Profile authoring guidance includes hooks (`pre_tool_use`, `on_outcome`, etc.), tool allowlists (`default_mcp`, `default_skills`, `default_shell_allowlist`), and sandbox intent. A maliciously authored profile could plausibly request a wider sandbox tier or wire a hook that exfiltrates data. This raises the question of whether the v0.1 registry should ship with a trust model — signature verification, publisher allowlists, content hashing — before users can pull profiles from outside the bundled set.

## Decision

**Trust and signature verification for profiles is explicitly deferred to post-v0.1.**

For v0.1 the registry resolves profiles only from two sources:

1. **Bundled profiles** shipped inside the `surge-cli` / `surge-daemon` binaries via `include_str!` (compiled from `crates/surge-core/bundled/profiles/*.toml`). Trusted because they ship as part of the binary the user installed.
2. **Local disk profiles** under `${SURGE_HOME}/profiles/` (default `~/.surge/profiles/`). Trusted because they live in the user's home directory and are written by the user (or by `surge profile new <name>`).

There is **no** remote profile fetch in v0.1, no signature header in the profile schema, no publisher allowlist, and no `surge profile install <url>` subcommand. See the milestone Out-of-Scope list:

> - Remote profile download / fetch — bundled + disk only in v0.1.
> - Profile signature verification — see this ADR.

## Rationale

- **Single-user, single-machine.** Surge's threat model (`.ai-factory/DESCRIPTION.md` § Non-Functional Requirements) is a local daemon talking to one developer. Trust boundaries that matter at that scale are: the agent runtime's sandbox, the user's filesystem, and the user's tracker credentials. None of those are crossed by adding a profile to `~/.surge/profiles/`.
- **The agent runtime owns enforcement, not surge.** A profile's `sandbox` field is an *intent* the agent runtime maps to its native flags. A profile cannot escape the runtime's sandbox just by claiming a wider tier; the runtime decides what actually happens. So even an adversarial profile has to go through the same elevation-approval round-trip as any other (`SandboxElevationRequested` event → notify channel → `ApprovalDecided`).
- **No distribution surface yet.** A signature-verification scheme without a distribution channel solves nothing. We would design a key format, a publisher registry, and a verification API, then ship none of them in v0.1 because there's nowhere for users to fetch profiles from. The decision-cost is real and the user-value is zero.
- **Signature design depends on distribution design.** When remote distribution lands (post-v0.1), the trust model can be designed against the actual delivery channel (cargo-style index? OCI registry? Git-based?) instead of guessing. Locking in a signature format now would constrain choices later.

## Out-of-scope guarantees in v0.1

This ADR explicitly does NOT promise:

- Sandbox enforcement against malicious profiles. The agent runtime owns this.
- Profile content hashing or integrity verification on disk reads.
- Publisher identity, key management, or revocation lists.
- Audit trail of which profile was used for which run beyond what the existing event log records.

## Consequences

- The registry code paths in `surge-core::profile::registry` and `surge-orchestrator::profile_loader` do not need to add signature-verification interfaces. The `Provenance` enum has three variants (`Versioned`, `Latest`, `Bundled`) — no `Signed` variant.
- `surge profile validate <path>` checks schema, Handlebars syntax, and `extends` parent existence. It does **not** check signatures or fetch trust metadata.
- `surge profile new <name>` writes a profile to disk with no integrity field. The user is the source of trust.
- A future `Profile trust & signature verification` milestone will land alongside (or after) a `Profile distribution channel` milestone. The two designs ship together because each constrains the other.

## Revisit conditions

This ADR should be revisited (and likely superseded) when any of the following becomes true:

- A remote profile distribution channel is on a roadmap milestone with a target version.
- A real user (not a hypothetical) reports a workflow that requires trust metadata on a local profile (e.g., team-shared profiles with auditability requirements).
- The agent runtime ecosystem adds a portable sandbox enforcement contract that surge can rely on for malicious-profile defense.

Until at least one of those holds, the answer is "no signature verification in v0.1."
