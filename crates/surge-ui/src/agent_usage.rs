//! What each agent actually did for this project, and whether it can work
//! right now.
//!
//! Usage is folded from the project's run logs: a `SessionOpened` names the
//! runtime (`agent_id`) a session ran on, and `TokensConsumed` for that
//! session is attributed to it. Readiness is the same check the engine
//! makes before launching a step: the runtime's required environment
//! resolves, and no exhausted usage window is on record.

use std::collections::{BTreeSet, HashMap};

use surge_core::{EventPayload, RunId};

/// Totals for one runtime.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub sessions: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Sum of the costs the runtime reported; `None` when it never did.
    pub cost_usd: Option<f64>,
    pub models: BTreeSet<String>,
    pub last_used_ms: Option<i64>,
}

/// Whether a runtime can run a step now.
#[derive(Clone, Debug, PartialEq)]
pub enum Readiness {
    Ready,
    /// Required environment (API key, login) is missing.
    NotConfigured(String),
    /// A provider usage window is exhausted.
    LimitReached(String),
}

/// Fold `(timestamp_ms, payload)` events into per-runtime usage.
pub fn fold(events: &[(i64, EventPayload)], into: &mut HashMap<String, Usage>) {
    let mut session_agent: HashMap<String, String> = HashMap::new();
    for (at, event) in events {
        match event {
            EventPayload::SessionOpened {
                session,
                agent_id: Some(runtime),
                ..
            } => {
                session_agent.insert(session.to_string(), runtime.clone());
                let usage = into.entry(runtime.clone()).or_default();
                usage.sessions += 1;
                usage.last_used_ms = Some(usage.last_used_ms.map_or(*at, |t| t.max(*at)));
            },
            EventPayload::TokensConsumed {
                session,
                prompt_tokens,
                output_tokens,
                model,
                cost_usd,
                ..
            } => {
                let Some(runtime) = session_agent.get(&session.to_string()) else {
                    continue;
                };
                let usage = into.entry(runtime.clone()).or_default();
                usage.tokens_in += u64::from(*prompt_tokens);
                usage.tokens_out += u64::from(*output_tokens);
                if let Some(cost) = cost_usd {
                    usage.cost_usd = Some(usage.cost_usd.unwrap_or(0.0) + cost);
                }
                if !model.trim().is_empty() {
                    usage.models.insert(model.clone());
                }
            },
            _ => {},
        }
    }
}

/// Usage across `runs` and readiness for `agents` (`(id, env)` pairs).
pub async fn load(
    runs: Vec<RunId>,
    agents: Vec<(
        String,
        std::collections::BTreeMap<String, surge_core::config::AgentEnvValue>,
    )>,
) -> (HashMap<String, Usage>, HashMap<String, Readiness>) {
    let mut usage = HashMap::new();
    let mut readiness = HashMap::new();
    let Some(home) = surge_core::home::surge_home_dir() else {
        return (usage, readiness);
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return (usage, readiness);
    }
    let root = home.join("runs");
    for run in runs {
        if let Ok(events) =
            surge_persistence::runs::Storage::inspect_existing_run_events(root.clone(), run).await
        {
            let events: Vec<(i64, EventPayload)> = events
                .into_iter()
                .map(|e| (e.timestamp_ms, e.payload.payload))
                .collect();
            fold(&events, &mut usage);
        }
    }
    let storage = surge_persistence::runs::Storage::open(&home).await.ok();
    for (id, env) in agents {
        let state = if let Err(error) = surge_acp::agent_env::resolve(&id, &env) {
            Readiness::NotConfigured(error.to_string())
        } else if let Some(why) = match &storage {
            Some(storage) => storage.capacity_status(&id).await.ok().and_then(|status| {
                surge_orchestrator::engine::capacity::exhausted_reason(&status, chrono::Utc::now())
            }),
            None => None,
        } {
            Readiness::LimitReached(why)
        } else {
            Readiness::Ready
        };
        readiness.insert(id, state);
    }
    (usage, readiness)
}

#[cfg(test)]
mod tests {
    use super::fold;
    use std::collections::HashMap;
    use surge_core::{EventPayload, SessionId};

    #[test]
    fn tokens_are_attributed_to_the_runtime_of_their_session() {
        let a = SessionId::new();
        let b = SessionId::new();
        let events = vec![
            (
                10,
                EventPayload::SessionOpened {
                    opened: None,
                    handoff: None,
                    node: "impl".try_into().unwrap(),
                    session: a,
                    agent: "implementer@2.0".into(),
                    agent_id: Some("claude-acp".into()),
                },
            ),
            (
                11,
                EventPayload::TokensConsumed {
                    session: a,
                    prompt_tokens: 100,
                    output_tokens: 20,
                    cache_hits: 0,
                    model: "opus".into(),
                    cost_usd: Some(0.5),
                },
            ),
            (
                20,
                EventPayload::SessionOpened {
                    opened: None,
                    handoff: None,
                    node: "verify".try_into().unwrap(),
                    session: b,
                    agent: "verifier@2.0".into(),
                    agent_id: Some("codex-acp".into()),
                },
            ),
            (
                21,
                EventPayload::TokensConsumed {
                    session: b,
                    prompt_tokens: 7,
                    output_tokens: 3,
                    cache_hits: 0,
                    model: String::new(),
                    cost_usd: None,
                },
            ),
        ];
        let mut usage = HashMap::new();
        fold(&events, &mut usage);
        let claude = &usage["claude-acp"];
        assert_eq!(
            (claude.sessions, claude.tokens_in, claude.tokens_out),
            (1, 100, 20)
        );
        assert_eq!(claude.cost_usd, Some(0.5));
        assert!(claude.models.contains("opus"));
        let codex = &usage["codex-acp"];
        // A runtime that never reported cost keeps "unknown", not $0.
        assert_eq!(codex.cost_usd, None);
        assert_eq!(codex.last_used_ms, Some(20));
    }
}
