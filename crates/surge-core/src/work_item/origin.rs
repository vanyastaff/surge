//! Validated accepted origin; flow prompts never become synthetic task criteria.
use super::WorkItemRequirements;
use crate::{ContentHash, Graph};
use serde::{Deserialize, Serialize};

const FLOW_HASH_DOMAIN: &[u8] = b"surge.accepted-flow-contract.v1\0";
const MAX_PROMPT_BYTES: usize = 131_072;

/// Invalid accepted flow content, without echoing user input.
#[derive(Debug, thiserror::Error)]
pub enum AcceptedFlowError {
    #[error("accepted flow prompt exceeds its byte bound")]
    PromptTooLarge,
    #[error("accepted flow graph is invalid")]
    InvalidGraph,
    #[error("accepted flow graph payload does not match its hash")]
    GraphHashMismatch,
    #[error("accepted flow content could not be encoded")]
    Encoding(#[source] serde_json::Error),
}

/// Exact graph payload and raw user prompt accepted for an ordinary flow.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawFlowContract")]
pub struct AcceptedFlowContract {
    graph: Box<Graph>,
    graph_hash: ContentHash,
    raw_prompt: String,
}

impl std::fmt::Debug for AcceptedFlowContract {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AcceptedFlowContract")
            .field("graph_hash", &self.graph_hash)
            .field("prompt_bytes", &self.raw_prompt.len())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFlowContract {
    graph: Box<Graph>,
    graph_hash: ContentHash,
    raw_prompt: String,
}

impl TryFrom<RawFlowContract> for AcceptedFlowContract {
    type Error = AcceptedFlowError;
    fn try_from(raw: RawFlowContract) -> Result<Self, Self::Error> {
        Self::from_parts(raw.graph, raw.graph_hash, raw.raw_prompt)
    }
}

impl AcceptedFlowContract {
    /// Accept a graph and prompt without trimming or decorating either.
    ///
    /// # Errors
    /// Rejects invalid graphs, oversized prompts and encoding failures.
    pub fn new(graph: Box<Graph>, raw_prompt: String) -> Result<Self, AcceptedFlowError> {
        let graph_hash = graph_identity(&graph)?;
        Self::from_parts(graph, graph_hash, raw_prompt)
    }

    /// Revalidate persisted content and its claimed graph identity.
    ///
    /// # Errors
    /// Rejects an invalid graph, graph/hash mismatch or oversized prompt.
    pub fn from_parts(
        graph: Box<Graph>,
        graph_hash: ContentHash,
        raw_prompt: String,
    ) -> Result<Self, AcceptedFlowError> {
        if raw_prompt.len() > MAX_PROMPT_BYTES {
            return Err(AcceptedFlowError::PromptTooLarge);
        }
        if graph.schema_version != crate::graph::SCHEMA_VERSION
            || crate::validation::validate(&graph).has_errors()
            || graph
                .nodes
                .values()
                .chain(
                    graph
                        .subgraphs
                        .values()
                        .flat_map(|subgraph| subgraph.nodes.values()),
                )
                .any(|node| !node.position.x.is_finite() || !node.position.y.is_finite())
        {
            return Err(AcceptedFlowError::InvalidGraph);
        }
        if graph_identity(&graph)? != graph_hash {
            return Err(AcceptedFlowError::GraphHashMismatch);
        }
        Ok(Self {
            graph,
            graph_hash,
            raw_prompt,
        })
    }

    /// Immutable accepted graph payload.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Existing graph identity used by pipeline materialization.
    #[must_use]
    pub fn graph_hash(&self) -> ContentHash {
        self.graph_hash
    }

    /// Exact submitted user prompt, including empty and whitespace-only input.
    #[must_use]
    pub fn raw_prompt(&self) -> &str {
        &self.raw_prompt
    }

