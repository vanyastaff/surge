//! Confidential preparation and immutable objects through retained native namespaces.
use super::*;
use std::io::{Read, Write};
use windows::{
    Wdk::Storage::FileSystem::{
        FILE_NAMES_INFORMATION, FILE_RENAME_INFORMATION, FileNamesInformation,
        FileRenameInformation, NtQueryDirectoryFile, NtSetInformationFile,
    },
    Win32::{
        Foundation::{BOOLEAN, NTSTATUS, STATUS_NO_MORE_FILES},
        Storage::FileSystem::{
            DELETE, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_INFO_EX, FILE_GENERIC_READ,
            FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FileDispositionInfoEx,
            SetFileInformationByHandle,
        },
    },
};
const LIMIT: u64 = 8 * 1024 * 1024;

fn namespace(home: &Path, leaf: &str, create: bool) -> NativeResult<Namespace> {
    // Existing-only opening still requires a protected home. It never repairs ACLs.
    let mut namespace = Namespace::existing_parent(home)?;
    for name in ["work-items", leaf] {
        namespace.writable_parent()?;
        namespace.push(
            OsStr::new(name),
            DirectoryPolicy::Protected { inheritable: false },
            create,
        )?;
        namespace.writable_parent()?;
        let mut files = namespace.files.iter().rev();
        let child = files
            .next()
            .ok_or_else(|| refusal("missing private child"))?;
        #[cfg(test)]
        observation::observe(observation::Phase::PrivateDirectoryBarrier, child)?;
        flush_complete(child)?;
        flush_complete(
            files
                .next()
                .ok_or_else(|| refusal("missing private parent"))?,
        )?;
        namespace.verify()?;
    }
    namespace.writable_parent()?;
    namespace.verify()?;
    Ok(namespace)
}

/// The actual locked original file and the only retained preparation namespace.
/// The file is declared first so it closes before its parent ownership is released.
pub(crate) struct PrivatePreparation {
    lock: File,
    name: OsString,
    identity: Identity,
    namespace: Namespace,
}
impl PrivatePreparation {
    pub(crate) fn acquire(home: &Path, name: &str) -> NativeResult<Self> {
        let namespace = namespace(home, "preparation-locks", true)?;
        let name = OsString::from(format!("{name}.lock"));
        let lock = namespace.runtime_file(&name, false)?.0;
        let identity = inspect(&lock, false)?;
        lock.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => NativeError::Busy,
            std::fs::TryLockError::Error(error) => NativeError::Io(error),
        })?;
        let owner = Self {
            lock,
            name,
            identity,
            namespace,
        };
        owner.require_private()?;
        Ok(owner)
    }
    pub(crate) fn verify(&self) -> NativeResult<()> {
        self.namespace.verify_database(&self.name, &self.lock)?;
        if inspect(&self.lock, false)? != self.identity {
            return Err(refusal("private preparation original identity changed"));
        }
        Ok(())
    }
    pub(crate) fn require_private(&self) -> NativeResult<()> {
        self.verify()?;
        #[cfg(test)]
        observation::observe(observation::Phase::PreparationFileBarrier, &self.lock)?;
        flush_complete(&self.lock)?;
        #[cfg(test)]
        observation::observe(observation::Phase::PreparationParentBarrier, &self.lock)?;
        self.namespace.flush_parent()?;
        self.verify()
    }
    pub(crate) fn legacy_identity(&self) -> NativeResult<String> {
        self.require_private()?;
        #[derive(serde::Serialize)]
        struct LegacyIdentity {
            volume: u64,
            file: [u8; 16],
        }
        let legacy = |identity: &Identity| LegacyIdentity {
            volume: identity.volume,
            file: identity.id,
        };
        let chain: Vec<_> = self
            .namespace
            .identities
            .iter()
            .enumerate()
            .map(|(index, identity)| {
                let name = if index == 0 {
                    self.namespace.root.as_os_str()
                } else {
                    &self.namespace.names[index - 1]
                };
                (name.encode_wide().collect::<Vec<_>>(), legacy(identity))
            })
            .collect();
        serde_json::to_string(&(
            chain,
            self.name.encode_wide().collect::<Vec<_>>(),
            legacy(&self.identity),
        ))
        .map_err(|_| refusal("native preparation identity encoding failed"))
    }
}

