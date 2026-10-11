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

    // Call only after writers, tasks, stores and pools have actually settled.
    // TempDir's implicit destructor is just a panic-unwind fallback.
    pub fn close(self) -> std::io::Result<()> {
        let Self {
            #[cfg(windows)]
            owner,
            path: _,
            outer,
        } = self;
        #[cfg(windows)]
        drop(owner);
        outer.close()
    }
}
