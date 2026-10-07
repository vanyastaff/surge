//! Independent filesystem mutations exercise refusal without security repair.
use super::{RuntimeHomeOwner, SqliteNamespaceOwner, tests::descriptor_text};
use std::{os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
            SetFileSecurityW, UNPROTECTED_DACL_SECURITY_INFORMATION,
        },
    },
    core::PCWSTR,
};

struct Descriptor(HLOCAL);
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: this unique allocation was returned by the SDK conversion API.
        unsafe { LocalFree(self.0) };
    }
}

/// Test setup only: mutate our fixture through the SDK, independently of production policy.
fn install_dacl(path: &Path, sddl: &str, protected: bool) {
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: terminated owned input and initialized SDK output; success transfers allocation.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(text.as_ptr()),
            SDDL_REVISION_1,
            &raw mut descriptor,
            None,
        )
    }
    .unwrap();
    let _owner = Descriptor(HLOCAL(descriptor.0));
    let flags = DACL_SECURITY_INFORMATION
        | if protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
    // SAFETY: live terminated fixture path and valid owned security descriptor.
    unsafe { SetFileSecurityW(PCWSTR(path.as_ptr()), flags, descriptor) }
        .ok()
        .unwrap();
}

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("surge-security-refusal-")
        .tempdir_in(RuntimeHomeOwner::user_profile_path().unwrap())
        .unwrap()
}

fn owner_sid(descriptor: &str) -> &str {
    let owner = descriptor.strip_prefix("O:").unwrap();
    let end = ["G:", "D:", "S:"]
        .iter()
        .filter_map(|section| owner.find(section))
        .min()
        .unwrap();
    &owner[..end]
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI native ACL refusal matrix"]
fn unsafe_home_dacls_are_refused_without_mutating_existing_objects() {
    let fixture = fixture();
    for policy in ["null", "unprotected", "widened", "inherited"] {
        let path = fixture.path().join(policy);
        let home = RuntimeHomeOwner::prepare(&path).unwrap();
        let original = descriptor_text(&path);
        let sid = owner_sid(&original);
        assert!(sid.starts_with("S-1-"));
        let sentinel = path.join("sentinel");
        std::fs::write(&sentinel, b"preexisting bytes").unwrap();
        drop(home);
        let descriptor = match policy {
            "null" => "D:NO_ACCESS_CONTROL".to_owned(),
            "unprotected" => format!("D:(A;OICI;FA;;;{sid})"),
            "widened" => format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FR;;;WD)"),
            "inherited" => format!("D:P(A;OICIID;FA;;;{sid})"),
            _ => unreachable!(),
        };
        install_dacl(&path, &descriptor, policy != "unprotected");
        let before = descriptor_text(&path);
        assert_ne!(before, original, "fixture must alter actual ACL: {policy}");
        let count = std::fs::read_dir(&path).unwrap().count();
        assert!(RuntimeHomeOwner::prepare(&path).is_err(), "{policy}");
        assert_eq!(descriptor_text(&path), before, "{policy}");
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"preexisting bytes");
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), count);
    }
    fixture.close().unwrap();
    println!("stage1 actual null/unprotected/widened/inherited ACL unchanged refusal=PASS");
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI native SQLite refusal matrix"]
fn unsafe_sidefile_and_hardlinked_database_are_unchanged_after_refusal() {
    let fixture = fixture();
    let home = RuntimeHomeOwner::prepare(&fixture.path().join("state")).unwrap();
    let path = home.path().join("probe.sqlite");
    drop(SqliteNamespaceOwner::standalone(&path).unwrap());
    let wal = path.with_file_name("probe.sqlite-wal");
    std::fs::write(&wal, b"unsafe existing WAL sentinel").unwrap();
    install_dacl(&wal, "D:NO_ACCESS_CONTROL", true);
    let before = descriptor_text(&wal);
    assert!(SqliteNamespaceOwner::standalone(&path).is_err());
    assert_eq!(descriptor_text(&wal), before);
    assert_eq!(
        std::fs::read(&wal).unwrap(),
        b"unsafe existing WAL sentinel"
    );
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
    std::fs::remove_file(&wal).unwrap();

    std::fs::write(&path, b"hardlinked database sentinel").unwrap();
    let link = home.path().join("other.sqlite");
    std::fs::hard_link(&path, &link).unwrap();
    let before = descriptor_text(&path);
    assert!(SqliteNamespaceOwner::standalone(&path).is_err());
    assert_eq!(descriptor_text(&path), before);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"hardlinked database sentinel"
    );
    assert_eq!(
        std::fs::read(&link).unwrap(),
        b"hardlinked database sentinel"
    );
    drop(home);
    fixture.close().unwrap();
    println!("stage1 unsafe WAL and hardlinked database unchanged refusal=PASS");
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI pathname alias refusal matrix"]
fn pathname_aliases_refuse_without_touching_the_existing_target() {
    let fixture = fixture();
    let home = RuntimeHomeOwner::prepare(&fixture.path().join("state")).unwrap();
    let sentinel = home.path().join("sentinel");
    std::fs::write(&sentinel, b"canonical state sentinel").unwrap();
    let descriptor = descriptor_text(home.path());
    let count = std::fs::read_dir(home.path()).unwrap().count();
    let mut aliases = Vec::new();
    for suffix in [".", " ", ":alternate"] {
        let mut path = home.path().as_os_str().to_owned();
        path.push(suffix);
        aliases.push(std::path::PathBuf::from(path));
    }
    for suffix in [".", " "] {
        let mut verbatim = std::ffi::OsString::from(r"\\?\");
        verbatim.push(home.path());
        verbatim.push(suffix);
        aliases.push(std::path::PathBuf::from(verbatim));
    }
    aliases.push(home.path().join("..").join("state"));
    aliases.push(fixture.path().join("NUL"));
    let registry = home.path().join("db").join("registry.sqlite");
    assert!(!registry.exists());
    let siblings = std::fs::read_dir(fixture.path()).unwrap().count();
    for alias in aliases {
        assert!(RuntimeHomeOwner::prepare(&alias).is_err(), "{alias:?}");
        assert_eq!(descriptor_text(home.path()), descriptor);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"canonical state sentinel"
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), count);
        assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), siblings);
        assert!(!registry.exists());
    }
    let directory = home.directory(super::RuntimeDirectory::Daemon).unwrap();
    let file = directory
        .open_append(std::ffi::OsStr::new("actual.log"))
        .unwrap();
    file.flush().unwrap();
    drop(file);
    let actual = home.path().join("daemon").join("actual.log");
    std::fs::write(&actual, b"canonical file sentinel").unwrap();
    let descriptor = descriptor_text(&actual);
    for alias in [
        "actual.log.",
        "actual.log ",
        "actual.log:alternate",
        "NUL",
        "..\\actual.log",
    ] {
        assert!(directory.open_append(std::ffi::OsStr::new(alias)).is_err());
        assert!(directory.open_control(std::ffi::OsStr::new(alias)).is_err());
        assert_eq!(std::fs::read(&actual).unwrap(), b"canonical file sentinel");
        assert_eq!(descriptor_text(&actual), descriptor);
    }
    assert_eq!(
        std::fs::read_dir(actual.parent().unwrap()).unwrap().count(),
        1
    );
    let stream = actual.with_file_name("actual.log:alternate");
    assert_eq!(
        std::fs::File::open(stream).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    drop(directory);
    drop(home);
    fixture.close().unwrap();
    println!("stage1 native pathname alias and ADS unchanged refusal=PASS");
}
