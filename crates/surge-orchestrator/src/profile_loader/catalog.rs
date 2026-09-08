//! Render the resolved profile set into a compact catalogue for prompts.
//!
//! `flow-generator@1.0` writes `flow.toml` by naming profiles for Agent
//! nodes to bind to. Before this module existed, it could only name
//! profiles it had memorized from its own prompt text (`implementer@2.0`,
//! `verifier@2.0`) — those names drift the moment a profile is renamed,
//! retired, or added, and a profile that exists only on disk (a
//! community extension) was invisible to generation entirely.
//!
//! [`render_profile_catalog`] is the fix: it turns
//! [`ProfileRegistry::list`] — bundled + disk, disk-shadows-bundled — into
//! one compact document. `Engine::start_run` seeds it as the
//! `profile_catalog` run artifact; `flow-generator@1.0` binds it and picks
//! profiles from what the table says exists, not from memory.

use surge_core::sandbox::SandboxMode;

use super::ProfileRegistry;

/// Render every profile [`ProfileRegistry::list`] returns into a markdown
/// table: `id@MAJOR.MINOR`, display name, canonical runtime, sandbox mode,
/// verification authority, declared outcome ids, and `when_to_use`.
///
/// **Format: markdown table, not TOML.** This text is spliced into an
/// agent's system prompt via `{{profile_catalog}}` and never parsed back by
/// any Surge code — unlike `flow.toml`/`profile.toml`, where TOML's
/// type-safety and git-friendly diffs earn their keep
/// (`docs/conventions/flow.md`), this is throwaway prompt content read
/// once by an LLM. A markdown table is the denser encoding for that
/// reader: one line per profile, versus the 4-6 lines a TOML
/// `[[profile]]` array-of-tables would need for the same fields — the
/// difference that keeps a 20-profile registry under a few KB instead of
/// blowing past it.
///
/// The runtime column is resolved through
/// [`crate::engine::stage::agent::resolve_profile_runtime_id`] — the one
/// place the engine normalizes `runtime.agent_id` through
/// `surge_acp::Registry`'s alias table — never `profile.runtime.agent_id`
/// raw. Raw `agent_id` values collide under different spellings
/// (`"claude"` and `"claude-code"` both mean `claude-acp`), which is
/// exactly the contrast this column exists to show (e.g. `cross-verifier`
/// resolving to a different runtime than `implementer`).
#[must_use]
pub fn render_profile_catalog(registry: &ProfileRegistry) -> String {
    let mut out = String::from(
        "| profile | display name | runtime | sandbox | verifier | outcomes | when to use |\n\
         |---|---|---|---|---|---|---|\n",
    );
    for entry in registry.list() {
        let profile = &entry.profile;
        let id = profile.role.id.as_str();
        let version = &profile.role.version;
        let display_key = format!("{id}@{}.{}", version.major, version.minor);
        // Full semver for the resolve lookup so it lands on the exact
        // entry `list()` produced, not "latest" (which could differ when
        // a disk profile shadows only some versions of a bundled name).
        let resolve_key = format!("{id}@{version}");
        let runtime =
            crate::engine::stage::agent::resolve_profile_runtime_id(Some(registry), &resolve_key)
                .map(crate::engine::capacity::CanonicalRuntimeId::into_string)
                .unwrap_or_else(|| "unknown".to_string());
        let sandbox = sandbox_mode_label(profile.sandbox.mode);
        let verifier = if profile.verification.authority {
            "yes"
        } else {
            "no"
        };
        let outcomes = profile
            .outcomes
            .iter()
            .map(|o| o.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "| `{display_key}` | {} | {runtime} | {sandbox} | {verifier} | {outcomes} | {} |\n",
            md_cell(&profile.role.display_name),
            md_cell(&profile.role.when_to_use),
        ));
    }
    out
}

/// `SandboxMode` has no `Display` impl (it's `#[non_exhaustive]` and
/// serializes kebab-case via serde, not `fmt::Display`) — this mirrors
/// that kebab-case rendering for the catalogue's `sandbox` column. The
/// wildcard arm is required by `#[non_exhaustive]` from outside
/// `surge_core`, not dead code: it is what keeps this table from breaking
/// (rather than degrading) if `surge_core` adds a sandbox mode this crate
/// doesn't know about yet.
fn sandbox_mode_label(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
        SandboxMode::WorkspaceNetwork => "workspace-network",
        SandboxMode::FullAccess => "full-access",
        SandboxMode::Custom => "custom",
        _ => "unknown",
    }
}

