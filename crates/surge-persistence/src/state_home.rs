//! Retained native state-home primitives; private capability integration remains closed.
#[path = "state_home/windows/mod.rs"]
mod windows;

pub(crate) use windows::NativeJournalLock;
pub(crate) use windows::native::NativeError;
pub(crate) use windows::{SqliteNamespaceOwner, StateHomeOwner};

impl From<windows::native::NativeError> for crate::runs::error::OpenError {
    fn from(error: windows::native::NativeError) -> Self {
        use windows::native::NativeError;
        let category = match &error {
            NativeError::Busy => "ownership busy",
            NativeError::Security(_) => "security refusal",
            NativeError::Flush(_) => "durability barrier",
            NativeError::Open(_) | NativeError::Windows(_) => "native API",
            NativeError::Io(_) => "filesystem I/O",
        };
        Self::StateHome {
            category,
            message: error.to_string(),
        }
    }
}

impl From<windows::native::NativeError> for crate::PersistenceError {
    fn from(error: windows::native::NativeError) -> Self {
        use windows::native::NativeError;
        if matches!(error, NativeError::Busy) {
            return Self::OwnershipBusy;
        }
        let category = match &error {
            NativeError::Busy => "ownership busy",
            NativeError::Security(_) => "security refusal",
            NativeError::Flush(_) => "durability barrier",
            NativeError::Open(_) | NativeError::Windows(_) => "native API",
            NativeError::Io(_) => "filesystem I/O",
        };
        Self::StateHome {
            category,
            message: error.to_string(),
        }
    }
}

use crate::PersistenceError;
impl From<windows::native::NativeError> for crate::work_items::WorkItemError {
    fn from(error: windows::native::NativeError) -> Self {
        use windows::native::NativeError;
        let category = match &error {
            NativeError::Busy => return Self::Busy,
            NativeError::Security(_) => "security refusal",
            NativeError::Flush(_) => "durability barrier",
            NativeError::Open(_) | NativeError::Windows(_) => "native API",
            NativeError::Io(_) => "filesystem I/O",
        };
        Self::StateHome {
            category,
            message: error.to_string(),
        }
    }
}

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

/// A retained Windows runtime namespace with protected creation and validation.
///
/// Keep this owner, or a derived directory/file owner, alive while using any path
/// inside it. Paths themselves do not grant ownership.
#[derive(Clone)]
pub struct RuntimeHomeOwner {
    owner: Arc<StateHomeOwner>,
    path: PathBuf,
}

/// Named runtime directories whose descriptors are retained by the owner.
#[derive(Clone, Copy, Debug)]
pub enum RuntimeDirectory {
    /// The state-home root.
    StateHome,
    /// Per-run databases and artifacts.
    Runs,
    /// Daemon control and output files.
    Daemon,
    /// Durable work-item lifecycle locks.
    Lifecycle,
}

impl RuntimeHomeOwner {
    /// Securely create or validate a complete runtime home before path-based I/O.
    ///
    /// # Errors
    /// Refuses unsafe owners, ACLs, reparse points, unsupported filesystems and
    /// failed complete durability barriers. Existing ACLs are never repaired.
    pub fn prepare(path: &Path) -> Result<Self, PersistenceError> {
        Ok(Self {
            owner: StateHomeOwner::open(path)?,
            path: path.to_owned(),
        })
    }

    /// Resolve the effective token user's real profile through the Windows API.
    /// Environment variables are not used as identity evidence.
    ///
    /// # Errors
    /// Returns the native profile lookup or effective-token validation error.
    pub fn user_profile_path() -> Result<PathBuf, PersistenceError> {
        windows::user_profile_path().map_err(Into::into)
    }

