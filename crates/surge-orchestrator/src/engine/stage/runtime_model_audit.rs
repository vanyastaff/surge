//! Bounded host-owned binding declarations and actual session observations.
use surge_core::{RunEvent, SessionId, graph::Graph};

pub(super) fn render(
    graph: Option<&Graph>,
    events: &[RunEvent],
    current: SessionId,
    cwd: &std::path::Path,
    node: &surge_core::NodeKey,
    agent: &surge_core::agent_config::AgentConfig,
) -> String {
    const MAX_CONTEXT_BYTES: usize = 64 * 1024;
    let mut declared = Vec::new();
    let mut observed = Vec::new();
    let mut used = 0;
    let mut omitted = 0;
    let mut append = |rows: &mut Vec<serde_json::Value>, value: serde_json::Value| {
        let size = value.to_string().len();
        if used + size > MAX_CONTEXT_BYTES {
            omitted += 1;
        } else {
            used += size;
            rows.push(value);
        }
    };
    // Newest observations first, so the actual current opening survives a bounded report.
    for event in events.iter().rev() {
        if let surge_core::EventPayload::SessionOpened {
            node,
            session,
            opened,
            agent_id,
            ..
        } = &event.payload
        {
            let descriptor = opened.as_ref().map(|opened| &opened.descriptor);
            append(
                &mut observed,
                serde_json::json!({"node":node,"session":session,
                "invocation":descriptor.map(surge_core::execution_recovery::ProviderSessionDescriptor::invocation),
                "provider_uuid":descriptor.map(|descriptor| descriptor.provider_session_id().as_str()),
                "runtime":descriptor.map(surge_core::execution_recovery::ProviderSessionDescriptor::runtime).or(agent_id.as_deref()),
                "observed_model":descriptor.and_then(|descriptor| descriptor.observed_model()),
                "model_evidence":if descriptor.and_then(|descriptor| descriptor.observed_model()).is_some() {"provider_option_response"} else {"unknown"},
                "origin":if *session == current {"current_opening"} else if descriptor.is_some_and(|descriptor| descriptor.cwd() != cwd) {"inherited_other_workspace"} else {"earlier_current_workspace"},
                "event_seq":event.seq}),
            );
        }
    }
    for (scope, nodes) in graph.into_iter().flat_map(|graph| {
        std::iter::once(("root", &graph.nodes)).chain(
            graph
                .subgraphs
                .iter()
                .map(|(key, body)| (key.as_str(), &body.nodes)),
        )
    }) {
        for node in nodes.values() {
            if let surge_core::node::NodeConfig::Agent(agent) = &node.config {
                append(
                    &mut declared,
                    serde_json::json!({"scope":scope,"node":node.id,
                    "profile":agent.profile,"declared_runtime":agent.runtime_override(),"declared_model":agent.model_override()}),
                );
            }
        }
    }
    format!(
        "Host runtime/model audit context. Declared bindings are requests; observed models come only from provider option responses. Legacy absent observations remain unknown and require independently attributable primary evidence.\n{}\n\n",
        serde_json::json!({"graph_declarations": if graph.is_some() { "available" } else { "unavailable" },
            "declared_bindings":graph.map(|_| declared),
            "current_stage_declaration":{"node":node,"profile":agent.profile,
                "declared_runtime":agent.runtime_override(),"declared_model":agent.model_override()},
            "observed_sessions":observed,"omitted_entries":omitted})
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audit_context_never_turns_requested_model_into_observed_evidence() {
        let mut graph = surge_core::bundled_flows::BundledFlows::by_name_latest("linear-3")
            .unwrap()
            .graph;
        for node in graph.nodes.values_mut() {
            if let surge_core::node::NodeConfig::Agent(agent) = &mut node.config {
                *agent = agent.with_runtime_override("codex-acp", Some("gpt-6.1-sol"));
            }
        }
        let context = render(
            Some(&graph),
            &[],
            SessionId::new(),
            std::path::Path::new("/fixture"),
            &graph.nodes.values().next().unwrap().id,
            graph
                .nodes
                .values()
                .find_map(|node| {
                    if let surge_core::node::NodeConfig::Agent(agent) = &node.config {
                        Some(agent)
                    } else {
                        None
                    }
                })
                .unwrap(),
        );
        assert!(context.contains("gpt-6.1-sol"));
        assert!(context.contains("declared_bindings"));
        assert!(context.contains("observed_sessions"));
        assert!(context.contains("unknown"));
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    use surge_core::execution_recovery::{
        OpenedSession, ProviderSessionDescriptor, ProviderSessionId, SessionOpenMode,
        SessionRestoreCapabilities,
    };
    #[test]
    fn current_observation_and_inherited_unknown_survive_bounded_declared_bindings() {
        let mut graph = surge_core::bundled_flows::BundledFlows::by_name_latest("linear-3")
            .unwrap()
            .graph;
        let template = graph
            .nodes
            .values()
            .find(|node| matches!(node.config, surge_core::node::NodeConfig::Agent(_)))
            .unwrap()
            .clone();
        graph.nodes.clear();
        graph.subgraphs.clear();
        for index in 0..1000 {
            let mut node = template.clone();
            node.id = surge_core::NodeKey::try_from(format!("agent_{index}").as_str()).unwrap();
            if let surge_core::node::NodeConfig::Agent(agent) = &mut node.config {
                *agent = agent.with_runtime_override("codex-acp", Some("gpt-6.1-sol"));
            }
            graph.nodes.insert(node.id.clone(), node);
        }
        let mut events = Vec::new();
        let current = SessionId::new();
        for (seq, session, cwd, model) in [
            (1, SessionId::new(), "/parent", None),
            (2, current, "/child", Some("provider-exact-id".into())),
        ] {
            let descriptor = ProviderSessionDescriptor::new(
                ProviderSessionId::new(format!("provider-{seq}")).unwrap(),
                surge_core::id::StageInvocationId::new(),
                "actual-runtime".into(),
                surge_core::ContentHash::compute(b"launch"),
                cwd.into(),
                SessionRestoreCapabilities::default(),
                model,
            )
            .unwrap();
            events.push(RunEvent {
                run_id: surge_core::RunId::new(),
                seq,
                timestamp: chrono::Utc::now(),
                payload: surge_core::EventPayload::SessionOpened {
                    handoff: None,
                    opened: Some(
                        OpenedSession::new(session, descriptor, SessionOpenMode::New).unwrap(),
                    ),
                    node: surge_core::NodeKey::try_from("stage").unwrap(),
                    session,
                    agent: "role".into(),
                    agent_id: Some("actual-runtime".into()),
                },
            });
        }
        let stage = graph.nodes.values().next().unwrap();
        let surge_core::node::NodeConfig::Agent(agent) = &stage.config else {
            panic!("fixture agent")
        };
        let context = render(
            Some(&graph),
            &events,
            current,
            std::path::Path::new("/child"),
            &stage.id,
            agent,
        );
        assert!(context.len() < 70 * 1024);
        let data: serde_json::Value =
            serde_json::from_str(context.lines().nth(1).unwrap()).unwrap();
        let observed = data["observed_sessions"].as_array().unwrap();
        assert_eq!(observed.len(), 2);
        assert_eq!(observed[0]["origin"], "current_opening");
        assert_eq!(observed[0]["observed_model"], "provider-exact-id");
        assert_eq!(observed[1]["origin"], "inherited_other_workspace");
        assert!(observed[1]["observed_model"].is_null());
        assert_eq!(observed[1]["model_evidence"], "unknown");
        assert!(data["omitted_entries"].as_u64().unwrap() > 0);
    }
}
