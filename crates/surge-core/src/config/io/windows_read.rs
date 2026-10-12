//! Re-resolve only an object deleted during a concurrent POSIX replacement.
use std::{
    fs::File,
    io::{self, Read},
    mem::size_of,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
};
use windows::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{FILE_SYNCHRONOUS_IO_NONALERT, NtOpenFile},
    },
    Win32::{
        Foundation::{
            HANDLE, NTSTATUS, RtlNtStatusToDosError, STATUS_FILE_DELETED, STATUS_PENDING,
            UNICODE_STRING,
        },
        Storage::FileSystem::{
            FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlDosPathNameToNtPathName_U_WithStatus(
        dos_name: *const u16,
        nt_name: *mut UNICODE_STRING,
        file_part: *mut *mut u16,
        relative_name: *mut std::ffi::c_void,
    ) -> NTSTATUS;
    fn RtlFreeUnicodeString(name: *mut UNICODE_STRING);
}

struct NativePath(UNICODE_STRING);
impl NativePath {
    fn new(path: &Path) -> io::Result<Self> {
        let mut dos: Vec<u16> = path.as_os_str().encode_wide().collect();
        if dos.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "configuration path contains NUL",
            ));
        }
        dos.push(0);
        let mut native = UNICODE_STRING::default();
        // SAFETY: terminated UTF-16 input and initialized output are live for
        // this synchronous conversion. Optional outputs are not requested.
        let status = unsafe {
            RtlDosPathNameToNtPathName_U_WithStatus(
                dos.as_ptr(),
                &raw mut native,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status.is_err() {
            return Err(status_error(status));
        }
        Ok(Self(native))
    }
}
impl Drop for NativePath {
    fn drop(&mut self) {
        // SAFETY: successful Rtl conversion transferred this allocation; free
        // it exactly once with its matching allocator after all opens finish.
        unsafe { RtlFreeUnicodeString(&raw mut self.0) };
    }
}

const OPEN_ATTEMPTS: usize = 16;

