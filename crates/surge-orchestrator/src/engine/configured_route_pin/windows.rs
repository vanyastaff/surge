//! Windows publication delegates to retained native persistence capabilities.
use super::{
    RoutePinError,
    key_format::{MAGIC, checksum, decode},
};
use rand::TryRngCore;
use std::path::Path;
const NAME: &str = "configured-route-pin.key";
pub(super) fn load_or_create(home: &Path) -> Result<[u8; 32], RoutePinError> {
    let files = surge_persistence::RuntimeImmutableFiles::open_existing_root(home)
        .map_err(|_| RoutePinError::UnsafeLocation)?;
    if let Some(bytes) = files.read(NAME).map_err(|_| RoutePinError::InvalidKey)? {
        return decode(&bytes);
    }
    let mut key = [0; 32];
    rand::rngs::OsRng
        .try_fill_bytes(&mut key)
        .map_err(|_| RoutePinError::Random)?;
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&key);
    bytes.extend_from_slice(&checksum(&key));
    files
        .publish(NAME, &bytes)
        .map_err(|_| RoutePinError::KeyIo)?;
    // Both the publisher and a concurrent loser independently validate and flush the winner.
    let winner = files
        .read(NAME)
        .map_err(|_| RoutePinError::InvalidKey)?
        .ok_or(RoutePinError::InvalidKey)?;
    decode(&winner)
}
pub(super) fn fingerprint_source(
    mac: &mut hmac::Hmac<sha2::Sha256>,
    path: &Path,
) -> Result<(), RoutePinError> {
    use hmac::Mac;
    const LIMIT: u64 = 16 * 1024 * 1024;
    let bytes = surge_persistence::work_items::read_stable_declared_source(path, LIMIT).map_err(
        |error| match error {
            surge_persistence::work_items::DeclaredSourceError::Unavailable => {
                RoutePinError::SourceUnavailable
            },
            surge_persistence::work_items::DeclaredSourceError::TooLarge => {
                RoutePinError::SourceTooLarge
            },
            surge_persistence::work_items::DeclaredSourceError::Changed => {
                RoutePinError::SourceChanged
            },
        },
    )?;
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(&bytes);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::configured_route_pin::RoutePinKey;
    use hmac::Mac;
    use std::collections::BTreeMap;
    #[test]
    fn native_key_concurrent_publication_and_corruption_refusal() {
        let fixture = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let home = fixture.path().to_path_buf();
        let keys: Vec<_> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| load_or_create(&home).unwrap()))
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect()
        });
        assert!(keys.iter().all(|key| key == &keys[0]));
        let path = home.join(NAME);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[MAGIC.len()] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(load_or_create(&home), Err(RoutePinError::InvalidKey));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        fixture.close().unwrap();
    }
    #[test]
    fn native_source_bytes_change_route_and_final_directory_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("auth.json");
        std::fs::write(&source, b"first").unwrap();
        let key = RoutePinKey([42; 32]);
        let pin = || {
            key.fingerprint(
                b"identity",
                &BTreeMap::new(),
                std::slice::from_ref(&source),
                temp.path(),
            )
        };
        let first = pin().unwrap();
        std::fs::write(&source, b"other").unwrap();
        assert_ne!(first, pin().unwrap());
        std::fs::remove_file(&source).unwrap();
        std::fs::create_dir(&source).unwrap();
        assert_eq!(pin(), Err(RoutePinError::SourceUnavailable));
    }
    #[test]
    fn native_source_size_limit_is_typed_and_hmac_framing_is_unchanged() {
        use hmac::Mac;
        if std::env::var_os("SURGE_ROUTE_PIN_BARE_SOURCE").is_some() {
            assert_eq!(
                surge_persistence::work_items::read_stable_declared_source(
                    Path::new("source"),
                    1024
                )
                .unwrap(),
                b"abc"
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::write(&source, b"abc").unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "engine::configured_route_pin::windows_key::tests::native_source_size_limit_is_typed_and_hmac_framing_is_unchanged", "--nocapture"])
            .env("SURGE_ROUTE_PIN_BARE_SOURCE", "1").current_dir(temp.path()).spawn().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("bare-source child timed out");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut actual = hmac::Hmac::<sha2::Sha256>::new_from_slice(&[7; 32]).unwrap();
        fingerprint_source(&mut actual, &source).unwrap();
        let mut expected = hmac::Hmac::<sha2::Sha256>::new_from_slice(&[7; 32]).unwrap();
        expected.update(&[0, 0, 0, 0, 0, 0, 0, 3, b'a', b'b', b'c']);
        assert_eq!(
            actual.finalize().into_bytes(),
            expected.finalize().into_bytes()
        );
        std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap()
            .set_len(16 * 1024 * 1024 + 1)
            .unwrap();
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&[7; 32]).unwrap();
        assert_eq!(
            fingerprint_source(&mut mac, &source),
            Err(RoutePinError::SourceTooLarge)
        );
    }
    #[test]
    fn native_source_allows_ancestor_junction_but_refuses_final_reparse() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual-資料");
        std::fs::create_dir(&actual).unwrap();
        std::fs::write(actual.join("auth"), b"declared bytes").unwrap();
        let alias = temp.path().join("alias-資料");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&alias)
            .arg(&actual)
            .status()
            .unwrap();
        assert!(status.success());
        let read =
            surge_persistence::work_items::read_stable_declared_source(&alias.join("auth"), 1024)
                .unwrap();
        assert_eq!(read, b"declared bytes");
        assert_eq!(
            surge_persistence::work_items::read_stable_declared_source(
                &alias.join("..").join("actual-資料").join("auth"),
                1024
            )
            .unwrap(),
            b"declared bytes"
        );
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&[7; 32]).unwrap();
        assert_eq!(
            fingerprint_source(&mut mac, &alias),
            Err(RoutePinError::SourceUnavailable)
        );
        std::fs::remove_dir(alias).unwrap();
    }
    #[test]
    fn native_existing_reparse_key_is_refused_without_target_mutation() {
        let fixture = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let home = fixture.path().to_path_buf();
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"unchanged").unwrap();
        let path = home.join(NAME);
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&path)
            .arg(&target)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(load_or_create(&home), Err(RoutePinError::InvalidKey));
        assert_eq!(
            std::fs::read(target.join("sentinel")).unwrap(),
            b"unchanged"
        );
        std::fs::remove_dir(path).unwrap();
        fixture.close().unwrap();
    }
    #[test]
    fn native_existing_hardlinked_key_is_refused_without_replacement() {
        let fixture = crate::runtime_home_fixture::FixtureHome::new().unwrap();
        let home = fixture.path().to_path_buf();
        load_or_create(&home).unwrap();
        let path = home.join(NAME);
        let original = std::fs::read(&path).unwrap();
        std::fs::hard_link(&path, home.join("alias-資料")).unwrap();
        assert_eq!(load_or_create(&home), Err(RoutePinError::InvalidKey));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        fixture.close().unwrap();
    }
}
