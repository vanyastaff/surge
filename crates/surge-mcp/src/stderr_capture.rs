//! Descriptor-retained stderr capture; repository paths cannot redirect writes.
use std::{fs::File, io, path::Path};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

pub(super) async fn publish(file: &mut tokio::fs::File, content: &str) -> io::Result<()> {
    file.rewind().await?;
    file.write_all(content.as_bytes()).await?;
    file.flush().await?;
    file.set_len(content.len() as u64).await
}

#[cfg(unix)]
pub(super) fn open(path: &Path) -> io::Result<File> {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::{Mode, mkdirat},
        unistd::Uid,
    };
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, PermissionsExt},
    };
    fn rejected() -> io::Error {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe MCP stderr capture location",
        )
    }
    fn child(parent: &File, name: &std::ffi::OsStr, directory: bool) -> io::Result<File> {
        let flags = OFlag::O_NOFOLLOW
            | OFlag::O_CLOEXEC
            | OFlag::O_NONBLOCK
            | if directory {
                OFlag::O_RDONLY | OFlag::O_DIRECTORY
            } else {
                OFlag::O_WRONLY | OFlag::O_CREAT
            };
        let fd = openat(
            Some(parent.as_raw_fd()),
            Path::new(name),
            flags,
            Mode::from_bits_truncate(0o600),
        )?;
        // SAFETY: openat returned one fresh descriptor; ownership transfers exactly once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    let parent = path.parent().ok_or_else(rejected)?;
    // Resolve only the trusted worktree/temp base, never repository-controlled capture components.
    let base = parent
        .parent()
        .and_then(Path::parent)
        .ok_or_else(rejected)?;
    let canonical = std::fs::canonicalize(base)?;
    let mut dir = File::open("/")?;
    for component in canonical.components() {
        if let std::path::Component::Normal(name) = component {
            dir = child(&dir, name, true)?;
        }
    }
    let uid = Uid::effective().as_raw();
    for (name, private) in [
        (
            parent
                .parent()
                .and_then(Path::file_name)
                .ok_or_else(rejected)?,
            false,
        ),
        (parent.file_name().ok_or_else(rejected)?, true),
    ] {
        match mkdirat(
            Some(dir.as_raw_fd()),
            Path::new(name),
            Mode::from_bits_truncate(0o700),
        ) {
            Ok(()) | Err(nix::errno::Errno::EEXIST) => {},
            Err(error) => return Err(error.into()),
        }
        dir = child(&dir, name, true)?;
        let metadata = dir.metadata()?;
        if metadata.uid() != uid
            || metadata.mode() & 0o022 != 0
            || (private && metadata.mode() & 0o777 != 0o700)
        {
            return Err(rejected());
        }
    }
    let file = child(&dir, path.file_name().ok_or_else(rejected)?, false)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.uid() != uid || metadata.nlink() != 1 {
        return Err(rejected());
    }
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.set_len(0)?;
    Ok(file)
}

#[cfg(not(unix))]
pub(super) fn open(_path: &Path) -> io::Result<File> {
    // No descriptor-relative directory API is available here yet. Fail closed:
    // child stderr still drains and emits safe tracing categories.
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "secure MCP stderr capture unavailable on this platform",
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn capture_rejects_leaf_and_directory_links_without_touching_victim() {
        let temp = tempfile::tempdir().unwrap();
        let victim = temp.path().join("victim");
        std::fs::write(&victim, b"valuable bytes").unwrap();
        let capture = temp.path().join(".surge/mcp-stderr/server.log");
        std::fs::create_dir_all(capture.parent().unwrap()).unwrap();
        std::fs::set_permissions(
            capture.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        symlink(&victim, &capture).unwrap();
        assert!(open(&capture).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"valuable bytes");
        std::fs::remove_file(&capture).unwrap();
        std::fs::hard_link(&victim, &capture).unwrap();
        assert!(open(&capture).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"valuable bytes");
        std::fs::remove_file(&capture).unwrap();
        std::fs::remove_dir(capture.parent().unwrap()).unwrap();
        symlink(temp.path(), capture.parent().unwrap()).unwrap();
        assert!(open(&capture).is_err());
        assert_eq!(std::fs::read(&victim).unwrap(), b"valuable bytes");
    }
    #[test]
    fn capture_rejects_runtime_ancestor_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), temp.path().join(".surge")).unwrap();
        assert!(open(&temp.path().join(".surge/mcp-stderr/server.log")).is_err());
        assert!(!outside.path().join("mcp-stderr").exists());
    }

    #[test]
    fn capture_creates_owner_only_file_and_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".surge/mcp-stderr/server.log");
        let file = open(&path).unwrap();
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[tokio::test]
    async fn capture_keeps_descriptor_after_path_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".surge/mcp-stderr/server.log");
        let mut file = tokio::fs::File::from_std(open(&path).unwrap());
        let retained = path.with_extension("old");
        std::fs::rename(&path, &retained).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        publish(&mut file, "safe record").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert_eq!(std::fs::read(&retained).unwrap(), b"safe record");
    }
}
