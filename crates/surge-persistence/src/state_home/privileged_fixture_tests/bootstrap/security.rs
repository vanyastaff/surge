//! Independent SDK observations for the privileged launch boundary, not runtime policy reuse.
use std::{
    mem::{align_of, offset_of, size_of},
    os::windows::io::{FromRawHandle, OwnedHandle},
};
use windows::{
    Win32::{
        Foundation::{ERROR_NO_TOKEN, HANDLE, HLOCAL, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, GENERIC_MAPPING, GetAce, GetSecurityDescriptorControl,
            GetSecurityDescriptorLength, IsValidAcl, IsValidSecurityDescriptor, IsValidSid,
            MapGenericMask, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            SE_DACL_PRESENT, SE_DACL_PROTECTED, TOKEN_QUERY,
        },
        Storage::FileSystem::{
            FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE,
            FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        },
        System::{
            SystemServices::{ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE},
            Threading::{GetCurrentThread, OpenThreadToken},
        },
    },
    core::PWSTR,
};

const INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Policy {
    Ancestor,
    Invocation,
    File,
}

struct Allocation(HLOCAL);
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: uniquely owns the successful SDK LocalAlloc output, freed exactly once.
        unsafe { LocalFree(self.0) };
    }
}

pub(super) fn reject_impersonation() {
    let mut token = HANDLE::default();
    // SAFETY: pseudo current-thread handle and initialized token output; query access only.
    let result = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &raw mut token) };
    match result {
        Ok(()) => {
            // SAFETY: successful token query returned one unique real handle.
            let _token = unsafe { OwnedHandle::from_raw_handle(token.0) };
            panic!("privileged bootstrap refuses thread impersonation");
        },
        Err(error) => assert_eq!(error.code(), ERROR_NO_TOKEN.to_hresult()),
    }
}

fn trusted(sid: &str, actor: &str) -> bool {
    sid == actor || matches!(sid, "S-1-5-18" | "S-1-5-32-544")
}

