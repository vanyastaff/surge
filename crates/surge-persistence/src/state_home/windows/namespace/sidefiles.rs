//! Transient SQLite observations never retain sidefile lifetime authority.
use super::*;
use windows::Win32::Foundation::{STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND};

#[derive(Debug)]
enum Observation {
    Live,
    Retiring(Identity),
}

fn open(parent: &File, name: &OsStr) -> NativeResult<Option<File>> {
    let result = relative_with_policy(
        parent,
        name,
        false,
        None,
        FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
        NativeOpenPolicy {
            shares: FILE_SHARE_READ
                | FILE_SHARE_WRITE
                | windows::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
            write_through: false,
            inspection: OpenInspection::Deferred,
        },
    );
    match result {
        Ok(file) => Ok(Some(file)),
        Err(NativeError::Open(STATUS_OBJECT_NAME_NOT_FOUND | STATUS_OBJECT_PATH_NOT_FOUND)) => {
            Ok(None)
        },
        Err(error) => Err(error),
    }
}

impl Namespace {
    fn classify_sidefile(&self, file: &File) -> NativeResult<Observation> {
        let snapshot = object_snapshot(file)?;
        self.owner
            .validate_directory(file, DirectoryPolicy::SqliteSidefile)?;
        if snapshot.attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(refusal("SQLite sidefile is a reparse object"));
        }
        if snapshot.directory {
            return Err(refusal("SQLite sidefile is a directory"));
        }
        if snapshot.links > 1 {
            return Err(refusal("SQLite sidefile has multiple links"));
        }
        if snapshot.delete_pending {
            // This is a validated original awaiting deletion, not a live sidefile.
            // The observer can be the last handle retaining SQLite's retired name.
            return Ok(Observation::Retiring(snapshot.identity));
        }
        if snapshot.links != 1 {
            return Err(refusal("live SQLite sidefile has no named link"));
        }
        Ok(Observation::Live)
    }

    pub(super) fn observe_sidefile(&self, parent: &File, name: &OsStr) -> NativeResult<()> {
        let Some(file) = open(parent, name)? else {
            return Ok(());
        };
        match self.classify_sidefile(&file)? {
            Observation::Live => Ok(()),
            Observation::Retiring(original) => {
                self.settle_retired_sidefile(parent, name, file, &original)
            },
        }
    }

    fn settle_retired_sidefile(
        &self,
        parent: &File,
        name: &OsStr,
        file: File,
        original: &Identity,
    ) -> NativeResult<()> {
        if object_snapshot(&file)?.identity != *original {
            return Err(refusal("retiring SQLite sidefile identity changed"));
        }
        close_observer(file)?;
        #[cfg(test)]
        AFTER_RETIRING_CLOSE.with(|slot| {
            if let Some(hook) = slot.borrow_mut().take() {
                hook();
            }
        });
        self.verify()?;
        // Exactly one fresh observation. A replacement has independent security
        // requirements; no authority transfers from the retired original ID.
        let Some(replacement) = open(parent, name)? else {
            return Ok(());
        };
        match self.classify_sidefile(&replacement)? {
            Observation::Live => Ok(()),
            Observation::Retiring(_) => {
                Err(refusal("fresh SQLite sidefile remains delete-pending"))
            },
        }
    }
}

fn close_observer(file: File) -> NativeResult<()> {
    use std::os::windows::io::IntoRawHandle;
    let handle = HANDLE(file.into_raw_handle());
    // SAFETY: consume the sole observation descriptor once. An uncertain native
    // close cannot be retried by Drop or treated as completed retirement.
    unsafe { windows::Win32::Foundation::CloseHandle(handle) }.map_err(NativeError::CloseUncertain)
}

