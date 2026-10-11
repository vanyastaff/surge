//! Effective-token ownership and immutable protected creation security.
use super::native::{NativeError, NativeResult};
use std::{
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows::{
    Win32::{
        Foundation::{ERROR_NO_TOKEN, HANDLE, HLOCAL, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            CopySid, GetLengthSid, GetTokenInformation, IsValidSid, PSECURITY_DESCRIPTOR, PSID,
            TOKEN_INFORMATION_CLASS, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER, TokenOwner, TokenUser,
        },
        System::Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
        },
    },
    core::PCWSTR,
};

/// SDK local allocation, released by the allocator that produced it.
struct LocalAllocation(HLOCAL);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: this allocation is uniquely owned and came from a LocalAlloc API.
        unsafe { LocalFree(self.0) };
    }
}

/// Aligned, owned SID; no borrowed token memory escapes capture.
#[derive(Clone)]
pub(super) struct EffectiveOwner {
    words: Vec<usize>,
    length: usize,
    default_words: Vec<usize>,
    default_length: usize,
}
impl EffectiveOwner {
    pub(super) fn capture() -> NativeResult<Self> {
        let token = effective_token()?;
        let (words, length) = token_sid(&token, TokenUser)?;
        let (default_words, default_length) = token_sid(&token, TokenOwner)?;
        Ok(Self {
            words,
            length,
            default_words,
            default_length,
        })
    }
    fn sid(&self) -> PSID {
        PSID(self.words.as_ptr().cast_mut().cast())
    }
    pub(super) fn same_user(&self, other: &Self) -> bool {
        self.length == other.length && self.words == other.words
    }
    /// Explicit owner and protected user-only DACL, supplied at first creation.
    pub(super) fn creation(&self, inheritable: bool) -> NativeResult<CreationSecurity> {
        let mut text = windows::core::PWSTR::null();
        // SAFETY: owned validated SID and SDK-owned output string.
        unsafe { ConvertSidToStringSidW(self.sid(), &raw mut text) }?;
        let _text_owner = LocalAllocation(HLOCAL(text.0.cast()));
        // SAFETY: successful SID conversion returned a terminated SDK-owned string.
        let sid = unsafe { text.to_string() }
            .map_err(|_| NativeError::Security("invalid SID string encoding"))?;
        let flags = if inheritable { "OICI" } else { "" };
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;{flags};FA;;;{sid})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: generated descriptor uses only the validated token SID, fixed rights
        // and fixed ACE flags. Output is uniquely owned LocalAlloc memory.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }?;
        Ok(CreationSecurity {
            allocation: LocalAllocation(HLOCAL(descriptor.0)),
        })
    }
}

pub(super) struct CreationSecurity {
    allocation: LocalAllocation,
}
impl CreationSecurity {
    pub(super) fn descriptor(&self) -> PSECURITY_DESCRIPTOR {
        PSECURITY_DESCRIPTOR(self.allocation.0.0)
    }
}

/// Validate SID extent before invoking SDK routines that walk its subauthorities.
fn bounded_sid_length(sid: PSID, start: usize, end: usize) -> NativeResult<usize> {
    let address = sid.0 as usize;
    let header_end = address
        .checked_add(8)
        .ok_or(NativeError::Security("SID address overflow"))?;
    if address < start || header_end > end || !address.is_multiple_of(std::mem::align_of::<u32>()) {
        return Err(NativeError::Security("SID header outside owned storage"));
    }
    // SAFETY: the two SID header bytes lie inside the validated owned range.
    let count = usize::from(unsafe { *sid.0.cast::<u8>().add(1) });
    let length = 8usize
        .checked_add(count * 4)
        .ok_or(NativeError::Security("SID length overflow"))?;
    if address.checked_add(length).is_none_or(|limit| limit > end) {
        return Err(NativeError::Security("SID outside owned storage"));
    }
    // SAFETY: complete SID lies inside owned storage; SDK reads no external memory.
    if !unsafe { IsValidSid(sid) }.as_bool()
        || usize::try_from(unsafe { GetLengthSid(sid) }).ok() != Some(length)
    {
        return Err(NativeError::Security("invalid SID"));
    }
    Ok(length)
}

#[derive(Clone, Copy)]
pub(super) enum DirectoryPolicy {
    Ancestor,
    Protected { inheritable: bool },
    SqliteSidefile,
}

