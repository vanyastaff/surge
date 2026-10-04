//! Debug-only, exact-item fault seam for an isolated host process. No marker grants authority.
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use surge_core::id::WorkItemId;
use surge_persistence::work_items::WorkItemError;

fn marker(path: &Path, create: bool) -> Result<File, WorkItemError> {
    let mut options = OpenOptions::new();
    options.read(true).write(create).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK | nix::libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > 64 {
        return Err(WorkItemError::Invalid(
            "unsafe preparation phase marker".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != nix::unistd::Uid::effective().as_raw()
            || meta.mode() & 0o7777 != 0o600
            || meta.nlink() != 1
        {
            return Err(WorkItemError::Invalid(
                "unsafe preparation phase marker".into(),
            ));
        }
    }
    Ok(file)
}
pub(super) fn wait(item: WorkItemId) -> Result<(), WorkItemError> {
    let Ok(selected) = std::env::var("SURGE_START_PREPARATION_ITEM") else {
        return Ok(());
    };
    let selected: WorkItemId = selected
        .parse()
        .map_err(|_| WorkItemError::Invalid("invalid preparation fault item".into()))?;
    if selected != item {
        return Ok(());
    }
    let ready = std::env::var_os("SURGE_START_PREPARATION_READY")
        .ok_or_else(|| WorkItemError::Invalid("missing preparation ready marker".into()))?;
    let release = std::env::var_os("SURGE_START_PREPARATION_RELEASE")
        .ok_or_else(|| WorkItemError::Invalid("missing preparation release marker".into()))?;
    let mut ready = marker(Path::new(&ready), true)?;
    ready.write_all(item.to_string().as_bytes())?;
    ready.sync_all()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match marker(Path::new(&release), false) {
            Ok(mut file) => {
                let before = file.metadata()?;
                let mut value = String::new();
                (&mut file).take(65).read_to_string(&mut value)?;
                let after = file.metadata()?;
                if before.len() != after.len() || value != item.to_string() {
                    return Err(WorkItemError::Invalid(
                        "invalid preparation release marker".into(),
                    ));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    let linked = std::fs::symlink_metadata(Path::new(&release))?;
                    if linked.dev() != after.dev()
                        || linked.ino() != after.ino()
                        || linked.file_type().is_symlink()
                    {
                        return Err(WorkItemError::Invalid(
                            "changed preparation release marker".into(),
                        ));
                    }
                }
                return Ok(());
            },
            Err(WorkItemError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        if std::time::Instant::now() >= deadline {
            return Err(WorkItemError::Invalid(
                "preparation fault barrier timed out".into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