#[cfg(test)]
thread_local! {
    static AFTER_RETIRING_CLOSE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(super) fn retirement_oracles() {
    tests::retired_original_releases_its_name_without_granting_live_authority();
    for substitute in [
        tests::Substitute::Hardlink,
        tests::Substitute::Acl,
        tests::Substitute::Reparse,
    ] {
        tests::unsafe_replacement_after_retirement_is_refused(substitute);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_home_fixture::FixtureHome;
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_INFO_EX, FileDispositionInfoEx,
        SetFileInformationByHandle,
    };

    fn retire(namespace: &Namespace, name: &OsStr) -> (File, Identity) {
        let parent = namespace.files.last().unwrap();
        let creation = namespace.owner.creation(false).unwrap();
        drop(relative(parent, name, false, Some(&creation), true).unwrap());
        let observer = open(parent, name).unwrap().unwrap();
        let original = object_snapshot(&observer).unwrap().identity;
        let deleter = relative_access_with_sharing(
            parent,
            name,
            false,
            None,
            DELETE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ
                | FILE_SHARE_WRITE
                | windows::Win32::Storage::FileSystem::FILE_SHARE_DELETE,
            false,
        )
        .unwrap();
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE,
        };
        // SAFETY: actual DELETE-capable original and exact initialized SDK structure.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(deleter.as_raw_handle()),
                FileDispositionInfoEx,
                (&raw const disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        }
        .unwrap();
        drop(deleter);
        assert!(
            matches!(namespace.classify_sidefile(&observer), Ok(Observation::Retiring(ref id)) if *id == original)
        );
        assert!(
            inspect(&observer, false).is_err(),
            "permanent/private inspection must still refuse retirement"
        );
        (observer, original)
    }

    pub(super) fn retired_original_releases_its_name_without_granting_live_authority() {
        let home = FixtureHome::new().unwrap();
        let namespace = Namespace::protected_parent(&home.path().join("retirement")).unwrap();
        let parent = namespace.files.last().unwrap();
        let name = OsStr::new("events.sqlite-shm");
        let (observer, original) = retire(&namespace, name);
        assert!(
            namespace.observe_sidefile(parent, name).is_err(),
            "unproven initial pending status must refuse"
        );
        namespace
            .settle_retired_sidefile(parent, name, observer, &original)
            .unwrap();
        assert!(!home.path().join("retirement").join(name).exists());
        namespace
            .verify_sidefiles(OsStr::new("events.sqlite"))
            .unwrap();
        drop(namespace);
        home.close().unwrap();
    }

    #[derive(Clone, Copy)]
    pub(super) enum Substitute {
        Hardlink,
        Acl,
        Reparse,
    }

    pub(super) fn unsafe_replacement_after_retirement_is_refused(substitute: Substitute) {
        let home = FixtureHome::new().unwrap();
        let root = home.path().join("substitution");
        let namespace = Namespace::protected_parent(&root).unwrap();
        let parent = namespace.files.last().unwrap();
        let name = OsStr::new("events.sqlite-shm");
        let (observer, original) = retire(&namespace, name);
        let path = root.join(name);
        let target = root.join("sentinel-target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"unchanged target").unwrap();
        let replacement = path.clone();
        let sentinel = target.clone();
        AFTER_RETIRING_CLOSE.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                assert!(
                    !replacement.exists(),
                    "closed retired original must leave no visible name"
                );
                install_substitute(substitute, &replacement, &sentinel);
            }))
        });
        let result = namespace.settle_retired_sidefile(parent, name, observer, &original);
        let refused = match substitute {
            Substitute::Hardlink | Substitute::Acl => {
                matches!(&result, Err(NativeError::Security(_)))
            },
            Substitute::Reparse => matches!(
                &result,
                Err(NativeError::Security(_))
                    | Err(NativeError::Open(
                        windows::Win32::Foundation::STATUS_REPARSE_POINT_ENCOUNTERED
                            | windows::Win32::Foundation::STATUS_FILE_IS_A_DIRECTORY
                    ))
            ),
        };
        assert!(
            refused,
            "unsafe replacement must fail actual validation: {result:?}"
        );
        assert_eq!(
            std::fs::read(target.join("sentinel")).unwrap(),
            b"unchanged target"
        );
        if matches!(substitute, Substitute::Reparse) {
            std::fs::remove_dir(&path).unwrap();
        } else {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                b"unsafe replacement sentinel"
            );
        }
        drop(namespace);
        home.close().unwrap();
    }
    fn install_substitute(kind: Substitute, path: &Path, target: &Path) {
        match kind {
            Substitute::Reparse => {
                let status = std::process::Command::new("cmd")
                    .args(["/d", "/c", "mklink", "/J"])
                    .arg(path)
                    .arg(target)
                    .status()
                    .unwrap();
                assert!(status.success());
            },
            Substitute::Hardlink => {
                std::fs::write(path, b"unsafe replacement sentinel").unwrap();
                std::fs::hard_link(path, path.with_extension("alias")).unwrap();
            },
            Substitute::Acl => {
                std::fs::write(path, b"unsafe replacement sentinel").unwrap();
                let status = std::process::Command::new("icacls")
                    .arg(path)
                    .args(["/grant", "*S-1-1-0:R"])
                    .status()
                    .unwrap();
                assert!(status.success());
            },
        }
    }
}
