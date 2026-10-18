//! Exec'd, no-SQL file observer: raw closes cannot release the producer's POSIX locks.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use surge_core::ContentHash;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Fact {
    pub(super) identity: Vec<u8>,
    pub(super) directory: bool,
    pub(super) length: u64,
    pub(super) hash: Option<ContentHash>,
    header: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Snapshot {
    root: PathBuf,
    pub(super) entries: BTreeMap<PathBuf, Fact>,
}
pub(super) fn files(root: &Path) -> Snapshot {
    observe(root).unwrap()
}
fn observe(root: &Path) -> Result<Snapshot, String> {
    let output = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args([
            "--exact",
            "runs::inspection::project_history_tests::snapshot::snapshot_worker",
            "--nocapture",
        ])
        .env("SURGE_TEST_SNAPSHOT_ROOT", root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout.len() > 4 * 1024 * 1024 {
        return Err(format!(
            "snapshot observer failed: {} {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|e| e.to_string())?;
    let json = text
        .lines()
        .find_map(|line| {
            line.split_once("SURGE_SNAPSHOT_JSON=")
                .map(|(_, value)| value)
        })
        .ok_or_else(|| "missing complete snapshot facts".to_owned())?;
    serde_json::from_str(json).map_err(|e| e.to_string())
}
#[test]
fn snapshot_worker() {
    let Some(root) = std::env::var_os("SURGE_TEST_SNAPSHOT_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let facts = collect(&root).unwrap();
    println!(
        "SURGE_SNAPSHOT_JSON={}",
        serde_json::to_string(&facts).unwrap()
    );
}
fn collect(root: &Path) -> Result<Snapshot, String> {
    let mut snapshot = Snapshot {
        root: root.to_path_buf(),
        entries: BTreeMap::new(),
    };
    if !root.exists() {
        return Ok(snapshot);
    }
    #[cfg(windows)]
    let anchor = crate::state_home::SnapshotRoot::capture(root)?;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        if snapshot.entries.len() + pending.len() >= 4096 {
            return Err("snapshot path bound".into());
        }
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || (!meta.is_dir() && !meta.is_file()) {
            return Err("snapshot alias/type".into());
        }
        let directory = meta.is_dir();
        let shm = coordination_shm(root, &path);
        let read = |file: &File| {
            let length = file.metadata()?.len();
            if directory || shm {
                return Ok((length, None, vec![]));
            }
            if length > 64 * 1024 * 1024 {
                return Err(std::io::Error::other("snapshot file bound"));
            }
            let mut input = file.take(length + 1);
            let mut hash = Sha256::new();
            let mut header = Vec::new();
            let mut buffer = [0_u8; 8192];
            // Empty Windows lock files can have a retained locked byte range.
            if length != 0 {
                loop {
                    let count = input.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    header.extend(buffer[..count].iter().take(32 - header.len()));
                    hash.update(&buffer[..count]);
                }
            }
            let hash = format!("sha256:{:x}", hash.finalize())
                .parse()
                .map_err(std::io::Error::other)?;
            if file.metadata()?.len() != length {
                return Err(std::io::Error::other("snapshot length changed"));
            }
            Ok((length, Some(hash), header))
        };
        #[cfg(windows)]
        let (identity, (length, hash, header)) = anchor.observe(&path, directory, read)?;
        #[cfg(unix)]
        let (identity, (length, hash, header)) = native_observe(&path, directory, read)?;
        if directory {
            for entry in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
                pending.push(entry.map_err(|e| e.to_string())?.path());
            }
        }
        snapshot.entries.insert(
            path,
            Fact {
                identity,
                directory,
                length,
                hash,
                header,
            },
        );
    }
    Ok(snapshot)
}
fn coordination_shm(root: &Path, path: &Path) -> bool {
    if path == root.join("db/registry.sqlite-shm") {
        return true;
    }
    let Ok(relative) = path.strip_prefix(root.join("runs")) else {
        return false;
    };
    let mut components = relative.components();
    let run = components
        .next()
        .and_then(|part| part.as_os_str().to_str())
        .and_then(|value| value.parse::<surge_core::RunId>().ok());
    run.is_some()
        && components
            .next()
            .is_some_and(|part| part.as_os_str() == "events.sqlite-shm")
        && components.next().is_none()
}
#[cfg(unix)]
fn native_observe<T>(
    path: &Path,
    directory: bool,
    read: impl FnOnce(&File) -> std::io::Result<T>,
) -> Result<(Vec<u8>, T), String> {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::Mode,
    };
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    };
    let parent_path = path.parent().ok_or("snapshot parent")?;
    let parent = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
        .open(parent_path)
        .map_err(|e| e.to_string())?;
    let parent_id = parent.metadata().map_err(|e| e.to_string())?;
    let name = std::ffi::CString::new(path.file_name().ok_or("snapshot name")?.as_bytes())
        .map_err(|e| e.to_string())?;
    let flags = OFlag::O_RDONLY
        | OFlag::O_CLOEXEC
        | OFlag::O_NOFOLLOW
        | OFlag::O_NONBLOCK
        | if directory {
            OFlag::O_DIRECTORY
        } else {
            OFlag::empty()
        };
    let fd = openat(
        Some(parent.as_raw_fd()),
        name.as_c_str(),
        flags,
        Mode::empty(),
    )
    .map_err(|e| e.to_string())?;
    // SAFETY: openat returns one fresh descriptor transferred exactly once to File.
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let validate = |meta: &std::fs::Metadata| {
        meta.is_dir() == directory
            && (directory || (meta.is_file() && meta.nlink() == 1))
            && meta.uid() == nix::unistd::Uid::effective().as_raw()
            && meta.mode() & 0o022 == 0
    };
    if !validate(&metadata) {
        return Err("snapshot owner/link/type/mode".into());
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    crate::work_items::start_preparation::secure_lock::validate_private_acl(&file)
        .map_err(|e| e.to_string())?;
    let value = read(&file).map_err(|e| e.to_string())?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    crate::work_items::start_preparation::secure_lock::validate_private_acl(&file)
        .map_err(|e| e.to_string())?;
    let after = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let parent_after = std::fs::symlink_metadata(parent_path).map_err(|e| e.to_string())?;
    if !validate(&after)
        || (metadata.dev(), metadata.ino()) != (after.dev(), after.ino())
        || (parent_id.dev(), parent_id.ino()) != (parent_after.dev(), parent_after.ino())
    {
        return Err("snapshot parent/named identity changed".into());
    }
    Ok((
        [metadata.dev().to_le_bytes(), metadata.ino().to_le_bytes()].concat(),
        value,
    ))
}
pub(super) fn validate_readonly(before: &Snapshot, after: &Snapshot) -> Result<(), String> {
    if before.root != after.root {
        return Err("snapshot home changed".into());
    }
    for (path, fact) in &before.entries {
        let current = after
            .entries
            .get(path)
            .ok_or_else(|| format!("removed path {}", path.display()))?;
        if fact.identity != current.identity
            || fact.directory != current.directory
            || fact.hash != current.hash
            || (fact.hash.is_some() && fact.length != current.length)
        {
            return Err(format!(
                "original identity/durable bytes changed {}",
                path.display()
            ));
        }
    }
    let wal = before.root.join("db/registry.sqlite-wal");
    let shm = before.root.join("db/registry.sqlite-shm");
    for (path, fact) in &after.entries {
        if before.entries.contains_key(path) {
            continue;
        }
        if fact.directory || (path != &wal && path != &shm) {
            return Err(format!("unexpected new path {}", path.display()));
        }
        if path == &wal && !empty_wal(fact.length, &fact.header) {
            return Err("new registry WAL contains invalid header or frames".into());
        }
    }
    Ok(())
}
fn empty_wal(length: u64, bytes: &[u8]) -> bool {
    if length == 0 {
        return bytes.is_empty();
    }
    if length != 32 || bytes.len() != 32 {
        return false;
    }
    let word = |at| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let magic = word(0);
    if !matches!(magic, 0x377f_0682 | 0x377f_0683)
        || word(4) != 3_007_000
        || !(512..=65536).contains(&word(8))
        || !word(8).is_power_of_two()
    {
        return false;
    }
    let mut first = 0_u32;
    let mut second = 0_u32;
    for pair in bytes[..24].as_chunks::<8>().0 {
        let read = |v: &[u8]| {
            if magic & 1 == 0 {
                u32::from_le_bytes(v.try_into().unwrap())
            } else {
                u32::from_be_bytes(v.try_into().unwrap())
            }
        };
        first = first.wrapping_add(read(&pair[..4])).wrapping_add(second);
        second = second.wrapping_add(read(&pair[4..])).wrapping_add(first);
    }
    (word(24), word(28)) == (first, second)
}

fn fixed_snapshot() -> Snapshot {
    Snapshot {
        root: PathBuf::from("fixed-home"),
        entries: BTreeMap::from([(
            PathBuf::from("fixed-home/db/registry.sqlite"),
            Fact {
                identity: vec![1; 16],
                directory: false,
                length: 8,
                hash: Some(ContentHash::compute(b"fixed DB")),
                header: b"fixed DB".to_vec(),
            },
        )]),
    }
}
fn fixed_companion(bytes: &[u8], identity: u8) -> Fact {
    Fact {
        identity: vec![identity; 16],
        directory: false,
        length: bytes.len() as u64,
        hash: Some(ContentHash::compute(bytes)),
        header: bytes.iter().take(32).copied().collect(),
    }
}
#[test]
fn fixed_registry_companion_oracles_reject_other_paths_mutations_headers_and_frames() {
    let before = fixed_snapshot();
    let wal = before.root.join("db/registry.sqlite-wal");
    let shm = before.root.join("db/registry.sqlite-shm");
    for header in [
        "",
        "377f0682002de21800001000000000000102030405060708d8ea051498d8e2be",
        "377f0683002de218000010000000000001020304050607081706e9dbc2e5d89c",
    ] {
        let mut after = before.clone();
        after.entries.insert(
            wal.clone(),
            fixed_companion(&hex::decode(header).unwrap(), 2),
        );
        let mut coordination = fixed_companion(&[], 3);
        coordination.hash = None;
        after.entries.insert(shm.clone(), coordination);
        validate_readonly(&before, &after).unwrap();
    }
    for path in [
        "other-home/db/registry.sqlite-wal",
        "fixed-home/db/other.sqlite-wal",
        "fixed-home/runs/run-1/events.sqlite-wal",
        "fixed-home/extra",
    ] {
        let mut after = before.clone();
        after.entries.insert(path.into(), fixed_companion(&[], 2));
        assert!(
            validate_readonly(&before, &after)
                .unwrap_err()
                .contains("unexpected new path")
        );
    }
    let valid =
        hex::decode("377f0682002de21800001000000000000102030405060708d8ea051498d8e2be").unwrap();
    let mut malformed = vec![
        vec![0; 1],
        vec![0; 31],
        vec![0; 32],
        vec![0; 33],
        valid.clone(),
    ];
    malformed.last_mut().unwrap().push(0); // A byte after the header is never an empty WAL.
    for at in [0, 4, 8, 24] {
        let mut bytes = valid.clone();
        bytes[at] ^= 1;
        malformed.push(bytes);
    }
    for bytes in malformed {
        let mut after = before.clone();
        after
            .entries
            .insert(wal.clone(), fixed_companion(&bytes, 2));
        assert!(
            validate_readonly(&before, &after)
                .unwrap_err()
                .contains("invalid header or frames")
        );
    }
    let mut old_wal = before.clone();
    old_wal
        .entries
        .insert(wal.clone(), fixed_companion(b"fixed old WAL frames", 2));
    let mut changed = old_wal.clone();
    changed
        .entries
        .insert(wal.clone(), fixed_companion(b"changed old WAL frames", 2));
    assert!(
        validate_readonly(&old_wal, &changed)
            .unwrap_err()
            .contains("durable bytes changed")
    );
    let mut removed = before.clone();
    removed.entries.clear();
    assert!(
        validate_readonly(&before, &removed)
            .unwrap_err()
            .contains("removed path")
    );
    let mut changed = before.clone();
    changed.entries.values_mut().next().unwrap().hash = Some(ContentHash::compute(b"changed DB"));
    assert!(
        validate_readonly(&before, &changed)
            .unwrap_err()
            .contains("durable bytes changed")
    );
}
#[test]
fn actual_same_byte_registry_wal_and_shm_replacements_never_inherit_identity() {
    for suffix in ["-wal", "-shm"] {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        std::fs::create_dir(home.path().join("db")).unwrap();
        let path = home.path().join(format!("db/registry.sqlite{suffix}"));
        std::fs::write(&path, b"fixed identical companion bytes").unwrap();
        let before = files(home.path());
        std::fs::rename(&path, home.path().join("held-original")).unwrap();
        std::fs::write(&path, b"fixed identical companion bytes").unwrap();
        let after = files(home.path());
        assert_eq!(before.entries[&path].hash, after.entries[&path].hash);
        let refused = validate_readonly(&before, &after)
            .unwrap_err()
            .contains("original identity");
        home.close().unwrap();
        assert!(refused);
    }
}
#[test]
fn actual_registry_companion_hardlinks_are_refused_by_native_observer_before_read() {
    for suffix in ["-wal", "-shm"] {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        std::fs::create_dir(home.path().join("db")).unwrap();
        let target = home.path().join("original");
        std::fs::write(&target, b"original target").unwrap();
        files(home.path()); // The identical owner/security fixture is healthy before linking.
        std::fs::hard_link(
            &target,
            home.path().join(format!("db/registry.sqlite{suffix}")),
        )
        .unwrap();
        let refused = observe(home.path()).is_err();
        assert_eq!(std::fs::read(&target).unwrap(), b"original target");
        home.close().unwrap();
        assert!(refused);
    }
}
#[cfg(unix)]
#[test]
fn actual_registry_companion_symlinks_are_refused_before_following_targets() {
    for suffix in ["-wal", "-shm"] {
        let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        std::fs::create_dir(home.path().join("db")).unwrap();
        let target = home.path().join("original");
        std::fs::write(&target, b"original target").unwrap();
        std::os::unix::fs::symlink(
            &target,
            home.path().join(format!("db/registry.sqlite{suffix}")),
        )
        .unwrap();
        let refused = observe(home.path())
            .unwrap_err()
            .contains("snapshot alias/type");
        assert_eq!(std::fs::read(&target).unwrap(), b"original target");
        home.close().unwrap();
        assert!(refused);
    }
}

#[test]
fn arbitrary_shm_suffix_remains_durable_content_without_a_coordination_exception() {
    let home = crate::runtime_home_fixture::FixtureHome::new().unwrap();
    let path = home.path().join("private-object-shm");
    std::fs::write(&path, b"fixed original durable bytes").unwrap();
    let before = files(home.path());
    assert_eq!(
        before.entries[&path].hash,
        Some(ContentHash::compute(b"fixed original durable bytes"))
    );
    std::fs::write(&path, b"changed durable bytes").unwrap();
    let after = files(home.path());
    let rejected = validate_readonly(&before, &after)
        .unwrap_err()
        .contains("durable bytes changed");
    home.close().unwrap();
    assert!(rejected);
}
