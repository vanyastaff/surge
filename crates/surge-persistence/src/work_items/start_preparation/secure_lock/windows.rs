//! Windows native ownership: descendants are resolved relative to held parent handles.
use super::{Result, WorkItemError};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    mem::size_of,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};
use windows::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NTCREATEFILE_CREATE_DISPOSITION,
            NtCreateFile,
        },
    },
    Win32::{
        Foundation::{
            HANDLE, NTSTATUS, STATUS_OBJECT_NAME_COLLISION, STATUS_PENDING,
            STATUS_SHARING_VIOLATION, STATUS_SUCCESS, UNICODE_STRING,
        },
        Storage::FileSystem::{
            FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
            FILE_GENERIC_WRITE, FILE_ID_INFO, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TYPE_DISK,
            FileAttributeTagInfo, FileIdInfo, FileStandardInfo, GETFINALPATHNAMEBYHANDLE_FLAGS,
            GetFileInformationByHandleEx, GetFileType, GetFinalPathNameByHandleW, SYNCHRONIZE,
            VOLUME_NAME_DOS,
        },
        System::{
            IO::IO_STATUS_BLOCK,
            Kernel::{OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE},
        },
    },
};
fn invalid() -> WorkItemError {
    WorkItemError::Invalid("unsafe stable preparation ownership".into())
}
fn handle(file: &File) -> HANDLE {
    HANDLE(file.as_raw_handle())
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Directory,
    Lock,
    Source,
}
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
struct Identity {
    volume: u64,
    file: [u8; 16],
}
fn attributes(file: &File) -> Result<FILE_ATTRIBUTE_TAG_INFO> {
    let mut output = FILE_ATTRIBUTE_TAG_INFO::default();
    // SAFETY: live borrowed handle; this exact SDK class writes this initialized
    // SDK structure, whose actual size is supplied. Read only after success.
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileAttributeTagInfo,
            (&raw mut output).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    }
    .map_err(|_| invalid())?;
    Ok(output)
}
fn standard(file: &File) -> Result<FILE_STANDARD_INFO> {
    let mut output = FILE_STANDARD_INFO::default();
    // SAFETY: class and output are the matching SDK FILE_STANDARD_INFO pair;
    // initialized writable storage remains live through the synchronous query.
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileStandardInfo,
            (&raw mut output).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        )
    }
    .map_err(|_| invalid())?;
    Ok(output)
}
fn file_identity(file: &File) -> Result<Identity> {
    let mut output = FILE_ID_INFO::default();
    // SAFETY: class and output are the matching SDK FILE_ID_INFO pair, supplied
    // with actual size; inspected only after the API reports success.
    unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileIdInfo,
            (&raw mut output).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .map_err(|_| invalid())?;
    Ok(Identity {
        volume: output.VolumeSerialNumber,
        file: output.FileId.Identifier,
    })
}
fn final_path(file: &File) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: live borrowed handle and initialized bounded writable UTF-16 buffer.
    // The SDK wrapper supplies its length. Failed/truncated results are not read.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle(file),
            &mut buffer,
            // windows 0.58 flag newtypes do not implement BitOr.
            GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(invalid());
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..length]));
    root(&path)?;
    Ok(path)
}
fn check(file: &File, kind: Kind) -> Result<Identity> {
    // SAFETY: file owns a live disk handle; this query does not acquire ownership.
    if unsafe { GetFileType(handle(file)) } != FILE_TYPE_DISK {
        return Err(invalid());
    }
    let tags = attributes(file)?;
    let standard = standard(file)?;
    if tags.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || tags.ReparseTag != 0
        || standard.DeletePending.0 != 0
        || (standard.Directory.0 != 0) != (kind == Kind::Directory)
        || (kind == Kind::Lock && standard.NumberOfLinks != 1)
    {
        return Err(invalid());
    }
    file_identity(file)
}
fn component(name: &OsStr) -> Result<Vec<u16>> {
    let encoded: Vec<u16> = name.encode_wide().collect();
    if encoded.is_empty()
        || encoded == [46]
        || encoded == [46, 46]
        || encoded.iter().any(|c| matches!(*c, 0 | 47 | 92 | 58))
        || encoded
            .len()
            .checked_mul(2)
            .and_then(|n| u16::try_from(n).ok())
            .is_none()
    {
        return Err(invalid());
    }
    Ok(encoded)
}
// This is the sole native handle-creation ownership boundary. All descendants,
// including verification reopens, use one component and the retained parent.
fn native_open(
    parent: &File,
    name: &OsStr,
    kind: Kind,
    disposition: NTCREATEFILE_CREATE_DISPOSITION,
) -> std::result::Result<File, NTSTATUS> {
    let mut name =
        component(name).map_err(|_| windows::Win32::Foundation::STATUS_OBJECT_NAME_INVALID)?;
    let length = u16::try_from(name.len() * 2)
        .map_err(|_| windows::Win32::Foundation::STATUS_OBJECT_NAME_INVALID)?;
    let unicode = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: windows::core::PWSTR(name.as_mut_ptr()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: handle(parent),
        ObjectName: &raw const unicode,
        Attributes: (OBJ_DONT_REPARSE | OBJ_CASE_INSENSITIVE) as u32,
        ..Default::default()
    };
    let mut output = HANDLE::default();
    let mut io = IO_STATUS_BLOCK::default();
    let access = match kind {
        Kind::Directory => FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        Kind::Lock => FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        Kind::Source => FILE_GENERIC_READ,
    };
    let shares = if kind == Kind::Source {
        FILE_SHARE_READ
    } else {
        FILE_SHARE_READ | FILE_SHARE_WRITE
    };
    let options = FILE_SYNCHRONOUS_IO_NONALERT
        | FILE_OPEN_REPARSE_POINT
        | if kind == Kind::Directory {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
    // SAFETY: SDK ABI/types/constants; all pointer storage is initialized and remains
    // live for this synchronous call. RootDirectory is borrowed, never transferred.
    // SYNCHRONIZE accompanies NONALERT. No inheritance or asynchronous options.
    let status = unsafe {
        NtCreateFile(
            &raw mut output,
            access,
            &raw const attributes,
            &raw mut io,
            None,
            FILE_ATTRIBUTE_NORMAL,
            shares,
            disposition,
            options,
            None,
            0,
        )
    };
    if status == STATUS_PENDING {
        // A conforming synchronous disk open cannot return pending. Abort keeps
        // borrowed/output storage alive until process termination rather than
        // dropping memory an unexpected unfinished kernel operation could reference.
        // This is an invariant containment path, never a capability or retry.
        std::process::abort();
    }
    if status != STATUS_SUCCESS {
        return Err(status);
    }
    if output.is_invalid() {
        return Err(windows::Win32::Foundation::STATUS_INVALID_HANDLE);
    }
    // SAFETY: completed STATUS_SUCCESS initialized a valid fresh owned handle.
    // It is transferred exactly once into OwnedHandle then File; errors thereafter
    // use RAII. The borrowed parent is never closed or cloned.
    let owned = unsafe { OwnedHandle::from_raw_handle(output.0) };
    Ok(File::from(owned))
}
fn opened(parent: &File, name: &OsStr, kind: Kind) -> Result<File> {
    native_open(parent, name, kind, FILE_OPEN).map_err(|_| invalid())
}
fn create_or_open(parent: &File, name: &OsStr, kind: Kind) -> Result<File> {
    match native_open(parent, name, kind, FILE_CREATE) {
        Ok(file) => Ok(file),
        Err(STATUS_OBJECT_NAME_COLLISION) => opened(parent, name, kind),
        Err(STATUS_SHARING_VIOLATION) if kind == Kind::Lock => Err(WorkItemError::Busy),
        Err(_) => Err(invalid()),
    }
}
fn root(path: &Path) -> Result<(PathBuf, Vec<OsString>)> {
    let mut parts = path.components();
    let prefix = match parts.next() {
        Some(Component::Prefix(prefix)) => prefix,
        _ => return Err(invalid()),
    };
    match prefix.kind() {
        Prefix::Disk(_)
        | Prefix::VerbatimDisk(_)
        | Prefix::UNC(_, _)
        | Prefix::VerbatimUNC(_, _) => (),
        _ => return Err(invalid()),
    }
    if !matches!(parts.next(), Some(Component::RootDir)) {
        return Err(invalid());
    }
    let mut root = PathBuf::from(prefix.as_os_str());
    root.push(r"\");
    let mut names = Vec::new();
    for part in parts {
        match part {
            Component::Normal(name) => {
                component(name)?;
                names.push(name.to_owned());
            },
            _ => return Err(invalid()),
        }
    }
    Ok((root, names))
}
fn open_root(path: &Path) -> Result<File> {
    // Only the parsed OS volume/share root uses a Win32 absolute open. No untrusted
    // descendant is concatenated here. All subsequent routing is native-relative.
    let file = OpenOptions::new()
        .access_mode((FILE_READ_ATTRIBUTES | SYNCHRONIZE).0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
        .map_err(|_| invalid())?;
    check(&file, Kind::Directory)?;
    Ok(file)
}
struct Chain {
    root: PathBuf,
    files: Vec<File>,
    names: Vec<OsString>,
    identities: Vec<Identity>,
}
impl Chain {
    fn open(path: &Path) -> Result<Self> {
        let (root, names) = root(path)?;
        let file = open_root(&root)?;
        let id = check(&file, Kind::Directory)?;
        let mut chain = Self {
            root,
            files: vec![file],
            names: Vec::new(),
            identities: vec![id],
        };
        for name in names {
            chain.push_existing(&name)?;
        }
        chain.verify()?;
        Ok(chain)
    }
    fn push_existing(&mut self, name: &OsStr) -> Result<()> {
        let file = opened(
            self.files.last().ok_or_else(invalid)?,
            name,
            Kind::Directory,
        )?;
        self.push(name, file)
    }
    fn push(&mut self, name: &OsStr, file: File) -> Result<()> {
        let id = check(&file, Kind::Directory)?;
        self.names.push(name.into());
        self.files.push(file);
        self.identities.push(id);
        Ok(())
    }
    fn verify(&self) -> Result<()> {
        if check(&open_root(&self.root)?, Kind::Directory)? != self.identities[0] {
            return Err(invalid());
        }
        for (i, file) in self.files.iter().enumerate() {
            if check(file, Kind::Directory)? != self.identities[i] {
                return Err(invalid());
            }
            if i > 0 {
                let named = opened(&self.files[i - 1], &self.names[i - 1], Kind::Directory)?;
                if check(&named, Kind::Directory)? != self.identities[i] {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    fn identity(&self) -> Vec<(Vec<u16>, Identity)> {
        self.identities
            .iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    (if i == 0 {
                        self.root.as_os_str()
                    } else {
                        &self.names[i - 1]
                    })
                    .encode_wide()
                    .collect(),
                    id.clone(),
                )
            })
            .collect()
    }
}
pub(super) struct Held {
    chain: Chain,
    lock: File,
    name: OsString,
    lock_identity: Identity,
}
impl Held {
    pub(super) fn identity(&self) -> Result<String> {
        Ok(serde_json::to_string(&(
            self.chain.identity(),
            self.name.encode_wide().collect::<Vec<_>>(),
            &self.lock_identity,
        ))?)
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.chain.verify()?;
        let named = opened(
            self.chain.files.last().ok_or_else(invalid)?,
            &self.name,
            Kind::Lock,
        )?;
        if check(&self.lock, Kind::Lock)? != self.lock_identity
            || check(&named, Kind::Lock)? != self.lock_identity
        {
            return Err(invalid());
        }
        Ok(())
    }
}
pub(super) fn acquire(home: &Path, name: &str) -> Result<Held> {
    let mut chain = Chain::open(home)?;
    for name in ["work-items", "preparation-locks"] {
        let name = OsStr::new(name);
        let file = create_or_open(
            chain.files.last().ok_or_else(invalid)?,
            name,
            Kind::Directory,
        )?;
        chain.push(name, file)?;
    }
    let name = OsString::from(format!("{name}.lock"));
    let lock = create_or_open(chain.files.last().ok_or_else(invalid)?, &name, Kind::Lock)?;
    let lock_identity = check(&lock, Kind::Lock)?;
    let held = Held {
        chain,
        lock,
        name,
        lock_identity,
    };
    held.verify()?;
    held.lock.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => WorkItemError::Busy,
        std::fs::TryLockError::Error(error) => WorkItemError::Io(error),
    })?;
    held.verify()?;
    Ok(held)
}

pub(in crate::work_items) struct Source {
    base: PathBuf,
    base_index: usize,
    chain: Chain,
    pub(in crate::work_items) file: File,
    name: OsString,
    identity: Identity,
    size: u64,
    modified: std::time::SystemTime,
}
impl Source {
    pub(in crate::work_items) fn capture(base: &Path, locator: &Path) -> Result<(PathBuf, Self)> {
        if locator.as_os_str().is_empty()
            || locator
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(invalid());
        }
        let mut chain = Chain::open(base)?;
        let base_index = chain.files.len() - 1;
        let canonical = final_path(chain.files.last().ok_or_else(invalid)?)?;
        for part in locator.parent().ok_or_else(invalid)?.components() {
            match part {
                Component::Normal(name) => chain.push_existing(name)?,
                _ => return Err(invalid()),
            }
        }
        let name = locator.file_name().ok_or_else(invalid)?.to_owned();
        let file = opened(chain.files.last().ok_or_else(invalid)?, &name, Kind::Source)?;
        let identity = check(&file, Kind::Source)?;
        let metadata = file.metadata()?;
        if metadata.len() > 8 * 1024 * 1024 {
            return Err(invalid());
        }
        let result = Self {
            base: canonical.clone(),
            base_index,
            chain,
            file,
            name,
            identity,
            size: metadata.len(),
            modified: metadata.modified()?,
        };
        result.verify()?;
        Ok((canonical, result))
    }
    pub(in crate::work_items) fn verify(&self) -> Result<()> {
        self.chain.verify()?;
        let base = self.chain.files.get(self.base_index).ok_or_else(invalid)?;
        if final_path(base)? != self.base {
            return Err(invalid());
        }
        let named = opened(
            self.chain.files.last().ok_or_else(invalid)?,
            &self.name,
            Kind::Source,
        )?;
        let metadata = self.file.metadata()?;
        if check(&self.file, Kind::Source)? != self.identity
            || check(&named, Kind::Source)? != self.identity
            || metadata.len() != self.size
            || metadata.modified()? != self.modified
        {
            return Err(invalid());
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_lock_is_real_exclusion_and_does_not_block_ordinary_children() {
        let home = tempfile::tempdir().unwrap();
        let first = acquire(home.path(), "flow-operation-v1-test").unwrap();
        assert!(matches!(
            acquire(home.path(), "flow-operation-v1-test"),
            Err(WorkItemError::Busy)
        ));
        std::fs::write(home.path().join("ordinary-child"), "allowed").unwrap();
        let path = home
            .path()
            .join("work-items/preparation-locks/flow-operation-v1-test.lock");
        assert!(std::fs::rename(&path, home.path().join("substitute")).is_err());
        let original = first.identity().unwrap();
        drop(first);
        let second = acquire(home.path(), "flow-operation-v1-test").unwrap();
        assert_eq!(original, second.identity().unwrap());
        drop(second);
        std::fs::rename(&path, home.path().join("original-lock")).unwrap();
        let third = acquire(home.path(), "flow-operation-v1-test").unwrap();
        assert_ne!(original, third.identity().unwrap());
    }
    struct ChildOwner {
        child: std::process::Child,
        ready: PathBuf,
    }
    impl Drop for ChildOwner {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    impl ChildOwner {
        fn spawn(home: &Path, suffix: &str, gate: &Path) -> Self {
            let ready = home.join(format!("child-{suffix}.ready"));
            let module = module_path!().split_once("::").unwrap().1;
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    &format!("{module}::native_child_owner"),
                    "--nocapture",
                ])
                .env("SURGE_NATIVE_LOCK_TEST_HOME", home)
                .env("SURGE_NATIVE_LOCK_TEST_READY", &ready)
                .env("SURGE_NATIVE_LOCK_TEST_GATE", gate)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
            Self { child, ready }
        }
        fn ready(&mut self) -> String {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                if let Ok(text) = std::fs::read_to_string(&self.ready) {
                    return text;
                }
                assert!(
                    self.child.try_wait().unwrap().is_none(),
                    "native child exited before readiness"
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "native child readiness timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        fn terminate_and_reap(&mut self) {
            self.child.kill().unwrap();
            self.child.wait().unwrap();
        }
    }
    #[test]
    fn native_child_owner() {
        let Some(home) = std::env::var_os("SURGE_NATIVE_LOCK_TEST_HOME") else {
            return;
        };
        let ready = PathBuf::from(std::env::var_os("SURGE_NATIVE_LOCK_TEST_READY").unwrap());
        let gate = PathBuf::from(std::env::var_os("SURGE_NATIVE_LOCK_TEST_GATE").unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !gate.exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let guard = match acquire(Path::new(&home), "flow-operation-v1-cross-process") {
            Ok(guard) => guard,
            Err(WorkItemError::Busy) => {
                write_status(&ready, "busy");
                return;
            },
            Err(error) => panic!("native lock child failed: {error}"),
        };
        write_status(&ready, &format!("held:{}", guard.identity().unwrap()));
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        guard.verify().unwrap();
    }
    #[test]
    fn simultaneous_native_children_have_one_owner_and_death_reclaims_only_original_object() {
        let home = tempfile::tempdir().unwrap();
        let gate = home.path().join("start-gate");
        let mut first = ChildOwner::spawn(home.path(), "one", &gate);
        let mut second = ChildOwner::spawn(home.path(), "two", &gate);
        std::fs::write(&gate, "start").unwrap();
        let first_state = first.ready();
        let second_state = second.ready();
        assert_eq!(
            usize::from(first_state.starts_with("held:"))
                + usize::from(second_state.starts_with("held:")),
            1
        );
        let (held, busy, expected) = if first_state.starts_with("held:") {
            (
                &mut first,
                &mut second,
                first_state.strip_prefix("held:").unwrap(),
            )
        } else {
            (
                &mut second,
                &mut first,
                second_state.strip_prefix("held:").unwrap(),
            )
        };
        assert_eq!(busy.ready(), "busy");
        assert!(matches!(
            acquire(home.path(), "flow-operation-v1-cross-process"),
            Err(WorkItemError::Busy)
        ));
        let locks = home.path().join("work-items/preparation-locks");
        assert!(std::fs::rename(&locks, home.path().join("replaced-parent")).is_err());
        let path = locks.join("flow-operation-v1-cross-process.lock");
        assert!(std::fs::remove_file(&path).is_err());
        held.terminate_and_reap();
        let reacquired = acquire(home.path(), "flow-operation-v1-cross-process").unwrap();
        assert_eq!(expected, reacquired.identity().unwrap());
        drop(reacquired);
        std::fs::rename(&path, home.path().join("original-lock")).unwrap();
        let replacement = acquire(home.path(), "flow-operation-v1-cross-process").unwrap();
        assert_ne!(expected, replacement.identity().unwrap());
    }
    #[test]
    fn changed_parent_after_owner_death_never_has_original_chain_identity() {
        let home = tempfile::tempdir().unwrap();
        let guard = acquire(home.path(), "flow-operation-v1-parent").unwrap();
        let original = guard.identity().unwrap();
        drop(guard);
        let locks = home.path().join("work-items/preparation-locks");
        std::fs::rename(&locks, home.path().join("original-parent")).unwrap();
        let replacement = acquire(home.path(), "flow-operation-v1-parent").unwrap();
        assert_ne!(original, replacement.identity().unwrap());
    }
    #[test]
    fn hardlinked_or_directory_lock_is_refused_without_mutating_its_content() {
        let home = tempfile::tempdir().unwrap();
        drop(acquire(home.path(), "flow-operation-v1-invalid").unwrap());
        let path = home
            .path()
            .join("work-items/preparation-locks/flow-operation-v1-invalid.lock");
        std::fs::write(&path, "never-truncate").unwrap();
        let alias = home.path().join("hardlink-alias");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(acquire(home.path(), "flow-operation-v1-invalid").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"never-truncate");
        std::fs::remove_file(alias).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(acquire(home.path(), "flow-operation-v1-invalid").is_err());
        assert!(path.is_dir());
    }
    #[test]
    fn unicode_and_long_paths_authenticate_the_same_held_objects() {
        let home = tempfile::tempdir().unwrap();
        let canonical = std::fs::canonicalize(home.path()).unwrap();
        let mut path = canonical;
        for _ in 0..6 {
            path.push("資料-long-component-abcdefghijklmnopqrstuvwxyz");
        }
        std::fs::create_dir_all(&path).unwrap();
        assert!(path.as_os_str().encode_wide().count() > 260);
        let original = acquire(&path, "flow-operation-v1-unicode").unwrap();
        original.verify().unwrap();
        assert!(matches!(
            acquire(&path, "flow-operation-v1-unicode"),
            Err(WorkItemError::Busy)
        ));
    }
    #[test]
    fn unsupported_device_and_drive_relative_roots_are_refused() {
        for name in [
            r"C:relative",
            r"\\.\pipe\surge",
            r"\\?\GLOBALROOT\Device\HarddiskVolume1\surge",
            r"\relative",
            r"C:\unsafe:stream",
        ] {
            assert!(root(Path::new(name)).is_err());
        }
        let (share, names) = root(Path::new(r"\\server\share\資料\lock")).unwrap();
        assert_eq!(share, PathBuf::from(r"\\server\share\"));
        assert_eq!(names, vec![OsString::from("資料"), OsString::from("lock")]);
    }
    fn write_status(path: &Path, text: &str) {
        let staging = path.with_extension("publishing");
        std::fs::write(&staging, text).unwrap();
        std::fs::rename(staging, path).unwrap();
    }
    fn mutation_not_available(error: &windows::core::Error) -> bool {
        // Explicit environment limits only: access/privilege, unsupported FS/API.
        // Unexpected sharing/ABI/argument failures remain failing test evidence.
        [5u32, 50, 1, 1314]
            .into_iter()
            .any(|code| error.code() == windows::core::HRESULT::from_win32(code))
    }
    fn mount_buffer(target: &Path) -> Vec<u8> {
        use windows::Wdk::Storage::FileSystem::{REPARSE_DATA_BUFFER, REPARSE_DATA_BUFFER_0_1};
        use windows::Win32::System::SystemServices::IO_REPARSE_TAG_MOUNT_POINT;
        let dos = std::fs::canonicalize(target).unwrap();
        let target = dos.to_string_lossy();
        let suffix = target.strip_prefix(r"\\?\").unwrap();
        let substitute: Vec<u16> = format!(r"\??\{suffix}").encode_utf16().collect();
        let mut names = substitute.clone();
        names.push(0);
        names.extend(dos.as_os_str().encode_wide());
        names.push(0);
        let header_size = std::mem::offset_of!(REPARSE_DATA_BUFFER, Anonymous);
        let mount_header_size = std::mem::offset_of!(REPARSE_DATA_BUFFER_0_1, PathBuffer);
        let mut header = REPARSE_DATA_BUFFER {
            ReparseTag: IO_REPARSE_TAG_MOUNT_POINT,
            ReparseDataLength: u16::try_from(mount_header_size + names.len() * 2).unwrap(),
            ..Default::default()
        };
        header.Anonymous.MountPointReparseBuffer = REPARSE_DATA_BUFFER_0_1 {
            SubstituteNameOffset: 0,
            SubstituteNameLength: u16::try_from(substitute.len() * 2).unwrap(),
            PrintNameOffset: u16::try_from((substitute.len() + 1) * 2).unwrap(),
            PrintNameLength: u16::try_from(dos.as_os_str().encode_wide().count() * 2).unwrap(),
            PathBuffer: [0],
        };
        // SAFETY: initialized SDK repr(C) header bytes up to the flexible PathBuffer.
        // Length/offsets derive from SDK layouts, not guessed ABI byte positions;
        // no out-of-bounds array reference or cast from unaligned byte storage.
        let mut buffer = unsafe {
            std::slice::from_raw_parts(
                (&raw const header).cast::<u8>(),
                header_size + mount_header_size,
            )
        }
        .to_vec();
        for unit in names {
            buffer.extend_from_slice(&unit.to_ne_bytes());
        }
        buffer
    }
    fn set_mount(file: &File, target: &Path) -> windows::core::Result<()> {
        use windows::Win32::System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_REPARSE_POINT};
        let buffer = mount_buffer(target);
        let mut written = 0u32;
        // SAFETY: valid borrowed directory handle, initialized SDK-shaped input
        // bytes remain live, and initialized byte-count output is writable.
        // Handle is synchronous; no OVERLAPPED or pending storage profile.
        unsafe {
            DeviceIoControl(
                handle(file),
                FSCTL_SET_REPARSE_POINT,
                Some(buffer.as_ptr().cast()),
                u32::try_from(buffer.len()).unwrap(),
                None,
                0,
                Some(&raw mut written),
                None,
            )
        }
    }
    fn clear_mount(file: &File) {
        use windows::{
            Wdk::Storage::FileSystem::REPARSE_DATA_BUFFER,
            Win32::System::{
                IO::DeviceIoControl, Ioctl::FSCTL_DELETE_REPARSE_POINT,
                SystemServices::IO_REPARSE_TAG_MOUNT_POINT,
            },
        };
        let buffer = REPARSE_DATA_BUFFER {
            ReparseTag: IO_REPARSE_TAG_MOUNT_POINT,
            ..Default::default()
        };
        let mut written = 0u32;
        // SAFETY: initialized SDK fixed header with zero data length, live borrowed
        // handle, synchronous call. Only actual fixed header bytes are submitted.
        unsafe {
            DeviceIoControl(
                handle(file),
                FSCTL_DELETE_REPARSE_POINT,
                Some((&raw const buffer).cast()),
                std::mem::offset_of!(REPARSE_DATA_BUFFER, Anonymous) as u32,
                None,
                0,
                Some(&raw mut written),
                None,
            )
        }
        .unwrap();
    }
    #[test]
    fn mutation_and_restoration_around_native_child_open_never_routes_into_substitute() {
        let home = tempfile::tempdir().unwrap();
        let parent_path = home.path().join("parent");
        let target_path = home.path().join("substitute");
        std::fs::create_dir(&parent_path).unwrap();
        std::fs::create_dir(&target_path).unwrap();
        std::fs::write(
            target_path.join("source.toml"),
            "substitute-source-must-not-be-opened",
        )
        .unwrap();
        let chain = Chain::open(&parent_path).unwrap();
        let parent = chain.files.last().unwrap();
        let mutation = OpenOptions::new()
            .write(true)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(&parent_path)
            .unwrap();
        match set_mount(&mutation, &target_path) {
            Ok(()) => (),
            Err(error) if mutation_not_available(&error) => {
                eprintln!(
                    "SKIP native mutation oracle: host privilege/filesystem refuses FSCTL_SET_REPARSE_POINT: {}",
                    error.code()
                );
                return;
            },
            Err(error) => panic!("unexpected native reparse mutation failure: {error}"),
        }
        // Deterministic interleaving: mutate immediately before the real native
        // relative OS call, restore before any ordinary endpoint verification.
        let opened = native_open(parent, OsStr::new("native-child"), Kind::Lock, FILE_CREATE);
        let source = native_open(parent, OsStr::new("source.toml"), Kind::Source, FILE_OPEN);
        clear_mount(&mutation);
        assert!(
            source.is_err(),
            "native source open routed to substitute-only file"
        );
        assert!(
            !target_path.join("native-child").exists(),
            "native open routed to substitute"
        );
        if let Ok(file) = opened {
            let original_named =
                self::opened(parent, OsStr::new("native-child"), Kind::Lock).unwrap();
            assert!(
                check(&file, Kind::Lock).unwrap() == check(&original_named, Kind::Lock).unwrap()
            );
            assert!(parent_path.join("native-child").is_file());
        }
        chain.verify().unwrap();
    }
    #[test]
    fn final_or_ancestor_reparse_objects_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let alias = home.path().join("alias");
        match std::os::windows::fs::symlink_dir(&target, &alias) {
            Ok(()) => (),
            Err(error) if matches!(error.raw_os_error(), Some(5 | 1314)) => {
                eprintln!(
                    "SKIP native symlink oracle: Windows developer-mode/privilege unavailable: {error}"
                );
                return;
            },
            Err(error) => panic!("unexpected directory symlink setup failure: {error}"),
        }
        assert!(Chain::open(&alias).is_err());
        let parent = Chain::open(home.path()).unwrap();
        assert!(
            opened(
                parent.files.last().unwrap(),
                OsStr::new("alias"),
                Kind::Directory
            )
            .and_then(|file| check(&file, Kind::Directory))
            .is_err()
        );
        drop(acquire(home.path(), "flow-operation-v1-reparse").unwrap());
        let lock = home
            .path()
            .join("work-items/preparation-locks/flow-operation-v1-reparse.lock");
        std::fs::remove_file(&lock).unwrap();
        std::fs::write(target.join("external"), "untouched").unwrap();
        std::os::windows::fs::symlink_file(target.join("external"), &lock).unwrap();
        assert!(acquire(home.path(), "flow-operation-v1-reparse").is_err());
        assert_eq!(
            std::fs::read(target.join("external")).unwrap(),
            b"untouched"
        );
    }
    #[test]
    fn delete_pending_lock_does_not_become_a_new_owner() {
        use windows::Win32::Storage::FileSystem::{FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE};
        let home = tempfile::tempdir().unwrap();
        drop(acquire(home.path(), "flow-operation-v1-delete-pending").unwrap());
        let path = home
            .path()
            .join("work-items/preparation-locks/flow-operation-v1-delete-pending.lock");
        let deleting = OpenOptions::new()
            .write(true)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .custom_flags(FILE_FLAG_DELETE_ON_CLOSE.0)
            .open(&path)
            .unwrap();
        assert!(acquire(home.path(), "flow-operation-v1-delete-pending").is_err());
        drop(deleting);
        assert!(!path.exists());
    }
    #[test]
    fn actual_unc_share_uses_native_relative_ownership_when_runner_provides_one() {
        let Some(share) = std::env::var_os("SURGE_WINDOWS_TEST_UNC_ROOT") else {
            eprintln!("SKIP actual UNC runtime oracle: SURGE_WINDOWS_TEST_UNC_ROOT not supplied");
            return;
        };
        let share = PathBuf::from(share);
        assert!(
            matches!(share.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::UNC(_,_) | Prefix::VerbatimUNC(_,_)))
        );
        let home = tempfile::Builder::new()
            .prefix("surge-native-")
            .tempdir_in(share)
            .unwrap();
        let guard = acquire(home.path(), "flow-operation-v1-unc").unwrap();
        guard.verify().unwrap();
        assert!(matches!(
            acquire(home.path(), "flow-operation-v1-unc"),
            Err(WorkItemError::Busy)
        ));
        std::fs::write(home.path().join("ordinary-child"), "allowed").unwrap();
    }
    #[test]
    fn native_components_are_one_name_never_stream_or_device_paths() {
        for name in ["", ".", "..", "a/b", "a\\b", "a:b", "nul\0x"] {
            assert!(component(std::ffi::OsStr::new(name)).is_err());
        }
        assert!(component(std::ffi::OsStr::new("資料-flow.lock")).is_ok());
    }
    #[test]
    fn held_parent_relative_child_cannot_route_to_replacement_tree() {
        let home = tempfile::tempdir().unwrap();
        let chain = Chain::open(home.path()).unwrap();
        let parent = chain.files.last().unwrap();
        let substitute = home.path().with_extension("substitute");
        assert!(std::fs::rename(home.path(), &substitute).is_err());
        let child = create_or_open(
            parent,
            std::ffi::OsStr::new("native-child"),
            Kind::Directory,
        )
        .unwrap();
        check(&child, Kind::Directory).unwrap();
        assert!(home.path().join("native-child").is_dir());
    }
}
