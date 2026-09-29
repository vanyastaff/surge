use super::*;
use crate::edge::{Edge, EdgeKind, EdgePolicy, PortRef};
use crate::graph::{GraphMetadata, SCHEMA_VERSION};
use crate::node::{Node, NodeConfig, Position};
use crate::terminal_config::{TerminalConfig, TerminalKind};
use std::collections::BTreeMap;

fn minimal_terminal_only_graph() -> Graph {
    let end = NodeKey::try_from("end").unwrap();
    let mut nodes = BTreeMap::new();
    nodes.insert(
        end.clone(),
        Node {
            id: end.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: end,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    }
}

#[test]
fn empty_terminal_graph_validates() {
    let g = minimal_terminal_only_graph();
    let result = validate(&g);
    assert!(result.is_valid(), "expected ok, got {:?}", result);
}

#[test]
fn missing_start_reports_rule_1() {
    let mut g = minimal_terminal_only_graph();
    g.start = NodeKey::try_from("nonexistent").unwrap();
    let result = validate(&g);
    let errs = result.expect_errors("expected errors");
    assert!(
        errs.iter()
            .any(|e| matches!(e.kind, ValidationErrorKind::StartNodeMissing))
    );
}

#[test]
fn edge_to_unknown_node_reports_rule_3() {
    let mut g = minimal_terminal_only_graph();
    g.edges.push(Edge {
        id: crate::keys::EdgeKey::try_from("e1").unwrap(),
        from: PortRef {
            node: g.start.clone(),
            outcome: OutcomeKey::try_from("done").unwrap(),
        },
        to: NodeKey::try_from("ghost").unwrap(),
        kind: EdgeKind::Forward,
        policy: EdgePolicy::default(),
    });
    let result = validate(&g);
    let errs = result.expect_errors("expected errors");
    assert!(
        errs.iter()
            .any(|e| matches!(e.kind, ValidationErrorKind::EdgeToUnknownNode))
    );
}

#[test]
fn graph_with_no_terminal_reports_rule_7() {
    let mut g = minimal_terminal_only_graph();
    let bn = NodeKey::try_from("branch_only").unwrap();
    g.nodes.clear();
    g.nodes.insert(
        bn.clone(),
        Node {
            id: bn.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Branch(crate::branch_config::BranchConfig {
                predicates: vec![],
                default_outcome: OutcomeKey::try_from("default").unwrap(),
            }),
        },
    );
    g.start = bn;
    let result = validate(&g);
    let errs = result.expect_errors("expected errors");
    assert!(
        errs.iter()
            .any(|e| matches!(e.kind, ValidationErrorKind::NoTerminalReachable))
    );
}

#[test]
fn missing_subgraph_ref_reports_rule_11b() {
    use crate::loop_config::{
        ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
    };
    let loop_node_key = NodeKey::try_from("loopn").unwrap();
    let mut g = minimal_terminal_only_graph();
    g.start = loop_node_key.clone();
    g.nodes.insert(
        loop_node_key.clone(),
        Node {
            id: loop_node_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Loop(LoopConfig {
                iterates_over: IterableSource::Static(vec![]),
                body: SubgraphKey::try_from("ghost_body").unwrap(),
                iteration_var_name: "x".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            }),
        },
    );
    let result = validate(&g);
    let errs = result.expect_errors("expected errors");
    assert!(
        errs.iter()
            .any(|e| matches!(e.kind, ValidationErrorKind::SubgraphRefMissing { .. }))
    );
}

#[test]
fn duplicate_node_key_in_subgraph_reports_rule_17() {
    use crate::loop_config::{
        ExitCondition, FailurePolicy, IterableSource, LoopConfig, ParallelismMode,
    };
    let shared = NodeKey::try_from("shared_id").unwrap();
    let loop_node_key = NodeKey::try_from("loopn").unwrap();
    let sub_key = SubgraphKey::try_from("sub").unwrap();

    let mut g = minimal_terminal_only_graph();
    g.start = loop_node_key.clone();
    g.nodes.insert(
        loop_node_key.clone(),
        Node {
            id: loop_node_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Loop(LoopConfig {
                iterates_over: IterableSource::Static(vec![]),
                body: sub_key.clone(),
                iteration_var_name: "x".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            }),
        },
    );
    g.nodes.insert(
        shared.clone(),
        Node {
            id: shared.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    let mut sub_nodes = BTreeMap::new();
    sub_nodes.insert(
        shared.clone(),
        Node {
            id: shared.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    g.subgraphs.insert(
        sub_key.clone(),
        Subgraph {
            start: shared.clone(),
            nodes: sub_nodes,
            edges: vec![],
        },
    );

    let result = validate(&g);
    let errs = result.expect_errors("expected errors");
    assert!(
        errs.iter()
            .any(|e| matches!(e.kind, ValidationErrorKind::NodeKeyCollision { .. }))
    );
}

#[test]
fn orphan_subgraph_reports_warning_not_error() {
    let mut g = minimal_terminal_only_graph();
    let mut m = BTreeMap::new();
    let inner_k = NodeKey::try_from("inner").unwrap();
    m.insert(
        inner_k.clone(),
        Node {
            id: inner_k.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    g.subgraphs.insert(
        SubgraphKey::try_from("orphan").unwrap(),
        Subgraph {
            start: inner_k,
            nodes: m,
            edges: vec![],
        },
    );
    let result = validate(&g);
    let warnings = result.expect_valid("expected ok-with-warnings");
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w.kind, ValidationErrorKind::OrphanSubgraph { .. }))
    );
    assert!(
        warnings
            .iter()
            .all(|w| w.kind.severity() == Severity::Warning)
    );
}

#[test]
fn severity_classification_correct() {
    assert_eq!(
        ValidationErrorKind::OrphanSubgraph {
            key: SubgraphKey::try_from("x").unwrap()
        }
        .severity(),
        Severity::Warning,
    );
    assert_eq!(
        ValidationErrorKind::StartNodeMissing.severity(),
        Severity::Error
    );
}

mod m6_loop_static_cap_tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
    use crate::keys::{NodeKey, OutcomeKey, SubgraphKey};
    use crate::loop_config::{
        ExitCondition, FailurePolicy, IterableSource, LoopConfig, MAX_LOOP_ITEMS_STATIC,
        ParallelismMode,
    };
    use crate::node::{Node, NodeConfig, OutcomeDecl, Position};
    use crate::terminal_config::{TerminalConfig, TerminalKind};
    use std::collections::BTreeMap;

    fn graph_with_loop_node(items: Vec<toml::Value>) -> Graph {
        let loop_key = NodeKey::try_from("loop_1").unwrap();
        let body_key = SubgraphKey::try_from("body").unwrap();
        let body_start = NodeKey::try_from("body_start").unwrap();

        let loop_node = Node {
            id: loop_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("completed").unwrap(),
                description: "done".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Loop(LoopConfig {
                iterates_over: IterableSource::Static(items),
                body: body_key.clone(),
                iteration_var_name: "item".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            }),
        };

        let body_node = Node {
            id: body_start.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        };

        let mut nodes = BTreeMap::new();
        nodes.insert(loop_key.clone(), loop_node);

        let mut body_nodes = BTreeMap::new();
        body_nodes.insert(body_start.clone(), body_node);

        let mut subgraphs = BTreeMap::new();
        subgraphs.insert(
            body_key,
            Subgraph {
                start: body_start,
                nodes: body_nodes,
                edges: vec![],
            },
        );

        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: loop_key,
            nodes,
            edges: vec![],
            subgraphs,
        }
    }

    #[test]
    fn loop_static_size_at_cap_is_ok() {
        let items: Vec<toml::Value> = (0..MAX_LOOP_ITEMS_STATIC)
            .map(|i| toml::Value::Integer(i as i64))
            .collect();
        let g = graph_with_loop_node(items);
        let result = validate(&g);
        // The minimal graph also triggers other structural errors (e.g., no Terminal at outer level).
        // Filter to only LoopStaticTooLarge findings.
        let findings = result.into_findings();
        assert!(
            !findings
                .iter()
                .any(|f| matches!(f.kind, ValidationErrorKind::LoopStaticTooLarge { .. })),
            "1000 items should not trigger LoopStaticTooLarge: {findings:?}"
        );
    }

    #[test]
    fn loop_static_size_above_cap_is_rejected() {
        let items: Vec<toml::Value> = (0..MAX_LOOP_ITEMS_STATIC + 1)
            .map(|i| toml::Value::Integer(i as i64))
            .collect();
        let g = graph_with_loop_node(items);
        let result = validate(&g);
        let errors = result.expect_errors("validation should fail");
        let cap_errors: Vec<_> = errors
            .iter()
            .filter(|f| matches!(f.kind, ValidationErrorKind::LoopStaticTooLarge { .. }))
            .collect();
        assert!(
            !cap_errors.is_empty(),
            "expected LoopStaticTooLarge, got {errors:?}"
        );
    }

    #[test]
    fn loop_in_subgraph_static_size_above_cap_is_rejected() {
        // Build a graph where the offending Loop is inside a subgraph,
        // not at the outer graph level. Cap enforcement must still catch it.
        let outer_loop = NodeKey::try_from("outer_loop").unwrap();
        let outer_body = SubgraphKey::try_from("outer_body").unwrap();
        let inner_loop = NodeKey::try_from("inner_loop").unwrap();
        let inner_body = SubgraphKey::try_from("inner_body").unwrap();
        let inner_terminal = NodeKey::try_from("inner_t").unwrap();

        // The OUTER loop has a small Static iterable (ok).
        let outer_loop_node = Node {
            id: outer_loop.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Loop(LoopConfig {
                iterates_over: IterableSource::Static(vec![toml::Value::Integer(1)]),
                body: outer_body.clone(),
                iteration_var_name: "i".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            }),
        };

        // The INNER loop (inside the outer's body subgraph) has too many items.
        let big_items: Vec<toml::Value> = (0..MAX_LOOP_ITEMS_STATIC + 5)
            .map(|i| toml::Value::Integer(i as i64))
            .collect();
        let inner_loop_node = Node {
            id: inner_loop.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Loop(LoopConfig {
                iterates_over: IterableSource::Static(big_items),
                body: inner_body.clone(),
                iteration_var_name: "j".into(),
                exit_condition: ExitCondition::AllItems,
                on_iteration_failure: FailurePolicy::Abort,
                parallelism: ParallelismMode::Sequential,
                gate_after_each: false,
            }),
        };

        let inner_t_node = Node {
            id: inner_terminal.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(crate::terminal_config::TerminalConfig {
                kind: crate::terminal_config::TerminalKind::Success,
                message: None,
            }),
        };

        let mut outer_nodes = BTreeMap::new();
        outer_nodes.insert(outer_loop.clone(), outer_loop_node);

        let mut outer_body_nodes = BTreeMap::new();
        outer_body_nodes.insert(inner_loop.clone(), inner_loop_node);

        let mut inner_body_nodes = BTreeMap::new();
        inner_body_nodes.insert(inner_terminal.clone(), inner_t_node);

        let mut subgraphs = BTreeMap::new();
        subgraphs.insert(
            outer_body,
            crate::graph::Subgraph {
                start: inner_loop.clone(),
                nodes: outer_body_nodes,
                edges: vec![],
            },
        );
        subgraphs.insert(
            inner_body,
            crate::graph::Subgraph {
                start: inner_terminal,
                nodes: inner_body_nodes,
                edges: vec![],
            },
        );

        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "nested".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: outer_loop,
            nodes: outer_nodes,
            edges: vec![],
            subgraphs,
        };

        let result = validate(&g);
        let errors = result.expect_errors("must fail");
        let cap_errors: Vec<_> = errors
            .iter()
            .filter(|f| matches!(f.kind, ValidationErrorKind::LoopStaticTooLarge { .. }))
            .collect();
        assert!(
            !cap_errors.is_empty(),
            "Loop in subgraph above cap must be rejected: {errors:?}"
        );
    }
}

mod m6_notify_validation_tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::{GraphMetadata, SCHEMA_VERSION};
    use crate::keys::{NodeKey, OutcomeKey};
    use crate::node::{Node, NodeConfig, OutcomeDecl, Position};
    use crate::notify_config::{
        NotifyChannel, NotifyConfig, NotifyFailureAction, NotifySeverity, NotifyTemplate,
    };
    use std::collections::BTreeMap;

    fn notify_node_with_outcomes(outcomes: Vec<&str>, on_failure: NotifyFailureAction) -> Node {
        let key = NodeKey::try_from("notify_1").unwrap();
        Node {
            id: key.clone(),
            position: Position::default(),
            declared_outcomes: outcomes
                .iter()
                .map(|o| OutcomeDecl {
                    id: OutcomeKey::try_from(*o).unwrap(),
                    description: format!("{o} outcome"),
                    edge_kind_hint: EdgeKind::Forward,
                    is_terminal: false,
                    ledger_effect: Default::default(),
                })
                .collect(),
            config: NodeConfig::Notify(NotifyConfig {
                channel: NotifyChannel::Desktop,
                template: NotifyTemplate {
                    severity: NotifySeverity::Info,
                    title: "t".into(),
                    body: "b".into(),
                    artifacts: vec![],
                },
                on_failure,
            }),
        }
    }

    fn graph_with_node(node: Node) -> Graph {
        let key = node.id.clone();
        let mut nodes = BTreeMap::new();
        nodes.insert(key.clone(), node);
        Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "t".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: key,
            nodes,
            edges: vec![],
            subgraphs: BTreeMap::new(),
        }
    }

    #[test]
    fn notify_missing_delivered_outcome_is_error() {
        let n = notify_node_with_outcomes(vec!["sent"], NotifyFailureAction::Continue);
        let g = graph_with_node(n);
        let result = validate(&g);
        let errors = result.expect_errors("validation should fail");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e.kind, ValidationErrorKind::NotifyMissingDelivered { .. })),
            "expected NotifyMissingDelivered, got {errors:?}"
        );
    }

    #[test]
    fn notify_with_delivered_only_continue_is_ok() {
        let n = notify_node_with_outcomes(vec!["delivered"], NotifyFailureAction::Continue);
        let g = graph_with_node(n);
        let result = validate(&g);
        // ok branch returns Vec<ValidationError> (warnings only); should be empty.
        let warnings = result.into_findings();
        assert!(
            !warnings
                .iter()
                .any(|w| matches!(w.kind, ValidationErrorKind::NotifyMissingDelivered { .. })),
            "delivered-only-Continue should not produce NotifyMissingDelivered, got {warnings:?}"
        );
    }

    #[test]
    fn notify_fail_without_undeliverable_is_warning() {
        let n = notify_node_with_outcomes(vec!["delivered"], NotifyFailureAction::Fail);
        let g = graph_with_node(n);
        // The minimal graph has no edges or Terminal node, so other structural
        // rules fire as errors. Use unwrap_or_else to get all findings regardless.
        let findings = validate(&g).into_findings();
        assert!(
            findings.iter().any(|w| matches!(
                w.kind,
                ValidationErrorKind::NotifyFailMissingUndeliverable { .. }
            ) && w.kind.severity() == Severity::Warning),
            "expected NotifyFailMissingUndeliverable warning, got {findings:?}"
        );
    }

    #[test]
    fn notify_in_subgraph_missing_delivered_is_error() {
        use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION, Subgraph};
        use crate::keys::SubgraphKey;

        let inner_notify = NodeKey::try_from("inner_notify").unwrap();
        let inner_body = SubgraphKey::try_from("body").unwrap();

        let inner_notify_node = Node {
            id: inner_notify.clone(),
            position: Position::default(),
            declared_outcomes: vec![OutcomeDecl {
                id: OutcomeKey::try_from("sent").unwrap(), // NOT "delivered"
                description: "wrong".into(),
                edge_kind_hint: EdgeKind::Forward,
                is_terminal: false,
                ledger_effect: Default::default(),
            }],
            config: NodeConfig::Notify(NotifyConfig {
                channel: NotifyChannel::Desktop,
                template: NotifyTemplate {
                    severity: NotifySeverity::Info,
                    title: "t".into(),
                    body: "b".into(),
                    artifacts: vec![],
                },
                on_failure: NotifyFailureAction::Continue,
            }),
        };

        // Outer node — Terminal so the graph has a valid start.
        let outer = NodeKey::try_from("outer_t").unwrap();
        let outer_node = Node {
            id: outer.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(crate::terminal_config::TerminalConfig {
                kind: crate::terminal_config::TerminalKind::Success,
                message: None,
            }),
        };

        let mut nodes = BTreeMap::new();
        nodes.insert(outer.clone(), outer_node);

        let mut inner_nodes = BTreeMap::new();
        inner_nodes.insert(inner_notify.clone(), inner_notify_node);

        let mut subgraphs = BTreeMap::new();
        subgraphs.insert(
            inner_body,
            Subgraph {
                start: inner_notify,
                nodes: inner_nodes,
                edges: vec![],
            },
        );

        let g = Graph {
            schema_version: SCHEMA_VERSION,
            metadata: GraphMetadata {
                name: "n".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: outer,
            nodes,
            edges: vec![],
            subgraphs,
        };

        let result = validate(&g);
        let errors = result.expect_errors("must fail");
        assert!(
            errors
                .iter()
                .any(|e| matches!(e.kind, ValidationErrorKind::NotifyMissingDelivered { .. })),
            "Notify-in-subgraph missing delivered must be caught: {errors:?}"
        );
    }
}

