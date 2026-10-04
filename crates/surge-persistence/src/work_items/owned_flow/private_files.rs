//! Descriptor-owned, immutable private objects. No path-based sensitive I/O.
use crate::work_items::{Result, WorkItemError};
use std::path::Path;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod platform {
    use super::*;
    use crate::work_items::start_preparation::secure_lock::validate_private_acl;
    use nix::{
        fcntl::{AtFlags, OFlag, openat},
        sys::stat::{Mode, fstatat, mkdirat},
        unistd::{Uid, UnlinkatFlags, linkat, unlinkat},
    };
    use std::{
        ffi::{CString, OsString},
        fs::File,
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::{Component, PathBuf},
    };
    const LIMIT: u64 = 8 * 1024 * 1024;
    fn unsafe_location() -> WorkItemError {
        WorkItemError::PrivateInputsUnsafe
    }
    fn component(name: &str) -> Result<CString> {
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            return Err(unsafe_location());
        }
        CString::new(name).map_err(|_| unsafe_location())
    }
    fn open(parent: &File, name: &std::ffi::CStr, flags: OFlag, mode: Mode) -> nix::Result<File> {
        let fd = openat(
            Some(parent.as_raw_fd()),
            name,
            flags | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
            mode,
        )?;
        // SAFETY: openat returns one fresh owned descriptor on success, transferred once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    fn directory(file: &File, private: bool) -> Result<()> {
        let m = file.metadata().map_err(|_| unsafe_location())?;
        let uid = Uid::effective().as_raw();
        let sticky_root = m.uid() == 0 && m.mode() & 0o1000 != 0;
        validate_private_acl(file)?;
        if !m.is_dir()
            || (m.uid() != 0 && m.uid() != uid)
            || (private && (m.uid() != uid || m.mode() & 0o7777 != 0o700))
            || (!private && m.mode() & 0o022 != 0 && !sticky_root)
        {
            return Err(unsafe_location());
        }
        Ok(())
    }
    fn file_identity(file: &File) -> Result<(u64, u64, u64)> {
        let m = file.metadata().map_err(|_| unsafe_location())?;
        validate_private_acl(file)?;
        if !m.is_file()
            || m.uid() != Uid::effective().as_raw()
            || m.mode() & 0o7777 != 0o600
            || m.nlink() != 1
            || m.len() > LIMIT
        {
            return Err(unsafe_location());
        }
        Ok((m.dev(), m.ino(), m.len()))
    }
    pub(super) struct Namespace {
        dirs: Vec<File>,
        names: Vec<OsString>,
        home: PathBuf,
        canonical: PathBuf,
        private_start: usize,
    }
    impl Namespace {
        pub(super) fn open(home: &Path, create: bool) -> Result<Self> {
            let original = std::fs::symlink_metadata(home).map_err(|_| unsafe_location())?;
            if original.file_type().is_symlink() || !original.is_dir() {
                return Err(unsafe_location());
            }
            let canonical = std::fs::canonicalize(home).map_err(|_| unsafe_location())?;
            let mut dirs = vec![File::open("/").map_err(|_| unsafe_location())?];
            let mut names = Vec::new();
            for part in canonical.components() {
                if let Component::Normal(name) = part {
                    let c = CString::new(name.as_bytes()).map_err(|_| unsafe_location())?;
                    let parent = dirs.last().ok_or_else(unsafe_location)?;
                    let child = open(
                        parent,
                        &c,
                        OFlag::O_RDONLY | OFlag::O_DIRECTORY,
                        Mode::empty(),
                    )
                    .map_err(|_| unsafe_location())?;
                    directory(&child, false)?;
                    names.push(name.to_owned());
                    dirs.push(child);
                }
            }
            let actual = dirs.last().ok_or_else(unsafe_location)?.metadata()?;
            if (actual.dev(), actual.ino()) != (original.dev(), original.ino()) {
                return Err(unsafe_location());
            }
            let private_start = dirs.len();
            for name in ["work-items", "private-flow-inputs"] {
                let c = component(name)?;
                let parent = dirs.last().ok_or_else(unsafe_location)?;
                if create {
                    match mkdirat(Some(parent.as_raw_fd()), c.as_c_str(), Mode::S_IRWXU) {
                        Ok(()) => parent.sync_all().map_err(|_| unsafe_location())?,
                        Err(nix::errno::Errno::EEXIST) => (),
                        Err(_) => return Err(unsafe_location()),
                    }
                }
                let child = open(
                    parent,
                    &c,
                    OFlag::O_RDONLY | OFlag::O_DIRECTORY,
                    Mode::empty(),
                )
                .map_err(|_| WorkItemError::PrivateInputsRecoveryRequired)?;
                directory(&child, true)?;
                names.push(name.into());
                dirs.push(child);
            }
            let namespace = Self {
                dirs,
                names,
                home: home.into(),
                canonical,
                private_start,
            };
            namespace.verify()?;
            Ok(namespace)
        }
        fn parent(&self) -> Result<&File> {
            self.dirs.last().ok_or_else(unsafe_location)
        }
        pub(super) fn verify(&self) -> Result<()> {
            let original = std::fs::symlink_metadata(&self.home).map_err(|_| unsafe_location())?;
            if original.file_type().is_symlink()
                || std::fs::canonicalize(&self.home).map_err(|_| unsafe_location())?
                    != self.canonical
            {
                return Err(unsafe_location());
            }
            for (i, name) in self.names.iter().enumerate() {
                let c = CString::new(name.as_bytes()).map_err(|_| unsafe_location())?;
                let linked = fstatat(
                    Some(self.dirs[i].as_raw_fd()),
                    c.as_c_str(),
                    AtFlags::AT_SYMLINK_NOFOLLOW,
                )
                .map_err(|_| unsafe_location())?;
                let held = self.dirs[i + 1].metadata()?;
                directory(&self.dirs[i + 1], i + 1 >= self.private_start)?;
                if linked.st_mode & nix::libc::S_IFMT != nix::libc::S_IFDIR
                    || (held.dev(), held.ino()) != (linked.st_dev as u64, linked.st_ino as u64)
                {
                    return Err(unsafe_location());
                }
            }
            Ok(())
        }
        pub(super) fn read(&self, name: &str) -> Result<Vec<u8>> {
            self.verify()?;
            let c = component(name)?;
            let mut file = open(self.parent()?, &c, OFlag::O_RDONLY, Mode::empty())
                .map_err(|_| WorkItemError::PrivateInputsRecoveryRequired)?;
            let identity = file_identity(&file)?;
            let mut bytes = Vec::new();
            (&mut file)
                .take(LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| WorkItemError::PrivateInputsCorrupt)?;
            let linked = fstatat(
                Some(self.parent()?.as_raw_fd()),
                c.as_c_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(|_| unsafe_location())?;
            if file_identity(&file)? != identity
                || (linked.st_dev as u64, linked.st_ino as u64) != (identity.0, identity.1)
                || bytes.len() as u64 != identity.2
            {
                return Err(WorkItemError::PrivateInputsCorrupt);
            }
            self.verify()?;
            // Every successful reader owns the publication durability barrier, even
            // when another process linked the name before its own directory fsync.
            self.parent()?.sync_all().map_err(|_| unsafe_location())?;
            Ok(bytes)
        }
        pub(super) fn publish(&self, name: &str, bytes: &[u8]) -> Result<bool> {
            if bytes.len() as u64 > LIMIT {
                return Err(WorkItemError::PrivateInputsCorrupt);
            }
            self.verify()?;
            let final_name = component(name)?;
            let temporary_name = component(&format!(
                ".pending-{}",
                hex::encode(rand::random::<[u8; 32]>())
            ))?;
            let parent = self.parent()?;
            let mut file = open(
                parent,
                &temporary_name,
                OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL,
                Mode::S_IRUSR | Mode::S_IWUSR,
            )
            .map_err(|_| unsafe_location())?;
            let owned = file_identity(&file)?;
            file.write_all(bytes).map_err(|_| unsafe_location())?;
            file.sync_all().map_err(|_| unsafe_location())?;
            let published = match linkat(
                Some(parent.as_raw_fd()),
                temporary_name.as_c_str(),
                Some(parent.as_raw_fd()),
                final_name.as_c_str(),
                AtFlags::empty(),
            ) {
                Ok(()) => true,
                Err(nix::errno::Errno::EEXIST) => false,
                Err(_) => return Err(unsafe_location()),
            };
            let named = fstatat(
                Some(parent.as_raw_fd()),
                temporary_name.as_c_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(|_| unsafe_location())?;
            if (named.st_dev as u64, named.st_ino as u64) != (owned.0, owned.1) {
                return Err(unsafe_location());
            }
            unlinkat(
                Some(parent.as_raw_fd()),
                temporary_name.as_c_str(),
                UnlinkatFlags::NoRemoveDir,
            )
            .map_err(|_| unsafe_location())?;
            self.verify()?;
            parent.sync_all().map_err(|_| unsafe_location())?;
            let stored = self.read(name)?;
            if published && stored != bytes {
                return Err(WorkItemError::PrivateInputsCorrupt);
            }
            Ok(published)
        }
        pub(super) fn is_empty(&self) -> Result<bool> {
            self.verify()?;
            let duplicate =
                nix::unistd::dup(self.parent()?.as_raw_fd()).map_err(|_| unsafe_location())?;
            let mut directory = nix::dir::Dir::from_fd(duplicate).map_err(|_| unsafe_location())?;
            let mut empty = true;
            for entry in directory.iter() {
                let entry = entry.map_err(|_| unsafe_location())?;
                let name = entry.file_name().to_bytes();
                if name != b"." && name != b".." {
                    empty = false;
                }
            }
            self.verify()?;
            Ok(empty)
        }
    }
}

pub(super) struct PrivateFiles {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    inner: platform::Namespace,
}
impl PrivateFiles {
    pub(super) fn open(home: &Path, create: bool) -> Result<Self> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            Ok(Self {
                inner: platform::Namespace::open(home, create)?,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (home, create);
            Err(WorkItemError::PrivateInputsUnsupported)
        }
    }
    pub(super) fn read(&self, name: &str) -> Result<Vec<u8>> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            self.inner.read(name)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = name;
            Err(WorkItemError::PrivateInputsUnsupported)
        }
    }
    pub(super) fn publish(&self, name: &str, bytes: &[u8]) -> Result<bool> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            self.inner.publish(name, bytes)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (name, bytes);
            Err(WorkItemError::PrivateInputsUnsupported)
        }
    }
    pub(super) fn is_empty(&self) -> Result<bool> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            self.inner.is_empty()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(WorkItemError::PrivateInputsUnsupported)
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    #[test]
    fn repeated_empty_checks_and_immutable_publication_use_real_fixed_names() {
        let home = tempfile::tempdir().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        assert!(files.is_empty().unwrap());
        assert!(files.publish("fixed-object", b"first").unwrap());
        for _ in 0..5 {
            assert!(!files.is_empty().unwrap());
        }
        assert!(!files.publish("fixed-object", b"replacement").unwrap());
        assert_eq!(files.read("fixed-object").unwrap(), b"first");
        let object = home
            .path()
            .join("work-items/private-flow-inputs/fixed-object");
        assert_eq!(std::fs::read(&object).unwrap(), b"first");
        assert_eq!(
            std::fs::metadata(&object).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(object.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
    }
    #[test]
    fn private_namespace_and_objects_reject_symlinks_hardlinks_and_replacement() {
        let home = tempfile::tempdir().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        files.publish("object", b"original").unwrap();
        let parent = home.path().join("work-items/private-flow-inputs");
        std::fs::hard_link(parent.join("object"), home.path().join("extra-link")).unwrap();
        assert!(files.read("object").is_err());
        std::fs::remove_file(home.path().join("extra-link")).unwrap();
        std::fs::rename(&parent, home.path().join("held-original")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(files.publish("new", b"must not write").is_err());
        assert!(!parent.join("new").exists());
        drop(files);
        symlink(
            home.path().join("held-original/object"),
            parent.join("symlink"),
        )
        .unwrap();
        let replaced = PrivateFiles::open(home.path(), false).unwrap();
        assert!(replaced.read("symlink").is_err());
        assert_eq!(
            std::fs::read(home.path().join("held-original/object")).unwrap(),
            b"original"
        );
    }
}
