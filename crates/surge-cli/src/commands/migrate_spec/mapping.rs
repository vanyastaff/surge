//! Pure mapping rules: legacy [`Spec`](surge_core::spec::Spec) → `flow.toml`.
//!
//! Linear dependency graphs translate deterministically. Non-linear cases
//! (fan-in, diamond, multiple roots) and missing-profile subtasks produce
//! `MappingWarning`s — the caller decides whether to fail or annotate the
//! output for human edit.
//!
//! See `docs/migrate-spec-to-flow.md` for the user-facing mapping reference.

use std::collections::{HashMap, HashSet};

use surge_core::id::SubtaskId;
use surge_core::spec::{Spec, Subtask};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, value};

const DEFAULT_PROFILE: &str = "implementer@1.0";
const SUCCESS_TERMINAL_ID: &str = "success";
const FAILURE_TERMINAL_ID: &str = "failure";
const POSITION_X_STEP: f64 = 200.0;

/// Fatal mapping problems. These cannot be auto-corrected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MappingError {
    /// Spec has zero subtasks; nothing to migrate.
    EmptySpec,
    /// A subtask depends on an unknown subtask id.
    UnknownDependency {
        /// Node id that declared the bad dependency.
        subtask: String,
        /// Missing dependency id (rendered as a string).
        missing: String,
    },
    /// A subtask depends on its own id.
    SelfDependency {
        /// Offending node id.
        subtask: String,
    },
    /// One or more dependency cycles detected; can't linearize.
    CyclicDependencies,
}

impl std::fmt::Display for MappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySpec => write!(f, "spec has zero subtasks; nothing to migrate"),
            Self::UnknownDependency { subtask, missing } => {
                write!(f, "subtask {subtask} depends on unknown subtask {missing}")
            },
            Self::SelfDependency { subtask } => {
                write!(f, "subtask {subtask} depends on itself")
            },
            Self::CyclicDependencies => write!(f, "dependency cycle detected"),
        }
    }
}

impl std::error::Error for MappingError {}

/// Soft mapping concerns that the migrator surfaces but does not treat as
/// failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WarningKind {
    /// Subtask has multiple incoming edges (fan-in or diamond); routing may
    /// need manual review.
    NonLinearDeps,
    /// Subtask had no `agent` set; defaulted to [`DEFAULT_PROFILE`].
    ProfileDefaulted,
    /// Spec had multiple root subtasks (no `depends_on`); the first one in
    /// spec order was chosen as `start`.
    MultipleRoots,
}

/// One mapping concern with enough context for a CLI to render it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct MappingWarning {
    /// Concern category.
    pub kind: WarningKind,
    /// Node id this warning attaches to (None for spec-level concerns).
    pub subtask: Option<String>,
    /// Human-readable rendering.
    pub message: String,
}

/// Output of a successful mapping pass.
#[derive(Debug)]
#[non_exhaustive]
pub struct MappingResult {
    /// Freshly built `flow.toml` document.
    pub document: DocumentMut,
    /// Non-fatal concerns gathered during mapping.
    pub warnings: Vec<MappingWarning>,
}

