//! Native Windows state-home ownership primitives.
pub(super) mod native;

mod namespace;
mod security;

#[cfg(test)]
pub(super) mod creation_observation;

pub(crate) use namespace::NativeJournalLock;
pub(super) use namespace::{Namespace, NativeAppendFile, NativeControlFile};

use native::{NativeError, NativeResult};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    path::Path,
    sync::Arc,
};
use windows::Win32::Foundation::STATUS_OBJECT_NAME_COLLISION;

/// The original state-home namespace, held throughout every derived capability.
pub(crate) struct StateHomeOwner {
    home: Namespace,
    db: Namespace,
    runs: Namespace,
    daemon: Namespace,
    lifecycle: Namespace,
}
impl StateHomeOwner {
    pub(super) fn reserve_run(
        &self,
        run: surge_core::RunId,
    ) -> NativeResult<(Namespace, Namespace)> {
        self.verify()?;
        let directory = self
            .runs
            .child_exclusive(OsStr::new(&run.to_string()), true)?;
        let artifacts = directory.child_exclusive(OsStr::new("artifacts"), true)?;
        Ok((directory, artifacts))
    }
    pub(super) fn directory(&self, kind: super::RuntimeDirectory) -> NativeResult<Namespace> {
        let namespace = match kind {
            super::RuntimeDirectory::StateHome => &self.home,
            super::RuntimeDirectory::Runs => &self.runs,
            super::RuntimeDirectory::Daemon => &self.daemon,
            super::RuntimeDirectory::Lifecycle => &self.lifecycle,
        };
        namespace.retained_clone()
    }
    pub(crate) fn open(path: &Path) -> Result<Arc<Self>, NativeError> {
        let home = Namespace::home(path)?;
        let db = home.child(OsStr::new("db"), true)?;
        let runs = home.child(OsStr::new("runs"), true)?;
        let daemon = home.child(OsStr::new("daemon"), false)?;
        let work_items = home.child(OsStr::new("work-items"), false)?;
        let lifecycle = work_items.child(OsStr::new("locks"), false)?;
        Ok(Arc::new(Self {
            home,
            db,
            runs,
            daemon,
            lifecycle,
        }))
    }
    pub(crate) fn registry(self: &Arc<Self>) -> NativeResult<Arc<SqliteNamespaceOwner>> {
        self.db.verify_sidefiles(OsStr::new("registry.sqlite"))?;
        let file = self.db.database(OsStr::new("registry.sqlite"), true)?;
        Ok(Arc::new(SqliteNamespaceOwner {
            home: Some(self.clone()),
            file,
            namespace: self.db.retained_clone()?,
            name: OsString::from("registry.sqlite"),
            artifacts: None,
        }))
    }
    pub(crate) fn verify(&self) -> NativeResult<()> {
        self.home.verify()?;
        self.db.verify()?;
        self.runs.verify()?;
        self.daemon.verify()?;
        self.lifecycle.verify()
    }
}
/// Retained by the actual SQLite manager, independently of Storage's lifetime.
pub(crate) struct SqliteNamespaceOwner {
    home: Option<Arc<StateHomeOwner>>,
    file: File,
    namespace: Namespace,
    name: OsString,
    artifacts: Option<Namespace>,
}

/// Existing-only root for registry-gated run namespace preparation.
pub(crate) struct RunRoot {
    home: Namespace,
    runs: Namespace,
}
impl RunRoot {
    pub(crate) fn existing(path: &Path) -> NativeResult<Arc<Self>> {
        let home = Namespace::existing_parent(path)?;
        let runs = home.existing_child(OsStr::new("runs"), true)?;
        Ok(Arc::new(Self { home, runs }))
    }
    pub(crate) fn verify(&self) -> NativeResult<()> {
        self.home.verify()?;
        self.runs.verify()
    }
    pub(crate) fn run(
        self: &Arc<Self>,
        run: surge_core::RunId,
        create: bool,
    ) -> NativeResult<RunRoute> {
        self.verify()?;
        let component = run.to_string();
        let namespace = if create {
            match self.runs.child_exclusive(OsStr::new(&component), true) {
                Ok(namespace) => namespace,
                Err(NativeError::Open(STATUS_OBJECT_NAME_COLLISION)) => {
                    self.runs.child(OsStr::new(&component), true)?
                },
                Err(error) => return Err(error),
            }
        } else {
            self.runs.existing_child(OsStr::new(&component), true)?
        };
        let artifacts = if create {
            match namespace.child_exclusive(OsStr::new("artifacts"), true) {
                Ok(artifacts) => artifacts,
                Err(NativeError::Open(STATUS_OBJECT_NAME_COLLISION)) => {
                    namespace.child(OsStr::new("artifacts"), true)?
                },
                Err(error) => return Err(error),
            }
        } else {
            namespace.existing_child(OsStr::new("artifacts"), true)?
        };
        if create {
            drop(namespace.exclusive_database(OsStr::new("events.sqlite.lock"))?);
        }
        Ok(RunRoute {
            root: self.clone(),
            namespace,
            artifacts,
        })
    }
}
pub(crate) struct RunRoute {
    root: Arc<RunRoot>,
    namespace: Namespace,
    artifacts: Namespace,
}
impl RunRoute {
    pub(crate) fn verify(&self) -> NativeResult<()> {
        self.root.verify()?;
        self.namespace.verify()?;
        self.artifacts.verify()
    }
    pub(crate) fn identities(&self) -> NativeResult<(Vec<u8>, Vec<u8>)> {
        self.verify()?;
        Ok((
            self.root.home.object_identity()?,
            self.namespace.object_identity()?,
        ))
    }
    pub(crate) fn database(&self, create: bool) -> NativeResult<Arc<SqliteNamespaceOwner>> {
        self.verify()?;
        let name = OsString::from("events.sqlite");
        self.namespace.verify_sidefiles(&name)?;
        let file = if create {
            self.namespace.exclusive_database(&name)?
        } else {
            self.namespace.database(&name, false)?
        };
        Ok(Arc::new(SqliteNamespaceOwner {
            home: None,
            namespace: self.namespace.retained_clone()?,
            artifacts: Some(self.artifacts.retained_clone()?),
            name,
            file,
        }))
    }
}
impl SqliteNamespaceOwner {
    pub(crate) fn identity(&self) -> NativeResult<Vec<u8>> {
        self.verify()?;
        namespace::file_identity(&self.file, false)
    }
    pub(crate) fn verify(&self) -> NativeResult<()> {
        if let Some(home) = &self.home {
            home.verify()?;
        }
        if let Some(artifacts) = &self.artifacts {
            artifacts.verify()?;
        }
        self.namespace.verify_database(&self.name, &self.file)?;
        self.namespace.verify_sidefiles(&self.name)
    }
}