impl EffectiveOwner {
    /// Query the held object, never a second pathname or effective-access guess.
    pub(super) fn validate_directory(
        &self,
        file: &std::fs::File,
        policy: DirectoryPolicy,
    ) -> NativeResult<()> {
        use windows::Win32::Security::{
            ACL,
            Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, GetSecurityDescriptorLength,
            IsValidSecurityDescriptor, OWNER_SECURITY_INFORMATION, SE_DACL_PRESENT,
            SE_DACL_PROTECTED,
        };
        let mut owner = PSID::default();
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: live held file and correctly typed initialized SDK outputs.
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
        .ok()?;
        let _allocation = LocalAllocation(HLOCAL(descriptor.0));
        // SAFETY: successful GetSecurityInfo returned a live SDK descriptor allocation.
        if !unsafe { IsValidSecurityDescriptor(descriptor) }.as_bool() {
            return Err(NativeError::Security("invalid security descriptor"));
        }
        let start = descriptor.0 as usize;
        // SAFETY: valid descriptor remains SDK-owned during the query.
        let length = usize::try_from(unsafe { GetSecurityDescriptorLength(descriptor) })
            .map_err(|_| NativeError::Security("descriptor length overflow"))?;
        let end = start
            .checked_add(length)
            .ok_or(NativeError::Security("descriptor address overflow"))?;
        bounded_sid_length(owner, start, end)?;
        let outsider_child_creation = self.validate_owner(owner, policy)?;
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: valid descriptor and initialized ABI outputs.
        unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }?;
        if control & SE_DACL_PRESENT.0 == 0 || acl.is_null() {
            return Err(NativeError::Security("missing directory DACL"));
        }
        if matches!(policy, DirectoryPolicy::Protected { .. }) && control & SE_DACL_PROTECTED.0 == 0
        {
            return Err(NativeError::Security("directory DACL is not protected"));
        }
        self.validate_acl(acl, start, end, policy, outsider_child_creation)
    }
    // The SID must already be bounded within the held descriptor. These are
    // separate roles: ancestor ownership never broadens mutation-ACE trust or
    // SQLite's captured SYSTEM/Admin default-owner exception.
    fn validate_owner(&self, owner: PSID, policy: DirectoryPolicy) -> NativeResult<bool> {
        let owner_is_user = self.matches_sid(owner)?;
        let owner_is_system = trusted_maintenance_sid(owner)?;
        let installer_ancestor =
            matches!(policy, DirectoryPolicy::Ancestor) && trusted_installer_owner_sid(owner)?;
        let trusted_owner = match policy {
            DirectoryPolicy::Ancestor => owner_is_system || installer_ancestor,
            DirectoryPolicy::SqliteSidefile => {
                owner_is_system && self.matches_default_owner(owner)?
            },
            DirectoryPolicy::Protected { .. } => false,
        };
        if !owner_is_user && !trusted_owner {
            return Err(NativeError::Security("object owner is not trusted"));
        }
        // Only OS-maintained ancestors may grant outsiders creation of new
        // sibling names. Existing entry mutation is still rejected by validate_acl.
        Ok(matches!(policy, DirectoryPolicy::Ancestor) && (owner_is_system || installer_ancestor))
    }
    fn matches_default_owner(&self, sid: PSID) -> NativeResult<bool> {
        // SAFETY: caller bounded and validated the object SID and capture owns the token SID.
        let length = usize::try_from(unsafe { GetLengthSid(sid) })
            .map_err(|_| NativeError::Security("SID length overflow"))?;
        if length != self.default_length {
            return Ok(false);
        }
        // SAFETY: both extents were independently validated and remain live.
        Ok(
            unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), length) }
                == unsafe {
                    std::slice::from_raw_parts(self.default_words.as_ptr().cast::<u8>(), length)
                },
        )
    }
    fn matches_sid(&self, sid: PSID) -> NativeResult<bool> {
        // SAFETY: caller already bounded and validated the queried SID.
        let length = usize::try_from(unsafe { GetLengthSid(sid) })
            .map_err(|_| NativeError::Security("SID length overflow"))?;
        if length != self.length {
            return Ok(false);
        }
        // SAFETY: both SID extents are validated and live for this byte comparison.
        Ok(
            unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), length) }
                == unsafe { std::slice::from_raw_parts(self.sid().0.cast::<u8>(), length) },
        )
    }
    fn validate_acl(
        &self,
        acl: *mut windows::Win32::Security::ACL,
        start: usize,
        end: usize,
        policy: DirectoryPolicy,
        outsider_child_creation: bool,
    ) -> NativeResult<()> {
        use windows::Win32::{
            Security::{
                ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, GENERIC_MAPPING, GetAce, IsValidAcl,
                MapGenericMask,
            },
            Storage::FileSystem::{
                FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE,
                FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            },
            System::SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
        };
        let address = acl as usize;
        if address < start
            || !address.is_multiple_of(std::mem::align_of::<ACL>())
            || address
                .checked_add(size_of::<ACL>())
                .is_none_or(|v| v > end)
        {
            return Err(NativeError::Security("ACL header outside descriptor"));
        }
        // SAFETY: complete aligned SDK ACL header is bounded inside its descriptor.
        let header = unsafe { &*acl };
        let acl_end = address
            .checked_add(usize::from(header.AclSize))
            .ok_or(NativeError::Security("ACL address overflow"))?;
        if acl_end > end
            || usize::from(header.AclSize) < size_of::<ACL>()
            || !unsafe { IsValidAcl(acl) }.as_bool()
        {
            return Err(NativeError::Security("invalid ACL extent"));
        }
        if matches!(
            policy,
            DirectoryPolicy::Protected { .. } | DirectoryPolicy::SqliteSidefile
        ) && header.AceCount != 1
        {
            return Err(NativeError::Security(
                "protected ACL needs one exact allow ACE",
            ));
        }
        for index in 0..u32::from(header.AceCount) {
            let mut ace = std::ptr::null_mut();
            // SAFETY: valid bounded ACL, bounded index and initialized output.
            unsafe { GetAce(acl, index, &raw mut ace) }?;
            let position = ace as usize;
            if position < address + size_of::<ACL>()
                || !position.is_multiple_of(std::mem::align_of::<ACCESS_ALLOWED_ACE>())
                || position
                    .checked_add(size_of::<ACE_HEADER>())
                    .is_none_or(|v| v > acl_end)
            {
                return Err(NativeError::Security("ACE header outside ACL"));
            }
            // SAFETY: returned aligned ACE header lies inside the validated ACL.
            let ace_header = unsafe { &*ace.cast::<ACE_HEADER>() };
            let ace_end = position
                .checked_add(usize::from(ace_header.AceSize))
                .ok_or(NativeError::Security("ACE address overflow"))?;
            let kind = u32::from(ace_header.AceType);
            if ace_end > acl_end
                || usize::from(ace_header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
                || (kind != ACCESS_ALLOWED_ACE_TYPE && kind != ACCESS_DENIED_ACE_TYPE)
                || ace_header.AceFlags & !0x1f != 0
            {
                return Err(NativeError::Security("unsupported ACE shape"));
            }
            let offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
            // SAFETY: checked ACE contains the complete allow/deny layout; both share this prefix.
            let grant = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            // SAFETY: SID offset lies inside this complete ACE layout.
            let sid = PSID(unsafe { ace.cast::<u8>().add(offset) }.cast());
            bounded_sid_length(sid, position + offset, ace_end)?;
            let mut mask = grant.Mask;
            let mapping = GENERIC_MAPPING {
                GenericRead: FILE_GENERIC_READ.0,
                GenericWrite: FILE_GENERIC_WRITE.0,
                GenericExecute: FILE_GENERIC_EXECUTE.0,
                GenericAll: FILE_ALL_ACCESS.0,
            };
            // SAFETY: initialized mask and fixed filesystem mapping.
            unsafe { MapGenericMask(&raw mut mask, &mapping) };
            if mask & !FILE_ALL_ACCESS.0 != 0 {
                return Err(NativeError::Security("unknown ACE access rights"));
            }
            let user = self.matches_sid(sid)?;
            match policy {
                DirectoryPolicy::Protected { inheritable } => {
                    let flags = if inheritable { 3 } else { 0 };
                    if !user
                        || kind != ACCESS_ALLOWED_ACE_TYPE
                        || mask != FILE_ALL_ACCESS.0
                        || ace_header.AceFlags != flags
                    {
                        return Err(NativeError::Security(
                            "protected ACL is not exact user-only",
                        ));
                    }
                },
                DirectoryPolicy::SqliteSidefile => {
                    if !user
                        || kind != ACCESS_ALLOWED_ACE_TYPE
                        || mask != FILE_ALL_ACCESS.0
                        || !matches!(ace_header.AceFlags, 0 | 16)
                    {
                        return Err(NativeError::Security("SQLite sidefile has unsafe grants"));
                    }
                },
                DirectoryPolicy::Ancestor => {
                    if kind == ACCESS_DENIED_ACE_TYPE
                        || ace_header.AceFlags & 8 != 0
                        || user
                        || trusted_maintenance_sid(sid)?
                    {
                        continue;
                    }
                    let mut permitted = FILE_GENERIC_READ.0 | FILE_GENERIC_EXECUTE.0;
                    if outsider_child_creation {
                        permitted |= FILE_ADD_FILE.0 | FILE_ADD_SUBDIRECTORY.0;
                    }
                    if mask & !permitted != 0 {
                        return Err(NativeError::Security("ancestor grants outsider mutation"));
                    }
                },
            }
        }
        Ok(())
    }
}
// This exact OS installer identity is accepted only in ancestor-owner roles.
// It deliberately does not join trusted_maintenance_sid's mutation-ACE principals.
fn trusted_installer_owner_sid(sid: PSID) -> NativeResult<bool> {
    let mut text = windows::core::PWSTR::null();
    // SAFETY: caller independently bounded and validated the descriptor owner SID.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }?;
    let _allocation = LocalAllocation(HLOCAL(text.0.cast()));
    // SAFETY: successful conversion returned a terminated SDK-owned string.
    let value = unsafe { text.to_string() }
        .map_err(|_| NativeError::Security("invalid SID string encoding"))?;
    Ok(value == "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464")
}
fn trusted_maintenance_sid(sid: PSID) -> NativeResult<bool> {
    let mut text = windows::core::PWSTR::null();
    // SAFETY: callers have independently bounded and validated this SID.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }?;
    let _allocation = LocalAllocation(HLOCAL(text.0.cast()));
    // SAFETY: successful conversion returned a terminated owned SDK string.
    let value = unsafe { text.to_string() }
        .map_err(|_| NativeError::Security("invalid SID string encoding"))?;
    Ok(value == "S-1-5-18" || value == "S-1-5-32-544")
}

