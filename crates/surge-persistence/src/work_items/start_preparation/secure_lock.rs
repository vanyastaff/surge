//! Held-object preparation ownership. It grants no provider or registry authority.
use super::super::{Result, WorkItemError};
use std::path::Path;
use surge_core::{
    RunId,
    id::{WorkItemId, WorkItemOperationId},
};

#[derive(Clone, Copy)]
pub(in crate::work_items) enum PreparationLockKey {
    Task(WorkItemId),
    FlowOperation(WorkItemOperationId),
    FlowLaunch {
        operation: WorkItemOperationId,
        run: RunId,
    },
}
impl PreparationLockKey {
    fn name(self) -> String {
        match self {
            Self::Task(item) => item.to_string(),
            Self::FlowOperation(operation) => format!("flow-operation-v1-{operation}"),
            Self::FlowLaunch { operation, run } => format!("flow-launch-v1-{operation}-{run}"),
        }
    }
}
enum Requirement {
    StableOwnership,
    RestrictedPrivateInputs,
}

pub(in crate::work_items) struct PreparationLock {
    key: Option<PreparationLockKey>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    inner: unix::Held,
    #[cfg(windows)]
    inner: windows::Held,
}
impl PreparationLock {
    pub(in crate::work_items) fn acquire_task(home: &Path, item: WorkItemId) -> Result<Self> {
        Self::acquire_required(
            home,
            PreparationLockKey::Task(item),
            Requirement::RestrictedPrivateInputs,
        )
    }
    pub(in crate::work_items) fn acquire_stable(
        home: &Path,
        key: PreparationLockKey,
    ) -> Result<Self> {
        match key {
            PreparationLockKey::Task(item) => Self::acquire_task(home, item),
            _ => Self::acquire_required(home, key, Requirement::StableOwnership),
        }
    }
    pub(in crate::work_items) fn acquire_private(
        home: &Path,
        key: PreparationLockKey,
    ) -> Result<Self> {
        Self::acquire_required(home, key, Requirement::RestrictedPrivateInputs)
    }
    fn acquire_required(
        home: &Path,
        key: PreparationLockKey,
        requirement: Requirement,
    ) -> Result<Self> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let _ = requirement;
            Ok(Self {
                key: Some(key),
                inner: unix::acquire(home, &key.name())?,
            })
        }
        #[cfg(windows)]
        {
            if matches!(requirement, Requirement::RestrictedPrivateInputs) {
                return Err(WorkItemError::Invalid(
                    "private preparation unsupported on this platform".into(),
                ));
            }
            Ok(Self {
                key: Some(key),
                inner: windows::acquire(home, &key.name())?,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = (home, key, requirement);
            Err(WorkItemError::Invalid(
                "stable preparation unsupported on this platform".into(),
            ))
        }
    }
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    fn acquire(home: &Path, name: &str) -> Result<Self> {
        Ok(Self {
            key: None,
            inner: unix::acquire(home, name)?,
        })
    }
    pub(in crate::work_items) fn require_private(&self) -> Result<()> {
        self.verify()?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(WorkItemError::Invalid(
                "private preparation unsupported on this platform".into(),
            ))
        }
    }
    pub(in crate::work_items) fn identity(&self) -> Result<String> {
        self.verify()?;
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        {
            let chain = self.inner.identity()?;
            match self.key {
                None | Some(PreparationLockKey::Task(_)) => Ok(chain),
                Some(key) => Ok(serde_json::to_string(&(
                    "stable-owned-flow-v1",
                    key.name(),
                    chain,
                ))?),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Err(WorkItemError::Invalid(
                "stable preparation unsupported on this platform".into(),
            ))
        }
    }
    pub(in crate::work_items) fn verify(&self) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        {
            self.inner.verify()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            Err(WorkItemError::Invalid(
                "stable preparation unsupported on this platform".into(),
            ))
        }
    }
}
/// Private owned-flow objects exist only on Linux and macOS
/// (`owned_flow::private_files`), so this check has no other callers.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(in crate::work_items) fn validate_private_acl(file: &std::fs::File) -> Result<()> {
    unix::check_acl(file)
        .map_err(|()| WorkItemError::Invalid("unsafe private preparation ACL".into()))
}
#[cfg(windows)]
pub(in crate::work_items) mod windows;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix {
    use super::*;
    use nix::{
        fcntl::{AtFlags, OFlag, openat},
        sys::stat::{Mode, fstatat, mkdirat},
        unistd::Uid,
    };
    use std::fs::File;
    use std::{
        ffi::{CString, OsString},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::{Component, PathBuf},
    };
    fn unsafe_location() -> WorkItemError {
        WorkItemError::Invalid("unsafe task preparation lock location".into())
    }
    fn open(parent: &File, name: &std::ffi::CStr, flags: OFlag, mode: Mode) -> Result<File> {
        open_descriptor(parent, name, flags, mode).map_err(|_| unsafe_location())
    }
    fn open_descriptor(
        parent: &File,
        name: &std::ffi::CStr,
        flags: OFlag,
        mode: Mode,
    ) -> nix::Result<File> {
        let fd = openat(
            Some(parent.as_raw_fd()),
            name,
            flags | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
            mode,
        )?;
        // SAFETY: successful openat returns a fresh owned descriptor. Exactly one File
        // takes ownership; no raw close or second ownership conversion occurs.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(target_os = "macos")]
    mod mac_acl {
        use std::{ffi::c_void, fs::File, os::fd::AsRawFd};
        // Signatures/constants from the platform SDK sys/acl.h. These are opaque libc objects.
        unsafe extern "C" {
            fn acl_get_fd_np(fd: nix::libc::c_int, kind: nix::libc::c_int) -> *mut c_void;
            fn acl_get_entry(
                acl: *mut c_void,
                id: nix::libc::c_int,
                entry: *mut *mut c_void,
            ) -> nix::libc::c_int;
            fn acl_get_tag_type(entry: *mut c_void, tag: *mut nix::libc::c_int)
            -> nix::libc::c_int;
            fn acl_free(acl: *mut c_void) -> nix::libc::c_int;
        }
        struct Acl(*mut c_void);
        impl Drop for Acl {
            fn drop(&mut self) {
                // SAFETY: this pointer is a nonnull ACL returned by libc, freed exactly once.
                unsafe {
                    acl_free(self.0);
                }
            }
        }
        pub(super) fn rejects_grants(file: &File) -> Result<(), ()> {
            // SAFETY: file owns a live descriptor; ACL_TYPE_EXTENDED=0x100 on macOS.
            let pointer = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
            if pointer.is_null() {
                // Darwin returns ENOENT for an absent extended ACL on a live descriptor.
                return (std::io::Error::last_os_error().raw_os_error() == Some(nix::libc::ENOENT))
                    .then_some(())
                    .ok_or(());
            }
            let acl = Acl(pointer);
            let mut id = 0; // ACL_FIRST_ENTRY
            // The SDK caps extended ACLs at 169 entries; a larger result fails closed.
            for _ in 0..170 {
                let mut entry = std::ptr::null_mut();
                // SAFETY: ACL is live and owned; entry output is valid writable storage.
                let result = unsafe { acl_get_entry(acl.0, id, &raw mut entry) };
                if result != 0 {
                    // Darwin returns EINVAL when no next entry exists, including an empty ACL.
                    return (std::io::Error::last_os_error().raw_os_error()
                        == Some(nix::libc::EINVAL))
                    .then_some(())
                    .ok_or(());
                }
                if entry.is_null() {
                    return Err(());
                }
                let mut tag = 0;
                // SAFETY: entry belongs to live ACL and tag output is valid writable storage.
                if unsafe { acl_get_tag_type(entry, &raw mut tag) } != 0 || tag != 2 {
                    // Only ACL_EXTENDED_DENY entries are safe; grant and unknown tags fail closed.
                    return Err(());
                }
                id = -1; // ACL_NEXT_ENTRY
            }
            Err(())
        }
    }
    pub(super) fn check_acl(file: &File) -> std::result::Result<(), ()> {
        #[cfg(target_os = "macos")]
        {
            mac_acl::rejects_grants(file)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = file;
            Ok(())
        }
    }

    fn directory(file: &File, final_dir: bool) -> Result<()> {
        check_acl(file).map_err(|()| unsafe_location())?;
        let m = file.metadata()?;
        let sticky_root = m.uid() == 0 && m.mode() & 0o1000 != 0;
        if !m.is_dir()
            || (m.uid() != 0 && m.uid() != Uid::effective().as_raw())
            || (m.mode() & 0o022 != 0 && (final_dir || !sticky_root))
            || (final_dir && m.uid() != Uid::effective().as_raw())
        {
            return Err(unsafe_location());
        }
        Ok(())
    }
    pub(super) struct Held {
        dirs: Vec<File>,
        names: Vec<OsString>,
        lock: File,
        name: CString,
        home: PathBuf,
        canonical: PathBuf,
    }
    impl Held {
        pub(super) fn identity(&self) -> Result<String> {
            let chain = self
                .dirs
                .iter()
                .chain(std::iter::once(&self.lock))
                .map(|file| {
                    let metadata = file.metadata()?;
                    Ok((metadata.dev(), metadata.ino()))
                })
                .collect::<std::io::Result<Vec<_>>>()?;
            Ok(serde_json::to_string(&chain)?)
        }
        pub(super) fn verify(&self) -> Result<()> {
            if std::fs::symlink_metadata(&self.home)?
                .file_type()
                .is_symlink()
                || std::fs::canonicalize(&self.home)? != self.canonical
            {
                return Err(unsafe_location());
            }
            for (index, name) in self.names.iter().enumerate() {
                let name = CString::new(name.as_bytes()).map_err(|_| unsafe_location())?;
                let linked = fstatat(
                    Some(self.dirs[index].as_raw_fd()),
                    name.as_c_str(),
                    AtFlags::AT_SYMLINK_NOFOLLOW,
                )
                .map_err(|_| unsafe_location())?;
                let held = self.dirs[index + 1].metadata()?;
                directory(&self.dirs[index + 1], index + 2 == self.dirs.len())?;
                if held.dev() != linked.st_dev as u64 || held.ino() != linked.st_ino as u64 {
                    return Err(unsafe_location());
                }
            }
            let parent = self.dirs.last().ok_or_else(unsafe_location)?;
            let linked = fstatat(
                Some(parent.as_raw_fd()),
                self.name.as_c_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(|_| unsafe_location())?;
            let held = self.lock.metadata()?;
            check_acl(&self.lock).map_err(|()| unsafe_location())?;
            if !held.is_file()
                || held.uid() != Uid::effective().as_raw()
                || held.mode() & 0o7777 != 0o600
                || held.nlink() != 1
                || held.dev() != linked.st_dev as u64
                || held.ino() != linked.st_ino as u64
            {
                return Err(unsafe_location());
            }
            Ok(())
        }
    }
    pub(super) fn acquire(home: &Path, name: &str) -> Result<Held> {
        let original = std::fs::symlink_metadata(home)?;
        if original.file_type().is_symlink() {
            return Err(unsafe_location());
        }
        let canonical = std::fs::canonicalize(home)?;
        let mut dirs = vec![File::open("/")?];
        directory(&dirs[0], false)?;
        let mut names = Vec::new();
        for component in canonical.components() {
            if let Component::Normal(name) = component {
                let c = CString::new(name.as_bytes()).map_err(|_| unsafe_location())?;
                let parent = dirs.last().ok_or_else(unsafe_location)?;
                let child = open(
                    parent,
                    &c,
                    OFlag::O_RDONLY | OFlag::O_DIRECTORY,
                    Mode::empty(),
                )?;
                directory(&child, false)?;
                names.push(name.to_owned());
                dirs.push(child);
            }
        }
        let actual = dirs.last().ok_or_else(unsafe_location)?.metadata()?;
        if actual.dev() != original.dev() || actual.ino() != original.ino() {
            return Err(unsafe_location());
        }
        directory(dirs.last().ok_or_else(unsafe_location)?, true)?;
        for name in ["work-items", "preparation-locks"] {
            let c = CString::new(name).map_err(|_| unsafe_location())?;
            let parent = dirs.last().ok_or_else(unsafe_location)?;
            match mkdirat(Some(parent.as_raw_fd()), c.as_c_str(), Mode::S_IRWXU) {
                Ok(()) | Err(nix::errno::Errno::EEXIST) => (),
                Err(_) => return Err(unsafe_location()),
            }
            let child = open(
                parent,
                &c,
                OFlag::O_RDONLY | OFlag::O_DIRECTORY,
                Mode::empty(),
            )?;
            directory(&child, true)?;
            names.push(OsString::from(name));
            dirs.push(child);
        }
        let name = CString::new(format!("{name}.lock")).map_err(|_| unsafe_location())?;
        let parent = dirs.last().ok_or_else(unsafe_location)?;
        // Exclusive creation avoids Darwin's concurrent O_CREAT ENOENT race.
        // Only EEXIST permits opening the existing name, with the same secure flags.
        let lock = match open_descriptor(
            parent,
            &name,
            OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL,
            Mode::S_IRUSR | Mode::S_IWUSR,
        ) {
            Ok(file) => file,
            Err(nix::errno::Errno::EEXIST) => open(parent, &name, OFlag::O_RDWR, Mode::empty())?,
            Err(_) => return Err(unsafe_location()),
        };
        let result = Held {
            dirs,
            names,
            lock,
            name,
            home: home.into(),
            canonical,
        };
        result.verify()?;
        result.lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => WorkItemError::Busy,
            std::fs::TryLockError::Error(error) => WorkItemError::Io(error),
        })?;
        result.verify()?;
        Ok(result)
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn typed_namespaces_hold_the_original_object_and_reject_replacement_after_death() {
        use surge_core::{
            RunId,
            id::{WorkItemId, WorkItemOperationId},
        };
        let home = tempfile::tempdir().unwrap();
        let operation = WorkItemOperationId::new();
        let key = PreparationLockKey::FlowOperation(operation);
        let guard = PreparationLock::acquire_stable(home.path(), key).unwrap();
        let identity = guard.identity().unwrap();
        assert!(matches!(
            PreparationLock::acquire_stable(home.path(), key),
            Err(WorkItemError::Busy)
        ));
        let launch = PreparationLock::acquire_stable(
            home.path(),
            PreparationLockKey::FlowLaunch {
                operation,
                run: RunId::new(),
            },
        )
        .unwrap();
        assert_ne!(identity, launch.identity().unwrap());
        let task = WorkItemId::new();
        let task_guard = PreparationLock::acquire_task(home.path(), task).unwrap();
        assert!(
            home.path()
                .join(format!("work-items/preparation-locks/{task}.lock"))
                .exists()
        );
        assert!(serde_json::from_str::<Vec<(u64, u64)>>(&task_guard.identity().unwrap()).is_ok());
        drop(guard);
        let resumed = PreparationLock::acquire_stable(home.path(), key).unwrap();
        assert_eq!(identity, resumed.identity().unwrap());
        drop(resumed);
        let parent = home.path().join("work-items/preparation-locks");
        let name = format!("{}.lock", key.name());
        std::fs::rename(parent.join(&name), home.path().join("retired-original")).unwrap();
        let replaced = PreparationLock::acquire_stable(home.path(), key).unwrap();
        assert_ne!(identity, replaced.identity().unwrap());
    }
    #[test]
    fn arc_retains_real_exclusion_until_last_effect_owner_exits() {
        use surge_core::{RunId, id::WorkItemOperationId};
        let home = tempfile::tempdir().unwrap();
        let key = PreparationLockKey::FlowLaunch {
            operation: WorkItemOperationId::new(),
            run: RunId::new(),
        };
        let supervisor =
            std::sync::Arc::new(PreparationLock::acquire_stable(home.path(), key).unwrap());
        let effect = supervisor.clone();
        drop(supervisor);
        assert!(matches!(
            PreparationLock::acquire_stable(home.path(), key),
            Err(WorkItemError::Busy)
        ));
        drop(effect);
        assert!(PreparationLock::acquire_stable(home.path(), key).is_ok());
    }
    #[test]
    fn preparation_lock_rejects_symlink_fifo_links_permissions_and_parent_replacement() {
        let home = tempfile::tempdir().unwrap();
        let lock = PreparationLock::acquire(home.path(), "item").unwrap();
        assert!(matches!(
            PreparationLock::acquire(home.path(), "item"),
            Err(WorkItemError::Busy)
        ));
        let parent = home.path().join("work-items/preparation-locks");
        std::fs::rename(&parent, home.path().join("old-locks")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        assert!(lock.verify().is_err());
        drop(lock);
        let path = parent.join("item.lock");
        let external = home.path().join("external");
        std::fs::write(&external, "untouched").unwrap();
        symlink(&external, &path).unwrap();
        assert!(PreparationLock::acquire(home.path(), "item").is_err());
        assert_eq!(std::fs::read(&external).unwrap(), b"untouched");
        std::fs::remove_file(&path).unwrap();
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let began = std::time::Instant::now();
        assert!(PreparationLock::acquire(home.path(), "item").is_err());
        assert!(began.elapsed() < std::time::Duration::from_secs(1));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(PreparationLock::acquire(home.path(), "item").is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&path, home.path().join("alias")).unwrap();
        assert!(PreparationLock::acquire(home.path(), "item").is_err());
    }
    #[test]
    fn preparation_lock_rejects_foreign_or_writable_ancestry_and_unsafe_home_alias() {
        let home = tempfile::tempdir().unwrap();
        let alias = home.path().join("alias");
        symlink(home.path(), &alias).unwrap();
        assert!(PreparationLock::acquire(&alias, "item").is_err());
        let unsafe_home = home.path().join("unsafe");
        std::fs::create_dir(&unsafe_home).unwrap();
        std::fs::set_permissions(&unsafe_home, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(PreparationLock::acquire(&unsafe_home, "item").is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn preparation_lock_rejects_extended_acl_grants() {
        let home = tempfile::tempdir().unwrap();
        drop(PreparationLock::acquire(home.path(), "item").unwrap());
        let path = home.path().join("work-items/preparation-locks/item.lock");
        assert!(
            std::process::Command::new("chmod")
                .args(["+a", "everyone allow read"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(PreparationLock::acquire(home.path(), "item").is_err());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    #[test]
    fn stable_flow_namespaces_remain_distinct_and_do_not_mint_private_capability() {
        let home = tempfile::tempdir().unwrap();
        let operation = WorkItemOperationId::new();
        let run = RunId::new();
        let preparation = PreparationLock::acquire_stable(
            home.path(),
            PreparationLockKey::FlowOperation(operation),
        )
        .unwrap();
        let launch = std::sync::Arc::new(
            PreparationLock::acquire_stable(
                home.path(),
                PreparationLockKey::FlowLaunch { operation, run },
            )
            .unwrap(),
        );
        assert_ne!(preparation.identity().unwrap(), launch.identity().unwrap());
        assert!(preparation.require_private().is_err());
        assert!(launch.require_private().is_err());
        let worker = launch.clone();
        drop(launch);
        assert!(matches!(
            PreparationLock::acquire_stable(
                home.path(),
                PreparationLockKey::FlowLaunch { operation, run }
            ),
            Err(WorkItemError::Busy)
        ));
        drop(worker);
        assert!(
            PreparationLock::acquire_stable(
                home.path(),
                PreparationLockKey::FlowLaunch { operation, run }
            )
            .is_ok()
        );
    }
    #[test]
    fn restricted_task_and_populated_flow_fail_before_touching_private_namespaces() {
        let home = tempfile::tempdir().unwrap();
        let absent = home.path().join("never-created");
        assert!(PreparationLock::acquire_task(&absent, WorkItemId::new()).is_err());
        assert!(
            PreparationLock::acquire_private(
                &absent,
                PreparationLockKey::FlowOperation(WorkItemOperationId::new())
            )
            .is_err()
        );
        assert!(!absent.exists());
        assert!(!home.path().join("work-items").exists());
    }
}
