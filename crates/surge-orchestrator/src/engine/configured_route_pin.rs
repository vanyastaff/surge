//! Host-local keyed snapshots of declared route inputs; never account identity proof.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RoutePinError {
    #[error("route snapshot unsupported on this platform")]
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    Unsupported,
    #[error("unsafe route key location")]
    UnsafeLocation,
    #[error("invalid route key")]
    InvalidKey,
    #[error("route key I/O failed")]
    KeyIo,
    #[error("secure random generation failed")]
    Random,
    #[error("declared route source unavailable")]
    SourceUnavailable,
    #[error("declared route source exceeds size limit")]
    SourceTooLarge,
    #[error("declared route source changed while reading")]
    SourceChanged,
}

pub struct RoutePinKey([u8; 32]);
impl std::fmt::Debug for RoutePinKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RoutePinKey([redacted])")
    }
}
impl RoutePinKey {
    /// Load the durable host-local key, creating it with restrictive atomic publication.
    /// A corrupt or unsafe existing key is never replaced. Linux/macOS are supported;
    /// other platforms return a typed unsupported error until their ACL rules are verified.
    pub fn load_or_create(host_home: &Path) -> Result<Self, RoutePinError> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            unix_key::load_or_create(host_home).map(Self)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = host_home;
            Err(RoutePinError::Unsupported)
        }
    }

    /// Fingerprint declared inputs from the same resolved environment used for launch.
    /// This proves a configured snapshot only, never authenticated account identity.
    pub fn fingerprint(
        &self,
        identity: &[u8],
        env: &BTreeMap<String, String>,
        sources: &[PathBuf],
        cwd: &Path,
    ) -> Result<String, RoutePinError> {
        use hmac::{Hmac, Mac};
        use std::fmt::Write;
        let mut mac =
            Hmac::<sha2::Sha256>::new_from_slice(&self.0).map_err(|_| RoutePinError::InvalidKey)?;
        frame(&mut mac, b"surge/configured-route/v1");
        frame(&mut mac, identity);
        mac.update(&(env.len() as u64).to_be_bytes());
        for (name, value) in env {
            frame(&mut mac, name.as_bytes());
            frame(&mut mac, value.as_bytes());
        }
        mac.update(&(sources.len() as u64).to_be_bytes());
        for source in sources {
            // The declaration, rather than the absolute worktree, identifies the source.
            frame(&mut mac, source.as_os_str().as_encoded_bytes());
            fingerprint_source(&mut mac, &cwd.join(source))?;
        }
        let bytes = mac.finalize().into_bytes();
        let mut output = String::with_capacity(64);
        for byte in bytes {
            write!(&mut output, "{byte:02x}").map_err(|_| RoutePinError::InvalidKey)?;
        }
        Ok(output)
    }
}

fn frame(mac: &mut hmac::Hmac<sha2::Sha256>, bytes: &[u8]) {
    use hmac::Mac;
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(bytes);
}

