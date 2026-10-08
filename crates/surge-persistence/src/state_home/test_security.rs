//! SDK-only acceptance observations, independent of production policy verification.
use std::{
    fs::OpenOptions,
    mem::{offset_of, size_of},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
use windows::{
    Win32::{
        Foundation::{ERROR_NO_TOKEN, HANDLE, HLOCAL, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl, GetTokenInformation,
            IsValidAcl, IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            SE_DACL_PRESENT, SE_DACL_PROTECTED, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER, TokenOwner,
            TokenUser,
        },
        Storage::FileSystem::{
            FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
        },
        System::{
            SystemServices::ACCESS_ALLOWED_ACE_TYPE,
            Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
        },
    },
    core::PWSTR,
};

struct Allocation(HLOCAL);
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: unique ownership of one SDK LocalAlloc allocation.
        unsafe { LocalFree(self.0) };
    }
}
fn sid_text(sid: PSID) -> String {
    // SAFETY: callers retain complete validated SDK SID storage for this call.
    assert!(unsafe { IsValidSid(sid) }.as_bool());
    let mut text = PWSTR::null();
    // SAFETY: valid SID and initialized SDK output; allocation lives through conversion.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.unwrap();
    let _allocation = Allocation(HLOCAL(text.0.cast()));
    // SAFETY: successful conversion returned a terminated owned UTF-16 string.
    unsafe { text.to_string() }.unwrap()
}
fn token_principals() -> (String, String) {
    token_principals_for_actor(&std::env::var("SURGE_NATIVE_PROBE_SID").unwrap())
}
pub(crate) fn token_principals_for_actor(expected_actor: &str) -> (String, String) {
    let mut handle = HANDLE::default();
    // SAFETY: query-only pseudo thread handle and initialized real-token output.
    let thread = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &raw mut handle) };
    if let Err(error) = thread {
        assert_eq!(error.code(), ERROR_NO_TOKEN.to_hresult());
        // SAFETY: absence of a thread token is the sole allowed process fallback.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut handle) }.unwrap();
    }
    // SAFETY: transfer the one newly returned handle into RAII ownership.
    let token = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let mut principals = Vec::new();
    for class in [TokenUser, TokenOwner] {
        let mut words = [0usize; 128];
        let mut returned = 0;
        // SAFETY: exact aligned capacity and initialized size output remain live.
        unsafe {
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                class,
                Some(words.as_mut_ptr().cast()),
                u32::try_from(size_of::<[usize; 128]>()).unwrap(),
                &raw mut returned,
            )
        }
        .unwrap();
        let header = if class == TokenUser {
            size_of::<TOKEN_USER>()
        } else {
            size_of::<TOKEN_OWNER>()
        };
        assert!((header..=size_of::<[usize; 128]>()).contains(&usize::try_from(returned).unwrap()));
        // SAFETY: successful class-specific query returned a complete aligned header.
        let sid = unsafe {
            if class == TokenUser {
                (*words.as_ptr().cast::<TOKEN_USER>()).User.Sid
            } else {
                (*words.as_ptr().cast::<TOKEN_OWNER>()).Owner
            }
        };
        let start = words.as_ptr() as usize;
        let end = start + usize::try_from(returned).unwrap();
        assert!(sid.0 as usize >= start && sid.0 as usize + 8 <= end);
        // SAFETY: checked SID header lies inside retained token storage.
        let count = usize::from(unsafe { *sid.0.cast::<u8>().add(1) });
        assert!(sid.0 as usize + 8 + count * 4 <= end);
        principals.push(sid_text(sid));
    }
    let user = principals.remove(0);
    assert_eq!(user, expected_actor);
    (user, principals.remove(0))
}
struct Security {
    owner: String,
    control: u16,
    principal: String,
    mask: u32,
    flags: u8,
}
pub(crate) fn open_observer(path: &Path) -> std::fs::File {
    OpenOptions::new()
        .access_mode((READ_CONTROL | FILE_READ_ATTRIBUTES).0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
        .unwrap()
}
fn observe(file: &std::fs::File) -> Security {
    let mut owner = PSID::default();
    let mut acl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: held descriptor and matching initialized SDK outputs.
    unsafe {
        GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&raw mut owner),
            None,
            Some(&raw mut acl),
            None,
            Some(&raw mut descriptor),
        )
    }
    .ok()
    .unwrap();
    let _allocation = Allocation(HLOCAL(descriptor.0));
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: successful SDK-owned descriptor stays live throughout observation.
    unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }
        .unwrap();
    assert_ne!(control & SE_DACL_PRESENT.0, 0);
    // SAFETY: returned ACL is retained by the successful security query allocation.
    assert!(unsafe { !acl.is_null() && IsValidAcl(acl).as_bool() });
    // SAFETY: SDK-validated ACL header remains live.
    assert_eq!(
        unsafe { (*acl).AceCount },
        1,
        "exactly one user allow ACE required"
    );
    let mut ace = std::ptr::null_mut();
    // SAFETY: validated one-ACE ACL and initialized pointer output.
    unsafe { GetAce(acl, 0, &raw mut ace) }.unwrap();
    // SAFETY: SDK returned a live aligned ACE header.
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    assert_eq!(u32::from(header.AceType), ACCESS_ALLOWED_ACE_TYPE);
    assert!(usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>());
    // SAFETY: full ACCESS_ALLOWED_ACE type and extent verified above.
    let grant = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    let offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    assert!(usize::from(header.AceSize) >= offset + 8);
    // SAFETY: complete SID header is inside the retained ACE.
    let sid = unsafe { ace.cast::<u8>().add(offset) };
    // SAFETY: count byte lies in the checked SID header.
    let count = usize::from(unsafe { *sid.add(1) });
    assert!(usize::from(header.AceSize) >= offset + 8 + count * 4);
    Security {
        owner: sid_text(owner),
        control,
        principal: sid_text(PSID(sid.cast())),
        mask: grant.Mask,
        flags: header.AceFlags,
    }
}

