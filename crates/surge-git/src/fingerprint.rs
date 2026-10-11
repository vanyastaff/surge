//! Read-only Git tree identity, including dirty and non-ignored untracked files.
//! Object hashes are calculated in memory: no objects, refs or index are written.

use crate::GitError;
use git2::{ObjectType, Oid, Repository};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFingerprint {
    pub repository: PathBuf,
    pub worktree: PathBuf,
    pub tree: String,
}

pub fn observe(worktree: &Path) -> Result<Option<WorkspaceFingerprint>, GitError> {
    let repo = match Repository::open(worktree) {
        Ok(repo) => repo,
        Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let Some(root) = repo.workdir() else {
        return Err(git2::Error::from_str("bare workspace cannot be observed").into());
    };
    let root = root.canonicalize()?;
    if root != worktree.canonicalize()? {
        return Ok(None);
    }
    let index = repo.index()?;
    if index.has_conflicts() {
        return Err(git2::Error::from_str("workspace index has unresolved conflicts").into());
    }
    let mut tracked = BTreeSet::new();
    for entry in index.iter() {
        if entry.mode == 0o160_000 {
            return Err(git2::Error::from_str("submodules cannot be observed").into());
        }
        tracked.insert(PathBuf::from(
            String::from_utf8(entry.path)
                .map_err(|_| git2::Error::from_str("non-UTF8 tracked path"))?,
        ));
    }
    if let Ok(head) = repo.head() {
        collect_tracked(&repo, &head.peel_to_tree()?, Path::new(""), &mut tracked)?;
    }
    let tree = tree_hash(&repo, &root, Path::new(""), &tracked)?;
    Ok(Some(WorkspaceFingerprint {
        repository: repo.commondir().canonicalize()?,
        worktree: root,
        tree: tree.to_string(),
    }))
}

fn collect_tracked(
    repo: &Repository,
    tree: &git2::Tree<'_>,
    prefix: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), GitError> {
    for entry in tree.iter() {
        let name = entry.name()?;
        let path = prefix.join(name);
        match entry.kind() {
            Some(ObjectType::Tree) => {
                collect_tracked(repo, &entry.to_object(repo)?.peel_to_tree()?, &path, paths)?
            },
            Some(ObjectType::Commit) => {
                return Err(git2::Error::from_str("submodules cannot be observed").into());
            },
            _ => {
                paths.insert(path);
            },
        }
    }
    Ok(())
}

fn tree_hash(
    repo: &Repository,
    root: &Path,
    relative: &Path,
    tracked: &BTreeSet<PathBuf>,
) -> Result<Oid, GitError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| git2::Error::from_str("non-UTF8 workspace path"))?;
        if name == ".git" {
            continue;
        }
        let path = relative.join(&name);
        let tracked_path =
            tracked.contains(&path) || tracked.iter().any(|candidate| candidate.starts_with(&path));
        if !tracked_path && repo.status_should_ignore(&path)? {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())?;
        let (mode, oid, sort_name) = if metadata.is_dir() {
            if entry.path().join(".git").exists() {
                return Err(git2::Error::from_str("nested repositories cannot be observed").into());
            }
            let oid = tree_hash(repo, root, &path, tracked)?;
            // Git does not store empty directories.
            if oid == Oid::hash_object(ObjectType::Tree, &[])? {
                continue;
            }
            ("40000", oid, format!("{name}/"))
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            let target = target
                .to_str()
                .ok_or_else(|| git2::Error::from_str("non-UTF8 symlink target"))?;
            (
                "120000",
                Oid::hash_object(ObjectType::Blob, target.as_bytes())?,
                name.clone(),
            )
        } else if metadata.is_file() {
            (
                file_mode(&metadata),
                Oid::hash_object(ObjectType::Blob, &std::fs::read(entry.path())?)?,
                name.clone(),
            )
        } else {
            return Err(git2::Error::from_str("unsupported workspace file type").into());
        };
        entries.push((sort_name, name, mode, oid));
    }
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    let mut bytes = Vec::new();
    for (_, name, mode, oid) in entries {
        bytes.extend_from_slice(mode.as_bytes());
        bytes.push(b' ');
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(oid.as_bytes());
    }
    Ok(Oid::hash_object(ObjectType::Tree, &bytes)?)
}

#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> &'static str {
    use std::os::unix::fs::PermissionsExt;
    if metadata.permissions().mode() & 0o111 == 0 {
        "100644"
    } else {
        "100755"
    }
}
#[cfg(not(unix))]
fn file_mode(_: &std::fs::Metadata) -> &'static str {
    "100644"
}

#[cfg(test)]
mod tests {
    use super::*;
    fn files(path: &Path) -> BTreeSet<PathBuf> {
        let mut out = BTreeSet::new();
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(files(&path));
            } else {
                out.insert(path);
            }
        }
        out
    }
    #[test]
    fn git_oracle_and_dirty_tree_without_any_repository_writes() {
        let (_dir, root) = crate::test_helpers::init_test_repo();
        let repo = Repository::open(&root).unwrap();
        let head = repo.head().unwrap().target().unwrap();
        let expected = repo.find_commit(head).unwrap().tree_id().to_string();
        let index = std::fs::read(repo.path().join("index")).unwrap();
        let object_files = files(&repo.commondir().join("objects"));
        let refs = files(&repo.commondir().join("refs"));
        assert_eq!(observe(&root).unwrap().unwrap().tree, expected);
        std::fs::write(root.join("README.md"), "changed without a commit").unwrap();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/new.rs"), "untracked code").unwrap();
        assert_ne!(observe(&root).unwrap().unwrap().tree, expected);
        assert_eq!(repo.head().unwrap().target(), Some(head));
        assert_eq!(std::fs::read(repo.path().join("index")).unwrap(), index);
        assert_eq!(files(&repo.commondir().join("objects")), object_files);
        assert_eq!(files(&repo.commondir().join("refs")), refs);
    }
}
