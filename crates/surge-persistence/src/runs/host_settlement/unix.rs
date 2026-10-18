//! Retained Unix objects; cooperative exclusion does not defend against a malicious same-user actor.
use super::super::error::OpenError;
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, openat},
    sys::stat::{Mode, fstatat, mkdirat},
};
use std::{
    ffi::{CString, OsStr},
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    sync::Arc,
};

fn invalid() -> OpenError {
    OpenError::Authority("run namespace identity changed or unsafe".into())
}
fn io(error: nix::errno::Errno) -> OpenError {
    std::io::Error::from_raw_os_error(error as i32).into()
}
fn name(value: &OsStr) -> Result<CString, OpenError> {
    CString::new(value.as_bytes()).map_err(|_| invalid())
}
fn open(
    parent: &File,
    component: &OsStr,
    directory: bool,
    exclusive: bool,
) -> Result<File, OpenError> {
    open_access(parent, component, directory, exclusive, true)
}
fn open_access(
    parent: &File,
    component: &OsStr,
    directory: bool,
    exclusive: bool,
    writable: bool,
) -> Result<File, OpenError> {
    let mut flags = OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK;
    flags |= if directory {
        OFlag::O_RDONLY | OFlag::O_DIRECTORY
    } else if writable {
        OFlag::O_RDWR
    } else {
        OFlag::O_RDONLY
    };
    if exclusive {
        flags |= OFlag::O_CREAT | OFlag::O_EXCL;
    }
    let fd = openat(
        Some(parent.as_raw_fd()),
        name(component)?.as_c_str(),
        flags,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(io)?;
    // SAFETY: successful openat returns one fresh descriptor whose sole owner is this File.
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if (directory && !meta.is_dir()) || (!directory && (!meta.is_file() || meta.nlink() != 1)) {
        return Err(invalid());
    }
    Ok(file)
}
fn identity(file: &File) -> Result<Vec<u8>, OpenError> {
    let meta = file.metadata()?;
    Ok([meta.dev().to_le_bytes(), meta.ino().to_le_bytes()].concat())
}
fn verify(parent: &File, component: &OsStr, file: &File, directory: bool) -> Result<(), OpenError> {
    let linked = fstatat(
        Some(parent.as_raw_fd()),
        name(component)?.as_c_str(),
        AtFlags::AT_SYMLINK_NOFOLLOW,
    )
    .map_err(io)?;
    let held = file.metadata()?;
    if held.dev() != linked.st_dev as u64
        || held.ino() != linked.st_ino as u64
        || (directory && !held.is_dir())
        || (!directory && (!held.is_file() || held.nlink() != 1))
    {
        return Err(invalid());
    }
    Ok(())
}
struct Chain {
    files: Vec<File>,
    names: Vec<std::ffi::OsString>,
    original: PathBuf,
    canonical: PathBuf,
}
impl Chain {
    fn existing(path: &Path) -> Result<Self, OpenError> {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(invalid());
        }
        let canonical = std::fs::canonicalize(path)?;
        let mut files = vec![File::open("/")?];
        let mut names = Vec::new();
        for part in canonical.components() {
            let std::path::Component::Normal(component) = part else {
                continue;
            };
            let parent = files.last().ok_or_else(invalid)?;
            let next = open(parent, component, true, false)?;
            names.push(component.to_owned());
            files.push(next);
        }
        let result = Self {
            files,
            names,
            original: path.to_owned(),
            canonical,
        };
        result.verify()?;
        Ok(result)
    }
    fn last(&self) -> Result<&File, OpenError> {
        self.files.last().ok_or_else(invalid)
    }
    fn verify(&self) -> Result<(), OpenError> {
        if std::fs::symlink_metadata(&self.original)?
            .file_type()
            .is_symlink()
            || std::fs::canonicalize(&self.original)? != self.canonical
        {
            return Err(invalid());
        }
        for (i, n) in self.names.iter().enumerate() {
            verify(&self.files[i], n, &self.files[i + 1], true)?;
        }
        Ok(())
    }
}
pub(super) struct RunRoot {
    home: Chain,
    runs: File,
}
impl RunRoot {
    pub(super) fn existing(home: &Path) -> Result<Arc<Self>, OpenError> {
        let home = Chain::existing(home)?;
        let runs = open(home.last()?, OsStr::new("runs"), true, false)?;
        Ok(Arc::new(Self { home, runs }))
    }
    pub(super) fn verify(&self) -> Result<(), OpenError> {
        self.home.verify()?;
        verify(self.home.last()?, OsStr::new("runs"), &self.runs, true)
    }
    pub(super) fn run(
        self: &Arc<Self>,
        run: surge_core::RunId,
        create: bool,
    ) -> Result<RunRoute, OpenError> {
        self.run_inner(run, create, !create)
    }
    pub(super) fn existing_run(
        self: &Arc<Self>,
        run: surge_core::RunId,
    ) -> Result<RunRoute, OpenError> {
        self.run_inner(run, false, false)
    }
    fn run_inner(
        self: &Arc<Self>,
        run: surge_core::RunId,
        create: bool,
        repair_artifacts: bool,
    ) -> Result<RunRoute, OpenError> {
        self.verify()?;
        let component = run.to_string();
        if create {
            match mkdirat(
                Some(self.runs.as_raw_fd()),
                component.as_str(),
                Mode::S_IRWXU,
            ) {
                Ok(()) => {},
                Err(Errno::EEXIST) => {},
                Err(error) => return Err(io(error)),
            }
        }
        let directory = open(&self.runs, OsStr::new(&component), true, false)?;
        if create {
            self.verify()?;
            verify(&self.runs, OsStr::new(&component), &directory, true)?;
            match mkdirat(Some(directory.as_raw_fd()), "artifacts", Mode::S_IRWXU) {
                Ok(()) => {},
                Err(Errno::EEXIST) => {},
                Err(error) => return Err(io(error)),
            }
        }
        let artifacts = match open(&directory, OsStr::new("artifacts"), true, false) {
            Ok(artifacts) => artifacts,
            Err(OpenError::Io(error))
                if repair_artifacts && error.kind() == std::io::ErrorKind::NotFound =>
            {
                mkdirat(Some(directory.as_raw_fd()), "artifacts", Mode::S_IRWXU).map_err(io)?;
                open(&directory, OsStr::new("artifacts"), true, false)?
            },
            Err(error) => return Err(error),
        };
        let result = RunRoute {
            root: self.clone(),
            component,
            directory,
            artifacts,
        };
        result.verify()?;
        if create {
            let lock = open(
                &result.directory,
                OsStr::new("events.sqlite.lock"),
                false,
                true,
            )?;
            lock.sync_all()?;
            result.directory.sync_all()?;
            self.runs.sync_all()?;
        }
        Ok(result)
    }
}
pub(super) struct RunRoute {
    root: Arc<RunRoot>,
    component: String,
    directory: File,
    artifacts: File,
}
impl RunRoute {
    pub(super) fn verify(&self) -> Result<(), OpenError> {
        self.root.verify()?;
        verify(
            &self.root.runs,
            OsStr::new(&self.component),
            &self.directory,
            true,
        )?;
        verify(
            &self.directory,
            OsStr::new("artifacts"),
            &self.artifacts,
            true,
        )
    }
    pub(super) fn identities(&self) -> Result<(Vec<u8>, Vec<u8>), OpenError> {
        self.verify()?;
        Ok((
            identity(self.root.home.last()?)?,
            identity(&self.directory)?,
        ))
    }
    pub(super) fn database(&self, create: bool) -> Result<Database, OpenError> {
        self.verify()?;
        let file = if create {
            match open(&self.directory, OsStr::new("events.sqlite"), false, true) {
                Ok(file) => file,
                Err(OpenError::Io(error)) if error.raw_os_error() == Some(Errno::EEXIST as i32) => {
                    open(&self.directory, OsStr::new("events.sqlite"), false, false)?
                },
                Err(error) => return Err(error),
            }
        } else {
            open(&self.directory, OsStr::new("events.sqlite"), false, false)?
        };
        if create {
            file.sync_all()?;
            self.directory.sync_all()?;
        }
        Ok(Database::Legacy { file })
    }
    pub(super) fn verify_database(&self, db: &Database) -> Result<(), OpenError> {
        self.verify()?;
        match db {
            Database::Legacy { file } => {
                verify(&self.directory, OsStr::new("events.sqlite"), file, false)
            },
            Database::Controlled { identity } => {
                if &self.controlled_database_identity()? != identity {
                    return Err(invalid());
                }
                Ok(())
            },
        }
    }
}