pub(super) fn verify(handle: HANDLE, actor: &str, policy: Policy) {
    let mut owner = PSID::default();
    let mut acl = std::ptr::null_mut::<ACL>();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: retained handle with READ_CONTROL and complete initialized SDK outputs.
    unsafe {
        GetSecurityInfo(
            handle,
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
    // SAFETY: successful query returned a retained SDK security descriptor allocation.
    assert!(unsafe { IsValidSecurityDescriptor(descriptor) }.as_bool());
    let start = descriptor.0 as usize;
    // SAFETY: independently SDK-validated descriptor remains live.
    let length = usize::try_from(unsafe { GetSecurityDescriptorLength(descriptor) }).unwrap();
    let end = start.checked_add(length).unwrap();
    let owner = sid_text(owner, start, end);
    assert!(
        trusted(&owner, actor) || (policy == Policy::Ancestor && owner == INSTALLER),
        "untrusted protected-route owner"
    );
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: validated retained descriptor and initialized exact outputs.
    unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }
        .unwrap();
    assert_ne!(control & SE_DACL_PRESENT.0, 0);
    if policy == Policy::Invocation {
        assert_ne!(control & SE_DACL_PROTECTED.0, 0);
    }
    validate_acl(acl, start, end, actor, policy);
}

fn sid_text(sid: PSID, start: usize, end: usize) -> String {
    let address = sid.0 as usize;
    assert!(address >= start && address.checked_add(8).is_some_and(|v| v <= end));
    // SAFETY: complete SID header has been bounded within the retained descriptor or ACE.
    let count = usize::from(unsafe { *sid.0.cast::<u8>().add(1) });
    assert!(address.checked_add(8 + count * 4).is_some_and(|v| v <= end));
    // SAFETY: full SID extent was independently bounded before SDK validation.
    assert!(unsafe { IsValidSid(sid) }.as_bool());
    let mut text = PWSTR::null();
    // SAFETY: valid retained SID, initialized output receiving one SDK-owned string.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.unwrap();
    let _allocation = Allocation(HLOCAL(text.0.cast()));
    // SAFETY: successful conversion guarantees terminated UTF-16 in retained allocation.
    unsafe { text.to_string() }.unwrap()
}

fn validate_acl(acl: *mut ACL, start: usize, end: usize, actor: &str, policy: Policy) {
    let address = acl as usize;
    assert!(address >= start && address.is_multiple_of(align_of::<ACL>()));
    assert!(
        address
            .checked_add(size_of::<ACL>())
            .is_some_and(|v| v <= end)
    );
    // SAFETY: complete aligned ACL header was bounded inside the descriptor allocation.
    let header = unsafe { &*acl };
    let acl_end = address.checked_add(usize::from(header.AclSize)).unwrap();
    assert!(usize::from(header.AclSize) >= size_of::<ACL>() && acl_end <= end);
    // SAFETY: complete ACL extent was independently bounded before SDK validation.
    assert!(unsafe { IsValidAcl(acl) }.as_bool());
    for index in 0..u32::from(header.AceCount) {
        let mut ace = std::ptr::null_mut();
        // SAFETY: validated counted ACL and initialized pointer output, bounded index.
        unsafe { GetAce(acl, index, &raw mut ace) }.unwrap();
        validate_ace(ace, address + size_of::<ACL>(), acl_end, actor, policy);
    }
}

fn validate_ace(ace: *mut std::ffi::c_void, start: usize, end: usize, actor: &str, policy: Policy) {
    let address = ace as usize;
    assert!(address >= start && address.is_multiple_of(align_of::<ACCESS_ALLOWED_ACE>()));
    assert!(
        address
            .checked_add(size_of::<ACE_HEADER>())
            .is_some_and(|v| v <= end)
    );
    // SAFETY: complete aligned ACE header lies inside the validated ACL.
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    let ace_end = address.checked_add(usize::from(header.AceSize)).unwrap();
    assert!(ace_end <= end && usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>());
    let kind = u32::from(header.AceType);
    assert!(
        kind == ACCESS_ALLOWED_ACE_TYPE || kind == ACCESS_DENIED_ACE_TYPE,
        "unsupported ACE type"
    );
    assert_eq!(header.AceFlags & !0x1f, 0);
    // SAFETY: complete aligned allow/deny shared prefix is within the checked ACE extent.
    let grant = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    let offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    // SAFETY: fixed SID offset lies in the complete validated shared layout.
    let sid = PSID(unsafe { ace.cast::<u8>().add(offset) }.cast());
    let principal = sid_text(sid, address + offset, ace_end);
    let mut mask = grant.Mask;
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ.0,
        GenericWrite: FILE_GENERIC_WRITE.0,
        GenericExecute: FILE_GENERIC_EXECUTE.0,
        GenericAll: FILE_ALL_ACCESS.0,
    };
    // SAFETY: initialized local mask and complete constant filesystem generic mapping.
    unsafe { MapGenericMask(&raw mut mask, &mapping) };
    assert_eq!(mask & !FILE_ALL_ACCESS.0, 0, "unknown file access rights");
    if kind == ACCESS_DENIED_ACE_TYPE {
        return;
    }
    if policy != Policy::Ancestor {
        assert!(
            trusted(&principal, actor),
            "outsider ACE in protected invocation object"
        );
    } else if header.AceFlags & 8 == 0 && !trusted(&principal, actor) {
        // Existing routes cannot be replaced: no WRITE_DATA-as-file, DELETE, DELETE_CHILD,
        // WRITE_DAC, WRITE_OWNER, attribute or EA mutation. OS roots may permit new children.
        let allowed = FILE_GENERIC_READ.0
            | FILE_GENERIC_EXECUTE.0
            | FILE_ADD_FILE.0
            | FILE_ADD_SUBDIRECTORY.0;
        assert_eq!(
            mask & !allowed,
            0,
            "ancestor grants outsider route mutation"
        );
    }
}