/// Translate a legacy spec into a `flow.toml` document.
///
/// # Errors
///
/// - [`MappingError::EmptySpec`] if there are no subtasks.
/// - [`MappingError::UnknownDependency`] for dangling dep ids.
/// - [`MappingError::SelfDependency`] when a subtask lists its own id.
/// - [`MappingError::CyclicDependencies`] if the deps form a cycle.
pub fn map_spec_to_flow(spec: &Spec) -> Result<MappingResult, MappingError> {
    if spec.subtasks.is_empty() {
        return Err(MappingError::EmptySpec);
    }

    let id_map = build_id_map(&spec.subtasks);
    validate_deps(&spec.subtasks, &id_map)?;
    if has_cycle(&spec.subtasks) {
        return Err(MappingError::CyclicDependencies);
    }

    let mut warnings = Vec::new();
    let start_id = resolve_start(&spec.subtasks, &id_map, &mut warnings);
    collect_soft_warnings(&spec.subtasks, &id_map, &mut warnings);

    let leaves = collect_leaves(&spec.subtasks);

    let mut doc = DocumentMut::new();
    doc.insert("schema_version", value(1_i64));
    doc.insert("start", value(start_id));
    doc.insert("metadata", Item::Table(build_metadata(spec)));
    doc.insert(
        "nodes",
        Item::Table(build_nodes_table(&spec.subtasks, &id_map)),
    );
    doc.insert(
        "edges",
        Item::ArrayOfTables(build_edges(&spec.subtasks, &id_map, &leaves)),
    );

    Ok(MappingResult {
        document: doc,
        warnings,
    })
}

fn build_id_map(subtasks: &[Subtask]) -> HashMap<SubtaskId, String> {
    subtasks
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id, format!("s{}", i + 1)))
        .collect()
}

fn validate_deps(
    subtasks: &[Subtask],
    id_map: &HashMap<SubtaskId, String>,
) -> Result<(), MappingError> {
    let known: HashSet<SubtaskId> = subtasks.iter().map(|s| s.id).collect();
    for subtask in subtasks {
        for dep in &subtask.depends_on {
            if *dep == subtask.id {
                return Err(MappingError::SelfDependency {
                    subtask: id_map[&subtask.id].clone(),
                });
            }
            if !known.contains(dep) {
                return Err(MappingError::UnknownDependency {
                    subtask: id_map[&subtask.id].clone(),
                    missing: dep.to_string(),
                });
            }
        }
    }
    Ok(())
}

fn has_cycle(subtasks: &[Subtask]) -> bool {
    let deps: HashMap<SubtaskId, Vec<SubtaskId>> = subtasks
        .iter()
        .map(|s| (s.id, s.depends_on.clone()))
        .collect();

    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();
    for subtask in subtasks {
        if !visited.contains(&subtask.id)
            && dfs_detect_cycle(subtask.id, &deps, &mut visited, &mut in_stack)
        {
            return true;
        }
    }
    false
}

fn dfs_detect_cycle(
    node: SubtaskId,
    deps: &HashMap<SubtaskId, Vec<SubtaskId>>,
    visited: &mut HashSet<SubtaskId>,
    in_stack: &mut HashSet<SubtaskId>,
) -> bool {
    visited.insert(node);
    in_stack.insert(node);
    if let Some(d) = deps.get(&node) {
        for next in d {
            if !visited.contains(next) {
                if dfs_detect_cycle(*next, deps, visited, in_stack) {
                    return true;
                }
            } else if in_stack.contains(next) {
                return true;
            }
        }
    }
    in_stack.remove(&node);
    false
}

fn resolve_start(
    subtasks: &[Subtask],
    id_map: &HashMap<SubtaskId, String>,
    warnings: &mut Vec<MappingWarning>,
) -> String {
    let roots: Vec<&Subtask> = subtasks
        .iter()
        .filter(|s| s.depends_on.is_empty())
        .collect();
    if roots.len() > 1 {
        warnings.push(MappingWarning {
            kind: WarningKind::MultipleRoots,
            subtask: None,
            message: format!(
                "spec has {} root subtasks; flow.toml uses the first ({}) as the start node",
                roots.len(),
                id_map[&roots[0].id]
            ),
        });
    }
    roots.first().map_or_else(
        || id_map[&subtasks[0].id].clone(),
        |s| id_map[&s.id].clone(),
    )
}

