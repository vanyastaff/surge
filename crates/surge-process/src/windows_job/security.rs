//! Owner-only protected security descriptors for named jobs and pipes.
use super::{invalid, owned, raw, size_u32, win_error};
use std::io;
use windows::Win32::Foundation::{BOOL, ERROR_NO_TOKEN, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorControl,
    GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation, IsValidAcl,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};
use windows::core::{PCWSTR, PWSTR};

pub(super) struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    pub(super) fn current_user() -> io::Result<Self> {
        Self::with_rights("0x001f003f")
    }

    pub(super) fn pipe() -> io::Result<Self> {
        Self::with_rights("GA")
    }

    fn with_rights(rights: &str) -> io::Result<Self> {
        let sid = current_sid()?;
        let sddl = format!("O:{sid}D:P(A;;{rights};;;{sid})");
        Self::from_sddl(&sddl)
    }

    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let wide = super::command::wide(std::ffi::OsStr::new(&sddl))?;
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: terminated SDDL input is live and SDK returns a LocalAlloc descriptor.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wide.as_ptr()),
                1,
                &raw mut descriptor,
                None,
            )
        }
        .map_err(win_error)?;
        Ok(Self(descriptor))
    }

    #[cfg(test)]
    pub(super) fn untrusted(permissive: bool) -> io::Result<Self> {
        let sid = current_sid()?;
        let acl = if permissive { "D:P(A;;GA;;;WD)" } else { "D:P" };
        Self::from_sddl(&format!("O:{sid}{acl}"))
    }

    pub(super) fn attributes(&self, inherit: bool) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0.0,
            bInheritHandle: inherit.into(),
        }
    }

    fn owner(&self) -> io::Result<PSID> {
        let mut owner = PSID::default();
        let mut defaulted = BOOL::default();
        // SAFETY: descriptor is live; SDK returns a borrowed SID inside it.
        unsafe { GetSecurityDescriptorOwner(self.0, &raw mut owner, &raw mut defaulted) }
            .map_err(win_error)?;
        if owner.0.is_null() || defaulted.as_bool() {
            return Err(invalid("job owner unavailable"));
        }
        Ok(owner)
    }

    fn acl(&self) -> io::Result<&[u8]> {
        let mut present = BOOL::default();
        let mut defaulted = BOOL::default();
        let mut acl = std::ptr::null_mut::<ACL>();
        // SAFETY: descriptor is live; SDK returns a borrowed ACL inside it.
        unsafe {
            GetSecurityDescriptorDacl(self.0, &raw mut present, &raw mut acl, &raw mut defaulted)
        }
        .map_err(win_error)?;
        if !present.as_bool() || defaulted.as_bool() || acl.is_null() {
            return Err(invalid("job DACL unavailable"));
        }
        // SAFETY: non-null ACL came from the retained valid SDK descriptor.
        if !unsafe { IsValidAcl(acl) }.as_bool() {
            return Err(invalid("invalid job DACL"));
        }
        // SAFETY: IsValidAcl validates the ACL header and bounded ACL allocation.
        let length = usize::from(unsafe { (*acl).AclSize });
        if length < std::mem::size_of::<ACL>() || length > 4096 {
            return Err(invalid("unexpected job DACL size"));
        }
        // SAFETY: SDK validated ACL size; returned borrow cannot outlive descriptor.
        Ok(unsafe { std::slice::from_raw_parts(acl.cast(), length) })
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: this object uniquely owns the LocalAlloc descriptor allocation.
        let _ = unsafe { LocalFree(HLOCAL(self.0.0)) };
    }
}

pub(super) fn verify(handle: HANDLE) -> io::Result<()> {
    let expected = Descriptor::current_user()?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: retained kernel-object handle and valid output slot; SDK allocates descriptor.
    unsafe {
        GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            Some(&raw mut descriptor),
        )
    }
    .ok()
    .map_err(win_error)?;
    let actual = Descriptor(descriptor);
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: actual descriptor is retained for the complete validation.
    unsafe { GetSecurityDescriptorControl(actual.0, &raw mut control, &raw mut revision) }
        .map_err(win_error)?;
    if control & 0x1000 == 0 {
        return Err(invalid("job DACL is not protected"));
    }
    // SAFETY: both owner SID pointers remain borrowed from live descriptors.
    unsafe { EqualSid(actual.owner()?, expected.owner()?) }.map_err(win_error)?;
    if actual.acl()? != expected.acl()? {
        return Err(invalid("job DACL does not match owner-only policy"));
    }
    Ok(())
}

pub(super) fn current_sid() -> io::Result<String> {
    reject_impersonation()?;
    let mut token = HANDLE::default();
    // SAFETY: pseudo current-process handle is valid; SDK returns a fresh token handle.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }
        .map_err(win_error)?;
    let token = owned(token)?;
    let mut length = 0;
    // SAFETY: zero-length discovery call writes required buffer size only.
    let _ = unsafe { GetTokenInformation(raw(&token), TokenUser, None, 0, &raw mut length) };
    if length < size_u32::<TOKEN_USER>()? || length > 65536 {
        return Err(invalid("unexpected token size"));
    }
    let words = (length as usize).div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0_usize; words];
    // SAFETY: usize allocation provides sufficient size/alignment for TOKEN_USER.
    unsafe {
        GetTokenInformation(
            raw(&token),
            TokenUser,
            Some(storage.as_mut_ptr().cast()),
            length,
            &raw mut length,
        )
    }
    .map_err(win_error)?;
    // SAFETY: successful query populated a complete aligned TOKEN_USER.
    let sid = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = PWSTR::null();
    // SAFETY: SID is live in storage; SDK allocates a terminated textual SID.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.map_err(win_error)?;
    struct Text(PWSTR);
    impl Drop for Text {
        fn drop(&mut self) {
            // SAFETY: uniquely owns the LocalAlloc textual SID.
            let _ = unsafe { LocalFree(HLOCAL(self.0.0.cast())) };
        }
    }
    let text = Text(text);
    // SAFETY: successful conversion returned an allocated, terminated UTF-16 string.
    unsafe { text.0.to_string() }.map_err(|_| invalid("invalid token SID text"))
}

fn reject_impersonation() -> io::Result<()> {
    let mut token = HANDLE::default();
    // SAFETY: query only; any returned token handle is immediately owned.
    match unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &raw mut token) } {
        Ok(()) => {
            drop(owned(token)?);
            Err(invalid("MCP job ownership rejects thread impersonation"))
        },
        Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_NO_TOKEN.0) => {
            Ok(())
        },
        Err(error) => Err(win_error(error)),
    }
}
