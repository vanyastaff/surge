//! Independent legacy byte-lock and payload-access compatibility oracles.
use super::{RuntimeDirectory, RuntimeHomeOwner};
use crate::PersistenceError;
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::Path,
};
use windows::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};

fn legacy_reader(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .open(path)
        .unwrap()
}

fn fixture() -> (tempfile::TempDir, RuntimeHomeOwner) {
    let outer = tempfile::Builder::new()
        .prefix("surge-control-lock-")
        .tempdir_in(RuntimeHomeOwner::user_profile_path().unwrap())
        .unwrap();
    let home = RuntimeHomeOwner::prepare(&outer.path().join("state")).unwrap();
    (outer, home)
}

#[test]
fn legacy_whole_file_and_control_lock_contend_in_both_directions() {
    let (outer, home) = fixture();
    let directory = home.directory(RuntimeDirectory::Daemon).unwrap();
    let path = home.path().join("daemon/compatibility.pid");
    let mut seed = directory
        .open_control("compatibility.pid".as_ref())
        .unwrap();
    assert!(seed.try_lock_exclusive().unwrap());
    seed.replace_bytes(b"12345\n").unwrap();
    drop(seed);

    // The legacy handle is read-only, so successful opening below proves that
    // sharing denial cannot masquerade as overlapping-range lock contention.
    let legacy = legacy_reader(&path);
    legacy.try_lock().unwrap();
    let mut current = directory
        .open_control("compatibility.pid".as_ref())
        .unwrap();
    assert!(!current.try_lock_exclusive().unwrap());
    drop(legacy);
    assert!(current.try_lock_exclusive().unwrap());
    assert!(current.try_lock_exclusive().unwrap(), "no stacked lock");

    let legacy = legacy_reader(&path);
    assert!(matches!(
        legacy.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"12345\n");
    drop(current);
    legacy.try_lock().unwrap();
    drop(legacy);
    assert_eq!(std::fs::read(&path).unwrap(), b"12345\n");
    drop(directory);
    drop(home);
    outer.close().unwrap();
}

#[test]
fn control_owner_excludes_independent_writers_without_blocking_readers() {
    let (outer, home) = fixture();
    let directory = home.directory(RuntimeDirectory::Daemon).unwrap();
    let path = home.path().join("daemon/payload.pid");
    let mut seed = directory.open_control("payload.pid".as_ref()).unwrap();
    assert!(seed.try_lock_exclusive().unwrap());
    seed.replace_bytes(b"12345\n").unwrap();
    drop(seed);

    let writable = OpenOptions::new()
        .write(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .open(&path)
        .unwrap();
    assert!(matches!(
        directory.open_control("payload.pid".as_ref()),
        Err(PersistenceError::OwnershipBusy)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"12345\n");
    drop(writable);

    let mut current = directory.open_control("payload.pid".as_ref()).unwrap();
    assert!(current.try_lock_exclusive().unwrap());
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        6,
        "lock does not extend file"
    );
    let writable = OpenOptions::new()
        .write(true)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .open(&path);
    assert_eq!(writable.unwrap_err().raw_os_error(), Some(32));
    assert!(std::fs::remove_file(&path).is_err());
    assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"12345\n");
    current.replace_bytes(b"67890\n").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"67890\n");
    current.remove().unwrap();
    assert!(!path.exists());
    drop(directory);
    drop(home);
    outer.close().unwrap();
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI control lock compatibility"]
fn standard_user_control_lock_and_payload_compatibility() {
    use std::{
        mem::size_of,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows::Win32::{
        Foundation::HANDLE,
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let expected = std::env::var("SURGE_NATIVE_PROBE_SID").unwrap();
    super::test_security::token_principals_for_actor(&expected);
    let mut handle = HANDLE::default();
    // SAFETY: query-only current process token and initialized handle output.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut handle) }.unwrap();
    // SAFETY: sole ownership of the successfully opened real token handle.
    let token = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    // SAFETY: exact class-specific initialized output and byte count.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenElevation,
            Some((&raw mut elevation).cast()),
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap(),
            &raw mut returned,
        )
    }
    .unwrap();
    assert_eq!(
        usize::try_from(returned).unwrap(),
        size_of::<TOKEN_ELEVATION>()
    );
    assert_eq!(elevation.TokenIsElevated, 0);
    legacy_whole_file_and_control_lock_contend_in_both_directions();
    control_owner_excludes_independent_writers_without_blocking_readers();
    println!(
        "stage1 non-elevated control readable payload exclusive writer and legacy lock compatibility=PASS"
    );
}