#[test]
fn mcp_server_undeclared_in_tool_override_is_error() {
    use crate::agent_config::{AgentConfig, NodeLimits, ToolOverride};
    use crate::keys::ProfileKey;
    use std::collections::BTreeMap;

    let stage_cfg = AgentConfig {
        profile: ProfileKey::try_from("implementer@1.0").unwrap(),
        prompt_overrides: None,
        tool_overrides: Some(ToolOverride {
            mcp_add: vec!["undeclared_server".into()],
            mcp_remove: vec![],
            skills_add: vec![],
            skills_remove: vec![],
            shell_allowlist_add: vec![],
        }),
        sandbox_override: None,
        approvals_override: None,
        bindings: vec![],
        rules_overrides: None,
        limits: NodeLimits::default(),
        hooks: vec![],
        custom_fields: BTreeMap::new(),
    };
    let registry: Vec<crate::mcp_config::McpServerRef> = vec![]; // empty registry
    let errors = crate::validation::validate_mcp_references("stage_x", &stage_cfg, &registry);
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0].kind,
        crate::validation::ValidationErrorKind::McpServerUndeclared { .. }
    ));
}

#[test]
fn empty_server_name_is_error() {
    use crate::mcp_config::{McpServerRef, McpTransportConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    let r = McpServerRef {
        name: "".into(),
        transport: McpTransportConfig::Stdio {
            command: PathBuf::from("nope"),
            args: vec![],
            env: HashMap::new(),
        },
        allowed_tools: None,
        call_timeout: Duration::from_secs(60),
        restart_on_crash: true,
        sandbox: None,
    };
    let errors = crate::validation::validate_mcp_server_ref(&r);
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        errors[0].kind,
        crate::validation::ValidationErrorKind::McpServerNameEmpty
    ));
}

