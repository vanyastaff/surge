//! Test fixtures for surge-orchestrator E2E tests.

pub mod mock_bridge;

use std::path::PathBuf;
use surge_spec::SpecFile;

/// Get the path to the fixtures directory.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Load the simple spec fixture.
pub fn load_simple_spec() -> SpecFile {
    let path = fixtures_dir().join("simple_spec.toml");
    SpecFile::load(&path).expect("Failed to load simple_spec.toml")
}

/// Load the dependency spec fixture.
pub fn load_dependency_spec() -> SpecFile {
    let path = fixtures_dir().join("dependency_spec.toml");
    SpecFile::load(&path).expect("Failed to load dependency_spec.toml")
}

/// Get path to a fixture file by name.
pub fn fixture_path(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_spec_loads() {
        let spec_file = load_simple_spec();
        assert_eq!(spec_file.spec.title, "Simple test feature");
        assert_eq!(spec_file.spec.subtasks.len(), 1);
        assert_eq!(spec_file.spec.subtasks[0].title, "Create test file");
        assert_eq!(spec_file.spec.subtasks[0].acceptance_criteria.len(), 2);
    }

    #[test]
    fn test_dependency_spec_loads() {
        let spec_file = load_dependency_spec();
        assert_eq!(spec_file.spec.title, "Feature with dependencies");
        assert_eq!(spec_file.spec.subtasks.len(), 3);

        // First subtask has no dependencies
        assert_eq!(spec_file.spec.subtasks[0].depends_on.len(), 0);

        // Second subtask depends on first
        assert_eq!(spec_file.spec.subtasks[1].depends_on.len(), 1);

        // Third subtask depends on first two
        assert_eq!(spec_file.spec.subtasks[2].depends_on.len(), 2);
    }

    #[test]
    fn test_fixture_path() {
        let path = fixture_path("simple_spec.toml");
        assert!(path.ends_with("fixtures/simple_spec.toml"));
    }

    #[test]
    fn test_fixtures_dir_exists() {
        let dir = fixtures_dir();
        assert!(dir.exists(), "Fixtures directory should exist");
        assert!(dir.is_dir(), "Fixtures path should be a directory");
    }
}
