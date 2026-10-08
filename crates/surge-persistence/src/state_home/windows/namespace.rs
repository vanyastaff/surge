//! One-component native routing through retained, validated directories.
use super::{
    native::{NativeError, NativeResult, flush_complete},
    security::{DirectoryPolicy, EffectiveOwner},
};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
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
            FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
        },
    },
    Win32::{
        Foundation::{
            HANDLE, STATUS_OBJECT_NAME_COLLISION, STATUS_PENDING, STATUS_SUCCESS, UNICODE_STRING,
        },
        Storage::FileSystem::{
            FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE,
            FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_STANDARD_INFO, FILE_TRAVERSE, FILE_TYPE_DISK, FileAttributeTagInfo, FileIdInfo,
            FileStandardInfo, GetFileInformationByHandleEx, GetFileType, READ_CONTROL, SYNCHRONIZE,
        },
        System::{
            IO::IO_STATUS_BLOCK,
            Kernel::{OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE},
        },
    },
};

#[derive(Clone, PartialEq, Eq)]
struct Identity {
    volume: u64,
    id: [u8; 16],
}
fn refusal(reason: &'static str) -> NativeError {
    NativeError::Security(reason)
}
fn inspect(file: &File, directory: bool) -> NativeResult<Identity> {
    let handle = HANDLE(file.as_raw_handle());
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    let mut standard = FILE_STANDARD_INFO::default();
    let mut identity = FILE_ID_INFO::default();
    // SAFETY: each SDK information class exactly matches the initialized output layout
    // and checked byte count. All borrowed handle/output memory lives through the calls.
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&raw mut attributes).cast(),
            u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>())
                .map_err(|_| refusal("attribute size overflow"))?,
        )?;
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&raw mut standard).cast(),
            u32::try_from(size_of::<FILE_STANDARD_INFO>())
                .map_err(|_| refusal("standard size overflow"))?,
        )?;
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut identity).cast(),
            u32::try_from(size_of::<FILE_ID_INFO>())
                .map_err(|_| refusal("identity size overflow"))?,
        )?;
        if GetFileType(handle) != FILE_TYPE_DISK {
            return Err(refusal("non-disk object"));
        }
    }
    if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        || standard.DeletePending.as_bool()
        || standard.Directory.as_bool() != directory
        || (!directory && standard.NumberOfLinks != 1)
    {
        return Err(refusal("unsafe native object type or identity"));
    }
    Ok(Identity {
        volume: identity.VolumeSerialNumber,
        id: identity.FileId.Identifier,
    })
}
fn component(name: &OsStr) -> NativeResult<Vec<u16>> {
    let text = name
        .to_str()
        .ok_or_else(|| refusal("non-Unicode Win32 component"))?;
    let basename = text
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_uppercase();
    let reserved = matches!(
        basename.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        basename.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    if text.ends_with(['.', ' '])
        || reserved
        || text
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err(refusal("component has a Win32 alias or invalid character"));
    }
    let value: Vec<u16> = name.encode_wide().collect();
    if value.is_empty()
        || value == [46]
        || value == [46, 46]
        || value.iter().any(|c| matches!(*c, 0 | 47 | 92 | 58))
        || value
            .len()
            .checked_mul(2)
            .and_then(|v| u16::try_from(v).ok())
            .is_none()
    {
        return Err(refusal("invalid native path component"));
    }
    Ok(value)
}
fn relative(
    parent: &File,
    name: &OsStr,
    directory: bool,
    creation: Option<&super::security::CreationSecurity>,
    writable: bool,
) -> NativeResult<File> {
    let mut access = FILE_READ_ATTRIBUTES
        | READ_CONTROL
        | SYNCHRONIZE
        | if directory {
            FILE_TRAVERSE
        } else {
            FILE_READ_DATA
        };
    if writable {
        access |= FILE_GENERIC_WRITE;
    }
    relative_access(parent, name, directory, creation, access)
}
fn relative_access(
    parent: &File,
    name: &OsStr,
    directory: bool,
    creation: Option<&super::security::CreationSecurity>,
    access: windows::Win32::Storage::FileSystem::FILE_ACCESS_RIGHTS,
) -> NativeResult<File> {
    relative_access_shared(parent, name, directory, creation, access, false)
}
fn relative_access_shared(
    parent: &File,
    name: &OsStr,
    directory: bool,
    creation: Option<&super::security::CreationSecurity>,
    access: windows::Win32::Storage::FileSystem::FILE_ACCESS_RIGHTS,
    control: bool,
) -> NativeResult<File> {
    #[cfg(test)]
    let observed_name = name;
    let mut name = component(name)?;
    let length = u16::try_from(name.len() * 2).map_err(|_| refusal("component size overflow"))?;
    let unicode = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: windows::core::PWSTR(name.as_mut_ptr()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>())
            .map_err(|_| refusal("object size overflow"))?,
        RootDirectory: HANDLE(parent.as_raw_handle()),
        ObjectName: &raw const unicode,
        Attributes: u32::try_from(OBJ_DONT_REPARSE | OBJ_CASE_INSENSITIVE)
            .map_err(|_| refusal("object flags overflow"))?,
        SecurityDescriptor: creation.map_or(std::ptr::null(), |v| v.descriptor().0),
        ..Default::default()
    };
    let mut output = HANDLE::default();
    let mut io = IO_STATUS_BLOCK::default();
    // SAFETY: synchronous native open with live parent and complete initialized
    // ABI storage. The single component and explicit descriptor remain live.
    let status = unsafe {
        NtCreateFile(
            &raw mut output,
            access,
            &attributes,
            &raw mut io,
            None,
            FILE_ATTRIBUTE_NORMAL,
            if control {
                FILE_SHARE_READ
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            },
            if creation.is_some() {
                FILE_CREATE
            } else {
                FILE_OPEN
            },
            FILE_SYNCHRONOUS_IO_NONALERT
                | FILE_OPEN_REPARSE_POINT
                | if directory {
                    FILE_DIRECTORY_FILE
                } else {
                    FILE_NON_DIRECTORY_FILE
                },
            None,
            0,
        )
    };
    if status == STATUS_PENDING {
        std::process::abort();
    }
    if status != STATUS_SUCCESS {
        return Err(NativeError::Open(status));
    }
    if output.is_invalid() {
        return Err(refusal("invalid native handle"));
    }
    // SAFETY: successful native open transfers one new real handle to RAII ownership.
    let owned = unsafe { OwnedHandle::from_raw_handle(output.0) };
    let file = File::from(owned);
    // SAFETY: completed synchronous call initialized the final status arm.
    let final_status = unsafe { io.Anonymous.Status };
    if final_status == STATUS_PENDING {
        std::process::abort();
    }
    if final_status != STATUS_SUCCESS {
        return Err(NativeError::Open(final_status));
    }
    #[cfg(test)]
    if creation.is_some() {
        super::creation_observation::created(parent, observed_name, directory, &file);
    }
    inspect(&file, directory)?;
    Ok(file)
}
fn parsed_root(path: &Path) -> NativeResult<(PathBuf, Vec<OsString>)> {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return Err(refusal("state home must be absolute"));
    };
    if !matches!(
        prefix.kind(),
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)
    ) || !matches!(parts.next(), Some(Component::RootDir))
    {
        return Err(refusal("invalid state home root"));
    }
    let mut root = PathBuf::from(prefix.as_os_str());
    root.push(r"\");
    let mut names = Vec::new();
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(refusal("invalid state home path"));
        };
        component(name)?;
        names.push(name.to_owned());
    }
    if names.is_empty() {
        return Err(refusal("state home cannot be a volume root"));
    }
    Ok((root, names))
}
pub(in crate::state_home) struct Namespace {
    owner: EffectiveOwner,
    root: PathBuf,
    names: Vec<OsString>,
    files: Vec<File>,
    identities: Vec<Identity>,
    policies: Vec<DirectoryPolicy>,
}
impl Namespace {
    pub(super) fn home(path: &Path) -> NativeResult<Self> {
        Self::protected_parent(path)
    }
    pub(super) fn protected_parent(path: &Path) -> NativeResult<Self> {
        Self::open_parent(path, true)
    }
    pub(super) fn existing_parent(path: &Path) -> NativeResult<Self> {
        Self::open_parent(path, false)
    }
    fn open_parent(path: &Path, create: bool) -> NativeResult<Self> {
        let owner = EffectiveOwner::capture()?;
        let (root, names) = parsed_root(path)?;
        let file = OpenOptions::new()
            .access_mode((FILE_READ_ATTRIBUTES | FILE_TRAVERSE | READ_CONTROL | SYNCHRONIZE).0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(&root)?;
        require_local_ntfs(&file, &root)?;
        let identity = inspect(&file, true)?;
        owner.validate_directory(&file, DirectoryPolicy::Ancestor)?;
        let mut namespace = Self {
            owner,
            root,
            names: Vec::new(),
            files: vec![file],
            identities: vec![identity],
            policies: vec![DirectoryPolicy::Ancestor],
        };
        let last = names.len() - 1;
        for (index, name) in names.iter().enumerate() {
            let policy = if index == last {
                DirectoryPolicy::Protected { inheritable: true }
            } else {
                DirectoryPolicy::Ancestor
            };
            namespace.push(name, policy, create && index == last)?;
        }
        namespace.verify()?;
        Ok(namespace)
    }
    pub(super) fn push(
        &mut self,
        name: &OsStr,
        policy: DirectoryPolicy,
        create: bool,
    ) -> NativeResult<()> {
        if create {
            self.writable_parent()?;
        }
        let parent = self
            .files
            .last()
            .ok_or_else(|| refusal("missing retained parent"))?;
        let file = if create {
            let DirectoryPolicy::Protected { inheritable } = policy else {
                return Err(refusal("ancestor creation forbidden"));
            };
            let security = self.owner.creation(inheritable)?;
            match relative(parent, name, true, Some(&security), true) {
                Ok(file) => {
                    self.owner.validate_directory(&file, policy)?;
                    flush_complete(&file)?;
                    flush_complete(parent)?;
                    file
                },
                Err(NativeError::Open(STATUS_OBJECT_NAME_COLLISION)) => {
                    relative(parent, name, true, None, true)?
                },
                Err(error) => return Err(error),
            }
        } else {
            relative(parent, name, true, None, false)?
        };
        self.owner.validate_directory(&file, policy)?;
        self.identities.push(inspect(&file, true)?);
        self.policies.push(policy);
        self.files.push(file);
        self.names.push(name.to_owned());
        Ok(())
    }
    fn writable_parent(&mut self) -> NativeResult<()> {
        let index = self
            .files
            .len()
            .checked_sub(1)
            .ok_or_else(|| refusal("missing parent"))?;
        let file = if index == 0 {
            OpenOptions::new()
                .access_mode((FILE_GENERIC_WRITE | FILE_TRAVERSE | READ_CONTROL | SYNCHRONIZE).0)
                .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
                .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
                .open(&self.root)?
        } else {
            relative(
                &self.files[index - 1],
                &self.names[index - 1],
                true,
                None,
                true,
            )?
        };
        if inspect(&file, true)? != self.identities[index] {
            return Err(refusal("writable parent identity changed"));
        }
        self.files[index] = file;
        Ok(())
    }
    pub(super) fn verify(&self) -> NativeResult<()> {
        if !self.owner.same_user(&EffectiveOwner::capture()?) {
            return Err(refusal("effective token user changed"));
        }
        for (index, file) in self.files.iter().enumerate() {
            self.owner.validate_directory(file, self.policies[index])?;
            if inspect(file, true)? != self.identities[index] {
                return Err(refusal("held identity changed"));
            }
            if index > 0 {
                let named = relative(
                    &self.files[index - 1],
                    &self.names[index - 1],
                    true,
                    None,
                    false,
                )?;
                if inspect(&named, true)? != self.identities[index] {
                    return Err(refusal("named identity changed"));
                }
            }
        }
        let root = OpenOptions::new()
            .access_mode((FILE_READ_ATTRIBUTES | FILE_TRAVERSE | READ_CONTROL | SYNCHRONIZE).0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(&self.root)?;
        if inspect(&root, true)? != self.identities[0] {
            return Err(refusal("root identity changed"));
        }
        Ok(())
    }
}
impl Namespace {
    pub(super) fn retained_clone(&self) -> NativeResult<Self> {
        self.verify()?;
        Ok(Self {
            owner: self.owner.clone(),
            root: self.root.clone(),
            names: self.names.clone(),
            identities: self.identities.clone(),
            policies: self.policies.clone(),
            files: self
                .files
                .iter()
                .map(File::try_clone)
                .collect::<std::io::Result<Vec<_>>>()?,
        })
    }
    pub(super) fn child(&self, name: &OsStr, inheritable: bool) -> NativeResult<Self> {
        let mut child = self.retained_clone()?;
        child.push(name, DirectoryPolicy::Protected { inheritable }, true)?;
        child.verify()?;
        Ok(child)
    }
    pub(super) fn child_exclusive(&self, name: &OsStr, inheritable: bool) -> NativeResult<Self> {
        let mut child = self.retained_clone()?;
        child.writable_parent()?;
        let parent = child
            .files
            .last()
            .ok_or_else(|| refusal("missing exclusive parent"))?;
        let policy = DirectoryPolicy::Protected { inheritable };
        let security = child.owner.creation(inheritable)?;
        // FILE_CREATE intentionally propagates collision, without opening the existing object.
        let file = relative(parent, name, true, Some(&security), true)?;
        child.owner.validate_directory(&file, policy)?;
        flush_complete(&file)?;
        flush_complete(parent)?;
        child.identities.push(inspect(&file, true)?);
        child.policies.push(policy);
        child.files.push(file);
        child.names.push(name.to_owned());
        child.verify()?;
        Ok(child)
    }
    pub(super) fn database(&self, name: &OsStr, create: bool) -> NativeResult<File> {
        self.verify()?;
        let parent = self
            .files
            .last()
            .ok_or_else(|| refusal("missing database parent"))?;
        let creation = self.owner.creation(false)?;
        let file = if !create {
            relative(parent, name, false, None, false)?
        } else {
            match relative(parent, name, false, Some(&creation), true) {
                Ok(file) => {
                    self.owner.validate_directory(
                        &file,
                        DirectoryPolicy::Protected { inheritable: false },
                    )?;
                    flush_complete(&file)?;
                    flush_complete(parent)?;
                    file
                },
                Err(NativeError::Open(STATUS_OBJECT_NAME_COLLISION)) => {
                    relative(parent, name, false, None, false)?
                },
                Err(error) => return Err(error),
            }
        };
        self.owner
            .validate_directory(&file, DirectoryPolicy::Protected { inheritable: false })?;
        // No write authority is retained by the permanent SQLite identity fence.
        let fence = relative(parent, name, false, None, false)?;
        if inspect(&fence, false)? != inspect(&file, false)? {
            return Err(refusal("database identity changed"));
        }
        Ok(fence)
    }
    pub(super) fn verify_database(&self, name: &OsStr, file: &File) -> NativeResult<()> {
        self.verify()?;
        let parent = self
            .files
            .last()
            .ok_or_else(|| refusal("missing database parent"))?;
        let named = relative(parent, name, false, None, false)?;
        if inspect(file, false)? != inspect(&named, false)? {
            return Err(refusal("database name no longer identifies held object"));
        }
        self.owner
            .validate_directory(file, DirectoryPolicy::Protected { inheritable: false })
    }
}

impl Namespace {
    pub(super) fn existing_child(&self, name: &OsStr, inheritable: bool) -> NativeResult<Self> {
        let mut child = self.retained_clone()?;
        child.push(name, DirectoryPolicy::Protected { inheritable }, false)?;
        child.verify()?;
        Ok(child)
    }
}

fn require_local_ntfs(file: &File, root: &Path) -> NativeResult<()> {
    use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationByHandleW};
    use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
    let root: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: parsed volume root is a terminated live string; this query performs no mutation.
    if unsafe { GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
        return Err(refusal("state home requires a local fixed volume"));
    }
    let mut name = [0u16; 32];
    // SAFETY: live root handle and bounded initialized UTF-16 filesystem-name output.
    unsafe {
        GetVolumeInformationByHandleW(
            HANDLE(file.as_raw_handle()),
            None,
            None,
            None,
            None,
            Some(&mut name),
        )
    }?;
    let end = name
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| refusal("unterminated filesystem name"))?;
    if name[..end] != [78, 84, 70, 83] {
        return Err(refusal("state home requires NTFS"));
    }
    Ok(())
}