/// Capture the effective token; absence is the sole permitted process fallback.
pub(super) fn effective_token() -> NativeResult<OwnedHandle> {
    let mut token = HANDLE::default();
    // SAFETY: pseudo thread handle and initialized output; TOKEN_QUERY only.
    match unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &raw mut token) } {
        Ok(()) => {},
        Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_NO_TOKEN.0) => {
            // SAFETY: fall back only when the calling thread has no token.
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }?;
        },
        Err(error) => return Err(error.into()),
    }
    // SAFETY: successful token query returned a uniquely owned real handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(token.0) })
}

fn token_sid(
    token: &OwnedHandle,
    class: TOKEN_INFORMATION_CLASS,
) -> NativeResult<(Vec<usize>, usize)> {
    let mut required = 0;
    let mut storage = [0usize; 128];
    let capacity = u32::try_from(size_of::<[usize; 128]>())
        .map_err(|_| NativeError::Security("token buffer length overflow"))?;
    // SAFETY: aligned buffer has the exact supplied capacity and lives through the call.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            class,
            Some(storage.as_mut_ptr().cast()),
            capacity,
            &raw mut required,
        )
    }?;
    let length =
        usize::try_from(required).map_err(|_| NativeError::Security("token length overflow"))?;
    if length
        < if class == TokenUser {
            size_of::<TOKEN_USER>()
        } else {
            size_of::<TOKEN_OWNER>()
        }
        || length > size_of::<[usize; 128]>()
    {
        return Err(NativeError::Security("invalid token user buffer"));
    }
    // SAFETY: aligned buffer has a complete TOKEN_USER header from the successful API.
    let sid = if class == TokenUser {
        unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    } else {
        // SAFETY: the successful TokenOwner query returned a complete aligned TOKEN_OWNER.
        unsafe { (*storage.as_ptr().cast::<TOKEN_OWNER>()).Owner }
    };
    let start = storage.as_ptr() as usize;
    let end = start
        .checked_add(length)
        .ok_or(NativeError::Security("token address overflow"))?;
    let sid_length = bounded_sid_length(sid, start, end)?;
    let word_count = sid_length.div_ceil(size_of::<usize>());
    let mut words = vec![0usize; word_count];
    // SAFETY: source SID is validated within the token buffer; destination is aligned
    // and rounded up to cover the complete SID length.
    unsafe {
        CopySid(
            u32::try_from(sid_length).map_err(|_| NativeError::Security("SID length overflow"))?,
            PSID(words.as_mut_ptr().cast()),
            sid,
        )
    }?;
    Ok((words, sid_length))
}