pub(crate) fn assert_private_directory(path: &Path) {
    assert_private_for_actor(
        path,
        &std::env::var("SURGE_NATIVE_PROBE_SID").unwrap(),
        true,
    );
}
pub(crate) fn assert_private_for_actor(path: &Path, expected_actor: &str, directory: bool) {
    assert_private_handle(&open_observer(path), expected_actor, directory);
}
pub(crate) fn assert_private_handle(file: &std::fs::File, expected_actor: &str, directory: bool) {
    let observed = observe(file);
    let (user, _) = token_principals_for_actor(expected_actor);
    assert_eq!(observed.owner, user);
    assert_eq!(observed.principal, user);
    assert_ne!(observed.control & SE_DACL_PROTECTED.0, 0);
    assert_eq!(observed.mask, FILE_ALL_ACCESS.0);
    assert_eq!(
        observed.flags,
        if directory { 3 } else { 0 },
        "exact protected object inheritance policy required"
    );
}
pub(crate) fn assert_sqlite_sidefile(path: &Path) {
    let observed = observe(&open_observer(path));
    let (user, default_owner) = token_principals();
    assert!(
        observed.owner == user
            || (observed.owner == default_owner
                && matches!(default_owner.as_str(), "S-1-5-18" | "S-1-5-32-544")),
        "unexpected actual SQLite sidefile owner: {}",
        observed.owner
    );
    assert_eq!(observed.principal, user);
    assert_eq!(observed.mask, FILE_ALL_ACCESS.0);
    assert!(
        matches!(observed.flags, 0 | 16),
        "only effective explicit/inherited file grants allowed"
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) volume: u64,
    pub(crate) id: [u8; 16],
}
pub(crate) fn file_identity(file: &std::fs::File) -> FileIdentity {
    use windows::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    let mut identity = FILE_ID_INFO::default();
    // SAFETY: actual open descriptor and exact initialized SDK output layout/size.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileIdInfo,
            (&raw mut identity).cast(),
            u32::try_from(size_of::<FILE_ID_INFO>()).unwrap(),
        )
    }
    .unwrap();
    FileIdentity {
        volume: identity.VolumeSerialNumber,
        id: identity.FileId.Identifier,
    }
}
pub(crate) fn path_identity(path: &Path) -> FileIdentity {
    file_identity(&open_observer(path))
}