impl Namespace {
    pub(super) fn verify_sidefiles(&self, database: &OsStr) -> NativeResult<()> {
        use windows::Win32::Foundation::{
            STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND,
        };
        self.verify()?;
        let parent = self
            .files
            .last()
            .ok_or_else(|| refusal("missing SQLite parent"))?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut name = database.to_owned();
            name.push(suffix);
            match relative(parent, &name, false, None, false) {
                Ok(file) => self
                    .owner
                    .validate_directory(&file, DirectoryPolicy::SqliteSidefile)?,
                Err(NativeError::Open(
                    STATUS_OBJECT_NAME_NOT_FOUND | STATUS_OBJECT_PATH_NOT_FOUND,
                )) => {},
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

pub(in crate::state_home) struct NativeAppendFile {
    namespace: Namespace,
    name: OsString,
    append: File,
    barrier: File,
}
pub(in crate::state_home) struct NativeControlFile {
    namespace: Namespace,
    file: File,
    locked: bool,
    created: bool,
}
const CONTROL_PAYLOAD_MAX: usize = 1_048_576;
const CONTROL_LOCK_OFFSET: u64 = 1_u64 << 32;
const _: () = assert!(CONTROL_PAYLOAD_MAX as u64 + 1 < CONTROL_LOCK_OFFSET);
const _: () = assert!(CONTROL_LOCK_OFFSET < u64::MAX);
impl Namespace {
    fn runtime_file(&self, name: &OsStr, control: bool) -> NativeResult<(File, bool)> {
        use windows::Win32::Storage::FileSystem::{DELETE, FILE_GENERIC_READ};
        self.verify()?;
        let parent = self
            .files
            .last()
            .ok_or_else(|| refusal("missing runtime parent"))?;
        let creation = self.owner.creation(false)?;
        let mut access = FILE_GENERIC_READ | FILE_GENERIC_WRITE | READ_CONTROL | SYNCHRONIZE;
        if control {
            access |= DELETE;
        }
        let (file, created) =
            match relative_access_shared(parent, name, false, Some(&creation), access, control) {
                Ok(file) => {
                    self.owner.validate_directory(
                        &file,
                        DirectoryPolicy::Protected { inheritable: false },
                    )?;
                    flush_complete(&file)?;
                    flush_complete(parent)?;
                    (file, true)
                },
                Err(NativeError::Open(STATUS_OBJECT_NAME_COLLISION)) => (
                    relative_access_shared(parent, name, false, None, access, control)?,
                    false,
                ),
                Err(error) => return Err(error),
            };
        self.owner
            .validate_directory(&file, DirectoryPolicy::Protected { inheritable: false })?;
        Ok((file, created))
    }
    pub(in crate::state_home) fn append_file(
        &self,
        name: &OsStr,
    ) -> NativeResult<NativeAppendFile> {
        use windows::Win32::Storage::FileSystem::FILE_APPEND_DATA;
        let namespace = self.retained_clone()?;
        let (barrier, _) = namespace.runtime_file(name, false)?;
        let parent = namespace
            .files
            .last()
            .ok_or_else(|| refusal("missing append parent"))?;
        let append = relative_access(
            parent,
            name,
            false,
            None,
            FILE_APPEND_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        )?;
        if inspect(&append, false)? != inspect(&barrier, false)? {
            return Err(refusal("append object identity changed"));
        }
        Ok(NativeAppendFile {
            namespace,
            name: name.to_owned(),
            append,
            barrier,
        })
    }
    pub(in crate::state_home) fn control_file(
        &self,
        name: &OsStr,
    ) -> NativeResult<NativeControlFile> {
        let namespace = self.retained_clone()?;
        let (file, created) = match namespace.runtime_file(name, true) {
            Err(NativeError::Open(windows::Win32::Foundation::STATUS_SHARING_VIOLATION)) => {
                return Err(NativeError::Busy);
            },
            result => result?,
        };
        Ok(NativeControlFile {
            namespace,
            file,
            locked: false,
            created,
        })
    }
    fn flush_parent(&self) -> NativeResult<()> {
        flush_complete(
            self.files
                .last()
                .ok_or_else(|| refusal("missing flush parent"))?,
        )
    }
}
impl NativeAppendFile {
    pub(in crate::state_home) fn stdio_clone(&self) -> NativeResult<(std::process::Stdio, Self)> {
        self.namespace.verify_database(&self.name, &self.barrier)?;
        if inspect(&self.append, false)? != inspect(&self.barrier, false)? {
            return Err(refusal("append identity changed"));
        }
        Ok((
            std::process::Stdio::from(self.append.try_clone()?),
            Self {
                namespace: self.namespace.retained_clone()?,
                name: self.name.clone(),
                append: self.append.try_clone()?,
                barrier: self.barrier.try_clone()?,
            },
        ))
    }
    pub(in crate::state_home) fn flush(&self) -> NativeResult<()> {
        self.namespace.verify_database(&self.name, &self.barrier)?;
        flush_complete(&self.barrier)?;
        self.namespace.flush_parent()
    }
}
impl NativeControlFile {
    pub(in crate::state_home) fn verify(&self) -> NativeResult<()> {
        if !self.locked {
            return Err(refusal("control file is not exclusively claimed"));
        }
        self.namespace.verify()?;
        inspect(&self.file, false)?;
        self.namespace.owner.validate_directory(
            &self.file,
            DirectoryPolicy::Protected { inheritable: false },
        )
    }
    pub(in crate::state_home) fn was_created(&self) -> bool {
        self.created
    }
    pub(in crate::state_home) fn try_lock(&mut self) -> NativeResult<bool> {
        use windows::Win32::{
            Foundation::{ERROR_IO_PENDING, ERROR_LOCK_VIOLATION},
            Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx},
            System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0},
        };
        // This verifies the held descriptor without reopening DELETE against itself.
        self.namespace.verify()?;
        self.namespace.owner.validate_directory(
            &self.file,
            DirectoryPolicy::Protected { inheritable: false },
        )?;
        inspect(&self.file, false)?;
        if self.locked {
            return Ok(true);
        }
        let mut overlapped = OVERLAPPED {
            Anonymous: OVERLAPPED_0 {
                Anonymous: OVERLAPPED_0_0 {
                    Offset: 0,
                    OffsetHigh: u32::try_from(CONTROL_LOCK_OFFSET >> 32)
                        .map_err(|_| refusal("control lock offset overflow"))?,
                },
            },
            ..Default::default()
        };
        // SAFETY: the synchronous held handle and initialized OVERLAPPED stay live
        // throughout this nonblocking call. The reserved byte is beyond all payload
        // reads and overlaps the legacy whole-file range without extending the file.
        let result = unsafe {
            LockFileEx(
                HANDLE(self.file.as_raw_handle()),
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &raw mut overlapped,
            )
        };
        match result {
            Ok(()) => {
                self.locked = true;
                Ok(true)
            },
            Err(error)
                if error.code() == windows::core::HRESULT::from_win32(ERROR_IO_PENDING.0) =>
            {
                // A nonconforming pending operation must not outlive stack OVERLAPPED.
                std::process::abort();
            },
            Err(error)
                if error.code() == windows::core::HRESULT::from_win32(ERROR_LOCK_VIOLATION.0) =>
            {
                Ok(false)
            },
            Err(error) => Err(error.into()),
        }
    }
    pub(in crate::state_home) fn read_bounded(&self, limit: usize) -> NativeResult<Vec<u8>> {
        if !self.locked {
            return Err(refusal("control file is not exclusively claimed"));
        }
        inspect(&self.file, false)?;
        use std::io::{Read, Seek, SeekFrom};
        if limit > CONTROL_PAYLOAD_MAX {
            return Err(refusal("control read limit too large"));
        }
        self.namespace.verify()?;
        self.namespace.owner.validate_directory(
            &self.file,
            DirectoryPolicy::Protected { inheritable: false },
        )?;
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.take(u64::try_from(limit).map_err(|_| refusal("read limit overflow"))? + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(refusal("control file exceeds read limit"));
        }
        Ok(bytes)
    }
    pub(in crate::state_home) fn replace_bytes(&mut self, bytes: &[u8]) -> NativeResult<()> {
        if !self.locked {
            return Err(refusal("control file is not exclusively claimed"));
        }
        inspect(&self.file, false)?;
        use std::io::{Seek, SeekFrom, Write};
        if bytes.len() > CONTROL_PAYLOAD_MAX {
            return Err(refusal("control write limit too large"));
        }
        self.namespace.verify()?;
        self.namespace.owner.validate_directory(
            &self.file,
            DirectoryPolicy::Protected { inheritable: false },
        )?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(bytes)?;
        self.flush()
    }
    pub(in crate::state_home) fn flush(&self) -> NativeResult<()> {
        self.namespace.verify()?;
        flush_complete(&self.file)?;
        self.namespace.flush_parent()
    }
    pub(in crate::state_home) fn remove(self) -> NativeResult<()> {
        if !self.locked {
            return Err(refusal("control file is not exclusively claimed"));
        }
        inspect(&self.file, false)?;
        use windows::Win32::Storage::FileSystem::{
            FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_INFO_EX, FileDispositionInfoEx,
            SetFileInformationByHandle,
        };
        self.namespace.verify()?;
        self.namespace.owner.validate_directory(
            &self.file,
            DirectoryPolicy::Protected { inheritable: false },
        )?;
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE,
        };
        // SAFETY: held original DELETE-capable file, exact SDK layout and checked byte count.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(self.file.as_raw_handle()),
                FileDispositionInfoEx,
                (&raw const disposition).cast(),
                u32::try_from(size_of::<FILE_DISPOSITION_INFO_EX>())
                    .map_err(|_| refusal("disposition size overflow"))?,
            )
        }?;
        drop(self.file);
        self.namespace.flush_parent()
    }
}