impl StateHomeOwner {
    pub(crate) fn run_database(
        self: &Arc<Self>,
        run: &OsStr,
        create: bool,
    ) -> NativeResult<Arc<SqliteNamespaceOwner>> {
        self.verify()?;
        let namespace = if create {
            self.runs.child(run, true)?
        } else {
            self.runs.existing_child(run, true)?
        };
        let artifacts = if create {
            Some(namespace.child(OsStr::new("artifacts"), true)?)
        } else {
            Some(namespace.existing_child(OsStr::new("artifacts"), true)?)
        };
        let name = OsString::from("events.sqlite");
        namespace.verify_sidefiles(&name)?;
        let file = namespace.database(&name, create)?;
        Ok(Arc::new(SqliteNamespaceOwner {
            home: Some(self.clone()),
            namespace,
            file,
            name,
            artifacts,
        }))
    }
}

impl SqliteNamespaceOwner {
    pub(crate) fn standalone(path: &Path) -> NativeResult<Arc<Self>> {
        Self::open_standalone(path, true)
    }
    pub(crate) fn existing(path: &Path) -> NativeResult<Arc<Self>> {
        Self::open_standalone(path, false)
    }
    fn open_standalone(path: &Path, create: bool) -> NativeResult<Arc<Self>> {
        let parent = path
            .parent()
            .ok_or(NativeError::Security("database has no parent"))?;
        let name = path
            .file_name()
            .ok_or(NativeError::Security("database has no filename"))?
            .to_owned();
        let namespace = if create {
            Namespace::protected_parent(parent)?
        } else {
            Namespace::existing_parent(parent)?
        };
        namespace.verify_sidefiles(&name)?;
        let file = namespace.database(&name, create)?;
        Ok(Arc::new(Self {
            home: None,
            namespace,
            name,
            file,
            artifacts: None,
        }))
    }
}

pub(super) fn user_profile_path() -> NativeResult<std::path::PathBuf> {
    use std::os::windows::{ffi::OsStringExt, io::AsRawHandle};
    use windows::{
        Win32::{
            Foundation::{ERROR_INSUFFICIENT_BUFFER, HANDLE},
            UI::Shell::GetUserProfileDirectoryW,
        },
        core::PWSTR,
    };
    let token = security::effective_token()?;
    let mut length = 0;
    // SAFETY: live effective token, null zero-capacity output, initialized required-size output.
    let result = unsafe {
        GetUserProfileDirectoryW(
            HANDLE(token.as_raw_handle()),
            PWSTR::null(),
            &raw mut length,
        )
    };
    match result {
        Err(error) if error.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() => {},
        Err(error) => return Err(error.into()),
        Ok(()) => {
            return Err(NativeError::Security(
                "profile size query unexpectedly succeeded",
            ));
        },
    }
    if !(2..=32_768).contains(&length) {
        return Err(NativeError::Security("invalid profile path capacity"));
    }
    let mut buffer = vec![
        0u16;
        usize::try_from(length)
            .map_err(|_| NativeError::Security("profile capacity overflow"))?
    ];
    // SAFETY: retained query token, initialized UTF-16 storage matching declared capacity.
    unsafe {
        GetUserProfileDirectoryW(
            HANDLE(token.as_raw_handle()),
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }?;
    let end =
        usize::try_from(length).map_err(|_| NativeError::Security("profile length overflow"))?;
    if !(2..=buffer.len()).contains(&end) || buffer[end - 1] != 0 || buffer[..end - 1].contains(&0)
    {
        return Err(NativeError::Security("invalid terminated profile path"));
    }
    let path = std::path::PathBuf::from(OsString::from_wide(&buffer[..end - 1]));
    if !path.is_absolute() {
        return Err(NativeError::Security("profile path is not absolute"));
    }
    Ok(path)
}