#[test]
fn command_path_with_dotdot_segment_is_error() {
    use crate::mcp_config::{McpServerRef, McpTransportConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Duration;

    let r = McpServerRef {
        name: "evil".into(),
        transport: McpTransportConfig::Stdio {
            command: PathBuf::from("../../../usr/bin/yes"),
            args: vec![],
            env: HashMap::new(),
        },
        allowed_tools: None,
        call_timeout: Duration::from_secs(60),
        restart_on_crash: true,
        sandbox: None,
    };
    let errors = crate::validation::validate_mcp_server_ref(&r);
    assert!(errors.iter().any(|e| matches!(
        e.kind,
        crate::validation::ValidationErrorKind::McpCommandPathUnsafe { .. }
    )));
}

#[test]
fn validate_with_run_config_surfaces_mcp_undeclared() {
    use crate::agent_config::{AgentConfig, NodeLimits, ToolOverride};
    use crate::approvals::ApprovalPolicy;
    use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use crate::keys::{NodeKey, ProfileKey};
    use crate::node::{Node, NodeConfig, Position};
    use crate::run_event::RunConfig;
    use crate::sandbox::SandboxMode;
    use crate::terminal_config::{TerminalConfig, TerminalKind};
    use std::collections::BTreeMap;

    // Build a minimal valid graph: one Terminal node (so graph has a
    // valid start and a terminal) plus one Agent node that references an
    // undeclared MCP server.
    let terminal_key = NodeKey::try_from("end").unwrap();
    let stage_key = NodeKey::try_from("research").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        terminal_key.clone(),
        Node {
            id: terminal_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );
    nodes.insert(
        stage_key.clone(),
        Node {
            id: stage_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("researcher@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: Some(ToolOverride {
                    mcp_add: vec!["nope".into()],
                    mcp_remove: vec![],
                    skills_add: vec![],
                    skills_remove: vec![],
                    shell_allowlist_add: vec![],
                }),
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: NodeLimits::default(),
                hooks: vec![],
                custom_fields: BTreeMap::new(),
            }),
        },
    );
    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: terminal_key,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };
    let run_cfg = RunConfig {
        budget: Default::default(),
        sandbox_default: SandboxMode::ReadOnly,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: vec![], // empty: stage refs an undeclared server
    };
    let errors = crate::validation::validate_with_run_config(&graph, &run_cfg).into_findings();
    assert!(errors.iter().any(|e| matches!(
        e.kind,
        crate::validation::ValidationErrorKind::McpServerUndeclared { .. }
    )));
}