pub(crate) struct PrivateFiles {
    namespace: Namespace,
}
impl PrivateFiles {
    pub(crate) fn open_root(home: &Path) -> NativeResult<Self> {
        let mut namespace = Namespace::existing_parent(home)?;
        namespace.writable_parent()?;
        namespace.flush_parent()?;
        namespace.verify()?;
        Ok(Self { namespace })
    }
    pub(crate) fn verify(&self) -> NativeResult<()> {
        self.namespace.verify()
    }
    pub(crate) fn identity(&self) -> NativeResult<Vec<Vec<u8>>> {
        self.namespace.verify()?;
        self.namespace
            .files
            .iter()
            .map(|file| file_identity(file, true))
            .collect()
    }
    pub(crate) fn retain(&self, name: &str) -> NativeResult<File> {
        self.namespace.verify()?;
        let file = self.opened(OsStr::new(name))?;
        self.validate(&file)?;
        self.namespace.verify()?;
        Ok(file)
    }
    pub(crate) fn retained_identity(&self, file: &File) -> NativeResult<Vec<u8>> {
        self.namespace.verify()?;
        self.validate(file)?;
        file_identity(file, false)
    }
    pub(crate) fn read_optional(&self, name: &str) -> NativeResult<Option<Vec<u8>>> {
        self.namespace.verify()?;
        let name = OsStr::new(name);
        match self.opened(name) {
            Ok(file) => self.read_opened(name, file).map(Some),
            Err(NativeError::Open(windows::Win32::Foundation::STATUS_OBJECT_NAME_NOT_FOUND)) => {
                Ok(None)
            },
            Err(error) => Err(error),
        }
    }
    pub(crate) fn open(home: &Path, create: bool) -> NativeResult<Self> {
        Ok(Self {
            namespace: namespace(home, "private-flow-inputs", create)?,
        })
    }
    fn parent(&self) -> NativeResult<&File> {
        self.namespace
            .files
            .last()
            .ok_or_else(|| refusal("missing private parent"))
    }
    fn validate(&self, file: &File) -> NativeResult<Identity> {
        self.namespace
            .owner
            .validate_directory(file, DirectoryPolicy::Protected { inheritable: false })?;
        inspect(file, false)
    }
    fn opened(&self, name: &OsStr) -> NativeResult<File> {
        private_open(self.parent()?, name, None, false)
    }
    pub(crate) fn read(&self, name: &str) -> NativeResult<Vec<u8>> {
        self.namespace.verify()?;
        let name = OsStr::new(name);
        let file = self.opened(name)?;
        self.read_opened(name, file)
    }
    fn read_opened(&self, name: &OsStr, mut file: File) -> NativeResult<Vec<u8>> {
        let original = self.validate(&file)?;
        let length = file.metadata()?.len();
        if length > LIMIT {
            return Err(refusal("private object exceeds size limit"));
        }
        let mut bytes = Vec::new();
        (&mut file).take(LIMIT + 1).read_to_end(&mut bytes)?;
        let named = self.opened(name)?;
        if bytes.len() as u64 != length
            || self.validate(&file)? != original
            || self.validate(&named)? != original
            || file.metadata()?.len() != length
        {
            return Err(refusal("private object changed during read"));
        }
        self.namespace.verify()?;
        #[cfg(test)]
        observation::observe(observation::Phase::ReadFileBarrier, &file)?;
        flush_complete(&file)?;
        #[cfg(test)]
        observation::observe(observation::Phase::ReadParentBarrier, self.parent()?)?;
        flush_complete(self.parent()?)?;
        self.namespace.verify()?;
        if self.validate(&file)? != original
            || self.validate(&self.opened(name)?)? != original
            || file.metadata()?.len() != length
        {
            return Err(refusal("private object changed around durability barrier"));
        }
        Ok(bytes)
    }
    pub(crate) fn publish(&self, name: &str, bytes: &[u8]) -> NativeResult<bool> {
        if bytes.len() as u64 > LIMIT {
            return Err(refusal("private object exceeds size limit"));
        }
        component(OsStr::new(name))?;
        self.namespace.verify()?;
        let pending = OsString::from(format!(
            ".pending-{}",
            hex::encode(rand::random::<[u8; 32]>())
        ));
        let creation = self.namespace.owner.creation(false)?;
        let mut file = private_open(self.parent()?, &pending, Some(&creation), true)?;
        let result = (|| {
            self.namespace.verify()?;
            let original = self.validate(&file)?;
            #[cfg(test)]
            observation::observe(observation::Phase::BeforeWrite, &file)?;
            file.write_all(bytes)?;
            #[cfg(test)]
            observation::observe(observation::Phase::AfterWrite, &file)?;
            flush_complete(&file)?;
            #[cfg(test)]
            observation::observe(observation::Phase::AfterFileFlush, &file)?;
            self.namespace.verify()?;
            if self.validate(&file)? != original {
                return Err(refusal("private staging identity changed"));
            }
            rename_no_replace(&file, self.parent()?, OsStr::new(name))?;
            Ok(original)
        })();
        let (published, original) = match result {
            // Successful rename disarms staging disposal immediately: no subsequent
            // error may send this now-published descriptor through cleanup.
            Ok(original) => (PublishedFile(file), original),
            Err(NativeError::Open(STATUS_OBJECT_NAME_COLLISION)) => {
                cleanup_pending(file, self.parent()?)?;
                self.read(name)?;
                return Ok(false);
            },
            Err(error) => {
                return Err(match cleanup_pending(file, self.parent()?) {
                    Ok(()) => error,
                    Err(cleanup) => NativeError::Cleanup {
                        primary: Box::new(error),
                        cleanup: Box::new(cleanup),
                    },
                });
            },
        };
        #[cfg(test)]
        observation::observe(observation::Phase::Published, &published.0)?;
        let named = self.opened(OsStr::new(name))?;
        if self.validate(&named)? != original || self.validate(&published.0)? != original {
            return Err(refusal("published private object identity changed"));
        }
        flush_complete(&published.0)?;
        drop(named);
        close_checked(published.0)?;
        self.namespace.verify()?;
        #[cfg(test)]
        observation::observe(observation::Phase::BeforePublishParentFlush, self.parent()?)?;
        flush_complete(self.parent()?)?;
        #[cfg(test)]
        observation::observe(observation::Phase::AfterParentFlush, self.parent()?)?;
        let stored = self.read(name)?;
        if stored != bytes {
            return Err(refusal("published private bytes changed"));
        }
        Ok(true)
    }
    pub(crate) fn is_empty(&self) -> NativeResult<bool> {
        self.namespace.verify()?;
        let last = self
            .namespace
            .names
            .last()
            .ok_or_else(|| refusal("missing private name"))?;
        let ancestor = self
            .namespace
            .files
            .iter()
            .rev()
            .nth(1)
            .ok_or_else(|| refusal("missing private ancestor"))?;
        let iterator = relative_access(
            ancestor,
            last,
            true,
            None,
            FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        )?;
        if inspect(&iterator, true)? != inspect(self.parent()?, true)? {
            return Err(refusal("private directory iterator identity changed"));
        }
        let mut restart = true;
        loop {
            // usize storage is aligned for the SDK header; only initialized returned bytes are read.
            let mut buffer = vec![0usize; 1024];
            let mut io = IO_STATUS_BLOCK::default();
            // SAFETY: synchronous live directory, SDK information class, aligned writable buffer.
            let status = unsafe {
                NtQueryDirectoryFile(
                    HANDLE(iterator.as_raw_handle()),
                    HANDLE::default(),
                    None,
                    None,
                    &raw mut io,
                    buffer.as_mut_ptr().cast(),
                    u32::try_from(buffer.len() * size_of::<usize>())
                        .map_err(|_| refusal("directory buffer overflow"))?,
                    FileNamesInformation,
                    BOOLEAN(1),
                    None,
                    BOOLEAN(u8::from(restart)),
                )
            };
            restart = false;
            if status == STATUS_NO_MORE_FILES {
                self.namespace.verify()?;
                return Ok(true);
            }
            completed(status, &io)?;
            let used = io.Information;
            let offset = std::mem::offset_of!(FILE_NAMES_INFORMATION, FileName);
            if used < offset || used > buffer.len() * size_of::<usize>() {
                return Err(refusal("invalid private directory result extent"));
            }
            // SAFETY: initialized returned header is aligned and its fixed fields are bounded.
            let header = unsafe { &*buffer.as_ptr().cast::<FILE_NAMES_INFORMATION>() };
            let length = usize::try_from(header.FileNameLength)
                .map_err(|_| refusal("directory name overflow"))?;
            if header.NextEntryOffset != 0
                || !length.is_multiple_of(2)
                || offset.checked_add(length).is_none_or(|end| end > used)
            {
                return Err(refusal("invalid private directory entry"));
            }
            // SAFETY: bounded even UTF-16 extent follows the initialized header.
            let name = unsafe {
                std::slice::from_raw_parts(
                    buffer.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
                    length / 2,
                )
            };
            if name != [46] && name != [46, 46] {
                self.namespace.verify()?;
                return Ok(false);
            }
        }
    }
}

