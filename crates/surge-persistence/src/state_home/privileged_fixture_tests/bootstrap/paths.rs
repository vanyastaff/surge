use super::{
    Output, decimal, hex,
    security::{self, Policy},
};
use std::{
    ffi::OsStr,
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
        Foundation::{HANDLE, STATUS_PENDING, STATUS_SUCCESS, UNICODE_STRING},
        Storage::FileSystem::{
            FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
            FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_STANDARD_INFO, FILE_TRAVERSE, FILE_TYPE_DISK, FILE_WRITE_DATA,
            FileAttributeTagInfo, FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx,
            GetFileType, READ_CONTROL, SYNCHRONIZE,
        },
        System::{
            IO::IO_STATUS_BLOCK,
            Kernel::{OBJ_CASE_INSENSITIVE, OBJ_DONT_REPARSE},
        },
    },
    core::PWSTR,
};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Identity {
    volume: u64,
    id: [u8; 16],
}

pub(super) struct Routes {
    path: PathBuf,
    actor: String,
    directories: Vec<File>,
    files: Vec<File>,
}

impl Routes {
    pub(super) fn open(manifest: &Path, actor: &str) -> Self {
        assert_eq!(manifest.file_name().unwrap(), "bootstrap.json");
        let path = manifest.parent().unwrap().to_path_buf();
        let mut parts = path.components();
        let Component::Prefix(prefix) = parts.next().unwrap() else {
            panic!("absolute drive path required")
        };
        assert!(matches!(prefix.kind(), Prefix::Disk(_)));
        assert_eq!(parts.next(), Some(Component::RootDir));
        let mut root = prefix.as_os_str().to_os_string();
        root.push("\\");
        let root = PathBuf::from(root);
        let file = OpenOptions::new()
            .access_mode((FILE_READ_ATTRIBUTES | FILE_TRAVERSE | READ_CONTROL | SYNCHRONIZE).0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(&root)
            .unwrap();
        require_local_ntfs(&file, &root);
        inspect(HANDLE(file.as_raw_handle()), true);
        security::verify(HANDLE(file.as_raw_handle()), actor, Policy::Ancestor);
        let mut directories = vec![file];
        for part in parts {
            let Component::Normal(name) = part else {
                panic!("noncanonical path component")
            };
            let child = relative(directories.last().unwrap(), name, true, false);
            security::verify(HANDLE(child.as_raw_handle()), actor, Policy::Ancestor);
            directories.push(child);
        }
        assert!(directories.len() > 1);
        security::verify(
            HANDLE(directories.last().unwrap().as_raw_handle()),
            actor,
            Policy::Invocation,
        );
        Self {
            path,
            actor: actor.into(),
            directories,
            files: Vec::new(),
        }
    }

    pub(super) fn require_path(&self, path: &Path, name: &str) {
        assert_eq!(
            path,
            self.path.join(name),
            "receipt escaped protected invocation directory"
        );
    }

    pub(super) fn open_file(&self, name: &str, create: bool) -> File {
        let file = relative(
            self.directories.last().unwrap(),
            OsStr::new(name),
            false,
            create,
        );
        security::verify(HANDLE(file.as_raw_handle()), &self.actor, Policy::File);
        file
    }

    pub(super) fn validate_output(
        &mut self,
        handle: HANDLE,
        name: &str,
        output: &Output,
    ) -> Identity {
        let observed = inspect(handle, false);
        security::verify(handle, &self.actor, Policy::File);
        assert_eq!(observed.volume, decimal(&output.volume));
        hex(&output.file_id, 32);
        let expected: Vec<u8> = output
            .file_id
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(observed.id.as_slice(), expected);
        let named = self.open_file(name, false);
        assert_eq!(inspect(HANDLE(named.as_raw_handle()), false), observed);
        self.files.push(named);
        observed
    }
}

fn component(name: &OsStr) -> Vec<u16> {
    let text = name.to_str().unwrap();
    assert!(!text.is_empty() && text != "." && text != "..");
    assert!(!text.ends_with(['.', ' ']));
    assert!(!text.chars().any(|v| v < ' ' || "<>:\"/\\|?*".contains(v)));
    let stem = text.split('.').next().unwrap().to_ascii_uppercase();
    assert!(!["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str()));
    assert!(
        !(stem.starts_with("COM") || stem.starts_with("LPT"))
            || !matches!(
                stem.get(3..),
                Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
            )
    );
    name.encode_wide().collect()
}

fn require_local_ntfs(file: &File, root: &Path) {
    use windows::{
        Win32::{
            Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationByHandleW},
            System::WindowsProgramming::DRIVE_FIXED,
        },
        core::PCWSTR,
    };
    let root: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: parsed absolute drive root is a terminated live string.
    assert_eq!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, DRIVE_FIXED);
    let mut name = [0u16; 32];
    // SAFETY: retained actual root handle and bounded initialized filesystem-name output.
    unsafe {
        GetVolumeInformationByHandleW(
            HANDLE(file.as_raw_handle()),
            None,
            None,
            None,
            None,
            Some(&mut name),
        )
    }
    .unwrap();
    let end = name.iter().position(|value| *value == 0).unwrap();
    assert_eq!(String::from_utf16(&name[..end]).unwrap(), "NTFS");
}

fn relative(parent: &File, name: &OsStr, directory: bool, create: bool) -> File {
    let mut name = component(name);
    let length = u16::try_from(name.len().checked_mul(2).unwrap()).unwrap();
    let unicode = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: PWSTR(name.as_mut_ptr()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>()).unwrap(),
        RootDirectory: HANDLE(parent.as_raw_handle()),
        ObjectName: &raw const unicode,
        Attributes: u32::try_from(OBJ_DONT_REPARSE | OBJ_CASE_INSENSITIVE).unwrap(),
        ..Default::default()
    };
    let access = FILE_READ_ATTRIBUTES
        | READ_CONTROL
        | SYNCHRONIZE
        | if directory {
            FILE_TRAVERSE
        } else if create {
            FILE_WRITE_DATA
        } else {
            FILE_READ_DATA
        };
    let mut handle = HANDLE::default();
    let mut io = IO_STATUS_BLOCK::default();
    // SAFETY: retained root plus one validated component; all exact SDK ABI storage lives
    // through this synchronous relative open. FILE_CREATE is exclusive, never an overwrite.
    let status = unsafe {
        NtCreateFile(
            &raw mut handle,
            access,
            &attributes,
            &raw mut io,
            None,
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            if create { FILE_CREATE } else { FILE_OPEN },
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
    assert_eq!(status, STATUS_SUCCESS);
    assert!(!handle.is_invalid());
    // SAFETY: successful synchronous creation returned one newly owned real handle.
    let file = File::from(unsafe { OwnedHandle::from_raw_handle(handle.0) });
    // SAFETY: synchronous completion initialized the final status arm.
    assert_eq!(unsafe { io.Anonymous.Status }, STATUS_SUCCESS);
    inspect(HANDLE(file.as_raw_handle()), directory);
    file
}

fn inspect(handle: HANDLE, directory: bool) -> Identity {
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    let mut standard = FILE_STANDARD_INFO::default();
    let mut identity = FILE_ID_INFO::default();
    // SAFETY: live held handle, exact information-class output layout and checked size.
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&raw mut attributes).cast(),
            u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>()).unwrap(),
        )
    }
    .unwrap();
    // SAFETY: live held handle, exact standard-information layout and checked size.
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&raw mut standard).cast(),
            u32::try_from(size_of::<FILE_STANDARD_INFO>()).unwrap(),
        )
    }
    .unwrap();
    // SAFETY: live held handle, exact file-identity layout and checked size.
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&raw mut identity).cast(),
            u32::try_from(size_of::<FILE_ID_INFO>()).unwrap(),
        )
    }
    .unwrap();
    // SAFETY: borrowed descriptor remains live throughout the SDK query.
    assert_eq!(unsafe { GetFileType(handle) }, FILE_TYPE_DISK);
    assert_eq!(
        attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0,
        0
    );
    assert!(!standard.DeletePending.as_bool());
    assert_eq!(standard.Directory.as_bool(), directory);
    if !directory {
        assert_eq!(standard.NumberOfLinks, 1);
    }
    Identity {
        volume: identity.VolumeSerialNumber,
        id: identity.FileId.Identifier,
    }
}
