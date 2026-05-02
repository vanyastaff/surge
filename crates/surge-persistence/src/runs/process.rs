//! Cross-platform process liveness probe (sysinfo, TTL-cached).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::{Pid, RefreshKind, System};

/// Liveness probe for daemon PIDs with a short TTL cache.
///
/// Refreshing sysinfo's process table is expensive (10-50 ms on busy systems);
/// the cache amortizes that across many `is_alive` calls within the TTL.
pub struct ProcessProbe {
    inner: Mutex<Inner>,
    ttl: Duration,
}

struct Inner {
    sys: System,
    last_refresh: Instant,
}

impl ProcessProbe {
    /// Default TTL: 500 ms.
    #[must_use]
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_millis(500))
    }

    /// Custom TTL.
    #[must_use]
    pub fn with_ttl(ttl: Duration) -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::new().with_processes(sysinfo::ProcessRefreshKind::new()),
        );
        // Force first refresh to be a real refresh.
        Self {
            inner: Mutex::new(Inner {
                sys,
                last_refresh: Instant::now() - ttl - Duration::from_secs(1),
            }),
            ttl,
        }
    }

    /// Returns true if the given pid currently maps to a live process.
    ///
    /// pid <= 0 is always false. Negative or zero pids are not valid on any
    /// supported platform.
    pub fn is_alive(&self, pid: i32) -> bool {
        if pid <= 0 {
            return false;
        }
        let mut g = self.inner.lock().expect("poisoned ProcessProbe mutex");
        if g.last_refresh.elapsed() > self.ttl {
            g.sys.refresh_processes(sysinfo::ProcessesToUpdate::All, false);
            g.last_refresh = Instant::now();
        }
        g.sys.process(Pid::from_u32(pid as u32)).is_some()
    }
}

impl Default for ProcessProbe {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        let probe = ProcessProbe::new();
        let me = std::process::id() as i32;
        assert!(probe.is_alive(me));
    }

    #[test]
    fn impossibly_high_pid_is_not_alive() {
        let probe = ProcessProbe::new();
        assert!(!probe.is_alive(i32::MAX));
    }

    #[test]
    fn zero_or_negative_pid_is_not_alive() {
        let probe = ProcessProbe::new();
        assert!(!probe.is_alive(0));
        assert!(!probe.is_alive(-1));
    }
}