    /// The requested path; its namespace remains owned by this value.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Exclusively reserve a run and its artifacts directory without creating SQL.
    /// The returned owner retains both original directory chains until handoff.
    ///
    /// # Errors
    /// Existing run names fail rather than being adopted. A partial failure may
    /// leave protected directories, but never removes an object by its pathname.
    pub fn reserve_run_directory(
        &self,
        run: surge_core::RunId,
    ) -> Result<RuntimeDirectoryOwner, PersistenceError> {
        let (namespace, artifacts) = self.owner.reserve_run(run)?;
        Ok(RuntimeDirectoryOwner {
            namespace,
            _auxiliary: Some(artifacts),
        })
    }

    /// Retain a supported directory independently of this home's lifetime.
    ///
    /// # Errors
    /// Refuses a changed identity, token or security descriptor.
    pub fn directory(
        &self,
        directory: RuntimeDirectory,
    ) -> Result<RuntimeDirectoryOwner, PersistenceError> {
        Ok(RuntimeDirectoryOwner {
            namespace: self.owner.directory(directory)?,
            _auxiliary: None,
        })
    }
}

/// A retained directory used to open runtime files by one relative component.
pub struct RuntimeDirectoryOwner {
    namespace: windows::Namespace,
    _auxiliary: Option<windows::Namespace>,
}
impl RuntimeDirectoryOwner {
    /// Open protected append-only output, with a separate complete-flush handle.
    ///
    /// # Errors
    /// Refuses invalid names, unsafe existing files and native I/O failures.
    pub fn open_append(&self, name: &OsStr) -> Result<RuntimeAppendFile, PersistenceError> {
        Ok(RuntimeAppendFile {
            file: self.namespace.append_file(name)?,
        })
    }
    /// Open a protected control file, retaining its original object identity.
    ///
    /// # Errors
    /// Actual sharing contention yields [`PersistenceError::OwnershipBusy`].
    /// Other security, identity and I/O failures remain explicit errors.
    pub fn open_control(&self, name: &OsStr) -> Result<RuntimeControlFile, PersistenceError> {
        Ok(RuntimeControlFile {
            file: self.namespace.control_file(name)?,
        })
    }
}

/// Append output and its retained namespace and durability handle.
pub struct RuntimeAppendFile {
    file: windows::NativeAppendFile,
}
impl RuntimeAppendFile {
    /// Duplicate append output for a child and a matching independent owner lease.
    /// Keep the returned lease until the child has reached readiness or failed
    /// launch and settled. A `Stdio` value alone does not retain parent directories.
    ///
    /// # Errors
    /// Refuses changed namespace/object identities and handle duplication failures.
    pub fn stdio_clone(&self) -> Result<(Stdio, Self), PersistenceError> {
        let (stdio, file) = self.file.stdio_clone()?;
        Ok((stdio, Self { file }))
    }
    /// Complete file and parent-directory durability barriers.
    ///
    /// # Errors
    /// Returns identity, security or native flush failures.
    pub fn flush(&self) -> Result<(), PersistenceError> {
        self.file.flush().map_err(Into::into)
    }
}

