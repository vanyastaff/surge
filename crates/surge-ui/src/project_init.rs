//! Create a new project in an explicitly selected empty directory.

use std::io::Write as _;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use surge_core::SurgeConfig;

/// Initialize only an empty, standalone directory. Runtime configuration stays
/// local and is never included in the initial commit.
pub fn initialize(path: &Path, config: &SurgeConfig) -> Result<()> {
    config.validate()?;
    let metadata = std::fs::symlink_metadata(path).context("Inspect project directory")?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("Choose an empty directory, not a file or symbolic link");
    }
    if std::fs::read_dir(path)?.next().transpose()?.is_some() {
        bail!("This directory is not empty. Use Open project for existing work");
    }
    if git2::Repository::discover(path).is_ok() {
        bail!("Choose a directory outside an existing Git repository");
    }
    let serialized = toml::to_string_pretty(config).context("Prepare project configuration")?;
    let mut options = git2::RepositoryInitOptions::new();
    options.initial_head("main").no_reinit(true);
    let repo = git2::Repository::init_opts(path, &options).context("Initialize Git repository")?;
    write_new(path, "surge.toml", serialized.as_bytes())?;
    write_new(path, ".gitignore", b"/surge.toml\n/.surge/\n/.worktrees/\n")?;
    write_new(
        path,
        "README.md",
        b"# New application\n\nCreated with Surge.\n",
    )?;
    let mut index = repo.index()?;
    index.add_path(Path::new(".gitignore"))?;
    index.add_path(Path::new("README.md"))?;
    index.write()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let signature = git2::Signature::now("Surge", "surge@localhost")?;
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        "Initialize application",
        &tree,
        &[],
    )?;
    Ok(())
}

fn write_new(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options
        .open(root.join(name))
        .with_context(|| format!("Create {name}"))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::initialize;
    use surge_core::SurgeConfig;

    #[test]
    fn new_project_has_a_base_commit_and_untracked_local_config() {
        let directory = tempfile::tempdir().unwrap();
        initialize(directory.path(), &SurgeConfig::default()).unwrap();
        let repo = git2::Repository::open(directory.path()).unwrap();
        let head = repo.head().unwrap();
        assert_eq!(head.shorthand().unwrap(), "main");
        let tree = head.peel_to_commit().unwrap().tree().unwrap();
        assert_eq!(tree.len(), 2);
        assert!(tree.get_name("README.md").is_some());
        assert!(tree.get_name(".gitignore").is_some());
        assert!(tree.get_name("surge.toml").is_none());
        assert!(repo.is_path_ignored("surge.toml").unwrap());
        SurgeConfig::load(&directory.path().join("surge.toml")).unwrap();
    }

    #[test]
    fn existing_files_are_never_modified() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("keep.txt"), "existing work").unwrap();
        assert!(initialize(directory.path(), &SurgeConfig::default()).is_err());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("keep.txt")).unwrap(),
            "existing work"
        );
        assert!(!directory.path().join(".git").exists());
    }

    #[test]
    fn nested_repository_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        git2::Repository::init(directory.path()).unwrap();
        let child = directory.path().join("child");
        std::fs::create_dir(&child).unwrap();
        assert!(initialize(&child, &SurgeConfig::default()).is_err());
        assert!(std::fs::read_dir(child).unwrap().next().is_none());
    }
}
