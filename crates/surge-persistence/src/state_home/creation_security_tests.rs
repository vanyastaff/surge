//! First-observable creation security, through the actual Storage creation path.
use super::{RuntimeHomeOwner, test_security, tests::descriptor_text};
use crate::runs::Storage;
use std::{
    os::windows::ffi::OsStrExt,
    path::Path,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::CreateDirectoryW,
    },
    core::PCWSTR,
};

#[path = "creation_security_tests/observation.rs"]
mod observation;

struct Descriptor(HLOCAL);
impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: unique ownership of a successful SDK LocalAlloc allocation.
        unsafe { LocalFree(self.0) };
    }
}

fn create_read_inheriting_parent(path: &Path, actor: &str) {
    // Only this independent fixture parent inherits outsider read. The missing
    // home and database are exclusively created by actual production operations.
    let sddl = format!("O:{actor}D:P(A;OICI;FA;;;{actor})(A;OICI;FR;;;WD)");
    let text: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: terminated owned input and initialized SDK output; allocation retained below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(text.as_ptr()),
            SDDL_REVISION_1,
            &raw mut descriptor,
            None,
        )
    }
    .unwrap();
    let _descriptor = Descriptor(HLOCAL(descriptor.0));
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap(),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: live initial descriptor and terminated path through exclusive SDK creation.
    unsafe { CreateDirectoryW(PCWSTR(path_wide.as_ptr()), Some(&raw const attributes)) }.unwrap();
    // SDK round-trip independently confirms the actual owner and complete DACL,
    // including the outsider read ACE that protected production children must exclude.
    assert_eq!(descriptor_text(path), sddl);
}

fn require_settlement(home: &Path) {
    for path in [home.join("db/registry.sqlite"), home.to_path_buf()] {
        let moved = path.with_extension("settled");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match std::fs::rename(&path, &moved) {
                Ok(()) => break,
                Err(error) => {
                    assert!(
                        Instant::now() < deadline,
                        "ownership did not settle: {error}"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                },
            }
        }
        std::fs::rename(&moved, &path).unwrap();
    }
}

#[test]
#[ignore = "mandatory dedicated-standard-user Windows CI first-observable creation oracle"]
fn first_observable_creation_has_private_security() {
    let actor = std::env::var("SURGE_NATIVE_PROBE_SID").unwrap();
    test_security::token_principals_for_actor(&actor);
    let fixture = tempfile::Builder::new()
        .prefix("surge-initial-security-")
        .tempdir_in(RuntimeHomeOwner::user_profile_path().unwrap())
        .unwrap();
    let parent = fixture.path().join("read-inheriting-parent");
    create_read_inheriting_parent(&parent, &actor);
    let parent_before = descriptor_text(&parent);
    let home = parent.join("state");
    assert!(!home.exists());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let observed = observation::run(home.clone(), actor, || {
        // block_on polls on this registering thread. Storage currently creates
        // home and registry synchronously before building its r2d2 worker pool;
        // moving either creation to a worker must fail the two-ack requirement.
        let storage = runtime.block_on(Storage::open(&home)).unwrap();
        let connection = storage.registry_pool.get().unwrap();
        let value: i64 = connection
            .query_row("SELECT 7", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 7);
        drop(connection);
        drop(storage);
    });
    drop(runtime);
    for observation in &observed {
        assert_eq!(
            test_security::path_identity(&observation.path),
            observation.identity
        );
    }
    // Final identity handles have already closed; only actual production owners
    // could still prevent these renames. No active RunWriter was created.
    require_settlement(&home);
    assert_eq!(descriptor_text(&parent), parent_before);
    fixture.close().unwrap();
    println!(
        "stage1 first-observable home/database private ACL zero-byte DB identity and settlement=PASS"
    );
}