#[cfg(test)]
mod tests {
    use super::{DirectoryPolicy, EffectiveOwner, NativeError, bounded_sid_length};
    use windows::Win32::{
        Foundation::BOOL,
        Security::{
            ACCESS_ALLOWED_ACE, ACL, GetAce, GetSecurityDescriptorDacl,
            GetSecurityDescriptorLength, PSID,
        },
    };

    #[test]
    fn malformed_acl_extents_and_unknown_ace_refuse_with_owned_buffers() {
        let owner = EffectiveOwner::capture().unwrap();
        for mutation in ["short extent", "oversized extent", "unknown ACE"] {
            let descriptor = owner.creation(true).unwrap();
            let mut present = BOOL(0);
            let mut defaulted = BOOL(0);
            let mut acl: *mut ACL = std::ptr::null_mut();
            // SAFETY: uniquely owned valid SDK allocation and initialized outputs.
            unsafe {
                GetSecurityDescriptorDacl(
                    descriptor.descriptor(),
                    &raw mut present,
                    &raw mut acl,
                    &raw mut defaulted,
                )
            }
            .unwrap();
            assert!(present.as_bool() && !acl.is_null());
            let start = descriptor.descriptor().0 as usize;
            // SAFETY: allocation is still valid before the deliberate mutation below.
            let length = unsafe { GetSecurityDescriptorLength(descriptor.descriptor()) } as usize;
            let end = start + length;
            owner
                .validate_acl(
                    acl,
                    start,
                    end,
                    DirectoryPolicy::Protected { inheritable: true },
                    false,
                )
                .unwrap();
            let mut ace = std::ptr::null_mut();
            // SAFETY: original ACL has exactly one valid ACE; output is bounded by the SDK.
            unsafe { GetAce(acl, 0, &raw mut ace) }.unwrap();
            // SAFETY: these fields are inside the owned SDK allocation. Mutation cannot
            // change the allocation extent; the production parser must reject the bytes.
            unsafe {
                match mutation {
                    "short extent" => (*acl).AclSize = 4,
                    "oversized extent" => (*acl).AclSize = u16::MAX,
                    "unknown ACE" => (*ace.cast::<ACCESS_ALLOWED_ACE>()).Header.AceType = u8::MAX,
                    _ => unreachable!(),
                }
            }
            assert!(matches!(
                owner.validate_acl(
                    acl,
                    start,
                    end,
                    DirectoryPolicy::Protected { inheritable: true },
                    false
                ),
                Err(NativeError::Security(_))
            ));
        }
    }