#[test]
fn validate_with_run_config_happy_path_returns_no_mcp_errors() {
    use crate::agent_config::{AgentConfig, NodeLimits, ToolOverride};
    use crate::approvals::ApprovalPolicy;
    use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use crate::keys::{NodeKey, ProfileKey};
    use crate::mcp_config::{McpServerRef, McpTransportConfig};
    use crate::node::{Node, NodeConfig, Position};
    use crate::run_event::RunConfig;
    use crate::sandbox::SandboxMode;
    use crate::terminal_config::{TerminalConfig, TerminalKind};
    use std::collections::{BTreeMap, HashMap};
    use std::path::PathBuf;
    use std::time::Duration;

    let stage_key = NodeKey::try_from("research").unwrap();
    let terminal_key = NodeKey::try_from("end").unwrap();

    let mut nodes = BTreeMap::new();
    nodes.insert(
        stage_key.clone(),
        Node {
            id: stage_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("researcher@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: Some(ToolOverride {
                    mcp_add: vec!["playwright".into()],
                    mcp_remove: vec![],
                    skills_add: vec![],
                    skills_remove: vec![],
                    shell_allowlist_add: vec![],
                }),
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: NodeLimits::default(),
                hooks: vec![],
                custom_fields: BTreeMap::new(),
            }),
        },
    );
    nodes.insert(
        terminal_key.clone(),
        Node {
            id: terminal_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );

    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata {
            name: "test".into(),
            description: None,
            template_origin: None,
            created_at: chrono::Utc::now(),
            author: None,
            archetype: None,
        },
        start: stage_key,
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };

    let run_cfg = RunConfig {
        budget: Default::default(),
        sandbox_default: SandboxMode::WorkspaceWrite,
        approval_default: ApprovalPolicy::OnRequest,
        auto_pr: false,
        mcp_servers: vec![McpServerRef {
            name: "playwright".into(),
            transport: McpTransportConfig::Stdio {
                command: PathBuf::from("/usr/local/bin/mcp-playwright"),
                args: vec![],
                env: HashMap::new(),
            },
            allowed_tools: None,
            call_timeout: Duration::from_secs(60),
            restart_on_crash: true,
            sandbox: None,
        }],
    };

    let errors = crate::validation::validate_with_run_config(&graph, &run_cfg).into_findings();
    // The graph has no edges (no routing) — graph-level validation may
    // still produce structural errors/warnings. Assert that NONE of the
    // M7 MCP-specific kinds are present (allowlist resolves cleanly).
    assert!(
        !errors.iter().any(|e| matches!(
            e.kind,
            crate::validation::ValidationErrorKind::McpServerUndeclared { .. }
                | crate::validation::ValidationErrorKind::McpServerNameEmpty
                | crate::validation::ValidationErrorKind::McpCommandPathUnsafe { .. }
        )),
        "happy path should not surface any MCP-specific errors; got: {errors:?}"
    );
}

