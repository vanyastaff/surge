//! Agent routing — determines which agent handles each subtask.

use std::collections::HashMap;
use surge_core::config::RoutingConfig;
use surge_core::spec::Subtask;
use tracing::debug;

/// Routing decision for a subtask.
#[derive(Debug, Clone)]
pub struct RouteDecision {
    pub agent_name: String,
    pub reason: String,
}

/// Routes subtasks to appropriate agents based on configuration.
pub struct AgentRouter {
    config: RoutingConfig,
    default_agent: String,
    phase_agents: HashMap<String, String>,
}

impl AgentRouter {
    pub fn new(config: RoutingConfig, default_agent: String) -> Self {
        Self {
            config,
            default_agent,
            phase_agents: HashMap::new(),
        }
    }

    pub fn set_phase_agent(&mut self, phase: &str, agent: &str) {
        self.phase_agents
            .insert(phase.to_string(), agent.to_string());
    }

    /// Route a subtask. Priority: phase override → complexity preference → default.
    pub fn route(&self, subtask: &Subtask, phase: Option<&str>) -> RouteDecision {
        if let Some(phase) = phase
            && let Some(agent) = self.phase_agents.get(phase)
        {
            debug!(agent, phase, "routed by phase override");
            return RouteDecision {
                agent_name: agent.clone(),
                reason: format!("phase override: {phase}"),
            };
        }

        let complexity_key = format!("{:?}", subtask.complexity).to_lowercase();
        if let Some(agent) = self.config.agent_preferences.get(&complexity_key) {
            debug!(agent, complexity = %complexity_key, "routed by complexity");
            return RouteDecision {
                agent_name: agent.clone(),
                reason: format!("complexity: {complexity_key}"),
            };
        }

        debug!(agent = %self.default_agent, "routed to default agent");
        RouteDecision {
            agent_name: self.default_agent.clone(),
            reason: "default".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::config::RoutingConfig;
    use surge_core::id::SubtaskId;
    use surge_core::spec::{Complexity, Subtask};

    fn make_subtask(complexity: Complexity) -> Subtask {
        Subtask {
            id: SubtaskId::new(),
            title: "test".to_string(),
            description: "test subtask".to_string(),
            complexity,
            files: vec![],
            acceptance_criteria: vec![],
            depends_on: vec![],
        }
    }

    #[test]
    fn test_default_routing() {
        let router = AgentRouter::new(RoutingConfig::default(), "claude-code".to_string());
        let subtask = make_subtask(Complexity::Standard);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "claude-code");
        assert_eq!(decision.reason, "default");
    }

    #[test]
    fn test_phase_routing() {
        let mut router = AgentRouter::new(RoutingConfig::default(), "claude-code".to_string());
        router.set_phase_agent("qa", "copilot");
        let subtask = make_subtask(Complexity::Standard);
        let decision = router.route(&subtask, Some("qa"));
        assert_eq!(decision.agent_name, "copilot");
        assert_eq!(decision.reason, "phase override: qa");
    }

    #[test]
    fn test_complexity_routing() {
        let mut prefs = HashMap::new();
        prefs.insert("complex".to_string(), "claude-opus".to_string());
        let config = RoutingConfig {
            strategy: surge_core::config::RoutingStrategy::Complexity,
            agent_preferences: prefs,
        };
        let router = AgentRouter::new(config, "claude-code".to_string());
        let subtask = make_subtask(Complexity::Complex);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "claude-opus");
        assert_eq!(decision.reason, "complexity: complex");
    }

    #[test]
    fn test_fallback_to_default() {
        let mut prefs = HashMap::new();
        prefs.insert("complex".to_string(), "claude-opus".to_string());
        let config = RoutingConfig {
            strategy: surge_core::config::RoutingStrategy::Complexity,
            agent_preferences: prefs,
        };
        let router = AgentRouter::new(config, "claude-code".to_string());
        let subtask = make_subtask(Complexity::Simple);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "claude-code");
        assert_eq!(decision.reason, "default");
    }
}
