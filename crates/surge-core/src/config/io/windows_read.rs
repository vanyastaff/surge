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
        let dos = dos_path_for_open(path)?;
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

// Match Rust std's maybe_verbatim/get_long_path(prefer_verbatim=true) before
// converting DOS to NT syntax. Adapted from Rust's MIT/Apache-2.0 std/sys/path/windows.rs.
// Lexical absolute resolution does not open ancestors or resolve reparse points.
fn dos_path_for_open(path: &Path) -> io::Result<Vec<u16>> {
    let mut dos: Vec<u16> = path.as_os_str().encode_wide().collect();
    if dos.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration path contains NUL",
        ));
    }
    dos.push(0);
    if dos.starts_with(VERBATIM_PREFIX) || dos.starts_with(NT_PREFIX) || dos == [0] {
        return Ok(dos);
    }
    if dos.len() < 248 {
        match dos.as_slice() {
            [drive, 58, 0] | [drive, 58, 92 | 47, ..] if *drive != 92 && *drive != 47 => {
                return Ok(dos);
            },
            [92 | 47, 92 | 47, ..] => return Ok(dos),
            _ => {},
        }
    }
    let absolute = std::path::absolute(path)?;
    let absolute: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    Ok(verbatim_absolute(&absolute))
}

const VERBATIM_PREFIX: &[u16] = &[92, 92, 63, 92]; // \\?\
const NT_PREFIX: &[u16] = &[92, 63, 63, 92]; // \??\
const UNC_PREFIX: &[u16] = &[92, 92, 63, 92, 85, 78, 67, 92]; // \\?\UNC\

fn verbatim_absolute(mut absolute: &[u16]) -> Vec<u16> {
    let prefix = match absolute {
        [_, 58, 92, ..] => VERBATIM_PREFIX,
        [92, 92, 46, 92, ..] => {
            absolute = &absolute[4..];
            VERBATIM_PREFIX
        },
        [92, 92, 63, 92, ..] | [92, 63, 63, 92, ..] => &[],
        [92, 92, ..] => {
            absolute = &absolute[2..];
            UNC_PREFIX
        },
        _ => &[],
    };
    let mut dos = Vec::with_capacity(prefix.len() + absolute.len() + 1);
    dos.extend_from_slice(prefix);
    dos.extend_from_slice(absolute);
    dos.push(0);
    dos
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
    fn lexical_prefix_conversion_preserves_windows_namespaces() {
        for (input, expected) in [
            (r"C:\folder\file", r"\\?\C:\folder\file"),
            (r"\\server\share\file", r"\\?\UNC\server\share\file"),
            (r"\\.\device", r"\\?\device"),
            (r"\\?\C:\literal.\file", r"\\?\C:\literal.\file"),
            (r"\??\C:\file", r"\??\C:\file"),
        ] {
            let input: Vec<u16> = input.encode_utf16().collect();
            let expected: Vec<u16> = expected.encode_utf16().chain(std::iter::once(0)).collect();
            assert_eq!(verbatim_absolute(&input), expected);
        }
        for unchanged in [
            r"C:\short\file.",
            r"\\server\share\file",
            r"\\?\C:\literal.\file",
            r"\??\C:\file",
            "",
        ] {
            let expected: Vec<u16> = unchanged.encode_utf16().chain(std::iter::once(0)).collect();
            assert_eq!(dos_path_for_open(Path::new(unchanged)).unwrap(), expected);
        }
    }

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

    #[test]
    fn short_relative_dot_and_trailing_dot_resolution_matches_standard_reads() {
        let current = std::env::current_dir().unwrap();
        let directory = tempfile::tempdir_in(&current).unwrap();
        std::fs::create_dir(directory.path().join("nested")).unwrap();
        let normal = directory.path().join("plain.toml");
        let verbatim_dot = directory.path().canonicalize().unwrap().join("plain.toml.");
        std::fs::write(&normal, b"normal name sentinel").unwrap();
        std::fs::write(&verbatim_dot, b"literal trailing dot sentinel").unwrap();
        for path in [
            directory.path().join("plain.toml."),
            directory
                .path()
                .join("nested")
                .join("..")
                .join("plain.toml"),
            directory.path().join(".").join("plain.toml"),
        ] {
            for candidate in [path.as_path(), path.strip_prefix(&current).unwrap()] {
                assert_eq!(
                    read_to_string(candidate).unwrap(),
                    std::fs::read_to_string(candidate).unwrap()
                );
            }
        }
        assert_eq!(std::fs::read(&normal).unwrap(), b"normal name sentinel");
        assert_eq!(
            std::fs::read(&verbatim_dot).unwrap(),
            b"literal trailing dot sentinel"
        );
    }
}
