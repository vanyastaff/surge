//! Branch node configuration with structured predicates.

use crate::keys::{NodeKey, OutcomeKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchConfig {
    pub predicates: Vec<BranchArm>,
    pub default_outcome: OutcomeKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BranchArm {
    pub condition: Predicate,
    pub outcome: OutcomeKey,
}

/// Predicate AST for branch-node conditions.
///
/// Leaf variants are distinguished by their unique field names under
/// `#[serde(untagged)]`; combinator variants (`And`/`Or`/`Not`) use
/// struct-variant syntax so their keys (`and`/`or`/`not`) also serve as
/// the discriminant, avoiding the recursion-limit overflow that
/// `#[serde(tag = "type")]` causes on recursive types.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Predicate {
    FileExists {
        path: String,
    },
    ArtifactSize {
        artifact: String,
        op: CompareOp,
        value: u64,
    },
    OutcomeMatches {
        node: NodeKey,
        outcome: OutcomeKey,
    },
    EnvVar {
        name: String,
        op: CompareOp,
        value: String,
    },
    And {
        and: Vec<Predicate>,
    },
    Or {
        or: Vec<Predicate>,
    },
    Not {
        not: Box<Predicate>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_with_file_exists_predicate_roundtrips() {
        let cfg = BranchConfig {
            predicates: vec![BranchArm {
                condition: Predicate::FileExists {
                    path: "Cargo.toml".into(),
                },
                outcome: OutcomeKey::try_from("rust").unwrap(),
            }],
            default_outcome: OutcomeKey::try_from("generic").unwrap(),
        };
        let toml_s = toml::to_string(&cfg).unwrap();
        let parsed: BranchConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn nested_and_or_not_predicates_roundtrip() {
        let p = Predicate::And {
            and: vec![
                Predicate::FileExists {
                    path: "Cargo.toml".into(),
                },
                Predicate::Or {
                    or: vec![
                        Predicate::FileExists {
                            path: "src/lib.rs".into(),
                        },
                        Predicate::Not {
                            not: Box::new(Predicate::FileExists {
                                path: "src/main.rs".into(),
                            }),
                        },
                    ],
                },
            ],
        };
        let toml_s = toml::to_string(&p).unwrap();
        let parsed: Predicate = toml::from_str(&toml_s).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn artifact_size_predicate_roundtrips() {
        let p = Predicate::ArtifactSize {
            artifact: "spec.md".into(),
            op: CompareOp::Gt,
            value: 1024,
        };
        let toml_s = toml::to_string(&p).unwrap();
        let parsed: Predicate = toml::from_str(&toml_s).unwrap();
        assert_eq!(p, parsed);
    }
}