/// Cooperative journal lock: no delete sharing, retained parent identity and OS lock.
pub(crate) struct NativeJournalLock {
    _file: File,
    _namespace: Namespace,
}
impl NativeJournalLock {
    pub(crate) fn acquire(path: &Path, create: bool) -> NativeResult<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| refusal("journal lock has no parent"))?;
        let name = path
            .file_name()
            .ok_or_else(|| refusal("journal lock has no filename"))?;
        let mut namespace = Namespace::existing_parent(parent)?;
        if create {
            namespace.writable_parent()?;
        }
        let file = if create {
            namespace.runtime_file(name, false)?.0
        } else {
            let parent = namespace
                .files
                .last()
                .ok_or_else(|| refusal("missing journal parent"))?;
            let file = relative(parent, name, false, None, true)?;
            namespace
                .owner
                .validate_directory(&file, DirectoryPolicy::Protected { inheritable: false })?;
            file
        };
        match file.try_lock() {
            Ok(()) => {},
            Err(std::fs::TryLockError::WouldBlock) => return Err(NativeError::Busy),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        Ok(Self {
            _file: file,
            _namespace: namespace,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::component;
    use std::ffi::OsStr;
    #[test]
    fn win32_aliases_are_rejected_before_native_open() {
        for name in [
            "state.",
            "state ",
            "CON",
            "con.txt",
            "NUL.sqlite",
            "COM1",
            "LPT9.log",
            "COM¹.txt",
            "LPT²",
            "COM³",
            "CONIN$",
            "CONOUT$",
            "CLOCK$",
            "AUX .db",
            "a:b",
            "a?b",
        ] {
            assert!(
                component(OsStr::new(name)).is_err(),
                "accepted alias {name}"
            );
        }
        for name in [
            "state",
            "registry.sqlite",
            "COM10",
            "company",
            "long path",
            "проект",
        ] {
            assert!(
                component(OsStr::new(name)).is_ok(),
                "rejected ordinary component {name}"
            );
        }
    }
}
