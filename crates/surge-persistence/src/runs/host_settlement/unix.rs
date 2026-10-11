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
        unix::{ffi::OsStrExt, fs::MetadataExt},
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
    let mut flags = OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK;
    flags |= if directory {
        OFlag::O_RDONLY | OFlag::O_DIRECTORY
    } else {
        OFlag::O_RDWR
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
        self.verify()?;
        let component = run.to_string();
        if create {
            match mkdirat(
                Some(self.runs.as_raw_fd()),
                component.as_str(),
                Mode::S_IRWXU,
            ) {
                Ok(()) => {},
                Err(error) if error == Errno::EEXIST => {},
                Err(error) => return Err(io(error)),
            }
        }
        let directory = open(&self.runs, OsStr::new(&component), true, false)?;
        if create {
            self.verify()?;
            verify(&self.runs, OsStr::new(&component), &directory, true)?;
            match mkdirat(Some(directory.as_raw_fd()), "artifacts", Mode::S_IRWXU) {
                Ok(()) => {},
                Err(error) if error == Errno::EEXIST => {},
                Err(error) => return Err(io(error)),
            }
        }
        let artifacts = open(&directory, OsStr::new("artifacts"), true, false)?;
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
        Ok(Database { file })
    }
    pub(super) fn verify_database(&self, db: &Database) -> Result<(), OpenError> {
        self.verify()?;
        verify(
            &self.directory,
            OsStr::new("events.sqlite"),
            &db.file,
            false,
        )
    }
}
pub(super) struct Database {
    file: File,
}
impl Database {
    pub(super) fn identity(&self) -> Result<Vec<u8>, OpenError> {
        identity(&self.file)
    }
}
