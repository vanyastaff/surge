//! First-resolution source capture; retained handles authenticate its original names.
use crate::work_items::{Result, WorkItemError};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::fs::File;
use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};
use surge_core::{Graph, work_item::AcceptedFlowContract};
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
fn invalid() -> WorkItemError {
    WorkItemError::Invalid("unsafe or invalid flow source snapshot".into())
}

/// Held first-resolution source. Dropping it releases the authenticated source handles.
pub struct CapturedSource {
    graph: Graph,
    base: PathBuf,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    held: unix::Held,
    #[cfg(windows)]
    held: crate::work_items::start_preparation::secure_lock::windows::Source,
}
impl CapturedSource {
    /// Validated original graph parsed from the retained source handle.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.graph
    }
    /// Canonical source base established from the actual held ancestry.
    #[must_use]
    pub fn base_path(&self) -> &Path {
        &self.base
    }
    /// Revalidate every held source name before committing the first snapshot.
    pub fn verify(&self) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        {
            self.held.verify().map_err(|_| invalid())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Err(invalid())
        }
    }
}
pub(in crate::work_items) fn capture_flow_source(
    project_base: &Path,
    relative_locator: &Path,
) -> Result<CapturedSource> {
    if !project_base.is_absolute()
        || relative_locator.as_os_str().is_empty()
        || relative_locator
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid());
    }
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let (base, mut held) =
            unix::capture(project_base, relative_locator).map_err(|_| invalid())?;
        #[cfg(windows)]
        let (base, mut held) =
            crate::work_items::start_preparation::secure_lock::windows::Source::capture(
                project_base,
                relative_locator,
            )
            .map_err(|_| invalid())?;
        let mut bytes = Vec::new();
        (&mut held.file)
            .take(MAX_SOURCE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() as u64 > MAX_SOURCE_BYTES {
            return Err(invalid());
        }
        held.verify().map_err(|_| invalid())?;
        let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
        let graph: Graph = toml::from_str(text).map_err(|_| invalid())?;
        AcceptedFlowContract::new(Box::new(graph.clone()), String::new()).map_err(|_| invalid())?;
        let captured = CapturedSource { graph, base, held };
        captured.verify()?;
        Ok(captured)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        Err(invalid())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix {
    use super::*;
    use nix::{
        fcntl::{AtFlags, OFlag, openat},
        sys::stat::{Mode, fstatat},
    };
    use std::{
        ffi::{CString, OsString},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
    };
    pub(super) struct Held {
        dirs: Vec<File>,
        names: Vec<OsString>,
        pub(super) file: File,
        source_name: OsString,
        source_identity: (u64, u64, u64, i64, i64, i64, i64),
        base: PathBuf,
        requested_base: PathBuf,
        base_index: usize,
    }
    fn identity(file: &File) -> Result<(u64, u64, u64, i64, i64, i64, i64)> {
        let m = file.metadata()?;
        if !m.is_file() || m.len() > MAX_SOURCE_BYTES {
            return Err(invalid());
        }
        Ok((
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        ))
    }
    fn open(parent: &File, name: &std::ffi::OsStr, directory: bool) -> Result<File> {
        let name = CString::new(name.as_bytes()).map_err(|_| invalid())?;
        let flags = OFlag::O_RDONLY
            | OFlag::O_NOFOLLOW
            | OFlag::O_NONBLOCK
            | OFlag::O_CLOEXEC
            | if directory {
                OFlag::O_DIRECTORY
            } else {
                OFlag::empty()
            };
        let fd = openat(
            Some(parent.as_raw_fd()),
            name.as_c_str(),
            flags,
            Mode::empty(),
        )
        .map_err(|_| invalid())?;
        // SAFETY: successful openat produced a fresh owned descriptor, transferred once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    impl Held {
        pub(super) fn verify(&self) -> Result<()> {
            let original = std::fs::symlink_metadata(&self.requested_base)?;
            let base = self.dirs[self.base_index].metadata()?;
            if !original.is_dir()
                || original.file_type().is_symlink()
                || original.dev() != base.dev()
                || original.ino() != base.ino()
                || std::fs::canonicalize(&self.requested_base)? != self.base
            {
                return Err(invalid());
            }
            for (i, name) in self.names.iter().enumerate() {
                let name = CString::new(name.as_bytes()).map_err(|_| invalid())?;
                let linked = fstatat(
                    Some(self.dirs[i].as_raw_fd()),
                    name.as_c_str(),
                    AtFlags::AT_SYMLINK_NOFOLLOW,
                )
                .map_err(|_| invalid())?;
                let held = self.dirs[i + 1].metadata()?;
                if !held.is_dir()
                    || linked.st_mode & nix::libc::S_IFMT != nix::libc::S_IFDIR
                    || held.dev() != linked.st_dev as u64
                    || held.ino() != linked.st_ino as u64
                {
                    return Err(invalid());
                }
            }
            let name = CString::new(self.source_name.as_bytes()).map_err(|_| invalid())?;
            let parent = self.dirs.last().ok_or_else(invalid)?;
            let linked = fstatat(
                Some(parent.as_raw_fd()),
                name.as_c_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(|_| invalid())?;
            let current = identity(&self.file)?;
            if current != self.source_identity
                || linked.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG
                || linked.st_dev as u64 != current.0
                || linked.st_ino as u64 != current.1
            {
                return Err(invalid());
            }
            Ok(())
        }
    }
    pub(super) fn capture(base: &Path, locator: &Path) -> Result<(PathBuf, Held)> {
        let original = std::fs::symlink_metadata(base)?;
        if !original.is_dir() || original.file_type().is_symlink() {
            return Err(invalid());
        }
        let canonical = std::fs::canonicalize(base)?;
        let mut dirs = vec![File::open("/")?];
        let mut names = Vec::new();
        for part in canonical.components() {
            if let Component::Normal(name) = part {
                let child = open(dirs.last().ok_or_else(invalid)?, name, true)?;
                names.push(name.to_owned());
                dirs.push(child);
            }
        }
        let base_index = dirs.len() - 1;
        let actual = dirs[base_index].metadata()?;
        if actual.dev() != original.dev() || actual.ino() != original.ino() {
            return Err(invalid());
        }
        let source_name = locator.file_name().ok_or_else(invalid)?.to_owned();
        let parent = locator.parent().ok_or_else(invalid)?;
        for part in parent.components() {
            if let Component::Normal(name) = part {
                let child = open(dirs.last().ok_or_else(invalid)?, name, true)?;
                names.push(name.to_owned());
                dirs.push(child);
            }
        }
        let file = open(dirs.last().ok_or_else(invalid)?, &source_name, false)?;
        let source_identity = identity(&file)?;
        let result = Held {
            dirs,
            names,
            file,
            source_name,
            source_identity,
            base: canonical.clone(),
            requested_base: base.into(),
            base_index,
        };
        result.verify()?;
        Ok((canonical, result))
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    const FLOW: &str = include_str!("../../../../../examples/flow_terminal_only.toml");
    #[test]
    fn source_capture_rejects_escape_symlinks_fifo_and_oversize_without_following() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("flow.toml"), FLOW).unwrap();
        assert!(capture_flow_source(root.path(), Path::new("../flow.toml")).is_err());
        symlink("flow.toml", root.path().join("alias.toml")).unwrap();
        assert!(capture_flow_source(root.path(), Path::new("alias.toml")).is_err());
        symlink(root.path(), root.path().join("alias-dir")).unwrap();
        assert!(capture_flow_source(root.path(), Path::new("alias-dir/flow.toml")).is_err());
        let fifo = root.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert!(capture_flow_source(root.path(), Path::new("fifo")).is_err());
        let huge = std::fs::File::create(root.path().join("huge")).unwrap();
        huge.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        assert!(capture_flow_source(root.path(), Path::new("huge")).is_err());
    }
    #[test]
    fn captured_graph_is_original_and_replacement_invalidates_held_name() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("flow.toml"), FLOW).unwrap();
        let captured = capture_flow_source(root.path(), Path::new("child/flow.toml")).unwrap();
        assert_eq!(captured.graph().metadata.name, "flow_terminal_only");
        std::fs::rename(&child, root.path().join("original")).unwrap();
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("flow.toml"), "malformed replacement").unwrap();
        assert!(captured.verify().is_err());
        assert_eq!(captured.graph().metadata.name, "flow_terminal_only");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    const FLOW: &str = include_str!("../../../../../examples/flow_terminal_only.toml");
    #[test]
    fn portable_capture_reads_one_component_and_returns_held_canonical_project_base() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("flow.toml"), FLOW).unwrap();
        let captured = capture_flow_source(home.path(), Path::new("flow.toml")).unwrap();
        assert_eq!(
            captured.base_path(),
            std::fs::canonicalize(home.path()).unwrap()
        );
        assert_eq!(captured.graph().metadata.name, "flow_terminal_only");
        captured.verify().unwrap();
        assert!(std::fs::write(home.path().join("flow.toml"), "replacement").is_err());
        assert!(
            std::fs::rename(
                home.path().join("flow.toml"),
                home.path().join("replacement")
            )
            .is_err()
        );
        assert!(std::fs::rename(home.path(), home.path().with_extension("replacement")).is_err());
        captured.verify().unwrap();
    }
    #[test]
    fn portable_capture_rejects_escape_stream_directory_and_oversize() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("flow.toml"), FLOW).unwrap();
        for locator in [
            "../flow.toml",
            "C:flow.toml",
            r"C:\flow.toml",
            "flow.toml:stream",
            "",
        ] {
            assert!(capture_flow_source(home.path(), Path::new(locator)).is_err());
        }
        std::fs::create_dir(home.path().join("directory")).unwrap();
        assert!(capture_flow_source(home.path(), Path::new("directory")).is_err());
        let huge = std::fs::File::create(home.path().join("huge.toml")).unwrap();
        huge.set_len(MAX_SOURCE_BYTES + 1).unwrap();
        drop(huge);
        assert!(capture_flow_source(home.path(), Path::new("huge.toml")).is_err());
    }
    #[test]
    fn portable_source_capture_rejects_final_and_ancestor_symlinks_without_external_read() {
        let home = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("flow.toml"), FLOW).unwrap();
        let alias = home.path().join("alias");
        match std::os::windows::fs::symlink_dir(external.path(), &alias) {
            Ok(()) => (),
            Err(error) if matches!(error.raw_os_error(), Some(5 | 1314)) => {
                eprintln!(
                    "SKIP source symlink oracle: Windows developer-mode/privilege unavailable: {error}"
                );
                return;
            },
            Err(error) => panic!("unexpected source symlink setup failure: {error}"),
        }
        assert!(capture_flow_source(home.path(), Path::new("alias/flow.toml")).is_err());
        assert!(capture_flow_source(&alias, Path::new("flow.toml")).is_err());
        std::os::windows::fs::symlink_file(
            external.path().join("flow.toml"),
            home.path().join("flow.toml"),
        )
        .unwrap();
        assert!(capture_flow_source(home.path(), Path::new("flow.toml")).is_err());
        assert_eq!(
            std::fs::read_to_string(external.path().join("flow.toml")).unwrap(),
            FLOW
        );
    }
    #[test]
    fn portable_source_allows_real_hardlinks_and_reads_unicode_nested_names() {
        let home = tempfile::tempdir().unwrap();
        let child = home.path().join("資料");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join("flow.toml"), FLOW).unwrap();
        std::fs::hard_link(child.join("flow.toml"), home.path().join("original-link")).unwrap();
        let captured = capture_flow_source(home.path(), Path::new("資料/flow.toml")).unwrap();
        assert_eq!(captured.graph().metadata.name, "flow_terminal_only");
        captured.verify().unwrap();
    }
}
