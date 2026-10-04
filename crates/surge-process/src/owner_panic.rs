//! Install once; protect ownership operations without displaying panic payloads.
use std::cell::Cell;
use std::sync::OnceLock;

thread_local! {
    static PROTECTED: Cell<u32> = const { Cell::new(0) };
}
static INSTALLED: OnceLock<()> = OnceLock::new();

/// Install the single shared protected-owner hook after host crash reporting.
/// Call before any protected ownership operation starts.
pub fn install_owner_panic_protection() {
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |information| {
            let protected = PROTECTED.try_with(|depth| depth.get() != 0).unwrap_or(true);
            if !protected {
                previous(information);
            }
        }));
    });
}

/// Run an owner-sensitive operation, aborting without displaying or dropping panic payloads.
/// The caller installs the shared hook before entering this scope.
pub fn abort_on_owner_panic<T>(operation: impl FnOnce() -> T) -> T {
    let Ok(previous) = PROTECTED.try_with(|depth| {
        let previous = depth.get();
        depth.set(previous.saturating_add(1));
        previous
    }) else {
        std::process::abort();
    };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(value) => {
            if PROTECTED.try_with(|depth| depth.set(previous)).is_err() {
                std::process::abort();
            }
            value
        },
        Err(payload) => {
            // A payload destructor can panic; abort with every outer owner alive.
            std::mem::forget(payload);
            std::process::abort();
        },
    }
}