fn fingerprint_source(
    mac: &mut hmac::Hmac<sha2::Sha256>,
    path: &Path,
) -> Result<(), RoutePinError> {
    use hmac::Mac;
    use std::io::Read;
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|_| RoutePinError::SourceUnavailable)?;
    let before = file
        .metadata()
        .map_err(|_| RoutePinError::SourceUnavailable)?;
    if !before.is_file() {
        return Err(RoutePinError::SourceUnavailable);
    }
    if before.len() > LIMIT {
        return Err(RoutePinError::SourceTooLarge);
    }
    mac.update(&before.len().to_be_bytes());
    let mut buffer = [0_u8; 65536];
    let mut count = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| RoutePinError::SourceUnavailable)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > LIMIT {
            return Err(RoutePinError::SourceTooLarge);
        }
        mac.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|_| RoutePinError::SourceUnavailable)?;
    if count != before.len()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(RoutePinError::SourceChanged);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.ctime() != after.ctime() || before.ctime_nsec() != after.ctime_nsec() {
            return Err(RoutePinError::SourceChanged);
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix_key {
    use super::RoutePinError;
    use rand::TryRngCore;
    use std::{
        ffi::{CStr, CString},
        fs::File,
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::{Component, Path},
    };

    const NAME: &CStr = c"configured-route-pin.key";
    const MAGIC: &[u8] = b"surge/route-pin-key/v1\0";
    const KEY_FILE_LEN: usize = MAGIC.len() + 32 + 32;

    fn checksum(key: &[u8]) -> [u8; 32] {
        use sha2::Digest;
        let mut digest = sha2::Sha256::new();
        digest.update(b"surge/route-pin-key-integrity/v1\0");
        digest.update(MAGIC);
        digest.update(key);
        digest.finalize().into()
    }
    fn uid() -> u32 {
        // SAFETY: geteuid takes no pointers and has no preconditions.
        unsafe { libc::geteuid() }
    }
    fn open_at(dir: &File, name: &CStr, flags: i32, mode: u32) -> std::io::Result<File> {
        // SAFETY: name is NUL terminated, dir is a live descriptor, and mode is supplied for O_CREAT.
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                mode as libc::c_uint,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor; File becomes its sole owner.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(target_os = "macos")]
    mod mac_acl {
        use std::{ffi::c_void, fs::File, os::fd::AsRawFd};
        // Signatures/constants from the platform SDK sys/acl.h. These are opaque libc objects.
        unsafe extern "C" {
            fn acl_get_fd_np(fd: libc::c_int, kind: libc::c_int) -> *mut c_void;
            fn acl_get_entry(
                acl: *mut c_void,
                id: libc::c_int,
                entry: *mut *mut c_void,
            ) -> libc::c_int;
            fn acl_get_tag_type(entry: *mut c_void, tag: *mut libc::c_int) -> libc::c_int;
            fn acl_free(acl: *mut c_void) -> libc::c_int;
        }
        struct Acl(*mut c_void);
        impl Drop for Acl {
            fn drop(&mut self) {
                // SAFETY: this pointer is a nonnull ACL returned by libc, freed exactly once.
                unsafe {
                    acl_free(self.0);
                }
            }
        }
        pub(super) fn rejects_grants(file: &File) -> Result<(), ()> {
            // SAFETY: file owns a live descriptor; ACL_TYPE_EXTENDED=0x100 on macOS.
            let pointer = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
            if pointer.is_null() {
                // Darwin returns ENOENT for an absent extended ACL on a live descriptor.
                return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                    Ok(())
                } else {
                    Err(())
                };
            }
            let acl = Acl(pointer);
            let mut id = 0; // ACL_FIRST_ENTRY
            // The SDK caps extended ACLs at 169 entries; a larger result fails closed.
            for _ in 0..170 {
                let mut entry = std::ptr::null_mut();
                // SAFETY: ACL is live and owned; entry output is valid writable storage.
                let result = unsafe { acl_get_entry(acl.0, id, &raw mut entry) };
                if result != 0 {
                    // Darwin returns EINVAL when no next entry exists, including an empty ACL.
                    return if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINVAL) {
                        Ok(())
                    } else {
                        Err(())
                    };
                }
                if entry.is_null() {
                    return Err(());
                }
                let mut tag = 0;
                // SAFETY: entry belongs to live ACL and tag output is valid writable storage.
                if unsafe { acl_get_tag_type(entry, &raw mut tag) } != 0 || tag != 2 {
                    // Only ACL_EXTENDED_DENY entries are safe; grant and unknown tags fail closed.
                    return Err(());
                }
                id = -1; // ACL_NEXT_ENTRY
            }
            Err(())
        }
    }
    fn safe_directory(file: &File, final_dir: bool) -> Result<(), RoutePinError> {
        // Only Darwin extended ACLs are inspected; Linux relies on mode bits below.
        #[cfg(target_os = "macos")]
        mac_acl::rejects_grants(file).map_err(|()| RoutePinError::UnsafeLocation)?;
        let m = file.metadata().map_err(|_| RoutePinError::KeyIo)?;
        let write = m.mode() & 0o022 != 0;
        let sticky_root = m.uid() == 0 && m.mode() & 0o1000 != 0;
        if !m.is_dir() || (write && (final_dir || !sticky_root)) || (final_dir && m.uid() != uid())
        {
            return Err(RoutePinError::UnsafeLocation);
        }
        Ok(())
    }
    fn directory(home: &Path) -> Result<File, RoutePinError> {
        let original =
            std::fs::symlink_metadata(home).map_err(|_| RoutePinError::UnsafeLocation)?;
        if original.file_type().is_symlink() {
            return Err(RoutePinError::UnsafeLocation);
        }
        // Resolve platform aliases such as macOS /var, then walk the resolved chain through held FDs.
        let path = std::fs::canonicalize(home).map_err(|_| RoutePinError::UnsafeLocation)?;
        let mut current = File::open("/").map_err(|_| RoutePinError::KeyIo)?;
        safe_directory(&current, false)?;
        for component in path.components() {
            if let Component::Normal(name) = component {
                let name =
                    CString::new(name.as_bytes()).map_err(|_| RoutePinError::UnsafeLocation)?;
                current = open_at(&current, &name, libc::O_RDONLY | libc::O_DIRECTORY, 0)
                    .map_err(|_| RoutePinError::UnsafeLocation)?;
                safe_directory(&current, false)?;
            }
        }
        safe_directory(&current, true)?;
        let m = current.metadata().map_err(|_| RoutePinError::KeyIo)?;
        if m.dev() != original.dev() || m.ino() != original.ino() {
            return Err(RoutePinError::UnsafeLocation);
        }
        Ok(current)
    }
    fn read_key(mut file: File) -> Result<[u8; 32], RoutePinError> {
        #[cfg(target_os = "macos")]
        mac_acl::rejects_grants(&file).map_err(|()| RoutePinError::InvalidKey)?;
        let m = file.metadata().map_err(|_| RoutePinError::KeyIo)?;
        if !m.is_file()
            || m.uid() != uid()
            || m.mode() & 0o7777 != 0o600
            || m.len() != KEY_FILE_LEN as u64
        {
            return Err(RoutePinError::InvalidKey);
        }
        let mut stored = [0; KEY_FILE_LEN];
        file.read_exact(&mut stored)
            .map_err(|_| RoutePinError::InvalidKey)?;
        let mut extra = [0; 1];
        if file.read(&mut extra).map_err(|_| RoutePinError::KeyIo)? != 0 {
            return Err(RoutePinError::InvalidKey);
        }
        let (magic, rest) = stored.split_at(MAGIC.len());
        let (key_bytes, integrity) = rest.split_at(32);
        if magic != MAGIC || integrity != checksum(key_bytes) {
            return Err(RoutePinError::InvalidKey);
        }
        let key = <[u8; 32]>::try_from(key_bytes).map_err(|_| RoutePinError::InvalidKey)?;
        Ok(key)
    }
    fn unlink(dir: &File, name: &CStr) -> Result<(), RoutePinError> {
        // SAFETY: live directory descriptor, valid NUL terminated name, unlink regular entry only.
        if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(RoutePinError::KeyIo);
        }
        Ok(())
    }
    #[cfg(test)]
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum SyncPhase {
        Before,
        After,
    }
    #[cfg(test)]
    type SyncHook = Box<dyn Fn(SyncPhase) -> Result<(), RoutePinError>>;
    #[cfg(test)]
    std::thread_local! {
        static SYNC_HOOK: std::cell::RefCell<Option<SyncHook>> = const { std::cell::RefCell::new(None) };
    }
    #[cfg(test)]
    pub(super) fn set_sync_hook(hook: Option<SyncHook>) {
        SYNC_HOOK.with(|slot| *slot.borrow_mut() = hook);
    }
    fn sync_directory(dir: &File) -> Result<(), RoutePinError> {
        #[cfg(test)]
        SYNC_HOOK.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(Ok(()), |hook| hook(SyncPhase::Before))
        })?;
        dir.sync_all().map_err(|_| RoutePinError::KeyIo)?;
        #[cfg(test)]
        SYNC_HOOK.with(|slot| {
            slot.borrow()
                .as_ref()
                .map_or(Ok(()), |hook| hook(SyncPhase::After))
        })?;
        Ok(())
    }
    pub(super) fn load_or_create(home: &Path) -> Result<[u8; 32], RoutePinError> {
        let dir = directory(home)?;
        match open_at(&dir, NAME, libc::O_RDONLY | libc::O_NONBLOCK, 0) {
            Ok(file) => {
                let key = read_key(file)?;
                // Another publisher may have linked the validated key but not synced its parent.
                // Establish entry durability ourselves before any caller may persist its pin.
                sync_directory(&dir)?;
                return Ok(key);
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(_) => return Err(RoutePinError::InvalidKey),
        }
        let mut key = [0; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut key)
            .map_err(|_| RoutePinError::Random)?;
        let mut nonce = [0; 16];
        rand::rngs::OsRng
            .try_fill_bytes(&mut nonce)
            .map_err(|_| RoutePinError::Random)?;
        let name = CString::new(format!(
            ".route-pin-{:032x}.tmp",
            u128::from_be_bytes(nonce)
        ))
        .map_err(|_| RoutePinError::KeyIo)?;
        let mut temp = open_at(
            &dir,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
        .map_err(|_| RoutePinError::KeyIo)?;
        let result = (|| {
            temp.write_all(MAGIC).map_err(|_| RoutePinError::KeyIo)?;
            temp.write_all(&key).map_err(|_| RoutePinError::KeyIo)?;
            temp.write_all(&checksum(&key))
                .map_err(|_| RoutePinError::KeyIo)?;
            temp.sync_all().map_err(|_| RoutePinError::KeyIo)?;
            // SAFETY: held directory descriptor and valid C strings; linkat never replaces NAME.
            let linked = unsafe {
                libc::linkat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    dir.as_raw_fd(),
                    NAME.as_ptr(),
                    0,
                )
            };
            if linked != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(RoutePinError::KeyIo);
            }
            Ok(())
        })();
        let cleanup = unlink(&dir, &name);
        sync_directory(&dir)?;
        result?;
        cleanup?;
        read_key(
            open_at(&dir, NAME, libc::O_RDONLY | libc::O_NONBLOCK, 0)
                .map_err(|_| RoutePinError::InvalidKey)?,
        )
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        sync::{Arc, Barrier},
    };

    fn home() -> tempfile::TempDir {
        let root = fs::canonicalize(std::env::temp_dir()).unwrap();
        let h = tempfile::tempdir_in(root).unwrap();
        fs::set_permissions(h.path(), fs::Permissions::from_mode(0o700)).unwrap();
        h
    }
    fn pin(key: &RoutePinKey, h: &Path) -> String {
        key.fingerprint(
            b"configured profile",
            &BTreeMap::from([("TOKEN".into(), "raw-secret-token".into())]),
            &[],
            h,
        )
        .unwrap()
    }
    #[test]
    fn existing_reader_syncs_publication_before_return_and_propagates_sync_failure() {
        use std::{cell::Cell, rc::Rc, sync::mpsc, time::Duration};
        use unix_key::{SyncPhase, set_sync_hook};
        let h = home();
        let path = h.path().to_path_buf();
        let (published_tx, published_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let winner = std::thread::spawn(move || {
            set_sync_hook(Some(Box::new(move |phase| {
                if phase == SyncPhase::Before {
                    published_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Ok(())
            })));
            let key = RoutePinKey::load_or_create(&path).unwrap();
            done_tx.send(()).unwrap();
            pin(&key, &path)
        });
        published_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        // The winner has linked the key but has not synced its directory yet.
        let completed = Rc::new(Cell::new(0));
        let observed = completed.clone();
        set_sync_hook(Some(Box::new(move |phase| {
            if phase == SyncPhase::After {
                observed.set(observed.get() + 1);
            }
            Ok(())
        })));
        let reader = RoutePinKey::load_or_create(h.path()).map(|key| pin(&key, h.path()));
        set_sync_hook(None);
        let winner_still_waiting = matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        release_tx.send(()).unwrap();
        let winner_pin = winner.join().unwrap();
        assert!(winner_still_waiting);
        assert_eq!(
            completed.get(),
            1,
            "reader returned before its own directory sync"
        );
        assert_eq!(reader.unwrap(), winner_pin);
        let before = fs::read(h.path().join("configured-route-pin.key")).unwrap();
        set_sync_hook(Some(Box::new(|_| Err(RoutePinError::KeyIo))));
        let failed = RoutePinKey::load_or_create(h.path());
        set_sync_hook(None);
        assert!(matches!(failed, Err(RoutePinError::KeyIo)));
        assert_eq!(
            fs::read(h.path().join("configured-route-pin.key")).unwrap(),
            before
        );
    }
    #[test]
    fn durable_key_and_concurrent_publication() {
        let h = home();
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let path = h.path().to_path_buf();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    b.wait();
                    pin(&RoutePinKey::load_or_create(&path).unwrap(), &path)
                })
            })
            .collect();
        let values: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(values.iter().all(|v| v == &values[0]));
        assert_eq!(
            pin(&RoutePinKey::load_or_create(h.path()).unwrap(), h.path()),
            values[0]
        );
    }
    #[test]
    fn snapshots_track_env_and_file_bytes_without_copying_secrets() {
        let h = home();
        let key = RoutePinKey::load_or_create(h.path()).unwrap();
        let source = h.path().join("credentials:local");
        fs::write(&source, b"raw-file-secret").unwrap();
        let env = BTreeMap::from([("TOKEN".into(), "raw-env-secret".into())]);
        let sources = [PathBuf::from("credentials:local")];
        let first = key
            .fingerprint(b"profile", &env, &sources, h.path())
            .unwrap();
        let changed = BTreeMap::from([("TOKEN".into(), "different-env-secret".into())]);
        assert_ne!(
            first,
            key.fingerprint(b"profile", &changed, &sources, h.path())
                .unwrap()
        );
        fs::write(&source, b"different-file-secret").unwrap();
        assert_ne!(
            first,
            key.fingerprint(b"profile", &env, &sources, h.path())
                .unwrap()
        );
        let disk = fs::read(h.path().join("configured-route-pin.key")).unwrap();
        for secret in [b"raw-file-secret".as_slice(), b"raw-env-secret".as_slice()] {
            assert!(!disk.windows(secret.len()).any(|w| w == secret));
            assert!(!format!("{key:?}").contains(std::str::from_utf8(secret).unwrap()));
        }
        assert_eq!(fs::read_dir(h.path()).unwrap().count(), 2);
    }
    #[test]
    fn missing_and_oversize_sources_are_not_empty_inputs() {
        let h = home();
        let key = RoutePinKey::load_or_create(h.path()).unwrap();
        assert_eq!(
            key.fingerprint(
                b"profile",
                &BTreeMap::new(),
                &[PathBuf::from("missing-secret")],
                h.path()
            ),
            Err(RoutePinError::SourceUnavailable)
        );
        let file = fs::File::create(h.path().join("large")).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();
        assert_eq!(
            key.fingerprint(
                b"profile",
                &BTreeMap::new(),
                &[PathBuf::from("large")],
                h.path()
            ),
            Err(RoutePinError::SourceTooLarge)
        );
    }
    #[test]
    fn input_framing_and_empty_source_are_unambiguous() {
        let h = home();
        let key = RoutePinKey::load_or_create(h.path()).unwrap();
        let left = BTreeMap::from([("a".into(), "bc".into())]);
        let right = BTreeMap::from([("ab".into(), "c".into())]);
        assert_ne!(
            key.fingerprint(b"profile", &left, &[], h.path()).unwrap(),
            key.fingerprint(b"profile", &right, &[], h.path()).unwrap()
        );
        fs::write(h.path().join("empty"), b"").unwrap();
        assert_ne!(
            key.fingerprint(b"profile", &left, &[], h.path()).unwrap(),
            key.fingerprint(b"profile", &left, &[PathBuf::from("empty")], h.path())
                .unwrap()
        );
        assert_eq!(
            format!("{:?}", RoutePinError::SourceUnavailable),
            "SourceUnavailable"
        );
        assert!(
            !RoutePinError::SourceUnavailable
                .to_string()
                .contains("secret")
        );
    }
    #[test]
    fn refuses_directory_and_symlink_sources_and_home() {
        let h = home();
        let key = RoutePinKey::load_or_create(h.path()).unwrap();
        let alias = h.path().join("alias");
        symlink(h.path(), &alias).unwrap();
        assert!(matches!(
            RoutePinKey::load_or_create(&alias),
            Err(RoutePinError::UnsafeLocation)
        ));
        for path in [PathBuf::from("alias"), PathBuf::from(".")] {
            assert_eq!(
                key.fingerprint(b"profile", &BTreeMap::new(), &[path], h.path()),
                Err(RoutePinError::SourceUnavailable)
            );
        }
    }
    #[test]
    fn same_size_key_corruption_is_rejected_without_replacement() {
        let h = home();
        let path = h.path().join("configured-route-pin.key");
        RoutePinKey::load_or_create(h.path()).unwrap();
        let mut corrupted = fs::read(&path).unwrap();
        let middle = corrupted.len() / 2;
        corrupted[middle] ^= 0x80;
        fs::write(&path, &corrupted).unwrap();
        assert!(matches!(
            RoutePinKey::load_or_create(h.path()),
            Err(RoutePinError::InvalidKey)
        ));
        assert_eq!(fs::read(&path).unwrap(), corrupted);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn rejects_extended_acl_grants_without_replacing_key() {
        let h = home();
        let path = h.path().join("configured-route-pin.key");
        RoutePinKey::load_or_create(h.path()).unwrap();
        let before = fs::read(&path).unwrap();
        let status = std::process::Command::new("chmod")
            .args(["+a", "everyone allow read"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(matches!(
            RoutePinKey::load_or_create(h.path()),
            Err(RoutePinError::InvalidKey)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(
            std::process::Command::new("chmod")
                .arg("-N")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("chmod")
                .args(["+a", "everyone allow add_file"])
                .arg(h.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(
            RoutePinKey::load_or_create(h.path()),
            Err(RoutePinError::UnsafeLocation)
        ));
    }
    #[test]
    fn rejects_corrupt_unsafe_and_symlink_keys_without_replacement() {
        let h = home();
        let path = h.path().join("configured-route-pin.key");
        fs::write(&path, b"corrupt-secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            RoutePinKey::load_or_create(h.path()),
            Err(RoutePinError::InvalidKey)
        ));
        assert_eq!(fs::read(&path).unwrap(), b"corrupt-secret");
        fs::remove_file(&path).unwrap();
        fs::write(&path, [3_u8; 32]).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            RoutePinKey::load_or_create(h.path()),
            Err(RoutePinError::InvalidKey)
        ));
        fs::remove_file(&path).unwrap();
        let target = h.path().join("target");
        fs::write(&target, [4_u8; 32]).unwrap();
        symlink(&target, &path).unwrap();
        assert!(RoutePinKey::load_or_create(h.path()).is_err());
        assert_eq!(fs::read(target).unwrap(), [4_u8; 32]);
        fs::set_permissions(h.path(), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(
            RoutePinKey::load_or_create(h.path()),
            Err(RoutePinError::UnsafeLocation)
        ));
    }
}