#[test]
fn invalid_skills_declaration_is_rejected_by_graph_validation() {
    use crate::agent_config::{AgentConfig, NodeLimits};
    use crate::graph::{Graph, GraphMetadata, SCHEMA_VERSION};
    use crate::keys::{NodeKey, ProfileKey};
    use crate::node::{Node, NodeConfig, Position};
    use crate::terminal_config::{TerminalConfig, TerminalKind};
    use std::collections::BTreeMap;

    let stage_key = NodeKey::try_from("implement").unwrap();
    let terminal_key = NodeKey::try_from("end").unwrap();

    // `skills` present but shaped wrong (missing the required `name`
    // key) — a graph-authoring mistake, not a missing key.
    let mut custom_fields = BTreeMap::new();
    custom_fields.insert(
        "skills".to_string(),
        toml::Value::Array(vec![toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "provider".to_string(),
                toml::Value::String("project_dir".into()),
            );
            t
        })]),
    );

    let mut nodes = BTreeMap::new();
    nodes.insert(
        stage_key.clone(),
        Node {
            id: stage_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Agent(AgentConfig {
                profile: ProfileKey::try_from("implementer@1.0").unwrap(),
                prompt_overrides: None,
                tool_overrides: None,
                sandbox_override: None,
                approvals_override: None,
                bindings: vec![],
                rules_overrides: None,
                limits: NodeLimits::default(),
                hooks: vec![],
                custom_fields,
            }),
        },
    );
    nodes.insert(
        terminal_key.clone(),
        Node {
            id: terminal_key.clone(),
            position: Position::default(),
            declared_outcomes: vec![],
            config: NodeConfig::Terminal(TerminalConfig {
                kind: TerminalKind::Success,
                message: None,
            }),
        },
    );

    let graph = Graph {
        schema_version: SCHEMA_VERSION,
        metadata: GraphMetadata::new("skills-load-validation", chrono::Utc::now()),
        start: stage_key.clone(),
        nodes,
        edges: vec![],
        subgraphs: BTreeMap::new(),
    };

    let errors = crate::validation::validate(&graph)
        .expect_errors("a malformed skills declaration must be a validation error");
    let finding = errors
        .iter()
        .find(|e| matches!(e.kind, ValidationErrorKind::InvalidSkillsDeclaration { .. }))
        .expect("expected an InvalidSkillsDeclaration finding");
    match &finding.kind {
        ValidationErrorKind::InvalidSkillsDeclaration { node, reason } => {
            assert_eq!(*node, stage_key);
            assert!(
                !reason.is_empty(),
                "the finding must name why the declaration is invalid"
            );
        },
        other => panic!("expected InvalidSkillsDeclaration, got {other:?}"),
    }
}
