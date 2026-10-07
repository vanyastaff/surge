//! Existing-API native acceptance oracles; no creation helper supplies the verdict.
use super::Storage;
use std::{
    fs::{File, OpenOptions},
    mem::{offset_of, size_of},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HLOCAL, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{
                ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
                GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl, GetTokenInformation,
            IsValidAcl, IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
            SE_DACL_PRESENT, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
            FILE_SHARE_WRITE, READ_CONTROL,
        },
        System::{
            Com::CoTaskMemFree,
            SystemServices::ACCESS_ALLOWED_ACE_TYPE,
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
        UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    },
    core::PWSTR,
};

struct Allocation(HLOCAL);
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: owns exactly one successful SDK LocalAlloc result.
        let _ = unsafe { LocalFree(self.0) };
    }
}
fn sid_text(sid: PSID) -> String {
    // SAFETY: callers hold validated token/descriptor SID storage throughout.
    assert!(unsafe { IsValidSid(sid) }.as_bool());
    let mut text = PWSTR::null();
    // SAFETY: valid borrowed SID and initialized output.
    unsafe { ConvertSidToStringSidW(sid, &raw mut text) }.unwrap();
    let _owned = Allocation(HLOCAL(text.0.cast()));
    // SAFETY: successful SDK conversion returned an owned terminated string.
    unsafe { text.to_string() }.unwrap()
}
fn actual_user() -> String {
    let mut handle = HANDLE::default();
    // SAFETY: borrowed process and initialized writable handle output.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut handle) }.unwrap();
    // SAFETY: fresh SDK-owned token handle transferred once.
    let token = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let mut bytes = [0usize; 128];
    let mut returned = 0;
    // SAFETY: matching aligned initialized token buffer and live token.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            Some(bytes.as_mut_ptr().cast()),
            u32::try_from(size_of::<[usize; 128]>()).unwrap(),
            &raw mut returned,
        )
    }
    .unwrap();
    assert!(
        usize::try_from(returned).unwrap() >= size_of::<TOKEN_USER>()
            && usize::try_from(returned).unwrap() <= size_of::<[usize; 128]>()
    );
    // SAFETY: successful API returned a matching aligned header.
    let sid = unsafe { (*bytes.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let start = bytes.as_ptr() as usize;
    assert!(
        sid.0 as usize >= start && sid.0 as usize + 8 <= start + usize::try_from(returned).unwrap()
    );
    // SAFETY: SID header bounds checked above.
    let count = usize::from(unsafe { *sid.0.cast::<u8>().add(1) });
    assert!(sid.0 as usize + 8 + count * 4 <= start + usize::try_from(returned).unwrap());
    let actual = sid_text(sid);
    assert_eq!(actual, std::env::var("SURGE_NATIVE_PROBE_SID").unwrap());
    actual
}
fn profile_fixture() -> tempfile::TempDir {
    // SAFETY: null token selects the actual caller identity, not an inherited TEMP.
    let path =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, HANDLE::default()) }
            .unwrap();
    // SAFETY: successful SDK path is terminated and live until freed below.
    let text = unsafe { path.to_string() }.unwrap();
    // SAFETY: matching allocator for the successful known-folder API result.
    unsafe { CoTaskMemFree(Some(path.0.cast())) };
    let parent = PathBuf::from(text);
    assert!(parent.is_absolute());
    let fixture = tempfile::Builder::new()
        .prefix("surge-ownership-")
        .tempdir_in(parent)
        .unwrap();
    assert_eq!(
        security(fixture.path()).owner,
        actual_user(),
        "fixture parent must belong to the actual token user"
    );
    fixture
}
struct Security {
    owner: String,
    descriptor: Allocation,
    acl: *mut ACL,
    control: u16,
    sddl: String,
}
fn security(path: &Path) -> Security {
    let directory = path.is_dir();
    let file: File = OpenOptions::new()
        .access_mode((READ_CONTROL | FILE_READ_ATTRIBUTES).0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
        .custom_flags(if directory {
            FILE_FLAG_BACKUP_SEMANTICS.0
        } else {
            0
        })
        .open(path)
        .unwrap();
    let mut owner = PSID::default();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: live file and initialized matching owner/ACL/descriptor outputs.
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
    let descriptor_allocation = Allocation(HLOCAL(descriptor.0));
    let owner = sid_text(owner);
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor is live and outputs match the SDK ABI.
    unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) }
        .unwrap();
    let mut sddl = PWSTR::null();
    // SAFETY: live descriptor and initialized SDK-owned string output.
    unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &raw mut sddl,
            None,
        )
    }
    .unwrap();
    let _sddl_owned = Allocation(HLOCAL(sddl.0.cast()));
    // SAFETY: successful conversion returned a terminated live string.
    let sddl = unsafe { sddl.to_string() }.unwrap();
    Security {
        owner,
        descriptor: descriptor_allocation,
        acl,
        control,
        sddl,
    }
}
fn assert_unsafe_outsider_grant(observed: &Security) {
    // SAFETY: descriptor keeps the SDK-owned ACL live throughout this inspection.
    assert!(unsafe { !observed.acl.is_null() && IsValidAcl(observed.acl).as_bool() });
    let current = actual_user();
    // SAFETY: validated ACL header remains owned.
    let count = unsafe { (*observed.acl).AceCount };
    let mut outsider_full = false;
    for index in 0..u32::from(count) {
        let mut ace = std::ptr::null_mut();
        // SAFETY: valid ACL and bounded index, matching initialized output.
        unsafe { GetAce(observed.acl, index, &raw mut ace) }.unwrap();
        // SAFETY: successful GetAce returned SDK-owned aligned header storage.
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
            continue;
        }
        assert!(usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>());
        // SAFETY: matching allow ACE with sufficient complete structure size.
        let grant = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
        let offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        assert!(usize::from(header.AceSize) >= offset + 8);
        // SAFETY: checked SID header within live ACE storage.
        let sid = unsafe { ace.cast::<u8>().add(offset) };
        // SAFETY: SID header bounds checked above.
        let authorities = usize::from(unsafe { *sid.add(1) });
        assert!(usize::from(header.AceSize) >= offset + 8 + authorities * 4);
        let principal = sid_text(PSID(sid.cast()));
        if principal != current
            && principal != "S-1-5-18"
            && principal != "S-1-5-32-544"
            && header.AceFlags & 8 == 0
            && grant.Mask & FILE_ALL_ACCESS.0 == FILE_ALL_ACCESS.0
        {
            outsider_full = true;
        }
    }
    assert!(
        outsider_full,
        "negative fixture needs an actual outsider effective Full grant: {}",
        observed.sddl
    );
}