/// An original control-file descriptor and an optional exclusive OS lock.
pub struct RuntimeControlFile {
    file: windows::NativeControlFile,
}
impl RuntimeControlFile {
    /// Revalidate exclusive ownership, effective user and original descriptor security.
    ///
    /// # Errors
    /// Refuses absent lock ownership, unsafe file identity or changed namespace security.
    pub fn verify(&self) -> Result<(), PersistenceError> {
        self.file.verify().map_err(Into::into)
    }
    /// Whether this opening created the held object using exclusive native creation.
    /// Existing empty files return `false`; contents cannot establish this fact.
    #[must_use]
    pub fn was_created(&self) -> bool {
        self.file.was_created()
    }
    /// Acquire the exclusive OS lock without waiting; `false` means contention.
    ///
    /// # Errors
    /// Returns structural, security and non-contention native failures.
    pub fn try_lock_exclusive(&mut self) -> Result<bool, PersistenceError> {
        self.file.try_lock().map_err(Into::into)
    }
    /// Read at most `limit` bytes under the exclusive lock (maximum 1 MiB).
    ///
    /// # Errors
    /// Refuses absent lock ownership, oversized contents and native I/O failures.
    pub fn read_bounded(&self, limit: usize) -> Result<Vec<u8>, PersistenceError> {
        self.file.read_bounded(limit).map_err(Into::into)
    }
    /// Replace bytes in the original exclusively held object and flush completely.
    ///
    /// # Errors
    /// Refuses absent ownership, oversized input and native write/flush failures.
    pub fn replace_bytes(&mut self, bytes: &[u8]) -> Result<(), PersistenceError> {
        self.file.replace_bytes(bytes).map_err(Into::into)
    }
    /// Complete the file and parent-directory durability barriers.
    ///
    /// # Errors
    /// Returns security or native flush failures.
    pub fn flush(&self) -> Result<(), PersistenceError> {
        self.file.flush().map_err(Into::into)
    }
    /// Delete the original exclusively held object and flush its retained parent.
    ///
    /// # Errors
    /// Refuses absent ownership or native deletion/flush failures.
    pub fn remove(self) -> Result<(), PersistenceError> {
        self.file.remove().map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::{RuntimeDirectory, RuntimeHomeOwner};
    use crate::PersistenceError;
    use std::ffi::OsStr;

    #[test]
    #[ignore = "mandatory dedicated-standard-user Windows CI runtime ownership oracle"]
    fn control_creation_fact_survives_empty_reopen_and_owns_removal() {
        let profile = RuntimeHomeOwner::user_profile_path().unwrap();
        let fixture = tempfile::Builder::new()
            .prefix("surge-control-")
            .tempdir_in(profile)
            .unwrap();
        let home = RuntimeHomeOwner::prepare(&fixture.path().join("state")).unwrap();
        let directory = home.directory(RuntimeDirectory::Daemon).unwrap();
        let path = home.path().join("daemon").join("test.pid");
        let mut first = directory.open_control(OsStr::new("test.pid")).unwrap();
        assert!(first.was_created());
        assert!(first.try_lock_exclusive().unwrap());
        assert!(first.read_bounded(32).unwrap().is_empty());
        assert!(matches!(
            directory.open_control(OsStr::new("test.pid")),
            Err(PersistenceError::OwnershipBusy)
        ));
        assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
        drop(first);
        let mut reopened = directory.open_control(OsStr::new("test.pid")).unwrap();
        assert!(
            !reopened.was_created(),
            "preexisting empty file is not a creation grant"
        );
        assert!(reopened.try_lock_exclusive().unwrap());
        assert!(reopened.read_bounded(32).unwrap().is_empty());
        reopened.replace_bytes(b"12345\n").unwrap();
        assert_eq!(reopened.read_bounded(32).unwrap(), b"12345\n");
        assert!(reopened.read_bounded(2).is_err());
        drop(directory);
        drop(home);
        assert!(
            std::fs::rename(path.parent().unwrap(), fixture.path().join("moved-daemon")).is_err()
        );
        reopened.remove().unwrap();
        assert!(!path.exists());
        println!("stage1 control creation-fact/exclusive-read/removal=PASS");
    }
    pub(super) fn descriptor_text(path: &std::path::Path) -> String {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows::{
            Win32::{
                Foundation::{HANDLE, HLOCAL, LocalFree},
                Security::{
                    Authorization::{
                        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo,
                        SDDL_REVISION_1, SE_FILE_OBJECT,
                    },
                    DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
                },
                Storage::FileSystem::{
                    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
                },
            },
            core::PWSTR,
        };
        struct Allocation(HLOCAL);
        impl Drop for Allocation {
            fn drop(&mut self) {
                // SAFETY: unique SDK LocalAlloc ownership.
                unsafe { LocalFree(self.0) };
            }
        }
        let file = std::fs::OpenOptions::new()
            .access_mode((READ_CONTROL | FILE_READ_ATTRIBUTES).0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(path)
            .unwrap();
        let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: held real directory handle and initialized SDK descriptor output.
        unsafe {
            GetSecurityInfo(
                HANDLE(file.as_raw_handle()),
                SE_FILE_OBJECT,
                information,
                None,
                None,
                None,
                None,
                Some(&raw mut descriptor),
            )
        }
        .ok()
        .unwrap();
        let _descriptor = Allocation(HLOCAL(descriptor.0));
        let mut text = PWSTR::null();
        // SAFETY: owned SDK descriptor remains live through conversion to SDK-owned text.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                information,
                &raw mut text,
                None,
            )
        }
        .unwrap();
        let _text = Allocation(HLOCAL(text.0.cast()));
        // SAFETY: successful conversion returned a live terminated UTF-16 allocation.
        unsafe { text.to_string() }.unwrap()
    }