fn private_open(
    parent: &File,
    name: &OsStr,
    creation: Option<&super::super::security::CreationSecurity>,
    writable: bool,
) -> NativeResult<File> {
    // Rename holds DELETE on the source. Readers permit that live publisher handle,
    // and independently revalidate original IDs/security around every read.
    relative_access_with_sharing(
        parent,
        name,
        false,
        creation,
        FILE_GENERIC_READ
            | FILE_GENERIC_WRITE
            | READ_CONTROL
            | SYNCHRONIZE
            | if writable { DELETE } else { Default::default() },
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        creation.is_some(),
    )
}
fn completed(status: NTSTATUS, io: &IO_STATUS_BLOCK) -> NativeResult<()> {
    if status == STATUS_PENDING {
        std::process::abort();
    }
    if status != STATUS_SUCCESS {
        return Err(NativeError::Open(status));
    }
    // SAFETY: synchronous successful call initialized the status union arm.
    let final_status = unsafe { io.Anonymous.Status };
    if final_status == STATUS_PENDING {
        std::process::abort();
    }
    if final_status != STATUS_SUCCESS {
        return Err(NativeError::Open(final_status));
    }
    Ok(())
}
fn rename_no_replace(file: &File, parent: &File, name: &OsStr) -> NativeResult<()> {
    let name = component(name)?;
    let bytes = name
        .len()
        .checked_mul(2)
        .ok_or_else(|| refusal("rename name overflow"))?;
    let offset = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let size = offset
        .checked_add(bytes)
        .ok_or_else(|| refusal("rename buffer overflow"))?
        .max(size_of::<FILE_RENAME_INFORMATION>());
    // Aligned allocation, with initialized padding and checked flexible-array extent.
    let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
    let header = FILE_RENAME_INFORMATION {
        RootDirectory: HANDLE(parent.as_raw_handle()),
        FileNameLength: u32::try_from(bytes).map_err(|_| refusal("rename length overflow"))?,
        ..Default::default()
    };
    // SAFETY: allocation alignment and extent cover SDK header and entire UTF-16 tail.
    unsafe {
        storage
            .as_mut_ptr()
            .cast::<FILE_RENAME_INFORMATION>()
            .write(header);
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            storage.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>(),
            name.len(),
        );
    }
    let mut io = IO_STATUS_BLOCK::default();
    // SAFETY: live DELETE-capable source and retained destination parent, initialized SDK buffer.
    let status = unsafe {
        NtSetInformationFile(
            HANDLE(file.as_raw_handle()),
            &raw mut io,
            storage.as_ptr().cast(),
            u32::try_from(size).map_err(|_| refusal("rename buffer overflow"))?,
            FileRenameInformation,
        )
    };
    completed(status, &io)
}
/// Separate state prevents published objects from reaching staging cleanup.
struct PublishedFile(File);
fn combine(primary: Option<NativeError>, secondary: NativeResult<()>) -> NativeResult<()> {
    match (primary, secondary) {
        (None, result) => result,
        (Some(primary), Ok(())) => Err(primary),
        (Some(primary), Err(cleanup)) => Err(NativeError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}
fn cleanup_pending(file: File, parent: &File) -> NativeResult<()> {
    let disposition = remove_original(&file);
    // Explicit close completes POSIX unlink even when a compatible reader remains.
    // It consumes the sole descriptor; a failed native close is never retried by Drop.
    let closed = close_checked(file);
    let close_succeeded = closed.is_ok();
    let cleanup = combine(disposition.err(), closed);
    if close_succeeded {
        combine(cleanup.err(), cleanup_barrier(parent))
    } else {
        cleanup
    }
}
fn cleanup_barrier(parent: &File) -> NativeResult<()> {
    #[cfg(test)]
    observation::observe(observation::Phase::CleanupBarrier, parent)?;
    flush_complete(parent)
}

#[cfg(test)]
pub(crate) mod observation {
    use super::*;
    use std::{cell::RefCell, marker::PhantomData, rc::Rc};
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Phase {
        BeforeWrite,
        AfterWrite,
        AfterFileFlush,
        BeforePublishParentFlush,
        AfterParentFlush,
        ReadFileBarrier,
        ReadParentBarrier,
        Published,
        CleanupBarrier,
        PreparationFileBarrier,
        PreparationParentBarrier,
        PrivateDirectoryBarrier,
        PrivateEstablishmentCommitted,
    }
    type Callback = Box<dyn FnMut(Phase, &File) -> NativeResult<()>>;
    thread_local! { static OBSERVER: RefCell<Option<Callback>> = const { RefCell::new(None) }; }
    pub(crate) struct Registration(PhantomData<Rc<()>>);
    impl Drop for Registration {
        fn drop(&mut self) {
            OBSERVER.with(|slot| {
                slot.borrow_mut().take();
            });
        }
    }
    pub(crate) fn register(
        callback: impl FnMut(Phase, &File) -> NativeResult<()> + 'static,
    ) -> Registration {
        OBSERVER.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(slot.is_none());
            *slot = Some(Box::new(callback));
        });
        Registration(PhantomData)
    }
    pub(crate) fn crash_child(home: &Path, filter: &str, phase: &str) {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", filter, "--nocapture"])
            .env("SURGE_PRIVATE_CRASH_HOME", home)
            .env("SURGE_PRIVATE_CRASH_PHASE", phase)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert_eq!(
                    status.code(),
                    Some(91),
                    "child failed before requested death boundary {phase}"
                );
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("child timed out at {phase}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    pub(super) fn observe(phase: Phase, file: &File) -> NativeResult<()> {
        OBSERVER.with(|slot| match slot.borrow_mut().as_mut() {
            Some(callback) => callback(phase, file),
            None => Ok(()),
        })
    }
}
fn close_checked(file: File) -> NativeResult<()> {
    use std::os::windows::io::IntoRawHandle;
    let handle = HANDLE(file.into_raw_handle());
    // SAFETY: consumes the only owned handle exactly once. On failure ownership is
    // explicitly uncertain; no automatic destructor or second close can run.
    unsafe { windows::Win32::Foundation::CloseHandle(handle) }.map_err(NativeError::CloseUncertain)
}
fn remove_original(file: &File) -> NativeResult<()> {
    use windows::Win32::Storage::FileSystem::FILE_DISPOSITION_FLAG_POSIX_SEMANTICS;
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: windows::Win32::Storage::FileSystem::FILE_DISPOSITION_INFO_EX_FLAGS(
            FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0,
        ),
    };
    // SAFETY: only the held original DELETE-capable staging handle is affected.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileDispositionInfoEx,
            (&raw const disposition).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO_EX>())
                .map_err(|_| refusal("disposition size overflow"))?,
        )
    }?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_home_fixture::FixtureHome;

    #[test]
    fn fresh_and_reused_private_directory_barrier_failure_returns_no_owner() {
        for occurrence in [1, 2] {
            for reused in [false, true] {
                for preparation in [false, true] {
                    directory_barrier_case(occurrence, reused, preparation);
                }
            }
        }
    }

    fn directory_barrier_case(occurrence: usize, reused: bool, preparation: bool) {
        let home = FixtureHome::new().unwrap();
        let open = || {
            if preparation {
                PrivatePreparation::acquire(home.path(), "directory-barrier").map(drop)
            } else {
                PrivateFiles::open(home.path(), true).map(drop)
            }
        };
        if reused {
            open().unwrap();
        }
        let mut seen = 0;
        let fault = observation::register(move |phase, _| {
            if phase == observation::Phase::PrivateDirectoryBarrier {
                seen += 1;
                if seen == occurrence {
                    return Err(refusal("injected directory publication barrier"));
                }
            }
            Ok(())
        });
        assert!(open().is_err());
        drop(fault);
        open().unwrap();
        home.close().unwrap();
    }

    #[test]
    fn preparation_barrier_failure_returns_no_identity_or_private_capability_and_keeps_exclusion() {
        let home = FixtureHome::new().unwrap();
        for phase in [
            observation::Phase::PreparationFileBarrier,
            observation::Phase::PreparationParentBarrier,
        ] {
            let acquisition_fault = observation::register(move |current, _| {
                if current == phase {
                    Err(refusal("injected first acquisition barrier"))
                } else {
                    Ok(())
                }
            });
            assert!(PrivatePreparation::acquire(home.path(), "barrier-owner").is_err());
            drop(acquisition_fault);
            let owner = PrivatePreparation::acquire(home.path(), "barrier-owner").unwrap();
            let original = owner.legacy_identity().unwrap();
            let fault = observation::register(move |current, _| {
                if current == phase {
                    Err(refusal("injected preparation barrier"))
                } else {
                    Ok(())
                }
            });
            assert!(owner.legacy_identity().is_err());
            assert!(owner.require_private().is_err());
            assert!(matches!(
                PrivatePreparation::acquire(home.path(), "barrier-owner"),
                Err(NativeError::Busy)
            ));
            drop(fault);
            assert_eq!(owner.legacy_identity().unwrap(), original);
            drop(owner);
        }
        home.close().unwrap();
    }

    #[test]
    fn failed_first_staging_security_check_cleans_original_before_returning() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        let root = home.path().join("work-items/private-flow-inputs");
        let created = crate::state_home::windows::creation_observation::register(
            move |_, name, directory, _| {
                if !directory && name.to_string_lossy().starts_with(".pending-") {
                    let status = std::process::Command::new("icacls")
                        .arg(root.join(name))
                        .args(["/grant", "*S-1-1-0:R"])
                        .output()
                        .unwrap();
                    assert!(status.status.success());
                }
            },
        );
        assert!(files.publish("never-published", b"private bytes").is_err());
        assert!(files.is_empty().unwrap());
        drop(created);
        drop(files);
        home.close().unwrap();
    }

    #[test]
    fn collision_disposes_original_link_before_parent_barrier_with_reader_retained() {
        use std::{cell::RefCell, rc::Rc};
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        files.publish("winner", b"original winner").unwrap();
        let reader = Rc::new(RefCell::new(None));
        let pending_path = Rc::new(RefCell::new(None));
        let captured = reader.clone();
        let pending = pending_path.clone();
        let root = home.path().join("work-items/private-flow-inputs");
        let created = crate::state_home::windows::creation_observation::register(
            move |parent, name, directory, _| {
                if !directory && name.to_string_lossy().starts_with(".pending-") {
                    *captured.borrow_mut() = Some(private_open(parent, name, None, false).unwrap());
                    *pending.borrow_mut() = Some(root.join(name));
                }
            },
        );
        let observed = Rc::new(RefCell::new(false));
        let seen = observed.clone();
        let barrier = observation::register(move |phase, _| {
            if phase == observation::Phase::CleanupBarrier {
                assert!(!pending_path.borrow().as_ref().unwrap().exists());
                let mut bytes = Vec::new();
                captured_read(&reader, &mut bytes);
                assert_eq!(bytes, b"losing complete bytes");
                *seen.borrow_mut() = true;
            }
            Ok(())
        });
        assert!(!files.publish("winner", b"losing complete bytes").unwrap());
        assert!(*observed.borrow());
        assert_eq!(files.read("winner").unwrap(), b"original winner");
        drop(barrier);
        drop(created);
        drop(files);
        home.close().unwrap();
    }
    fn captured_read(reader: &std::rc::Rc<std::cell::RefCell<Option<File>>>, bytes: &mut Vec<u8>) {
        reader
            .borrow_mut()
            .as_mut()
            .unwrap()
            .read_to_end(bytes)
            .unwrap();
    }
    #[test]
    fn after_rename_failure_preserves_final_and_prepublication_failure_retains_cleanup_error() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        let after = observation::register(|phase, _| {
            if phase == observation::Phase::Published {
                Err(refusal("injected after rename"))
            } else {
                Ok(())
            }
        });
        assert!(files.publish("published", b"must survive").is_err());
        drop(after);
        assert_eq!(files.read("published").unwrap(), b"must survive");
        let before = observation::register(|phase, _| match phase {
            observation::Phase::BeforeWrite => Err(refusal("injected before write")),
            observation::Phase::CleanupBarrier => Err(refusal("injected cleanup barrier")),
            _ => Ok(()),
        });
        let error = files
            .publish("never-published", b"private input")
            .unwrap_err();
        assert!(matches!(error, NativeError::Cleanup { .. }));
        let message = error.to_string();
        assert!(message.contains("injected before write"));
        assert!(message.contains("injected cleanup barrier"));
        drop(before);
        assert!(files.read("never-published").is_err());
        assert_eq!(
            std::fs::read_dir(home.path().join("work-items/private-flow-inputs"))
                .unwrap()
                .count(),
            1
        );
        drop(files);
        home.close().unwrap();
    }

    #[test]
    fn publication_is_complete_immutable_and_concurrent_losers_read_the_winner() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        assert!(files.is_empty().unwrap());
        assert!(files.read("missing").is_err());
        assert!(files.publish("first", b"complete original").unwrap());
        assert!(!files.is_empty().unwrap());
        assert!(!files.publish("first", b"must never replace").unwrap());
        assert_eq!(files.read("first").unwrap(), b"complete original");
        let path = home.path().to_owned();
        let outcomes = std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..4)
                .map(|index| {
                    let path = &path;
                    scope.spawn(move || {
                        let files = PrivateFiles::open(path, false).unwrap();
                        files.publish("concurrent", &[index; 4096]).unwrap()
                    })
                })
                .collect();
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(outcomes.iter().filter(|value| **value).count(), 1);
        let winner = files.read("concurrent").unwrap();
        assert_eq!(winner.len(), 4096);
        assert!(winner.iter().all(|byte| *byte == winner[0]));
        assert_eq!(
            std::fs::read_dir(home.path().join("work-items/private-flow-inputs"))
                .unwrap()
                .count(),
            2
        );
        drop(files);
        home.close().unwrap();
    }
    #[test]
    fn private_inputs_refuse_aliases_hardlinks_and_unsafe_existing_acl_without_repair() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        assert!(files.publish("original", b"untouched").unwrap());
        for name in ["../escaped", "a:b", "a/b", "a\\b", ".", "CON", "trailing."] {
            assert!(files.publish(name, b"never written").is_err());
            assert!(files.read(name).is_err());
        }
        let parent = home.path().join("work-items/private-flow-inputs");
        std::fs::hard_link(parent.join("original"), parent.join("alias")).unwrap();
        assert!(files.read("original").is_err());
        std::fs::remove_file(parent.join("alias")).unwrap();
        let status = std::process::Command::new("icacls")
            .arg(parent.join("original"))
            .args(["/grant", "*S-1-1-0:R"])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(files.read("original").is_err());
        assert!(files.publish("original", b"must not repair").is_err());
        assert_eq!(
            std::fs::read(parent.join("original")).unwrap(),
            b"untouched"
        );
        drop(files);
        home.close().unwrap();
    }

    #[test]
    fn private_size_bounds_delete_pending_and_retained_parent_replacement_fail_closed() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        let parent = home.path().join("work-items/private-flow-inputs");
        assert!(std::fs::rename(&parent, home.path().join("replacement-private")).is_err());
        assert!(
            files
                .publish("oversize", &vec![0; LIMIT as usize + 1])
                .is_err()
        );
        assert!(files.is_empty().unwrap());
        files.publish("too-large-to-read", b"initial").unwrap();
        let oversized = OpenOptions::new()
            .write(true)
            .open(parent.join("too-large-to-read"))
            .unwrap();
        oversized.set_len(LIMIT + 1).unwrap();
        drop(oversized);
        assert!(files.read("too-large-to-read").is_err());
        files.publish("delete-pending", b"original").unwrap();
        let deleting = private_open(
            files.parent().unwrap(),
            OsStr::new("delete-pending"),
            None,
            true,
        )
        .unwrap();
        mark_delete_pending(&deleting);
        assert!(files.read("delete-pending").is_err());
        assert!(
            files
                .publish("delete-pending", b"must not replace")
                .is_err()
        );
        drop(deleting);
        assert!(!parent.join("delete-pending").exists());
        drop(files);
        home.close().unwrap();
    }
    fn mark_delete_pending(file: &File) {
        use windows::Win32::Storage::FileSystem::{
            FILE_STANDARD_INFO, FileStandardInfo, GetFileInformationByHandleEx,
        };
        // DELETE_ON_CLOSE is only a future cleanup intent. Explicit non-POSIX
        // disposition creates the actual DeletePending state required by this oracle.
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE,
        };
        // SAFETY: a retained DELETE-capable original and initialized SDK input.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfoEx,
                (&raw const disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        }
        .unwrap();
        let mut standard = FILE_STANDARD_INFO::default();
        // SAFETY: initialized, correctly sized SDK output and a live original handle.
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileStandardInfo,
                (&raw mut standard).cast(),
                size_of::<FILE_STANDARD_INFO>() as u32,
            )
        }
        .unwrap();
        assert_ne!(
            standard.DeletePending.0, 0,
            "SDK DeletePending must be TRUE, observed {}",
            standard.DeletePending.0
        );
    }

    #[test]
    fn moved_staging_cleanup_disposes_only_original_before_barrier() {
        use std::{cell::RefCell, rc::Rc};
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        files.publish("final", b"final sentinel").unwrap();
        let root = home.path().join("work-items/private-flow-inputs");
        let pending = Rc::new(RefCell::new(None::<OsString>));
        let captured = pending.clone();
        let created = crate::state_home::windows::creation_observation::register(
            move |_, name, directory, _| {
                if !directory && captured.borrow().is_none() {
                    *captured.borrow_mut() = Some(name.to_owned());
                }
            },
        );
        let observed = Rc::new(RefCell::new(None::<Identity>));
        let checked = observed.clone();
        let barrier_seen = Rc::new(std::cell::Cell::new(false));
        let reached_barrier = barrier_seen.clone();
        let competing = PrivateFiles::open(home.path(), false).unwrap();
        let fault = observation::register(move |phase, original| {
            let pending = pending.borrow();
            let name = pending.as_ref().unwrap();
            if phase == observation::Phase::BeforeWrite {
                let original_id = inspect(original, false).unwrap();
                std::fs::rename(root.join(name), root.join("moved-original")).unwrap();
                let creation = competing.namespace.owner.creation(false).unwrap();
                let mut substitute =
                    private_open(competing.parent().unwrap(), name, Some(&creation), true).unwrap();
                substitute.write_all(b"substitute sentinel").unwrap();
                flush_complete(&substitute).unwrap();
                let substitute_id = competing.validate(&substitute).unwrap();
                assert!(substitute_id != original_id);
                *checked.borrow_mut() = Some(substitute_id);
                return Err(refusal("force original-only cleanup"));
            }
            if phase == observation::Phase::CleanupBarrier {
                reached_barrier.set(true);
                assert!(!root.join("moved-original").exists());
                let substitute = competing.opened(name).unwrap();
                assert!(
                    competing.validate(&substitute).unwrap() == *checked.borrow().as_ref().unwrap()
                );
                assert_eq!(
                    std::fs::read(root.join(name)).unwrap(),
                    b"substitute sentinel"
                );
                assert_eq!(
                    std::fs::read(root.join("final")).unwrap(),
                    b"final sentinel"
                );
            }
            Ok(())
        });
        assert!(matches!(
            files.publish("unpublished", b"never public"),
            Err(NativeError::Security("force original-only cleanup"))
        ));
        assert!(observed.borrow().is_some());
        assert!(
            barrier_seen.get(),
            "cleanup must reach its checked-close parent barrier"
        );
        drop(fault);
        drop(created);
        drop(files);
        home.close().unwrap();
    }

    fn junction(link: &Path, target: &Path) {
        use std::os::windows::{ffi::OsStringExt, fs::MetadataExt};
        // Windows accepts either separator, but cmd's built-in mklink parses '/'
        // as a switch. Normalize command arguments without lossy Unicode conversion.
        let argument = |path: &Path| {
            OsString::from_wide(
                &path
                    .as_os_str()
                    .encode_wide()
                    .map(|unit| {
                        if unit == u16::from(b'/') {
                            u16::from(b'\\')
                        } else {
                            unit
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let result = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(argument(link))
            .arg(argument(target))
            .output()
            .unwrap();
        assert!(result.status.success(), "junction creation: {:?}", result);
        assert_ne!(
            std::fs::symlink_metadata(link).unwrap().file_attributes()
                & FILE_ATTRIBUTE_REPARSE_POINT.0,
            0,
            "created junction must be an actual reparse point"
        );
    }

    #[test]
    fn final_and_private_ancestor_reparse_points_are_refused_without_following_target() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        let root = home.path().join("work-items/private-flow-inputs");
        let target = home.path().join("junction-target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"unchanged target").unwrap();
        junction(&root.join("reparse"), &target);
        assert!(files.read("reparse").is_err());
        assert!(files.publish("reparse", b"never replace target").is_err());
        std::fs::remove_dir(root.join("reparse")).unwrap();
        drop(files);
        let saved = home.path().join("saved-private-inputs");
        std::fs::rename(&root, &saved).unwrap();
        junction(&root, &target);
        assert!(PrivateFiles::open(home.path(), false).is_err());
        assert!(PrivateFiles::open(home.path(), true).is_err());
        assert_eq!(
            std::fs::read(target.join("sentinel")).unwrap(),
            b"unchanged target"
        );
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 1);
        std::fs::remove_dir(&root).unwrap();
        std::fs::rename(saved, root).unwrap();
        home.close().unwrap();
    }

    #[test]
    fn optional_root_read_never_reclassifies_late_disappearance_as_missing() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open_root(home.path()).unwrap();
        assert!(files.read_optional("route-key").unwrap().is_none());
        files.publish("route-key", b"original key bytes").unwrap();
        let original = home.path().join("route-key");
        let moved = home.path().join("held-original");
        let from = original.clone();
        let to = moved.clone();
        let hook = observation::register(move |phase, _| {
            if phase == observation::Phase::ReadFileBarrier {
                std::fs::rename(&from, &to)?;
            }
            Ok(())
        });
        assert!(matches!(
            files.read_optional("route-key"),
            Err(NativeError::Open(
                windows::Win32::Foundation::STATUS_OBJECT_NAME_NOT_FOUND
            ))
        ));
        drop(hook);
        assert_eq!(std::fs::read(&moved).unwrap(), b"original key bytes");
        assert!(!original.exists());
        drop(files);
        home.close().unwrap();
    }

    #[test]
    fn independent_final_reader_completes_own_barriers_before_publisher_parent_flush() {
        let home = FixtureHome::new().unwrap();
        let files = PrivateFiles::open(home.path(), true).unwrap();
        let root = home.path().to_owned();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = seen.clone();
        let hook = observation::register(move |phase, _| {
            if phase == observation::Phase::BeforePublishParentFlush {
                let root = root.clone();
                std::thread::spawn(move || read_final_with_own_barriers(&root))
                    .join()
                    .unwrap();
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        });
        files
            .publish("complete-final", b"durable final bytes")
            .unwrap();
        assert!(seen.load(std::sync::atomic::Ordering::SeqCst));
        drop(hook);
        drop(files);
        home.close().unwrap();
    }

    fn read_final_with_own_barriers(root: &Path) {
        let barriers = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let checked = barriers.clone();
        let hook = observation::register(move |phase, _| {
            if matches!(
                phase,
                observation::Phase::ReadFileBarrier | observation::Phase::ReadParentBarrier
            ) {
                checked.borrow_mut().push(phase);
            }
            Ok(())
        });
        let reader = PrivateFiles::open(root, false).unwrap();
        assert_eq!(
            reader.read("complete-final").unwrap(),
            b"durable final bytes"
        );
        assert!(
            *barriers.borrow()
                == [
                    observation::Phase::ReadFileBarrier,
                    observation::Phase::ReadParentBarrier
                ]
        );
        drop(hook);
    }

    #[test]
    fn subprocess_death_at_publication_boundaries_never_accepts_partial_final() {
        const FILTER: &str = "state_home::windows::namespace::private_objects::tests::subprocess_death_at_publication_boundaries_never_accepts_partial_final";
        if let Some(home) = std::env::var_os("SURGE_PRIVATE_CRASH_HOME") {
            let phase = std::env::var("SURGE_PRIVATE_CRASH_PHASE").unwrap();
            let hook = observation::register(move |current, _| {
                if crash_phase(current) == phase {
                    std::process::exit(91);
                }
                Ok(())
            });
            PrivateFiles::open(Path::new(&home), true)
                .unwrap()
                .publish("crash-final", b"complete crash bytes")
                .unwrap();
            drop(hook);
            panic!("crash boundary not reached");
        }
        for phase in [
            "before-write",
            "after-write",
            "after-file-flush",
            "after-rename",
            "before-parent-flush",
            "after-parent-flush",
        ] {
            let home = FixtureHome::new().unwrap();
            observation::crash_child(home.path(), FILTER, phase);
            let files = PrivateFiles::open(home.path(), false).unwrap();
            if matches!(phase, "before-write" | "after-write" | "after-file-flush") {
                assert!(files.read("crash-final").is_err());
            } else {
                assert_eq!(files.read("crash-final").unwrap(), b"complete crash bytes");
            }
            drop(files);
            home.close().unwrap();
        }
    }
    fn crash_phase(phase: observation::Phase) -> &'static str {
        match phase {
            observation::Phase::BeforeWrite => "before-write",
            observation::Phase::AfterWrite => "after-write",
            observation::Phase::AfterFileFlush => "after-file-flush",
            observation::Phase::Published => "after-rename",
            observation::Phase::BeforePublishParentFlush => "before-parent-flush",
            observation::Phase::AfterParentFlush => "after-parent-flush",
            _ => "unrelated",
        }
    }
    #[test]
    fn killed_native_preparation_owner_releases_os_lock_without_changing_original_identity() {
        const FILTER: &str = "state_home::windows::namespace::private_objects::tests::killed_native_preparation_owner_releases_os_lock_without_changing_original_identity";
        if let Some(home) = std::env::var_os("SURGE_PRIVATE_CRASH_HOME") {
            let hook = observation::register(|phase, _| {
                if phase == observation::Phase::PreparationParentBarrier {
                    // acquire has already obtained the actual OS lock; exit skips Drop.
                    std::process::exit(91);
                }
                Ok(())
            });
            let owner = PrivatePreparation::acquire(Path::new(&home), "killed-owner").unwrap();
            drop(owner);
            drop(hook);
            panic!("locked native owner boundary not reached");
        }
        let home = FixtureHome::new().unwrap();
        let owner = PrivatePreparation::acquire(home.path(), "killed-owner").unwrap();
        let original = owner.legacy_identity().unwrap();
        drop(owner);
        observation::crash_child(home.path(), FILTER, "native-owner-held");
        let owner = PrivatePreparation::acquire(home.path(), "killed-owner").unwrap();
        assert_eq!(owner.legacy_identity().unwrap(), original);
        owner.require_private().unwrap();
        drop(owner);
        home.close().unwrap();
    }
}
