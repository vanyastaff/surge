//! Complete native flush barrier; this grants no private-input capability.
use std::{fs::File, os::windows::io::AsRawHandle};
use windows::{
    Wdk::Storage::FileSystem::NtFlushBuffersFileEx,
    Win32::{
        Foundation::{HANDLE, NTSTATUS, STATUS_PENDING, STATUS_SUCCESS},
        System::IO::IO_STATUS_BLOCK,
    },
};

#[derive(Debug, thiserror::Error)]
pub(super) enum NativeError {
    #[error("native complete flush failed with {0:?}")]
    Flush(NTSTATUS),
}
pub(super) type NativeResult<T> = Result<T, NativeError>;

// Integration is deliberately closed until the native durability design probe passes.
#[cfg_attr(not(test), expect(dead_code))]
pub(super) fn flush_complete(file: &File) -> NativeResult<()> {
    let mut io = IO_STATUS_BLOCK::default();
    // SAFETY: borrowed live handle and initialized SDK output stay live throughout
    // this synchronous native call. No parameters or asynchronous storage are supplied.
    let status = unsafe {
        NtFlushBuffersFileEx(
            HANDLE(file.as_raw_handle()),
            0,
            std::ptr::null(),
            0,
            &raw mut io,
        )
    };
    if status == STATUS_PENDING {
        // Preserve stack output lifetime through termination if a nonconforming
        // synchronous flush unexpectedly remains outstanding in the kernel.
        std::process::abort();
    }
    if status != STATUS_SUCCESS {
        return Err(NativeError::Flush(status));
    }
    // SAFETY: completed native call initialized the status arm of this SDK union.
    let final_status = unsafe { io.Anonymous.Status };
    if final_status == STATUS_PENDING {
        std::process::abort();
    }
    if final_status != STATUS_SUCCESS {
        return Err(NativeError::Flush(final_status));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{File, OpenOptions},
        io::Write,
        mem::size_of,
        os::windows::{
            ffi::OsStrExt,
            fs::OpenOptionsExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
    };
    use windows::Win32::{
        Foundation::{HANDLE, HLOCAL, LocalFree, STATUS_ACCESS_DENIED},
        Security::{
            ACL,
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, GetTokenInformation,
            IsValidAcl, IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            TOKEN_ELEVATION, TOKEN_OWNER, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenOwner,
            TokenUser,
        },
        Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FILE_TRAVERSE, GetDriveTypeW, GetVolumeInformationByHandleW,
            GetVolumePathNameW, READ_CONTROL, SYNCHRONIZE,
        },
        System::{
            Threading::{GetCurrentProcess, OpenProcessToken},
            WindowsProgramming::DRIVE_FIXED,
        },
    };

    struct LocalAllocation(HLOCAL);
    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            // SAFETY: owns one allocation returned by a documented LocalAlloc API.
            let _ = unsafe { LocalFree(self.0) };
        }
    }
    fn sid_text(sid: PSID) -> String {
        // SAFETY: callers provide SID storage owned by the successful SDK query.
        assert!(unsafe { IsValidSid(sid) }.as_bool());
        let mut text = windows::core::PWSTR::null();
        // SAFETY: valid live SID and initialized writable output.
        unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.unwrap();
        let _allocation = LocalAllocation(HLOCAL(text.0.cast()));
        // SAFETY: successful SDK conversion returned a valid NUL-terminated string.
        unsafe { text.to_string() }.unwrap()
    }
    fn token_sid(token: &OwnedHandle, owner: bool) -> String {
        let mut storage = [0usize; 128];
        let mut returned = 0;
        let class = if owner { TokenOwner } else { TokenUser };
        // SAFETY: aligned initialized owned buffer; matching length and live token.
        unsafe {
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                class,
                Some(storage.as_mut_ptr().cast()),
                size_of::<[usize; 128]>() as u32,
                &raw mut returned,
            )
        }
        .unwrap();
        assert!(returned as usize >= size_of::<TOKEN_USER>());
        assert!(returned as usize <= size_of::<[usize; 128]>());
        // SAFETY: returned aligned SDK header remains in initialized owned storage.
        let pointer = unsafe {
            if owner {
                (*storage.as_ptr().cast::<TOKEN_OWNER>()).Owner
            } else {
                (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid
            }
        };
        let start = storage.as_ptr() as usize;
        let sid = pointer.0 as usize;
        assert!(sid >= start && sid + 8 <= start + returned as usize);
        // SAFETY: checked SID header lies within initialized returned token storage.
        let count = unsafe { *pointer.0.cast::<u8>().add(1) } as usize;
        assert!(sid + 8 + count * 4 <= start + returned as usize);
        sid_text(pointer)
    }
    fn verify_expected_user(token: &OwnedHandle) {
        let actual = token_sid(token, false);
        assert_eq!(
            actual,
            std::env::var("SURGE_NATIVE_PROBE_SID").unwrap(),
            "probe must execute as the dedicated standard account"
        );
        println!(
            "isolated probe token: user={actual} default-owner={}",
            token_sid(token, true)
        );
    }
    fn inspect_actual_directory_security(file: &File) {
        let mut owner = PSID::default();
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: live retained handle; matching initialized outputs remain valid.
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
        let _allocation = LocalAllocation(HLOCAL(descriptor.0));
        // SAFETY: successful query owns live descriptor storage containing owner/ACL.
        assert!(unsafe {
            !owner.0.is_null()
                && IsValidSid(owner).as_bool()
                && !acl.is_null()
                && IsValidAcl(acl).as_bool()
        });
        let mut control = 0u16;
        let mut revision = 0u32;
        // SAFETY: live SDK descriptor and initialized matching output storage.
        unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }
            .unwrap();
        // SAFETY: IsValidAcl accepted the SDK-owned live ACL header above.
        let (ace_count, acl_revision) = unsafe { ((*acl).AceCount, (*acl).AclRevision) };
        println!(
            "isolated probe directory: owner={} dacl-aces={ace_count} acl-revision={acl_revision} descriptor-revision={revision} control={control:#06x}",
            sid_text(owner)
        );
    }

    #[test]
    #[ignore = "CI executes this exact native probe under a dedicated non-elevated user"]
    fn non_admin_ntfs_complete_flush_probe() {
        let mut token = HANDLE::default();
        // SAFETY: writable handle output; pseudo process handle remains borrowed.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.unwrap();
        // SAFETY: successful call returned a fresh owned token handle, transferred once.
        let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
        verify_expected_user(&token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0;
        // SAFETY: SDK matching class/structure with actual initialized buffer size.
        unsafe {
            GetTokenInformation(
                HANDLE(token.as_raw_handle()),
                TokenElevation,
                Some((&raw mut elevation).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &raw mut returned,
            )
        }
        .unwrap();
        assert_eq!(returned as usize, size_of::<TOKEN_ELEVATION>());
        assert_eq!(
            elevation.TokenIsElevated, 0,
            "probe must execute without elevation"
        );
        let root = std::env::var_os("SURGE_NATIVE_PROBE_ROOT")
            .expect("dedicated native probe root required");
        let home = tempfile::Builder::new()
            .prefix("flush-")
            .tempdir_in(root)
            .unwrap();
        let directory = OpenOptions::new()
            .access_mode(
                (FILE_GENERIC_WRITE
                    | FILE_TRAVERSE
                    | FILE_READ_ATTRIBUTES
                    | READ_CONTROL
                    | SYNCHRONIZE)
                    .0,
            )
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(home.path())
            .unwrap();
        let path: Vec<u16> = home
            .path()
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut volume = [0u16; 32768];
        // SAFETY: NUL-terminated live input and initialized bounded output buffer.
        unsafe { GetVolumePathNameW(windows::core::PCWSTR(path.as_ptr()), &mut volume) }.unwrap();
        // SAFETY: successful path query returned a NUL-terminated volume root.
        assert_eq!(
            unsafe { GetDriveTypeW(windows::core::PCWSTR(volume.as_ptr())) },
            DRIVE_FIXED,
            "probe needs local fixed storage"
        );
        inspect_actual_directory_security(&directory);
        let mut filesystem = [0u16; 32];
        // SAFETY: live directory handle and initialized bounded UTF-16 output.
        unsafe {
            GetVolumeInformationByHandleW(
                HANDLE(directory.as_raw_handle()),
                None,
                None,
                None,
                None,
                Some(&mut filesystem),
            )
        }
        .unwrap();
        let end = filesystem.iter().position(|unit| *unit == 0).unwrap();
        assert_eq!(String::from_utf16(&filesystem[..end]).unwrap(), "NTFS");
        println!(
            "isolated probe before flush: elevation=0 filesystem=NTFS drive=fixed directory-access={:#010x} share={:#x}",
            (FILE_GENERIC_WRITE
                | FILE_TRAVERSE
                | FILE_READ_ATTRIBUTES
                | READ_CONTROL
                | SYNCHRONIZE)
                .0,
            (FILE_SHARE_READ | FILE_SHARE_WRITE).0
        );
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(home.path().join("flush-probe"))
            .unwrap();
        file.write_all(b"isolated native durability probe").unwrap();
        println!(
            "isolated probe file: access=GENERIC_READ|GENERIC_WRITE share=std-default flags=0"
        );
        flush_complete(&file).unwrap();
        println!("isolated probe file flush: returned=STATUS_SUCCESS final=STATUS_SUCCESS");
        flush_complete(&directory).unwrap();
        println!("isolated probe directory flush: returned=STATUS_SUCCESS final=STATUS_SUCCESS");
        drop(file);
        let readonly = File::open(home.path().join("flush-probe")).unwrap();
        assert!(
            matches!(
                flush_complete(&readonly),
                Err(NativeError::Flush(STATUS_ACCESS_DENIED))
            ),
            "read-only barrier failure must propagate"
        );
        println!(
            "native probe: non-elevated=true filesystem=NTFS file=PASS directory=PASS readonly-error=PASS"
        );
    }
}