fn collect_soft_warnings(
    subtasks: &[Subtask],
    id_map: &HashMap<SubtaskId, String>,
    warnings: &mut Vec<MappingWarning>,
) {
    for subtask in subtasks {
        if subtask.depends_on.len() > 1 {
            warnings.push(MappingWarning {
                kind: WarningKind::NonLinearDeps,
                subtask: Some(id_map[&subtask.id].clone()),
                message: format!(
                    "subtask {} has {} incoming edges; verify fan-in is intended",
                    id_map[&subtask.id],
                    subtask.depends_on.len()
                ),
            });
        }
        if subtask.agent.is_none() {
            warnings.push(MappingWarning {
                kind: WarningKind::ProfileDefaulted,
                subtask: Some(id_map[&subtask.id].clone()),
                message: format!(
                    "subtask {} had no agent; defaulted to {DEFAULT_PROFILE}",
                    id_map[&subtask.id],
                ),
            });
        }
    }
}

fn collect_leaves(subtasks: &[Subtask]) -> HashSet<SubtaskId> {
    let mut has_successor: HashSet<SubtaskId> = HashSet::new();
    for subtask in subtasks {
        for dep in &subtask.depends_on {
            has_successor.insert(*dep);
        }
    }
    subtasks
        .iter()
        .filter(|s| !has_successor.contains(&s.id))
        .map(|s| s.id)
        .collect()
}

fn build_metadata(spec: &Spec) -> Table {
    let mut metadata = Table::new();
    metadata.insert("name", value(slugify(&spec.title)));
    metadata.insert(
        "description",
        value(if spec.description.trim().is_empty() {
            spec.title.clone()
        } else {
            spec.description.clone()
        }),
    );
    metadata
}

fn build_nodes_table(subtasks: &[Subtask], id_map: &HashMap<SubtaskId, String>) -> Table {
    let mut nodes = Table::new();
    nodes.set_implicit(true);
    for (i, subtask) in subtasks.iter().enumerate() {
        let node_id = &id_map[&subtask.id];
        nodes.insert(node_id, Item::Table(build_agent_node(node_id, subtask, i)));
    }
    nodes.insert(
        SUCCESS_TERMINAL_ID,
        Item::Table(build_terminal_node(
            SUCCESS_TERMINAL_ID,
            "success",
            subtasks.len(),
        )),
    );
    nodes.insert(
        FAILURE_TERMINAL_ID,
        Item::Table(build_terminal_node(
            FAILURE_TERMINAL_ID,
            "failure",
            subtasks.len(),
        )),
    );
    nodes
}

fn build_agent_node(node_id: &str, subtask: &Subtask, index: usize) -> Table {
    let mut node = Table::new();
    node.insert("id", value(node_id.to_string()));
    node.insert("position", Item::Table(build_position(index, 0.0)));
    node.insert(
        "declared_outcomes",
        Item::ArrayOfTables(build_agent_outcomes(subtask)),
    );
    node.insert("config", Item::Table(build_agent_config(subtask)));
    node
}

fn build_position(index: usize, y: f64) -> Table {
    let mut position = Table::new();
    let x = position_x_for(index);
    position.insert("x", value(x));
    position.insert("y", value(y));
    position
}

fn position_x_for(index: usize) -> f64 {
    let bounded = i32::try_from(index).unwrap_or(0);
    f64::from(bounded) * POSITION_X_STEP
}

fn build_agent_outcomes(subtask: &Subtask) -> ArrayOfTables {
    let mut outcomes = ArrayOfTables::new();

    let mut pass = Table::new();
    pass.insert("id", value("pass"));
    pass.insert("description", value(format!("{} passed", subtask.title)));
    pass.insert("edge_kind_hint", value("forward"));
    pass.insert("is_terminal", value(false));
    outcomes.push(pass);

    let mut fail = Table::new();
    fail.insert("id", value("fail"));
    fail.insert("description", value(format!("{} failed", subtask.title)));
    fail.insert("edge_kind_hint", value("escalate"));
    fail.insert("is_terminal", value(false));
    outcomes.push(fail);

    outcomes
}