fn assert_private_home(path: &Path, inheritance: Option<u8>) {
    let observed = security(path);
    assert_eq!(observed.owner, actual_user());
    assert_eq!(
        observed.control & (SE_DACL_PRESENT.0 | SE_DACL_PROTECTED.0),
        SE_DACL_PRESENT.0 | SE_DACL_PROTECTED.0,
        "state home must be protected at creation: {}",
        observed.sddl
    );
    // SAFETY: returned ACL belongs to retained descriptor allocation.
    assert!(unsafe { !observed.acl.is_null() && IsValidAcl(observed.acl).as_bool() });
    // SAFETY: validated live SDK ACL header.
    assert_eq!(unsafe { (*observed.acl).AceCount }, 1);
    let mut ace = std::ptr::null_mut();
    // SAFETY: live validated ACL; initialized writable output.
    unsafe { GetAce(observed.acl, 0, &raw mut ace) }.unwrap();
    // SAFETY: GetAce returned an SDK-owned aligned ACE header.
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    assert_eq!(u32::from(header.AceType), ACCESS_ALLOWED_ACE_TYPE);
    assert!(usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>());
    // SAFETY: matching allow ACE type and complete structure size checked above.
    let grant = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    assert_eq!(grant.Mask, FILE_ALL_ACCESS.0);
    assert!(
        matches!(grant.Header.AceFlags, 0 | 3),
        "inheritance flags must be exactly none or object+container"
    );
    if let Some(expected) = inheritance {
        assert_eq!(
            grant.Header.AceFlags, expected,
            "SQLite directory must propagate only the user grant"
        );
    }
    let offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    assert!(usize::from(grant.Header.AceSize) >= offset + 8);
    // SAFETY: checked SID header inside live ACE.
    let sid = unsafe { ace.cast::<u8>().add(offset) };
    // SAFETY: bounded SID header provides subauthority count.
    let count = usize::from(unsafe { *sid.add(1) });
    assert!(usize::from(grant.Header.AceSize) >= offset + 8 + count * 4);
    assert_eq!(sid_text(PSID(sid.cast())), actual_user());
    let _retain = observed.descriptor;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "mandatory dedicated-standard-user Windows CI existing-API oracle"]
