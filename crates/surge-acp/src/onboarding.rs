//! Onboarding defaults shared by every entry point that creates a project.

use surge_core::SurgeConfig;
use tracing::{debug, warn};

use crate::{DetectedAgent, Registry};

/// Configuration for a new project: the best installed coding agent as the
/// default, falling back to the installable `claude-acp` entry.
///
/// Shared by `surge init --default` and the desktop app's "New app" flow so
/// both onboarding paths pick the same agent.
#[must_use]
pub fn default_config() -> SurgeConfig {
    let mut config = SurgeConfig::default();
    let registry = Registry::builtin();
    let detected = registry.detect_installed_with_paths();
    debug!(
        detected = detected.len(),
        registry_entries = registry.len(),
        "detected installed registry agents"
    );

    let selected = select_default_registry_id(&detected)
        .or_else(|| registry.find("claude-acp").map(|entry| entry.id.clone()));

    if let Some(agent_id) = selected
        && let Some(entry) = registry.find(&agent_id)
    {
        config.default_agent = entry.id.clone();
        config
            .agents
            .insert(entry.id.clone(), entry.to_agent_config());
        if !detected.iter().any(|agent| agent.entry.id == entry.id) {
            warn!(
                agent_id = %entry.id,
                reason = "no_detected_agent",
                "using installable registry fallback for default agent"
            );
        }
    }

    config
}

/// Pick the default agent among detected runtimes: a preferred runtime first,
/// then any genuinely installed one; never a launcher-only (`npx`/`uvx`)
/// detection.
#[must_use]
pub fn select_default_registry_id(detected: &[DetectedAgent]) -> Option<String> {
    const PREFERENCE: &[&str] = &["claude-acp", "codex-acp", "gemini", "github-copilot-cli"];
    for preferred in PREFERENCE {
        if detected.iter().any(|agent| agent.entry.id == *preferred) {
            debug!(agent_id = %preferred, "selected preferred detected agent");
            return Some((*preferred).to_string());
        }
    }
    // Fall through to whatever else was detected — but never to a runtime
    // that was "detected" only because its launcher is on PATH. An npx- or
    // uvx-only entry carries no `cli_binary`, so discovery resolves
    // `npx`/`uvx` itself; on any machine with Node installed that reports
    // the runtime as present when nothing of it is installed at all. Letting
    // that win would make a developer-preview runtime the default agent on a
    // clean machine, ahead of the installable `claude-acp` fallback the
    // caller applies when this returns `None`.
    detected
        .iter()
        .find(|agent| {
            !(agent.entry.cli_binary.is_none() && (agent.entry.is_npx() || agent.entry.is_uvx()))
        })
        .map(|agent| {
            debug!(agent_id = %agent.entry.id, "selected first detected agent");
            agent.entry.id.clone()
        })
}

#[cfg(test)]
mod default_agent_selection_tests {
    use super::select_default_registry_id;
    use crate::{DetectedAgent, Registry};

    fn detected(id: &str) -> DetectedAgent {
        let entry = Registry::builtin()
            .find(id)
            .unwrap_or_else(|| panic!("`{id}` must be a builtin registry entry"))
            .clone();
        DetectedAgent {
            entry,
            command_path: Some(format!("/usr/bin/{id}")),
            detected_version: None,
        }
    }

    #[test]
    fn a_preferred_agent_wins_even_when_listed_later() {
        let picked = select_default_registry_id(&[detected("gemini"), detected("claude-acp")]);
        assert_eq!(picked.as_deref(), Some("claude-acp"));
    }

    /// `dsh-acp` carries no `cli_binary` and launches through `npx`, so
    /// discovery "finds" it whenever Node is installed — which says nothing
    /// about whether the runtime itself is present. Selecting it would make a
    /// developer-preview runtime the default agent on a clean machine, ahead
    /// of the installable `claude-acp` fallback the caller applies on `None`.
    #[test]
    fn a_launcher_only_runtime_never_becomes_the_default() {
        let dsh = detected("dsh-acp");
        assert!(
            dsh.entry.cli_binary.is_none() && dsh.entry.is_npx(),
            "fixture must be the launcher-only shape this rule is about"
        );
        assert_eq!(select_default_registry_id(&[dsh]), None);
    }

    /// The fall-through still works for a runtime that is genuinely installed
    /// and simply not in `PREFERENCE` — the rule skips launcher-only
    /// detections, not unfamiliar ones.
    ///
    /// The fixture has to be synthesised: every builtin entry carrying a
    /// `cli_binary` today *is* the preference list, so this arm is otherwise
    /// reachable only through a remote or custom registry entry.
    #[test]
    fn an_installed_runtime_outside_the_preference_list_still_wins() {
        let mut installed = detected("dsh-acp");
        installed.entry.id = "some-remote-agent".to_owned();
        installed.entry.cli_binary = Some("some-remote-agent".to_owned());

        let picked = select_default_registry_id(&[detected("dsh-acp"), installed]);
        assert_eq!(picked.as_deref(), Some("some-remote-agent"));
    }
}