pub(super) fn read_to_string(path: &Path) -> io::Result<String> {
    let name = NativePath::new(path)?;
    let mut file = open_resolving_deleted(|| open_once(&name)).map_err(status_error)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

fn open_resolving_deleted<T>(mut open: impl FnMut() -> Result<T, NTSTATUS>) -> Result<T, NTSTATUS> {
    for _ in 1..OPEN_ATTEMPTS {
        match open() {
            // A POSIX replacement can retire the object resolved by an open.
            // Resolve its name again, without broadening access or treating a
            // denied, pending, missing or reparse result as successful evidence.
            Err(STATUS_FILE_DELETED) => {},
            result => return result,
        }
    }
    open()
}

fn open_once(name: &NativePath) -> Result<File, NTSTATUS> {
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        ObjectName: &raw const name.0,
        // Same ordinary case-insensitive path resolution as std File::open:
        // follow reparse points, enforce native access checks, do not create.
        Attributes: 0x40, // OBJ_CASE_INSENSITIVE
        ..Default::default()
    };
    let mut handle = HANDLE::default();
    let mut completion = IO_STATUS_BLOCK::default();
    // SAFETY: exact initialized SDK structures and allocated native name stay
    // live through synchronous open. READ includes SYNCHRONIZE; no async or
    // inheritance flags are requested. Kernel access checks remain enabled.
    let status = unsafe {
        NtOpenFile(
            &raw mut handle,
            FILE_GENERIC_READ.0,
            &raw const attributes,
            &raw mut completion,
            (FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0,
            FILE_SYNCHRONOUS_IO_NONALERT.0,
        )
    };
    if status == STATUS_PENDING {
        // A pending operation would outlive our stack output storage.
        std::process::abort();
    }
    if status.is_err() {
        return Err(status);
    }
    if handle.is_invalid() {
        std::process::abort();
    }
    // SAFETY: successful native open transferred this one valid handle.
    let file = unsafe { File::from_raw_handle(handle.0) };
    // SAFETY: synchronous completion initialized the status union arm.
    let final_status = unsafe { completion.Anonymous.Status };
    if final_status == STATUS_PENDING {
        std::process::abort();
    }
    if final_status.is_err() {
        return Err(final_status);
    }
    Ok(file)
}

fn status_error(status: NTSTATUS) -> io::Error {
    // SAFETY: pure numeric conversion; no handle or pointer is supplied.
    let error = unsafe { RtlNtStatusToDosError(status) };
    let mapped = io::Error::from_raw_os_error(error as i32);
    io::Error::new(
        mapped.kind(),
        format!(
            "native configuration open status {:#010x}: {mapped}",
            status.0 as u32
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{
        STATUS_ACCESS_DENIED, STATUS_DELETE_PENDING, STATUS_OBJECT_NAME_NOT_FOUND,
        STATUS_REPARSE_POINT_ENCOUNTERED, STATUS_SHARING_VIOLATION,
    };

    #[test]
    fn only_deleted_open_is_resolved_again_and_attempts_are_bounded() {
        let mut attempts = 0;
        assert_eq!(
            open_resolving_deleted(|| {
                attempts += 1;
                if attempts < 3 {
                    Err(STATUS_FILE_DELETED)
                } else {
                    Ok("complete")
                }
            }),
            Ok("complete")
        );
        assert_eq!(attempts, 3);
        attempts = 0;
        assert_eq!(
            open_resolving_deleted::<()>(|| {
                attempts += 1;
                Err(STATUS_FILE_DELETED)
            }),
            Err(STATUS_FILE_DELETED)
        );
        assert_eq!(attempts, OPEN_ATTEMPTS);
        for status in [
            STATUS_ACCESS_DENIED,
            STATUS_DELETE_PENDING,
            STATUS_OBJECT_NAME_NOT_FOUND,
            STATUS_REPARSE_POINT_ENCOUNTERED,
            STATUS_SHARING_VIOLATION,
        ] {
            attempts = 0;
            assert_eq!(
                open_resolving_deleted::<()>(|| {
                    attempts += 1;
                    Err(status)
                }),
                Err(status)
            );
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn public_load_uses_native_unicode_read_and_preserves_failures() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("конфигурация-設定.toml");
        let config = super::super::SurgeConfig::default();
        config.save(&path).unwrap();
        assert_eq!(
            read_to_string(&path).unwrap(),
            toml::to_string_pretty(&config).unwrap()
        );
        assert_eq!(
            super::super::SurgeConfig::load(&path)
                .unwrap()
                .default_agent,
            config.default_agent
        );
        assert!(super::super::SurgeConfig::load(&directory.path().join("missing.toml")).is_err());
        assert!(read_to_string(directory.path()).is_err());
        assert_eq!(
            read_to_string(Path::new("invalid\0path"))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn native_read_preserves_actual_acl_and_sharing_denials() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("denied.toml");
        std::fs::write(&path, b"retained sentinel").unwrap();
        let exclusive = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(read_to_string(&path).is_err());
        drop(exclusive);
        let status = std::process::Command::new("icacls")
            .arg(&path)
            .args(["/deny", "*S-1-1-0:R"])
            .status()
            .unwrap();
        assert!(status.success());
        let standard = std::fs::read_to_string(&path);
        let native = read_to_string(&path);
        let restored = std::process::Command::new("icacls")
            .arg(&path)
            .args(["/remove:d", "*S-1-1-0"])
            .status()
            .unwrap();
        assert!(restored.success());
        assert_eq!(
            standard.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(native.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(&path).unwrap(), b"retained sentinel");
    }

    #[test]
    fn native_path_resolution_matches_standard_relative_long_and_verbatim_reads() {
        let current = std::env::current_dir().unwrap();
        let directory = tempfile::tempdir_in(&current).unwrap();
        let mut parent = directory.path().to_owned();
        for _ in 0..10 {
            parent.push("long-configuration-directory-segment");
        }
        std::fs::create_dir_all(&parent).unwrap();
        let absolute = parent.join("surge.toml");
        std::fs::write(&absolute, b"exact long-path sentinel").unwrap();
        let relative = absolute.strip_prefix(&current).unwrap();
        let verbatim = absolute.canonicalize().unwrap();
        for path in [absolute.as_path(), relative, verbatim.as_path()] {
            let standard = std::fs::read_to_string(path).unwrap();
            assert_eq!(read_to_string(path).unwrap(), standard);
            assert_eq!(standard, "exact long-path sentinel");
        }
    }
}
