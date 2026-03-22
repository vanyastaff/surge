//! Type-safe identifiers for Surge entities.

use serde::{Deserialize, Serialize};
use std::fmt;
use ulid::Ulid;

macro_rules! define_id {
    ($name:ident, $prefix:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(Ulid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            #[must_use]
            pub fn as_ulid(&self) -> Ulid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}-{}", $prefix, self.0)
            }
        }
    };
}

define_id!(SpecId, "spec");
define_id!(TaskId, "task");
define_id!(SubtaskId, "sub");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let a = SpecId::new();
        let b = SpecId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn display_has_prefix() {
        let id = SpecId::new();
        let s = id.to_string();
        assert!(s.starts_with("spec-"));
    }
}