    /// Domain-separated artifact bytes authenticating graph and prompt together.
    ///
    /// # Errors
    /// Returns an encoding failure if accepted content cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        let mut bytes = FLOW_HASH_DOMAIN.to_vec();
        bytes.extend(serde_json::to_vec(self)?);
        Ok(bytes)
    }

    /// Domain-separated accepted content identity.
    ///
    /// # Errors
    /// Returns an encoding failure if accepted content cannot be encoded.
    pub fn hash(&self) -> Result<ContentHash, serde_json::Error> {
        self.canonical_bytes()
            .map(|bytes| ContentHash::compute(&bytes))
    }

    /// Recheck a frozen attempt graph against the accepted payload and hash.
    ///
    /// # Errors
    /// Rejects any graph whose canonical payload differs from the accepted graph.
    pub fn validate_graph(&self, graph: &Graph) -> Result<(), AcceptedFlowError> {
        if graph_identity(graph)? != self.graph_hash
            || graph_identity(&self.graph)? != self.graph_hash
        {
            return Err(AcceptedFlowError::GraphHashMismatch);
        }
        Ok(())
    }
}

fn graph_identity(graph: &Graph) -> Result<ContentHash, AcceptedFlowError> {
    serde_json::to_vec(graph)
        .map(|bytes| ContentHash::compute(&bytes))
        .map_err(AcceptedFlowError::Encoding)
}

/// Typed accepted origin of an ordinary graph-backed flow.
pub type FlowOrigin = AcceptedFlowContract;

/// Accepted task specification or accepted ordinary flow, without conversion.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "RawAcceptedOrigin")]
pub enum AcceptedWorkItemOrigin {
    /// Legacy immutable specification and acceptance criteria.
    Requirements(WorkItemRequirements),
    /// Exact ordinary flow graph and prompt.
    Flow(AcceptedFlowContract),
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum RawAcceptedOrigin {
    Requirements(LegacyOrigin),
    Flow(TaggedFlowOrigin),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyOrigin {
    requirements: WorkItemRequirements,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaggedFlowOrigin {
    origin: FlowTag,
    flow: AcceptedFlowContract,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FlowTag {
    Flow,
}
impl From<RawAcceptedOrigin> for AcceptedWorkItemOrigin {
    fn from(raw: RawAcceptedOrigin) -> Self {
        match raw {
            RawAcceptedOrigin::Requirements(raw) => Self::Requirements(raw.requirements),
            RawAcceptedOrigin::Flow(raw) => Self::Flow(raw.flow),
        }
    }
}
impl Serialize for AcceptedWorkItemOrigin {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            Self::Requirements(requirements) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("requirements", requirements)?;
                map.end()
            },
            Self::Flow(flow) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("origin", "flow")?;
                map.serialize_entry("flow", flow)?;
                map.end()
            },
        }
    }
}
impl AcceptedWorkItemOrigin {
    /// Exact legacy specification, absent for ordinary flows.
    #[must_use]
    pub fn requirements(&self) -> Option<&WorkItemRequirements> {
        match self {
            Self::Requirements(requirements) => Some(requirements),
            Self::Flow(_) => None,
        }
    }
    /// Exact accepted flow contract, absent for legacy specifications.
    #[must_use]
    pub fn flow(&self) -> Option<&AcceptedFlowContract> {
        match self {
            Self::Requirements(_) => None,
            Self::Flow(flow) => Some(flow),
        }
    }
    /// Accepted artifact bytes; legacy serialization is unchanged.
    ///
    /// # Errors
    /// Returns an encoding failure if accepted content cannot be encoded.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::Requirements(requirements) => serde_json::to_vec(requirements),
            Self::Flow(flow) => flow.canonical_bytes(),
        }
    }
    /// Origin-specific immutable content identity.
    ///
    /// # Errors
    /// Returns an encoding failure if accepted content cannot be encoded.
    pub fn hash(&self) -> Result<ContentHash, serde_json::Error> {
        self.canonical_bytes()
            .map(|bytes| ContentHash::compute(&bytes))
    }
}