    #[test]
    fn malformed_sid_lengths_refuse_before_sdk_walks_outside_owned_storage() {
        let mut words = [0usize; 4];
        let pointer = words.as_mut_ptr().cast::<u8>();
        // SAFETY: both header bytes are inside the live aligned stack allocation.
        unsafe {
            *pointer = 1;
            *pointer.add(1) = u8::MAX;
        }
        let start = pointer as usize;
        let end = start + std::mem::size_of_val(&words);
        assert!(matches!(
            bounded_sid_length(PSID(pointer.cast()), start, end),
            Err(NativeError::Security(_))
        ));
        assert!(matches!(
            bounded_sid_length(PSID(pointer.cast()), start, start + 4),
            Err(NativeError::Security(_))
        ));
    }
}

#[cfg(test)]
mod ancestor_owner_role_tests {
    use super::{
        CreationSecurity, DirectoryPolicy, EffectiveOwner, LocalAllocation, NativeError,
        NativeResult, bounded_sid_length,
    };
    use std::mem::size_of;
    use windows::{
        Win32::{
            Foundation::{BOOL, HLOCAL},
            Security::{
                ACL,
                Authorization::{
                    ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
                    SDDL_REVISION_1,
                },
                CopySid, GetLengthSid, GetSecurityDescriptorDacl, GetSecurityDescriptorLength,
                GetSecurityDescriptorOwner, IsValidSid, PSECURITY_DESCRIPTOR, PSID,
            },
        },
        core::PCWSTR,
    };