async fn new_state_home_has_protected_current_user_acl_after_open() {
    let fixture = profile_fixture();
    let home = fixture.path().join("state");
    let storage = Storage::open(&home).await.unwrap();
    assert_private_home(&home, None);
    assert_private_home(&home.join("db"), Some(3));
    assert_private_home(&home.join("runs"), Some(3));
    drop(storage);
    println!("stage1 new-home protected-user-only=PASS");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "mandatory dedicated-standard-user Windows CI existing-API oracle"]
async fn derived_pool_retains_original_database_until_every_owner_drops() {
    let fixture = profile_fixture();
    let home = fixture.path().join("state");
    let storage = Storage::open(&home).await.unwrap();
    let pool = storage.registry_pool.clone();
    let path = storage.registry_db_path();
    let moved = path.with_extension("moved");
    assert!(path.is_file());
    assert!(!moved.exists());
    let maximum = pool.max_size();
    let mut connections = Vec::new();
    for _ in 0..maximum {
        let mut connection = pool.get().unwrap();
        let disk_path = connection
            .path()
            .expect("must retire an actual disk connection");
        assert_ne!(disk_path, "");
        assert_eq!(
            std::path::Path::new(disk_path).canonicalize().unwrap(),
            path.canonicalize().unwrap()
        );
        let original = std::mem::replace(
            &mut *connection,
            rusqlite::Connection::open_in_memory().unwrap(),
        );
        original.close().unwrap();
        assert_eq!(
            connection.path(),
            Some(""),
            "retained slot must contain the actual in-memory connection"
        );
        connections.push(connection);
    }
    assert_eq!(pool.state().connections, maximum);
    assert_eq!(pool.state().idle_connections, 0);
    drop(storage);
    assert!(
        connections
            .iter()
            .all(|connection| connection.path() == Some(""))
    );
    assert!(
        std::fs::rename(&path, &moved).is_err(),
        "manager ownership must fence DB without any live SQLite disk connection"
    );
    drop(connections);
    drop(pool);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if std::fs::rename(&path, &moved).is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "database ownership failed to settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    println!("stage1 derived-pool ownership-and-settlement=PASS");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "mandatory dedicated-standard-user Windows CI existing-API oracle"]
async fn unsafe_existing_home_refuses_without_acl_or_byte_repair() {
    let root = std::env::var_os("SURGE_NATIVE_PROBE_ROOT").unwrap();
    let fixture = tempfile::Builder::new()
        .prefix("unsafe-state-")
        .tempdir_in(root)
        .unwrap();
    let home = fixture.path().to_path_buf();
    let observed = security(&home);
    assert_unsafe_outsider_grant(&observed);
    let before = observed.sddl.clone();
    drop(observed);
    std::fs::write(home.join("sentinel"), b"public isolated refusal sentinel").unwrap();
    let result = Storage::open(&home).await;
    assert!(
        result.is_err(),
        "unsafe inherited outsider Full grant must refuse before SQL"
    );
    assert_eq!(security(&home).sddl, before);
    assert_eq!(
        std::fs::read(home.join("sentinel")).unwrap(),
        b"public isolated refusal sentinel"
    );
    assert!(!home.join("db").exists());
    assert!(!home.join("runs").exists());
    println!("stage1 unsafe-home unchanged-refusal=PASS");
}
