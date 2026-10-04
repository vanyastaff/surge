//! Bounded process-terminal ownership registry; handles stay here until joined.
use super::{ChildSlot, panic_boundary};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicU8, Ordering},
    mpsc,
};

const LIMIT: usize = 128;
pub(super) const ACTIVE: u8 = 0;
pub(super) const GRACEFUL: u8 = 1;
pub(super) const IMMEDIATE: u8 = 2;

pub(super) enum Start {
    Child(ChildSlot, mpsc::Sender<()>),
    NoChild,
    Settled,
}
pub(super) struct Completion {
    done: Mutex<bool>,
    condition: Condvar,
    notify: tokio::sync::Notify,
}
impl Completion {
    fn new() -> Self {
        Self {
            done: Mutex::new(false),
            condition: Condvar::new(),
            notify: tokio::sync::Notify::new(),
        }
    }
    pub(super) fn finish(&self) {
        *lock(&self.done) = true;
        self.condition.notify_all();
        self.notify.notify_waiters();
    }
    pub(super) fn wait(&self) {
        let mut done = lock(&self.done);
        while !*done {
            done = match self.condition.wait(done) {
                Ok(done) => done,
                Err(_) => std::process::abort(),
            };
        }
    }
    pub(super) async fn notified(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if *lock(&self.done) {
                return;
            }
            notified.await;
        }
    }
}
pub(super) struct Ticket {
    pub(super) control: Arc<AtomicU8>,
    pub(super) completion: Arc<Completion>,
}
impl Ticket {
    pub(super) fn request(&self, graceful: bool) {
        self.control.fetch_max(
            if graceful { GRACEFUL } else { IMMEDIATE },
            Ordering::SeqCst,
        );
    }
}
struct Entry {
    control: Arc<AtomicU8>,
    handle: std::thread::JoinHandle<()>,
}
#[derive(Default)]
struct Registry {
    closed: bool,
    entries: Vec<Entry>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
static JOINING: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
#[cfg(all(test, unix))]
static WAITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[cfg(all(test, unix))]
pub(super) fn waiting_barriers() -> usize {
    WAITING.load(Ordering::SeqCst)
}
struct JoinDomain;
impl Drop for JoinDomain {
    fn drop(&mut self) {
        *lock(&JOINING.0) = false;
        JOINING.1.notify_all();
    }
}
fn join_domain() -> JoinDomain {
    let mut busy = lock(&JOINING.0);
    while *busy {
        #[cfg(all(test, unix))]
        WAITING.fetch_add(1, Ordering::SeqCst);
        busy = match JOINING.1.wait(busy) {
            Ok(busy) => busy,
            Err(_) => std::process::abort(),
        };
        #[cfg(all(test, unix))]
        WAITING.fetch_sub(1, Ordering::SeqCst);
    }
    *busy = true;
    // The token serializes reaping/barriers; no mutex guard crosses a join.
    JoinDomain
}

pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(_) => std::process::abort(),
    }
}
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(Mutex::default)
}

pub(super) fn reserve() -> std::io::Result<(mpsc::Sender<Start>, Ticket)> {
    panic_boundary::install();
    let _joining = join_domain();
    reap_finished();
    let mut registry = lock(registry());
    if registry.closed || registry.entries.len() >= LIMIT {
        return Err(std::io::Error::other("mcp_child_admission_unavailable"));
    }
    let (send, receive) = mpsc::channel();
    let control = Arc::new(AtomicU8::new(ACTIVE));
    let completion = Arc::new(Completion::new());
    let worker_control = control.clone();
    let worker_completion = completion.clone();
    let handle = std::thread::Builder::new()
        .name("mcp-child-owner".into())
        .stack_size(512 * 1024)
        .spawn(move || super::worker::run(&receive, &worker_control, &worker_completion))
        .map_err(|_| std::io::Error::other("mcp_child_owner_thread_unavailable"))?;
    registry.entries.push(Entry {
        control: control.clone(),
        handle,
    });
    Ok((
        send,
        Ticket {
            control,
            completion,
        },
    ))
}

fn reap_finished() {
    let handles = {
        let mut registry = lock(registry());
        let mut handles = Vec::new();
        let mut index = 0;
        while index < registry.entries.len() {
            if registry.entries[index].handle.is_finished() {
                handles.push(registry.entries.swap_remove(index).handle);
            } else {
                index += 1;
            }
        }
        handles
    };
    join(handles);
}
fn join(handles: impl IntoIterator<Item = std::thread::JoinHandle<()>>) {
    for handle in handles {
        if handle.join().is_err() {
            std::process::abort();
        }
    }
}

pub(super) fn shutdown() {
    let _joining = join_domain();
    let entries = {
        let mut registry = lock(registry());
        registry.closed = true;
        for entry in &registry.entries {
            entry.control.store(IMMEDIATE, Ordering::SeqCst);
        }
        std::mem::take(&mut registry.entries)
    };
    join(entries.into_iter().map(|entry| entry.handle));
}
