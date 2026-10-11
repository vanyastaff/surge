//! Query the narrowly duplicated setup token without adjusting privileges.
pub(super) fn assert_restore_privilege() {
    use std::{
        mem::{offset_of, size_of},
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows::Win32::{
        Foundation::{HANDLE, LUID},
        Security::{
            GetTokenInformation, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_PRIVILEGES,
            TOKEN_QUERY, TokenPrivileges,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    use windows::core::PCWSTR;
    let mut handle = HANDLE::default();
    // SAFETY: process pseudo handle and valid initialized output; query-only token ownership.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut handle) }.unwrap();
    // SAFETY: newly returned owned handle transfers exactly once.
    let token = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let mut words = [0usize; 256];
    let mut returned = 0;
    // SAFETY: matching aligned writable capacity and live token.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenPrivileges,
            Some(words.as_mut_ptr().cast()),
            u32::try_from(size_of::<[usize; 256]>()).unwrap(),
            &raw mut returned,
        )
    }
    .unwrap();
    assert!(
        (size_of::<TOKEN_PRIVILEGES>()..=size_of::<[usize; 256]>())
            .contains(&usize::try_from(returned).unwrap())
    );
    let name: Vec<u16> = "SeRestorePrivilege".encode_utf16().chain(Some(0)).collect();
    let mut luid = LUID::default();
    // SAFETY: initialized output and terminated privilege-name input.
    unsafe { LookupPrivilegeValueW(None, PCWSTR(name.as_ptr()), &raw mut luid) }.unwrap();
    // SAFETY: class-specific aligned header is completely contained in the returned allocation.
    let privileges = unsafe { &*words.as_ptr().cast::<TOKEN_PRIVILEGES>() };
    let count = usize::try_from(privileges.PrivilegeCount).unwrap();
    let offset = offset_of!(TOKEN_PRIVILEGES, Privileges);
    assert!(
        offset + count * size_of::<windows::Win32::Security::LUID_AND_ATTRIBUTES>()
            <= usize::try_from(returned).unwrap()
    );
    // SAFETY: complete counted array extent was checked against actual SDK returned length.
    let entries = unsafe { std::slice::from_raw_parts(privileges.Privileges.as_ptr(), count) };
    assert!(
        entries
            .iter()
            .any(|entry| entry.Luid.LowPart == luid.LowPart
                && entry.Luid.HighPart == luid.HighPart
                && entry.Attributes & SE_PRIVILEGE_ENABLED != Default::default())
    );
}
