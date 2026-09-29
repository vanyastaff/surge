//! Typed identifiers for the roadmap model.
//!
//! Each id is a transparent string on the wire (TOML, JSON and the generated
//! JSON schema are identical to a bare `String`), but a distinct type in Rust,
//! so a milestone id cannot be passed where a task id is expected.

use std::borrow::Borrow;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! roadmap_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        // Hand-written instead of derived: the derive would attach the doc
        // comment as a `description` at every use site, changing the
        // generated JSON schemas. The wire shape is exactly `String`'s.
        impl JsonSchema for $name {
            fn inline_schema() -> bool {
                true
            }

            fn schema_name() -> std::borrow::Cow<'static, str> {
                <String as JsonSchema>::schema_name()
            }

            fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
                <String as JsonSchema>::json_schema(generator)
            }
        }

        impl $name {
            /// Borrow the id as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for str {
            fn eq(&self, other: &$name) -> bool {
                self == other.0
            }
        }

        impl PartialEq<$name> for &str {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }
    };
}

roadmap_id! {
    /// Identifier of a [`RoadmapMilestone`](super::RoadmapMilestone), for example `m1`.
    MilestoneId
}
roadmap_id! {
    /// Identifier of a [`RoadmapTask`](super::RoadmapTask), for example `m1-t1`.
    ///
    /// Named `RoadmapTaskId` to stay distinct from the FSM [`TaskId`](crate::TaskId).
    RoadmapTaskId
}
roadmap_id! {
    /// Identifier of a [`RoadmapMission`](super::RoadmapMission), for example `mission-1`.
    MissionId
}
roadmap_id! {
    /// Identifier of a [`ValidationAssertion`](super::ValidationAssertion), for example `VAL-AUTH-001`.
    AssertionId
}
roadmap_id! {
    /// Identifier of a [`RoadmapStage`](super::RoadmapStage), for example `stage-1`.
    StageId
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn serde_is_a_bare_string() {
        let id = MilestoneId::from("m1");
        assert_eq!(serde_json::to_string(&id).unwrap(), "\"m1\"");
        let back: MilestoneId = serde_json::from_str("\"m1\"").unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn compares_with_str_and_converts_back() {
        let id = MilestoneId::from("m1");
        assert!(id == "m1");
        assert!(id == *"m1");
        assert!("m1" == id);
        assert_eq!(String::from(id), "m1");
    }

    #[test]
    fn hash_map_is_queryable_by_str() {
        let mut map: HashMap<MilestoneId, u32> = HashMap::new();
        map.insert("m1".into(), 7);
        assert_eq!(map.get("m1"), Some(&7));
    }

    #[test]
    fn display_is_the_bare_id() {
        assert_eq!(RoadmapTaskId::from("m1-t1".to_owned()).to_string(), "m1-t1");
        assert_eq!(AssertionId::from("VAL-1").as_str(), "VAL-1");
    }
}
