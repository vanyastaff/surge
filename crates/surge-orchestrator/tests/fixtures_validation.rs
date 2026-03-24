//! Test that fixture files are valid and loadable.

mod fixtures;

#[test]
fn test_simple_spec_fixture_loads() {
    let spec_file = fixtures::load_simple_spec();
    assert_eq!(spec_file.spec.title, "Simple test feature");
    assert_eq!(spec_file.spec.subtasks.len(), 1);
    assert_eq!(spec_file.spec.subtasks[0].title, "Create test file");
    assert_eq!(spec_file.spec.subtasks[0].acceptance_criteria.len(), 2);
}

#[test]
fn test_dependency_spec_fixture_loads() {
    let spec_file = fixtures::load_dependency_spec();
    assert_eq!(spec_file.spec.title, "Feature with dependencies");
    assert_eq!(spec_file.spec.subtasks.len(), 3);

    // Verify dependency structure
    assert_eq!(spec_file.spec.subtasks[0].depends_on.len(), 0);
    assert_eq!(spec_file.spec.subtasks[1].depends_on.len(), 1);
    assert_eq!(spec_file.spec.subtasks[2].depends_on.len(), 2);
}

#[test]
fn test_fixture_paths_exist() {
    let simple_path = fixtures::fixture_path("simple_spec.toml");
    let dependency_path = fixtures::fixture_path("dependency_spec.toml");

    assert!(simple_path.exists(), "simple_spec.toml should exist");
    assert!(dependency_path.exists(), "dependency_spec.toml should exist");
}
