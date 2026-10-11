//! Choosing the agent a stage moves to when its own agent's usage limit is
//! exhausted (v1 task 1.4).
//!
//! The engine gathers the facts — which candidates are configured and
//! runnable, which have capacity left, which agent the stage's
//! verifier/implementer partner runs on — and this module decides, so the
//! rule is one pure, tested function.

/// The agent a stage should move to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RotationChoice {
    /// Registry id of the chosen agent, as configured.
    pub to: String,
    /// The choice runs on the same agent as the stage's partner: no other
    /// available candidate differed from it.
    pub same_as_partner: bool,
}

/// One candidate agent with the facts the choice needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Registry id as configured (what the stage override will name).
    pub id: String,
    /// Canonical runtime id, comparable with `current` and the partners.
    pub canonical: String,
    /// Configured, launchable, and its capacity is not exhausted.
    pub available: bool,
}

/// Pick the first available candidate that is not the exhausted `current`
/// runtime, preferring one whose runtime differs from every `partners`
/// runtime (a verifier should not check its implementer's work on the same
/// agent). Falls back to a partner-matching candidate, flagged, when it is
/// the only one available. `None` means no candidate fits and the stage
/// parks.
#[must_use]
pub fn choose(
    candidates: &[Candidate],
    current: &str,
    partners: &[String],
) -> Option<RotationChoice> {
    let mut seen = std::collections::BTreeSet::new();
    let usable: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.available
                && candidate.canonical != current
                && seen.insert(candidate.canonical.clone())
        })
        .collect();
    let differs = |candidate: &&&Candidate| !partners.contains(&candidate.canonical);
    if let Some(candidate) = usable.iter().find(differs) {
        return Some(RotationChoice {
            to: candidate.id.clone(),
            same_as_partner: false,
        });
    }
    usable.first().map(|candidate| RotationChoice {
        to: candidate.id.clone(),
        same_as_partner: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, canonical: &str, available: bool) -> Candidate {
        Candidate {
            id: id.into(),
            canonical: canonical.into(),
            available,
        }
    }

    #[test]
    fn picks_the_first_available_agent_that_is_not_the_exhausted_one() {
        let candidates = [
            candidate("claude-acp", "claude-code", true),
            candidate("gemini", "gemini", false),
            candidate("codex-acp", "codex", true),
        ];
        assert_eq!(
            choose(&candidates, "claude-code", &[]),
            Some(RotationChoice {
                to: "codex-acp".into(),
                same_as_partner: false
            })
        );
    }

    #[test]
    fn prefers_an_agent_that_differs_from_the_partner_and_flags_otherwise() {
        let candidates = [
            candidate("codex-acp", "codex", true),
            candidate("gemini", "gemini", true),
        ];
        let partners = vec!["codex".to_owned()];
        assert_eq!(
            choose(&candidates, "claude-code", &partners),
            Some(RotationChoice {
                to: "gemini".into(),
                same_as_partner: false
            })
        );
        assert_eq!(
            choose(&candidates[..1], "claude-code", &partners),
            Some(RotationChoice {
                to: "codex-acp".into(),
                same_as_partner: true
            })
        );
    }

    #[test]
    fn no_available_candidate_means_park() {
        let candidates = [
            candidate("claude-acp", "claude-code", true),
            candidate("codex-acp", "codex", false),
        ];
        assert_eq!(choose(&candidates, "claude-code", &[]), None);
        assert_eq!(choose(&[], "claude-code", &[]), None);
    }
}
