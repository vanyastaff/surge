//! Surgical edits to a project's `surge.toml`.
//!
//! Settings changes one key at a time, so saving must too: only the keys
//! the person touched are written, everything else in the file — comments,
//! ordering, keys edited on disk since the screen opened — stays exactly as
//! it is. The edited document is parsed and validated as a whole
//! [`SurgeConfig`] before anything is written, and a file that does not
//! parse is never overwritten: the error is returned for the screen to show.

use std::collections::BTreeMap;
use std::path::Path;

use surge_core::SurgeConfig;
use toml_edit::{DocumentMut, Item, Table, Value};

/// A new value for one key.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// Remove the key (fall back to the built-in default).
    Unset,
}

/// Pending edits, keyed by dotted path (`pipeline.max_parallel`).
pub type Changes = BTreeMap<String, Change>;

fn value_of(change: &Change) -> Option<Value> {
    Some(match change {
        Change::Bool(b) => Value::from(*b),
        Change::Int(i) => Value::from(*i),
        Change::Float(f) => Value::from(*f),
        Change::Str(s) => Value::from(s.as_str()),
        Change::Unset => return None,
    })
}

/// Apply `changes` to TOML `source`, returning the new text and the
/// validated config it describes.
pub fn apply_to_str(source: &str, changes: &Changes) -> Result<(String, SurgeConfig), String> {
    let mut doc: DocumentMut = source.parse().map_err(|e| {
        format!("surge.toml has a syntax error, so nothing was saved — fix it first ({e})")
    })?;
    for (path, change) in changes {
        let parts: Vec<&str> = path.split('.').collect();
        let Some((key, tables)) = parts.split_last() else {
            continue;
        };
        let mut table: &mut Table = doc.as_table_mut();
        for name in tables {
            let entry = table
                .entry(name)
                .or_insert_with(|| Item::Table(Table::new()));
            table = entry
                .as_table_mut()
                .ok_or_else(|| format!("surge.toml: `{name}` is not a table, cannot set {path}"))?;
        }
        match value_of(change) {
            Some(value) => {
                // Keep the key's own decoration (trailing comment) when present.
                match table.get_mut(key).and_then(Item::as_value_mut) {
                    Some(existing) => {
                        let decor = existing.decor().clone();
                        *existing = value;
                        *existing.decor_mut() = decor;
                    },
                    None => {
                        table.insert(key, Item::Value(value));
                    },
                }
            },
            None => {
                table.remove(key);
            },
        }
    }
    let text = doc.to_string();
    let config: SurgeConfig = toml::from_str(&text)
        .map_err(|e| format!("These settings would make surge.toml invalid: {e}"))?;
    config.validate().map_err(|e| e.to_string())?;
    Ok((text, config))
}

/// Apply `changes` to the `surge.toml` at `path` (created if missing) and
/// write it atomically.
pub fn apply(path: &Path, changes: &Changes) -> Result<SurgeConfig, String> {
    let source = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };
    let (text, config) = apply_to_str(&source, changes)?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::{Change, Changes, apply, apply_to_str};

    const SOURCE: &str = r#"# my project
schema_version = 1
default_agent = "claude-acp"

[pipeline]
# keep this low on a laptop
max_parallel = 2 # cores are scarce
max_qa_iterations = 10
"#;

    fn changes(items: &[(&str, Change)]) -> Changes {
        items
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn only_touched_keys_change_and_comments_survive() {
        let (text, config) = apply_to_str(
            SOURCE,
            &changes(&[("pipeline.max_parallel", Change::Int(4))]),
        )
        .unwrap();
        assert!(text.contains("# my project"));
        assert!(text.contains("# keep this low on a laptop"));
        assert!(text.contains("max_parallel = 4 # cores are scarce"));
        assert!(text.contains("max_qa_iterations = 10"));
        assert_eq!(config.pipeline.max_parallel, 4);
    }

    #[test]
    fn missing_tables_are_created_and_unset_removes() {
        let (text, config) = apply_to_str(
            SOURCE,
            &changes(&[
                ("analytics.budget_usd", Change::Float(25.0)),
                ("pipeline.max_qa_iterations", Change::Unset),
            ]),
        )
        .unwrap();
        assert!(text.contains("[analytics]"));
        assert_eq!(config.analytics.budget_usd, Some(25.0));
        assert!(!text.contains("max_qa_iterations"));
    }

    #[test]
    fn a_broken_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("surge.toml");
        std::fs::write(&path, "[pipeline\nmax_parallel = 2").unwrap();
        let err = apply(
            &path,
            &changes(&[("pipeline.max_parallel", Change::Int(3))]),
        )
        .unwrap_err();
        assert!(err.contains("syntax error"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[pipeline\nmax_parallel = 2"
        );
    }

    #[test]
    fn an_invalid_result_is_rejected_before_writing() {
        let err = apply_to_str(
            SOURCE,
            &changes(&[("pipeline.max_qa_iterations", Change::Int(0))]),
        )
        .unwrap_err();
        assert!(err.contains("max_qa_iterations"), "{err}");
    }
}