    const USER: &str = "S-1-5-21-101-202-303-1001";
    const FOREIGN: &str = "S-1-5-21-101-202-303-1002";
    const INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

    // Synthetic owned token SID buffers exercise policy separation only. They
    // do not claim an actual token, privileged owner assignment or filesystem proof.
    fn sid_words(text: &str) -> (Vec<usize>, usize) {
        let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut sid = PSID::default();
        // SAFETY: terminated literal input and initialized SDK-owned output.
        unsafe { ConvertStringSidToSidW(PCWSTR(text.as_ptr()), &raw mut sid) }.unwrap();
        let _allocation = LocalAllocation(HLOCAL(sid.0));
        // SAFETY: successful SDK conversion owns a complete SID allocation.
        assert!(unsafe { IsValidSid(sid) }.as_bool());
        // SAFETY: the SDK-owned SID was validated above and remains live.
        let length = usize::try_from(unsafe { GetLengthSid(sid) }).unwrap();
        let mut words = vec![0usize; length.div_ceil(size_of::<usize>())];
        // SAFETY: validated source, aligned initialized destination covering the entire SID.
        unsafe {
            CopySid(
                u32::try_from(length).unwrap(),
                PSID(words.as_mut_ptr().cast()),
                sid,
            )
        }
        .unwrap();
        (words, length)
    }
    fn owner(default: &str) -> EffectiveOwner {
        let (words, length) = sid_words(USER);
        let (default_words, default_length) = sid_words(default);
        EffectiveOwner {
            words,
            length,
            default_words,
            default_length,
        }
    }
    fn descriptor(text: &str) -> CreationSecurity {
        let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: terminated test SDDL and initialized uniquely owned SDK output.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(text.as_ptr()),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )
        }
        .unwrap();
        CreationSecurity {
            allocation: LocalAllocation(HLOCAL(descriptor.0)),
        }
    }
    fn validate_descriptor(
        effective: &EffectiveOwner,
        object_owner: &str,
        aces: &str,
        policy: DirectoryPolicy,
    ) -> NativeResult<bool> {
        let descriptor = descriptor(&format!("O:{object_owner}D:P{aces}"));
        let mut sid = PSID::default();
        let mut defaulted = BOOL(0);
        let mut present = BOOL(0);
        let mut acl: *mut ACL = std::ptr::null_mut();
        // SAFETY: immutable SDK-created descriptor remains alive through both queries.
        unsafe {
            GetSecurityDescriptorOwner(descriptor.descriptor(), &raw mut sid, &raw mut defaulted)
        }
        .unwrap();
        // SAFETY: same valid descriptor and initialized ABI outputs.
        unsafe {
            GetSecurityDescriptorDacl(
                descriptor.descriptor(),
                &raw mut present,
                &raw mut acl,
                &raw mut defaulted,
            )
        }
        .unwrap();
        assert!(present.as_bool() && !acl.is_null());
        let start = descriptor.descriptor().0 as usize;
        // SAFETY: SDK-created descriptor is unchanged and still owned.
        let end = start
            + usize::try_from(unsafe { GetSecurityDescriptorLength(descriptor.descriptor()) })
                .unwrap();
        bounded_sid_length(sid, start, end).unwrap();
        let permits_creation = effective.validate_owner(sid, policy)?;
        effective.validate_acl(acl, start, end, policy, permits_creation)?;
        Ok(permits_creation)
    }

    #[test]
    fn exact_installer_owner_is_ancestor_only_even_when_token_default_owner_matches() {
        let user_only = format!("(A;;FA;;;{USER})");
        let effective = owner(USER);
        for sid in ["S-1-5-18", "S-1-5-32-544", INSTALLER] {
            assert!(
                validate_descriptor(&effective, sid, &user_only, DirectoryPolicy::Ancestor)
                    .unwrap()
            );
            assert!(matches!(
                validate_descriptor(
                    &effective,
                    sid,
                    &user_only,
                    DirectoryPolicy::Protected { inheritable: false }
                ),
                Err(NativeError::Security("object owner is not trusted"))
            ));
        }
        for sid in [
            FOREIGN,
            "S-1-5-80-1-2-3-4-5",
            "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478465",
        ] {
            assert!(matches!(
                validate_descriptor(&effective, sid, &user_only, DirectoryPolicy::Ancestor),
                Err(NativeError::Security("object owner is not trusted"))
            ));
        }
        assert!(
            !validate_descriptor(&effective, USER, &user_only, DirectoryPolicy::Ancestor).unwrap()
        );
        assert!(
            !validate_descriptor(
                &effective,
                USER,
                &user_only,
                DirectoryPolicy::Protected { inheritable: false }
            )
            .unwrap()
        );
        for sid in ["S-1-5-18", "S-1-5-32-544"] {
            assert!(
                !validate_descriptor(
                    &owner(sid),
                    sid,
                    &user_only,
                    DirectoryPolicy::SqliteSidefile
                )
                .unwrap()
            );
            assert!(matches!(
                validate_descriptor(&effective, sid, &user_only, DirectoryPolicy::SqliteSidefile),
                Err(NativeError::Security("object owner is not trusted"))
            ));
        }
        assert!(matches!(
            validate_descriptor(
                &owner(INSTALLER),
                INSTALLER,
                &user_only,
                DirectoryPolicy::SqliteSidefile
            ),
            Err(NativeError::Security("object owner is not trusted"))
        ));
    }

    #[test]
    fn installer_owner_does_not_trust_installer_or_other_service_mutation_aces() {
        let effective = owner(USER);
        for sid in [USER, "S-1-5-18", "S-1-5-32-544"] {
            let aces = format!("(A;;FA;;;{USER})(A;;FA;;;{sid})");
            assert!(
                validate_descriptor(&effective, INSTALLER, &aces, DirectoryPolicy::Ancestor)
                    .unwrap()
            );
        }
        for sid in [INSTALLER, "S-1-5-80-1-2-3-4-5", FOREIGN] {
            let read_only = format!("(A;;FA;;;{USER})(A;;0x1200a9;;;{sid})");
            assert!(
                validate_descriptor(&effective, INSTALLER, &read_only, DirectoryPolicy::Ancestor)
                    .unwrap()
            );
            let mutation = format!("(A;;FA;;;{USER})(A;;FA;;;{sid})");
            assert!(matches!(
                validate_descriptor(&effective, INSTALLER, &mutation, DirectoryPolicy::Ancestor),
                Err(NativeError::Security("ancestor grants outsider mutation"))
            ));
        }
        let extra_installer = format!("(A;;FA;;;{USER})(A;;FA;;;{INSTALLER})");
        assert!(matches!(
            validate_descriptor(
                &effective,
                USER,
                &extra_installer,
                DirectoryPolicy::Protected { inheritable: false }
            ),
            Err(NativeError::Security(
                "protected ACL needs one exact allow ACE"
            ))
        ));
    }

    #[test]
    fn installer_child_creation_allowance_excludes_existing_entry_mutation() {
        let effective = owner(USER);
        for mask in [0x2u32, 0x4, 0x6] {
            let aces = format!("(A;;FA;;;{USER})(A;;{mask:#x};;;{FOREIGN})");
            for sid in [INSTALLER, "S-1-5-18", "S-1-5-32-544"] {
                assert!(
                    validate_descriptor(&effective, sid, &aces, DirectoryPolicy::Ancestor).unwrap()
                );
            }
            assert!(matches!(
                validate_descriptor(&effective, USER, &aces, DirectoryPolicy::Ancestor),
                Err(NativeError::Security("ancestor grants outsider mutation"))
            ));
        }
        // Independent literal SDK rights: DELETE_CHILD, DELETE, WRITE_DAC,
        // WRITE_OWNER, WRITE_ATTRIBUTES, WRITE_EA and GENERIC_WRITE respectively.
        for forbidden in [0x40u32, 0x10000, 0x40000, 0x80000, 0x100, 0x10, 0x4000_0000] {
            let mask = 0x6 | forbidden;
            let aces = format!("(A;;FA;;;{USER})(A;;{mask:#x};;;{FOREIGN})");
            assert!(matches!(
                validate_descriptor(&effective, INSTALLER, &aces, DirectoryPolicy::Ancestor),
                Err(NativeError::Security("ancestor grants outsider mutation"))
            ));
        }
    }
}
