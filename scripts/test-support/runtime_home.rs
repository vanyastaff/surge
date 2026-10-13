// Test-only runtime homes. The including module supplies RuntimeHomeOwner on Windows.
// This owns directories only; Storage must acquire its own database identity fences.
pub struct FixtureHome {
    #[cfg(windows)]
    owner: RuntimeHomeOwner,
    path: std::path::PathBuf,
    outer: tempfile::TempDir,
}

impl FixtureHome {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(windows)]
        {
            let profile = RuntimeHomeOwner::user_profile_path()?;
            let outer = tempfile::Builder::new()
                .prefix("surge-runtime-fixture-")
                .tempdir_in(profile)?;
            let path = outer.path().join("state");
            let owner = RuntimeHomeOwner::prepare(&path)?;
            Ok(Self { owner, path, outer })
        }
        #[cfg(not(windows))]
        {
            let outer = tempfile::tempdir()?;
            let path = outer.path().to_path_buf();
            Ok(Self { path, outer })
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    // Call only after writers, tasks, stores and pools have been dropped.
    // TempDir's implicit destructor is just a panic-unwind fallback.
    //
    // Windows releases a dropped SQLite pool's file handles on background
    // threads, after the last owner is gone, so removal can briefly hit a
    // sharing violation (os error 32). Retry that, and only that, for a
    // bounded time: a handle that is genuinely leaked still fails the test.
    pub fn close(self) -> std::io::Result<()> {
        let Self {
            #[cfg(windows)]
            owner,
            path: _,
            outer,
        } = self;
        #[cfg(windows)]
        {
            drop(owner);
            const SHARING_VIOLATION: i32 = 32;
            let path = outer.keep();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => return Ok(()),
                    Err(error)
                        if error.raw_os_error() == Some(SHARING_VIOLATION)
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    },
                    Err(error) => return Err(error),
                }
            }
        }
        #[cfg(not(windows))]
        outer.close()
    }
}