fn stat_identity(metadata: &nix::sys::stat::FileStat) -> Result<Vec<u8>, OpenError> {
    let dev = u64::try_from(i128::from(metadata.st_dev)).map_err(|_| invalid())?;
    let ino = u64::try_from(i128::from(metadata.st_ino)).map_err(|_| invalid())?;
    Ok([dev.to_le_bytes(), ino.to_le_bytes()].concat())
}
pub(super) enum Database {
    Legacy { file: File },
    Controlled { identity: Vec<u8> },
}
impl Database {
    pub(super) fn identity(&self) -> Result<Vec<u8>, OpenError> {
        match self {
            Self::Legacy { file } => identity(file),
            Self::Controlled { identity } => Ok(identity.clone()),
        }
    }
}
impl RunRoute {
    fn controlled_database_identity(&self) -> Result<Vec<u8>, OpenError> {
        self.verify()?;
        let metadata = fstatat(
            Some(self.directory.as_raw_fd()),
            "events.sqlite",
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(io)?;
        if metadata.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG
            || metadata.st_nlink != 1
            || metadata.st_uid != nix::unistd::Uid::effective().as_raw()
            || metadata.st_mode & 0o7777 != 0o600
        {
            return Err(invalid());
        }
        stat_identity(&metadata)
    }
    pub(super) fn reader_route_identity(&self) -> Result<Vec<u8>, OpenError> {
        let (home, run) = self.identities()?;
        let lock = fstatat(
            Some(self.directory.as_raw_fd()),
            "events.sqlite.lock",
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(io)?;
        if lock.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG || lock.st_nlink != 1 {
            return Err(invalid());
        }
        let mut value = vec![1];
        value.extend(home);
        value.extend(run);
        value.extend(stat_identity(&lock)?);
        Ok(value)
    }
    pub(super) fn controlled_database(&self, create: bool) -> Result<Database, OpenError> {
        self.verify()?;
        if create {
            match fstatat(
                Some(self.directory.as_raw_fd()),
                "events.sqlite",
                AtFlags::AT_SYMLINK_NOFOLLOW,
            ) {
                Err(Errno::ENOENT) => {},
                Err(error) => return Err(io(error)),
                Ok(_) => return Err(invalid()),
            }
            let path = self
                .root
                .home
                .original
                .join("runs")
                .join(&self.component)
                .join("events.sqlite");
            // SQLite owns every database descriptor, including creation and final close.
            let connection = rusqlite::Connection::open_with_flags(
                &path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                    | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            connection
                .close()
                .map_err(|(_, error)| OpenError::Sqlite(error))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            self.directory.sync_all()?;
        }
        Ok(Database::Controlled {
            identity: self.controlled_database_identity()?,
        })
    }
}

// This is an outside control file, never a descriptor for a SQLite database.
pub(super) struct NativeRunControl {
    home: Chain,
    parent: File,
    file: File,
    component: String,
}
fn private_acl(file: &File) -> Result<(), OpenError> {
    crate::work_items::start_preparation::secure_lock::validate_private_acl(file)
        .map_err(|_| invalid())
}
fn control_directory(file: &File) -> Result<(), OpenError> {
    let meta = file.metadata()?;
    private_acl(file)?;
    if !meta.is_dir()
        || meta.uid() != nix::unistd::Uid::effective().as_raw()
        || meta.mode() & 0o7777 != 0o700
    {
        return Err(invalid());
    }
    Ok(())
}
impl NativeRunControl {
    pub(super) fn acquire(
        home: &Path,
        run: surge_core::RunId,
        create: bool,
        exclusive: bool,
    ) -> Result<Self, OpenError> {
        let home = Chain::existing(home)?;
        if create {
            match mkdirat(
                Some(home.last()?.as_raw_fd()),
                "run-controls",
                Mode::S_IRWXU,
            ) {
                Ok(()) => home.last()?.sync_all()?,
                Err(Errno::EEXIST) => {},
                Err(error) => return Err(io(error)),
            }
        }
        let parent = open(home.last()?, OsStr::new("run-controls"), true, false)?;
        control_directory(&parent)?;
        let component = format!("{run}.lock");
        let file = open(&parent, OsStr::new(&component), false, create)?;
        let locked = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        locked.map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => OpenError::RunInUse { run_id: run },
            std::fs::TryLockError::Error(error) => OpenError::Io(error),
        })?;
        let owner = Self {
            home,
            parent,
            file,
            component,
        };
        owner.verify()?;
        if create {
            owner.file.sync_all()?;
            owner.parent.sync_all()?;
        }
        Ok(owner)
    }
    pub(super) fn verify(&self) -> Result<(), OpenError> {
        self.home.verify()?;
        verify(
            self.home.last()?,
            OsStr::new("run-controls"),
            &self.parent,
            true,
        )?;
        control_directory(&self.parent)?;
        verify(&self.parent, OsStr::new(&self.component), &self.file, false)?;
        let meta = self.file.metadata()?;
        private_acl(&self.file)?;
        if meta.uid() != nix::unistd::Uid::effective().as_raw()
            || meta.mode() & 0o7777 != 0o600
            || meta.len() != 0
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub(super) fn identities(&self) -> Result<super::control::NativeControlIdentities, OpenError> {
        self.verify()?;
        Ok((
            identity(self.home.last()?)?,
            identity(&self.parent)?,
            identity(&self.file)?,
        ))
    }
}

/// Existing-only deletion route; no artifacts or journal objects are repaired.
pub(super) struct DeletionNamespace {
    root: Arc<RunRoot>,
    component: String,
    directory: Option<File>,
}
impl DeletionNamespace {
    pub(super) fn existing(home: &Path, run: surge_core::RunId) -> Result<Self, OpenError> {
        let root = RunRoot::existing(home)?;
        let component = run.to_string();
        let directory = match open(&root.runs, OsStr::new(&component), true, false) {
            Ok(file) => Some(file),
            Err(OpenError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        Ok(Self {
            root,
            component,
            directory,
        })
    }
    pub(super) fn verify(&self) -> Result<(), OpenError> {
        self.root.verify()?;
        match &self.directory {
            Some(directory) => verify(
                &self.root.runs,
                OsStr::new(&self.component),
                directory,
                true,
            ),
            None => match fstatat(
                Some(self.root.runs.as_raw_fd()),
                self.component.as_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            ) {
                Err(Errno::ENOENT) => Ok(()),
                Err(error) => Err(io(error)),
                Ok(_) => Err(invalid()),
            },
        }
    }
    pub(super) fn outside_parent(&self) -> Result<(Vec<u8>, Vec<u8>), OpenError> {
        self.root.verify()?;
        Ok((
            identity(self.root.home.last()?)?,
            identity(&self.root.runs)?,
        ))
    }
    pub(super) fn path(&self) -> PathBuf {
        self.root.home.original.join("runs").join(&self.component)
    }
    pub(super) fn absent(&self) -> Result<bool, OpenError> {
        self.verify()?;
        Ok(self.directory.is_none())
    }
    fn parent(&self, path: &Path) -> Result<(File, std::ffi::OsString), OpenError> {
        self.verify()?;
        let root = self.directory.as_ref().ok_or_else(invalid)?;
        let mut parent = root.try_clone()?;
        let mut parts = path.components().peekable();
        while let Some(component) = parts.next() {
            let std::path::Component::Normal(component) = component else {
                return Err(invalid());
            };
            if parts.peek().is_none() {
                return Ok((parent, component.to_owned()));
            }
            parent = open(&parent, component, true, false)?;
        }
        Err(invalid())
    }
    pub(super) fn entry(
        &self,
        path: &Path,
    ) -> Result<Option<super::deletion::NativeEntry>, OpenError> {
        self.verify()?;
        if path.as_os_str().is_empty() {
            return self
                .directory
                .as_ref()
                .map(|file| super::deletion::read_native_entry(file, identity(file)?, true))
                .transpose();
        }
        let (parent, name) = match self.parent(path) {
            Ok(value) => value,
            Err(OpenError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            },
            Err(error) => return Err(error),
        };
        let metadata = match fstatat(
            Some(parent.as_raw_fd()),
            name.as_os_str(),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        ) {
            Ok(value) => value,
            Err(Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(io(error)),
        };
        let directory = metadata.st_mode & nix::libc::S_IFMT == nix::libc::S_IFDIR;
        if !directory && metadata.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG {
            return Err(invalid());
        }
        let file = open_access(&parent, &name, directory, false, false)?;
        super::deletion::read_native_entry(&file, identity(&file)?, directory).map(Some)
    }
    pub(super) fn remove(
        &mut self,
        path: &Path,
        expected: &super::deletion::NativeEntry,
        parent_identity: &[u8],
    ) -> Result<(), OpenError> {
        use nix::unistd::{UnlinkatFlags, unlinkat};
        self.verify()?;
        if path.as_os_str().is_empty() {
            let root = self.directory.as_ref().ok_or_else(invalid)?;
            if identity(root)? != expected.identity || identity(&self.root.runs)? != parent_identity
            {
                return Err(invalid());
            }
            unlinkat(
                Some(self.root.runs.as_raw_fd()),
                self.component.as_str(),
                UnlinkatFlags::RemoveDir,
            )
            .map_err(io)?;
            self.directory.take();
            self.root.runs.sync_all()?;
        } else {
            let (parent, name) = self.parent(path)?;
            let linked = fstatat(
                Some(parent.as_raw_fd()),
                name.as_os_str(),
                AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_err(io)?;
            if identity(&parent)? != parent_identity || stat_identity(&linked)? != expected.identity
            {
                return Err(invalid());
            }
            let flags = if expected.directory {
                UnlinkatFlags::RemoveDir
            } else {
                UnlinkatFlags::NoRemoveDir
            };
            unlinkat(Some(parent.as_raw_fd()), name.as_os_str(), flags).map_err(io)?;
            parent.sync_all()?;
        }
        self.verify()
    }
}