    #[test]
    #[ignore = "mandatory dedicated-standard-user Windows CI runtime ownership oracle"]
    fn run_reservation_is_exclusive_retains_both_directories_and_creates_no_sql() {
        let profile = RuntimeHomeOwner::user_profile_path().unwrap();
        let fixture = tempfile::Builder::new()
            .prefix("surge-reserve-")
            .tempdir_in(profile)
            .unwrap();
        let home = RuntimeHomeOwner::prepare(&fixture.path().join("state")).unwrap();
        let run = surge_core::RunId::new();
        let owner = home.reserve_run_directory(run).unwrap();
        let directory = home.path().join("runs").join(run.to_string());
        let artifacts = directory.join("artifacts");
        super::test_security::assert_private_directory(&directory);
        super::test_security::assert_private_directory(&artifacts);
        assert_eq!(
            std::fs::read_dir(&directory)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            [OsStr::new("artifacts")]
        );
        assert_eq!(std::fs::read_dir(&artifacts).unwrap().count(), 0);
        let before_run = descriptor_text(&directory);
        let before_artifacts = descriptor_text(&artifacts);
        let sentinel = artifacts.join("sentinel");
        std::fs::write(&sentinel, b"existing reservation").unwrap();
        assert!(home.reserve_run_directory(run).is_err());
        assert_eq!(descriptor_text(&directory), before_run);
        assert_eq!(descriptor_text(&artifacts), before_artifacts);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"existing reservation");
        assert!(std::fs::rename(&directory, directory.with_extension("moved")).is_err());
        assert!(std::fs::rename(&artifacts, directory.join("moved-artifacts")).is_err());
        let empty = owner.open_append(OsStr::new("events.sqlite")).unwrap();
        empty.flush().unwrap();
        drop(empty);
        let database = directory.join("events.sqlite");
        assert_eq!(std::fs::metadata(&database).unwrap().len(), 0);
        let namespace = super::SqliteNamespaceOwner::existing(&database).unwrap();
        let connection = crate::runs::connection::RetainedConnection::open_owned(
            &database,
            rusqlite::OpenFlags::default(),
            namespace,
        )
        .unwrap();
        connection
            .execute_batch("CREATE TABLE actual(value INTEGER)")
            .unwrap();
        connection.close().unwrap();
        drop(owner);
        let moved_artifacts = directory.join("moved-artifacts");
        std::fs::rename(&artifacts, &moved_artifacts).unwrap();
        std::fs::rename(&moved_artifacts, &artifacts).unwrap();
        let moved = directory.with_extension("moved");
        std::fs::rename(&directory, &moved).unwrap();
        drop(home);
        fixture.close().unwrap();
        println!("stage1 exclusive directory-only reservation and retained artifacts=PASS");
    }
}

#[cfg(test)]
#[path = "state_home/security_tests.rs"]
mod security_tests;

#[cfg(test)]
#[path = "state_home/test_security.rs"]
pub(crate) mod test_security;

#[cfg(test)]
#[path = "state_home/creation_security_tests.rs"]
mod creation_security_tests;