fn build_agent_config(subtask: &Subtask) -> Table {
    let mut config = Table::new();
    config.insert("node_kind", value("agent"));
    config.insert(
        "profile",
        value(
            subtask
                .agent
                .clone()
                .unwrap_or_else(|| DEFAULT_PROFILE.to_string()),
        ),
    );
    config.insert("bindings", value(Array::new()));
    config.insert("hooks", value(Array::new()));
    config.insert("limits", Item::Table(build_default_limits()));
    config.insert("custom_fields", Item::Table(build_custom_fields(subtask)));
    config
}

fn build_default_limits() -> Table {
    let mut limits = Table::new();
    limits.insert("timeout_seconds", value(900_i64));
    limits.insert("max_retries", value(3_i64));
    limits.insert("max_tokens", value(200_000_i64));
    limits
}

fn build_custom_fields(subtask: &Subtask) -> Table {
    let mut custom_fields = Table::new();
    if !subtask.acceptance_criteria.is_empty() {
        let mut crits = Array::new();
        for c in &subtask.acceptance_criteria {
            crits.push(c.description.clone());
        }
        custom_fields.insert("acceptance_criteria", value(crits));
    }
    if !subtask.files.is_empty() {
        let mut files = Array::new();
        for f in &subtask.files {
            files.push(f.clone());
        }
        custom_fields.insert("files", value(files));
    }
    custom_fields.insert("complexity", value(complexity_str(subtask)));
    custom_fields
}

fn complexity_str(subtask: &Subtask) -> String {
    match subtask.complexity {
        surge_core::spec::Complexity::Simple => "simple",
        surge_core::spec::Complexity::Standard => "standard",
        surge_core::spec::Complexity::Complex => "complex",
    }
    .to_string()
}

fn build_terminal_node(id: &str, terminal_kind: &str, index: usize) -> Table {
    let mut node = Table::new();
    node.insert("id", value(id.to_string()));
    let y = if terminal_kind == "success" {
        0.0
    } else {
        200.0
    };
    node.insert("position", Item::Table(build_position(index, y)));
    node.insert("declared_outcomes", value(Array::new()));

    let mut config = Table::new();
    config.insert("node_kind", value("terminal"));
    let mut kind = Table::new();
    kind.insert("type", value(terminal_kind.to_string()));
    if terminal_kind == "failure" {
        kind.insert("exit_code", value(1_i64));
    }
    config.insert("kind", Item::Table(kind));
    node.insert("config", Item::Table(config));
    node
}

fn build_edges(
    subtasks: &[Subtask],
    id_map: &HashMap<SubtaskId, String>,
    leaves: &HashSet<SubtaskId>,
) -> ArrayOfTables {
    let mut edges = ArrayOfTables::new();
    let mut edge_counter = 0_usize;

    for subtask in subtasks {
        let to = &id_map[&subtask.id];
        for dep in &subtask.depends_on {
            let from_id = &id_map[dep];
            edge_counter += 1;
            edges.push(build_edge_table(
                &format!("e{edge_counter}_pass"),
                from_id,
                "pass",
                to,
                "forward",
            ));
        }
        edge_counter += 1;
        edges.push(build_edge_table(
            &format!("e{edge_counter}_fail"),
            to,
            "fail",
            FAILURE_TERMINAL_ID,
            "forward",
        ));
    }

    for subtask in subtasks {
        if leaves.contains(&subtask.id) {
            let from_id = &id_map[&subtask.id];
            edge_counter += 1;
            edges.push(build_edge_table(
                &format!("e{edge_counter}_success"),
                from_id,
                "pass",
                SUCCESS_TERMINAL_ID,
                "forward",
            ));
        }
    }

    edges
}

fn build_edge_table(id: &str, from_node: &str, outcome: &str, to: &str, kind: &str) -> Table {
    let mut edge = Table::new();
    edge.insert("id", value(id.to_string()));
    edge.insert("to", value(to.to_string()));
    edge.insert("kind", value(kind.to_string()));

    let mut from = Table::new();
    from.insert("node", value(from_node.to_string()));
    from.insert("outcome", value(outcome.to_string()));
    edge.insert("from", Item::Table(from));

    let mut policy = Table::new();
    policy.insert("on_max_exceeded", value("escalate"));
    edge.insert("policy", Item::Table(policy));
    edge
}

