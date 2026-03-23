//! Agent routing — determines which agent handles each subtask.

use std::collections::HashMap;
use surge_core::config::RoutingConfig;
use surge_core::spec::Subtask;
use tracing::{debug, warn};

use crate::registry::{AgentCapability, Registry};

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

    /// Route with a required capability check.
    ///
    /// If `required_capability` is `Some` and the initially chosen agent lacks
    /// it, falls back to the first builtin-catalog agent that has it.
    ///
    /// Agents not listed in `registry` are assumed capable of anything (custom
    /// user-configured agents). Returns the original decision if no capable
    /// fallback exists — callers may choose to propagate an error in that case.
    pub fn route_with_capability(
        &self,
        subtask: &Subtask,
        phase: Option<&str>,
        required_capability: Option<&AgentCapability>,
        registry: &Registry,
    ) -> RouteDecision {
        let decision = self.route(subtask, phase);

        let Some(cap) = required_capability else {
            return decision;
        };

        // Agents absent from the registry are assumed capable.
        let capable = registry
            .find(&decision.agent_name)
            .is_none_or(|e| e.capabilities.contains(cap));

        if capable {
            return decision;
        }

        warn!(
            agent = decision.agent_name.as_str(),
            capability = %cap,
            "chosen agent lacks required capability, searching for fallback"
        );

        if let Some(fallback) = registry.by_capability(cap).first() {
            debug!(fallback = fallback.id.as_str(), capability = %cap, "capability fallback selected");
            return RouteDecision {
                agent_name: fallback.id.clone(),
                reason: format!(
                    "capability fallback: {} lacks {cap}, using {}",
                    decision.agent_name, fallback.id
                ),
            };
        }

        warn!(capability = %cap, "no builtin agent has required capability, keeping original route");
        decision
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
            story_file: None,
            agent: None,
            execution: surge_core::spec::SubtaskExecution::default(),
        }
    }

    #[test]
    fn test_default_routing() {
        let router = AgentRouter::new(RoutingConfig::default(), "claude-acp".to_string());
        let subtask = make_subtask(Complexity::Standard);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "claude-acp");
        assert_eq!(decision.reason, "default");
    }

    #[test]
    fn test_phase_routing() {
        let mut router = AgentRouter::new(RoutingConfig::default(), "claude-acp".to_string());
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
        let router = AgentRouter::new(config, "claude-acp".to_string());
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
        let router = AgentRouter::new(config, "claude-acp".to_string());
        let subtask = make_subtask(Complexity::Simple);
        let decision = router.route(&subtask, None);
        assert_eq!(decision.agent_name, "claude-acp");
        assert_eq!(decision.reason, "default");
    }

    #[test]
    fn test_route_with_capability_capable_agent() {
        use crate::registry::{AgentCapability, Registry};
        let registry = Registry::builtin();
        // claude-acp supports AgentCapability::Plan
        let router = AgentRouter::new(RoutingConfig::default(), "claude-acp".to_string());
        let subtask = make_subtask(Complexity::Standard);
        let decision =
            router.route_with_capability(&subtask, None, Some(&AgentCapability::Plan), &registry);
        assert_eq!(decision.agent_name, "claude-acp");
    }

    #[test]
    fn test_route_with_capability_incapable_agent_falls_back() {
        use crate::registry::{AgentCapability, Registry};
        let registry = Registry::builtin();
        // gemini does NOT support AgentCapability::Plan — expects fallback to
        // a capable agent (claude-acp supports Plan and is first in catalog).
        let router = AgentRouter::new(RoutingConfig::default(), "gemini".to_string());
        let subtask = make_subtask(Complexity::Standard);
        let decision =
            router.route_with_capability(&subtask, None, Some(&AgentCapability::Plan), &registry);
        assert_ne!(
            decision.agent_name, "gemini",
            "should have fallen back away from gemini"
        );
        assert!(
            decision.reason.contains("capability fallback"),
            "expected fallback reason, got: {}",
            decision.reason
        );
    }

    #[test]
    fn test_route_with_capability_none_is_passthrough() {
        use crate::registry::Registry;
        let registry = Registry::builtin();
        let router = AgentRouter::new(RoutingConfig::default(), "claude-acp".to_string());
        let subtask = make_subtask(Complexity::Standard);
        // No required capability → should behave identically to plain route()
        let plain = router.route(&subtask, None);
        let with_cap = router.route_with_capability(&subtask, None, None, &registry);
        assert_eq!(plain.agent_name, with_cap.agent_name);
    }

    #[test]
    fn test_route_with_capability_unknown_agent_assumed_capable() {
        use crate::registry::{AgentCapability, Registry};
        let registry = Registry::builtin();
        // "my-custom-agent" is not in the builtin registry — assumed capable
        let router = AgentRouter::new(RoutingConfig::default(), "my-custom-agent".to_string());
        let subtask = make_subtask(Complexity::Standard);
        let decision =
            router.route_with_capability(&subtask, None, Some(&AgentCapability::Plan), &registry);
        assert_eq!(decision.agent_name, "my-custom-agent");
    }
}
