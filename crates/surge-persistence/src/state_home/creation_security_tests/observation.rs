//! Independent SDK observer and a bounded, thread-bound two-phase handshake.
use super::super::{test_security, windows::creation_observation};
use std::{
    any::Any,
    fs::File,
    os::windows::fs::MetadataExt,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    time::Duration,
};

const LIMIT: Duration = Duration::from_secs(10);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    Home,
    Database,
}
impl Phase {
    fn directory(self) -> bool {
        self == Self::Home
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    token: u64,
    phase: Phase,
    path: PathBuf,
    identity: test_security::FileIdentity,
}

#[derive(Debug)]
pub(super) struct Observation {
    pub(super) path: PathBuf,
    pub(super) identity: test_security::FileIdentity,
}

fn panic_text(panic: Box<dyn Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "non-string panic".to_owned()
    }
}

fn observe(notice: &Notice, actor: &str) -> Observation {
    let file = test_security::open_observer(&notice.path);
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.is_dir(), notice.phase.directory());
    assert_eq!(metadata.file_attributes() & 0x400, 0, "no reparse point");
    let identity = test_security::file_identity(&file);
    assert_eq!(
        identity, notice.identity,
        "observe the actual created object"
    );
    test_security::assert_private_handle(&file, actor, notice.phase.directory());
    if notice.phase == Phase::Database {
        assert_eq!(metadata.len(), 0, "database must still be unwritten");
    }
    drop(file); // No observation handle survives the acknowledgement.
    Observation {
        path: notice.path.clone(),
        identity,
    }
}

fn observer(
    token: u64,
    paths: [(Phase, PathBuf); 2],
    actor: &str,
    requests: &Receiver<Notice>,
    acknowledgements: &SyncSender<Result<Notice, String>>,
) -> Result<Vec<Observation>, String> {
    let mut observations = Vec::new();
    for (phase, path) in paths {
        let notice = requests
            .recv_timeout(LIMIT)
            .map_err(|error| error.to_string())?;
        let observation = catch_unwind(AssertUnwindSafe(|| {
            assert_eq!(notice.token, token);
            assert_eq!(notice.phase, phase);
            assert_eq!(notice.path, path);
            observe(&notice, actor)
        }))
        .map_err(panic_text);
        let acknowledgement = observation.as_ref().map(|_| notice).map_err(Clone::clone);
        acknowledgements
            .try_send(acknowledgement)
            .map_err(|error| error.to_string())?;
        observations.push(observation?);
    }
    // No third request can be acknowledged; the producer requires exactly two.
    Ok(observations)
}

fn matches_parent(parent: &File, expected: &std::path::Path) -> bool {
    // For the database this path only exists at its FILE_CREATE boundary. Obtain
    // the expected identity by a separate SDK open, never from the production handle.
    let expected = test_security::open_observer(expected.parent().unwrap());
    test_security::file_identity(&expected) == test_security::file_identity(parent)
}

pub(super) fn run(home: PathBuf, actor: String, operation: impl FnOnce()) -> Vec<Observation> {
    let token = NEXT_TOKEN
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
        .unwrap();
    let paths = [
        (Phase::Home, home.clone()),
        (Phase::Database, home.join("db/registry.sqlite")),
    ];
    let expected = paths.clone();
    let (requests, receiver) = sync_channel(1);
    let (acknowledgements, replies) = sync_channel::<Result<Notice, String>>(1);
    let mut acknowledged = 0;
    let guard = creation_observation::register(move |parent, name, directory, created| {
        let Some((index, (phase, path))) =
            expected.iter().enumerate().find(|(_, (phase, path))| {
                directory == phase.directory() && path.file_name() == Some(name)
            })
        else {
            return;
        };
        if !matches_parent(parent, path) {
            return;
        }
        assert_eq!(index, acknowledged, "duplicate/out-of-order creation phase");
        let notice = Notice {
            token,
            phase: *phase,
            path: path.clone(),
            identity: test_security::file_identity(created),
        };
        requests.try_send(notice.clone()).unwrap();
        let acknowledged_notice = replies.recv_timeout(LIMIT).unwrap().unwrap();
        assert_eq!(
            acknowledged_notice, notice,
            "exact creation acknowledgement"
        );
        acknowledged += 1;
    });
    let observer =
        std::thread::spawn(move || observer(token, paths, &actor, &receiver, &acknowledgements));
    let producer = catch_unwind(AssertUnwindSafe(operation)).map_err(panic_text);
    // Also drops the request sender captured by the TLS callback. A producer
    // error, callback timeout or observer panic cannot leave the peer at a barrier.
    drop(guard);
    let observed = observer
        .join()
        .map_err(panic_text)
        .and_then(|result| result);
    assert!(
        producer.is_ok() && observed.is_ok(),
        "producer: {:?}; observer: {:?}",
        producer.err(),
        observed.as_ref().err()
    );
    let observed = observed.unwrap();
    assert_eq!(
        observed.len(),
        2,
        "both creation acknowledgements are mandatory"
    );
    observed
}