fn slugify(title: &str) -> String {
    let lower = title.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut last_dash = false;
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('_');
            last_dash = true;
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::spec::{AcceptanceCriteria, Complexity, Spec, Subtask};

    fn make_subtask(title: &str, agent: Option<&str>, deps: Vec<SubtaskId>) -> Subtask {
        let mut s = Subtask::new(title, format!("Do {title}"), Complexity::Simple);
        s.depends_on = deps;
        s.agent = agent.map(str::to_string);
        s
    }

    fn make_spec(title: &str, subtasks: Vec<Subtask>) -> Spec {
        let mut spec = Spec::new(title, format!("{title} description"), Complexity::Standard);
        spec.subtasks = subtasks;
        spec
    }

    fn render(result: &MappingResult) -> String {
        result.document.to_string()
    }

    #[test]
    fn single_subtask_maps_to_one_node_plus_terminals() {
        let s1 = make_subtask("Build", Some("implementer@1.0"), vec![]);
        let spec = make_spec("Single", vec![s1]);

        let result = map_spec_to_flow(&spec).unwrap();
        let rendered = render(&result);

        assert!(rendered.contains("schema_version = 1"));
        assert!(rendered.contains("start = \"s1\""));
        assert!(rendered.contains("[nodes.s1]"));
        assert!(rendered.contains("[nodes.success]"));
        assert!(rendered.contains("[nodes.failure]"));
        assert!(rendered.contains("profile = \"implementer@1.0\""));
        assert_eq!(
            result
                .warnings
                .iter()
                .filter(|w| w.kind == WarningKind::ProfileDefaulted)
                .count(),
            0
        );
    }

    #[test]
    fn linear_chain_emits_sequential_edges() {
        let s1 = make_subtask("A", Some("implementer@1.0"), vec![]);
        let s2 = make_subtask("B", Some("implementer@1.0"), vec![s1.id]);
        let s3 = make_subtask("C", Some("implementer@1.0"), vec![s2.id]);
        let spec = make_spec("Linear", vec![s1, s2, s3]);

        let result = map_spec_to_flow(&spec).unwrap();
        let rendered = render(&result);

        assert!(rendered.contains("[nodes.s1]"));
        assert!(rendered.contains("[nodes.s2]"));
        assert!(rendered.contains("[nodes.s3]"));
        let pass_edges = rendered.matches("outcome = \"pass\"").count();
        assert!(
            pass_edges >= 3,
            "expected at least 3 pass edges (deps + leaf→success), got {pass_edges}",
        );
        assert!(
            result.warnings.is_empty(),
            "warnings: {:?}",
            result.warnings
        );
    }

    #[test]
    fn fan_out_keeps_warnings_empty() {
        let a = make_subtask("A", Some("implementer@1.0"), vec![]);
        let b = make_subtask("B", Some("implementer@1.0"), vec![a.id]);
        let c = make_subtask("C", Some("implementer@1.0"), vec![a.id]);
        let spec = make_spec("FanOut", vec![a, b, c]);

        let result = map_spec_to_flow(&spec).unwrap();
        assert!(
            result.warnings.is_empty(),
            "warnings: {:?}",
            result.warnings
        );
        let rendered = render(&result);
        assert!(rendered.contains("[nodes.s2]"));
        assert!(rendered.contains("[nodes.s3]"));
    }

    #[test]
    fn diamond_flags_fan_in_warning() {
        let a = make_subtask("A", Some("implementer@1.0"), vec![]);
        let b = make_subtask("B", Some("implementer@1.0"), vec![a.id]);
        let c = make_subtask("C", Some("implementer@1.0"), vec![a.id]);
        let b_id = b.id;
        let c_id = c.id;
        let d = make_subtask("D", Some("implementer@1.0"), vec![b_id, c_id]);
        let spec = make_spec("Diamond", vec![a, b, c, d]);

        let result = map_spec_to_flow(&spec).unwrap();
        assert_eq!(
            result
                .warnings
                .iter()
                .filter(|w| w.kind == WarningKind::NonLinearDeps)
                .count(),
            1
        );
    }

    #[test]
    fn multiple_roots_warning() {
        let a = make_subtask("A", Some("implementer@1.0"), vec![]);
        let b = make_subtask("B", Some("implementer@1.0"), vec![]);
        let spec = make_spec("Multi", vec![a, b]);

        let result = map_spec_to_flow(&spec).unwrap();
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.kind == WarningKind::MultipleRoots)
        );
    }

    #[test]
    fn no_profile_defaults_and_warns() {
        let s1 = make_subtask("Solo", None, vec![]);
        let spec = make_spec("NoProfile", vec![s1]);

        let result = map_spec_to_flow(&spec).unwrap();
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.kind == WarningKind::ProfileDefaulted)
        );
        let rendered = render(&result);
        assert!(rendered.contains(&format!("profile = \"{DEFAULT_PROFILE}\"")));
    }

    #[test]
    fn acceptance_criteria_embedded_in_custom_fields() {
        let mut s1 = make_subtask("WithCrit", Some("implementer@1.0"), vec![]);
        s1.acceptance_criteria = vec![
            AcceptanceCriteria::new("Compiles"),
            AcceptanceCriteria::new("Tests pass"),
        ];
        let spec = make_spec("WithCrits", vec![s1]);

        let result = map_spec_to_flow(&spec).unwrap();
        let rendered = render(&result);
        assert!(rendered.contains("acceptance_criteria"));
        assert!(rendered.contains("Compiles"));
        assert!(rendered.contains("Tests pass"));
    }

    #[test]
    fn cycle_rejected() {
        let mut a = Subtask::new("A", "A", Complexity::Simple);
        let mut b = Subtask::new("B", "B", Complexity::Simple);
        a.depends_on = vec![b.id];
        b.depends_on = vec![a.id];
        let spec = make_spec("Cycle", vec![a, b]);

        let err = map_spec_to_flow(&spec).unwrap_err();
        assert_eq!(err, MappingError::CyclicDependencies);
    }

    #[test]
    fn empty_rejected() {
        let spec = make_spec("Empty", vec![]);
        let err = map_spec_to_flow(&spec).unwrap_err();
        assert_eq!(err, MappingError::EmptySpec);
    }

    #[test]
    fn self_dependency_rejected() {
        let mut a = Subtask::new("A", "A", Complexity::Simple);
        a.depends_on = vec![a.id];
        let spec = make_spec("Self", vec![a]);
        let err = map_spec_to_flow(&spec).unwrap_err();
        match err {
            MappingError::SelfDependency { subtask } => assert_eq!(subtask, "s1"),
            other => panic!("expected SelfDependency, got {other:?}"),
        }
    }

    #[test]
    fn unknown_dependency_rejected() {
        let stray = SubtaskId::new();
        let mut a = Subtask::new("A", "A", Complexity::Simple);
        a.depends_on = vec![stray];
        let spec = make_spec("Stray", vec![a]);
        let err = map_spec_to_flow(&spec).unwrap_err();
        match err {
            MappingError::UnknownDependency { subtask, .. } => assert_eq!(subtask, "s1"),
            other => panic!("expected UnknownDependency, got {other:?}"),
        }
    }

    #[test]
    fn slugify_handles_punctuation() {
        assert_eq!(slugify("Hello World!"), "hello_world");
        assert_eq!(slugify("Foo --- Bar"), "foo_bar");
        assert_eq!(slugify("ABC"), "abc");
    }
}
