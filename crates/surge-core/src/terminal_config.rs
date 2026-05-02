//! Terminal node configuration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TerminalConfig {
    pub kind: TerminalKind,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalKind {
    Success,
    Failure { exit_code: i32 },
    Aborted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_terminal_roundtrip() {
        let t = TerminalConfig {
            kind: TerminalKind::Success,
            message: Some("All done".into()),
        };
        let toml_s = toml::to_string(&t).unwrap();
        let parsed: TerminalConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(t, parsed);
    }

    #[test]
    fn failure_carries_exit_code() {
        let t = TerminalConfig {
            kind: TerminalKind::Failure { exit_code: 42 },
            message: None,
        };
        let toml_s = toml::to_string(&t).unwrap();
        let parsed: TerminalConfig = toml::from_str(&toml_s).unwrap();
        assert_eq!(t, parsed);
    }
}