/// Neutralize characters that would corrupt markdown table structure.
/// `display_name` / `when_to_use` are free text — bundled profiles are
/// known-clean, but a disk profile is community-authored and this table
/// has no other defense against a `|` or embedded newline in either
/// field.
fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::profile_loader::DiskProfileSet;

    fn registry_with_disk(dir: &Path) -> ProfileRegistry {
        let disk = DiskProfileSet::scan(dir).unwrap();
        ProfileRegistry::new(disk)
    }

    fn minimal_toml(id: &str, version: &str, when_to_use: &str, agent_id: &str) -> String {
        format!(
            r#"
schema_version = 1

[role]
id = "{id}"
version = "{version}"
display_name = "{id}"
category = "agents"
description = "test"
when_to_use = "{when_to_use}"

[runtime]
recommended_model = "test-model"
agent_id = "{agent_id}"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "test fixture prompt"
"#
        )
    }

    /// Every entry `ProfileRegistry::list()` returns shows up in the
    /// rendered catalogue, by its `id@MAJOR.MINOR` key.
    #[test]
    fn catalog_renders_every_registry_entry() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let catalog = render_profile_catalog(&reg);

        for entry in reg.list() {
            let version = &entry.profile.role.version;
            let key = format!(
                "`{}@{}.{}`",
                entry.profile.role.id.as_str(),
                version.major,
                version.minor
            );
            assert!(
                catalog.contains(&key),
                "catalog missing entry {key}, catalog:\n{catalog}"
            );
        }
    }

    /// The whole reason the runtime column exists: `cross-verifier@1.0`
    /// (bundled, `agent_id = "codex"`) must show a Codex runtime, and
    /// `implementer@1.0` (bundled, `agent_id = "claude-code"`) a Claude
    /// one — a flow author picking a verifier off this table needs to be
    /// able to see the two differ.
    #[test]
    fn catalog_shows_distinct_canonical_runtimes_for_implementer_and_cross_verifier() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let catalog = render_profile_catalog(&reg);

        let implementer_row = catalog
            .lines()
            .find(|l| l.contains("`implementer@1.0`"))
            .unwrap_or_else(|| panic!("no implementer@1.0 row in catalog:\n{catalog}"));
        assert!(
            implementer_row.contains("claude-acp"),
            "implementer@1.0 row should show the claude-acp runtime, got: {implementer_row}"
        );

        let cross_verifier_row = catalog
            .lines()
            .find(|l| l.contains("`cross-verifier@1.0`"))
            .unwrap_or_else(|| panic!("no cross-verifier@1.0 row in catalog:\n{catalog}"));
        assert!(
            cross_verifier_row.contains("codex-acp"),
            "cross-verifier@1.0 row should show the codex-acp runtime, got: {cross_verifier_row}"
        );
        assert_ne!(
            implementer_row.contains("claude-acp"),
            cross_verifier_row.contains("claude-acp"),
            "implementer and cross-verifier must show different runtimes"
        );
    }

    /// Community-extension property: a profile dropped into the disk
    /// profiles directory (not bundled) appears in the catalogue for
    /// free, because rendering goes through `ProfileRegistry::list()`
    /// rather than the bundled set directly.
    #[test]
    fn catalog_includes_a_disk_only_profile() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("community-reviewer-1.0.toml"),
            minimal_toml(
                "community-reviewer",
                "1.0.0",
                "Custom community-authored review role.",
                "mock",
            ),
        )
        .unwrap();
        let reg = registry_with_disk(tmp.path());
        let catalog = render_profile_catalog(&reg);

        assert!(
            catalog.contains("`community-reviewer@1.0`"),
            "catalog should include the disk-only profile, catalog:\n{catalog}"
        );
        assert!(catalog.contains("Custom community-authored review role."));
    }

    /// Verification authority and sandbox mode both render — `verifier@2.0`
    /// declares `[verification] authority = true`; `implementer@1.0` does
    /// not.
    #[test]
    fn catalog_marks_verification_authority() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let catalog = render_profile_catalog(&reg);

        let verifier_row = catalog
            .lines()
            .find(|l| l.contains("`verifier@2.0`"))
            .unwrap_or_else(|| panic!("no verifier@2.0 row in catalog:\n{catalog}"));
        assert!(
            verifier_row.contains("| yes |"),
            "verifier@2.0 should be marked as carrying verification authority: {verifier_row}"
        );

        let implementer_row = catalog
            .lines()
            .find(|l| l.contains("`implementer@1.0`"))
            .unwrap_or_else(|| panic!("no implementer@1.0 row in catalog:\n{catalog}"));
        assert!(
            implementer_row.contains("| no |"),
            "implementer@1.0 should not carry verification authority: {implementer_row}"
        );
    }

    /// Stays small enough to live in a prompt: bundled-only registry
    /// today is `surge_core::BUNDLED_COUNT` (20) profiles; the rendered
    /// table must stay well under the "a few KB" budget this task set.
    #[test]
    fn catalog_stays_within_prompt_budget_for_bundled_registry() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let catalog = render_profile_catalog(&reg);

        assert_eq!(reg.list().len(), surge_core::BUNDLED_COUNT);
        assert!(
            catalog.len() < 8 * 1024,
            "catalog for {} bundled profiles is {} bytes, expected well under 8KB",
            surge_core::BUNDLED_COUNT,
            catalog.len()
        );
    }
}
