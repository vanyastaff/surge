//! Independent SDK creation/inspection of isolated fixture objects.
use super::test_security;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HLOCAL, LocalFree},
        Security::{
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
                ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
                SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, GetSecurityDescriptorOwner,
            IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
            SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE,
            FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileStandardInfo,
            GetFileInformationByHandleEx, READ_CONTROL, WRITE_OWNER,
        },
    },
    core::{PCWSTR, PWSTR},
};

struct Allocation(HLOCAL);
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: one uniquely owned successful SDK LocalAlloc result.
        unsafe { LocalFree(self.0) };
    }
}
fn descriptor(text: &str) -> (PSECURITY_DESCRIPTOR, Allocation) {
    let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: terminated live input and initialized output; result owned until release.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(text.as_ptr()),
            SDDL_REVISION_1,
            &raw mut descriptor,
            None,
        )
    }
    .unwrap();
    (descriptor, Allocation(HLOCAL(descriptor.0)))
}

pub(super) fn create_directory(path: &Path, sddl: &str) {
    let (descriptor, _allocation) = descriptor(sddl);
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap(),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: live initial security descriptor and terminated path through exclusive creation.
    unsafe { CreateDirectoryW(PCWSTR(path.as_ptr()), Some(&raw const attributes)) }.unwrap();
}

/// The standard actor creates its own exact user-only sentinel, not an admin-created file.
pub(super) fn create_sentinel(path: &Path, actor: &str, bytes: &[u8]) {
    let (descriptor, _allocation) = descriptor(&format!("O:{actor}D:P(A;;FA;;;{actor})"));
    let path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap(),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: terminated path and initial descriptor are live for exclusive new-file creation.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            Some(&raw const attributes),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            HANDLE::default(),
        )
    }
    .unwrap();
    // SAFETY: successful create returned one unique owned handle, transferred once.
    let mut file = File::from(unsafe { OwnedHandle::from_raw_handle(handle.0) });
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

pub(super) fn regular_sentinel_identity(path: &Path) -> test_security::FileIdentity {
    let file = test_security::open_observer(path);
    let metadata = file.metadata().unwrap();
    assert!(metadata.is_file());
    assert_eq!(
        metadata.file_attributes() & 0x400,
        0,
        "sentinel must not be a reparse point"
    );
    let mut standard = FILE_STANDARD_INFO::default();
    // SAFETY: held file and exact initialized class-specific output layout.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileStandardInfo,
            (&raw mut standard).cast(),
            u32::try_from(std::mem::size_of::<FILE_STANDARD_INFO>()).unwrap(),
        )
    }
    .unwrap();
    assert!(!standard.Directory.as_bool() && !standard.DeletePending.as_bool());
    assert_eq!(standard.NumberOfLinks, 1, "sentinel must have one link");
    test_security::file_identity(&file)
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Snapshot {
    pub(super) owner: String,
    pub(super) sddl: String,
    protected: bool,
}
impl Snapshot {
    pub(super) fn exact_user_directory(&self, owner: &str, standard: &str) {
        assert_eq!(self.owner, owner);
        assert!(self.protected);
        assert_eq!(self.dacl(), format!("P(A;OICI;FA;;;{standard})"));
    }
    pub(super) fn dacl(&self) -> &str {
        self.sddl.split_once("D:").unwrap().1
    }
}

pub(super) fn snapshot(path: &Path) -> Snapshot {
    inspect(&test_security::open_observer(path))
}

pub(super) fn inspect(file: &File) -> Snapshot {
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    // SAFETY: held READ_CONTROL file and initialized SDK-owned output pointers.
    unsafe {
        GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            information,
            Some(&raw mut owner),
            None,
            None,
            None,
            Some(&raw mut descriptor),
        )
    }
    .ok()
    .unwrap();
    let _descriptor = Allocation(HLOCAL(descriptor.0));
    // SAFETY: owner SID returned within the retained successful SDK descriptor.
    assert!(unsafe { IsValidSid(owner) }.as_bool());
    let mut text = PWSTR::null();
    // SAFETY: live SDK-owned SID; successful conversion allocation retained below.
    unsafe { ConvertSidToStringSidW(owner, &raw mut text) }.unwrap();
    let _owner_text = Allocation(HLOCAL(text.0.cast()));
    // SAFETY: successful SDK-owned terminated string remains live.
    let owner = unsafe { text.to_string() }.unwrap();
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: valid retained SDK descriptor and initialized outputs.
    unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }
        .unwrap();
    let mut text = PWSTR::null();
    // SAFETY: same live descriptor; conversion output owned below.
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
    // SAFETY: successful terminated SDK string lives throughout conversion.
    let sddl = unsafe { text.to_string() }.unwrap();
    Snapshot {
        owner,
        sddl,
        protected: control & SE_DACL_PROTECTED.0 != 0,
    }
}

/// Setup-only owner assignment: no DACL change and no extra backup privilege.
pub(super) fn create_foreign_home(
    path: &Path,
    actor: &str,
    standard: &str,
    foreign: &str,
) -> Snapshot {
    create_directory(path, &format!("O:{actor}D:P(A;OICI;FA;;;{standard})"));
    // While actor owns this exact object, its owner rights grant READ_CONTROL.
    // Retain that already-granted descriptor right through the owner change.
    let reader = OpenOptions::new()
        .access_mode(READ_CONTROL.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
        .unwrap();
    inspect(&reader).exact_user_directory(actor, standard);
    // Only the separate verified SeRestore token authorizes this WRITE_OWNER open.
    let writer = OpenOptions::new()
        .access_mode(WRITE_OWNER.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
        .unwrap();
    let (descriptor, _allocation) = descriptor(&format!("O:{foreign}"));
    let mut sid = PSID::default();
    let mut defaulted = windows::Win32::Foundation::BOOL(0);
    // SAFETY: owned SDK-created owner-only descriptor and initialized outputs.
    unsafe { GetSecurityDescriptorOwner(descriptor, &raw mut sid, &raw mut defaulted) }.unwrap();
    // SAFETY: live write-only owner handle and retained foreign SID; no DACL/SACL/group flags.
    unsafe {
        SetSecurityInfo(
            HANDLE(writer.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            sid,
            PSID::default(),
            None,
            None,
        )
    }
    .ok()
    .unwrap();
    let final_state = inspect(&reader);
    final_state.exact_user_directory(foreign, standard);
    drop(writer);
    drop(reader);
    final_state
}
