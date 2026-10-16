//! Authored project files are proposed inputs, never accepted execution evidence.
use super::*;
use std::io::Read;
const MAX_PLAN_BYTES: u64 = 1024 * 1024;
pub(crate) async fn load_planning_inputs(root: PathBuf) -> Vec<PlanningInput> {
    let paths = [root.join("roadmap.toml"), root.join(".surge/roadmap.toml")];
    let fallback = paths.clone();
    match tokio::task::spawn_blocking(move || paths.into_iter().map(inspect).collect()).await {
        Ok(inputs) => inputs,
        Err(error) => fallback
            .into_iter()
            .map(|path| PlanningInput {
                path,
                roadmap: None,
                source: observation(
                    Availability::Unavailable(error.to_string()),
                    Coverage::Unknown,
                ),
            })
            .collect(),
    }
}
fn observation(availability: Availability, coverage: Coverage) -> SourceObservation {
    SourceObservation {
        availability,
        coverage,
        freshness: Freshness::Observed,
        through: None,
    }
}
fn inspect(path: PathBuf) -> PlanningInput {
    let result = (|| {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err("Planning input must be a regular file, not a link or device".into());
            },
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
        let bytes = read_regular_bounded(&path)?;
        let text = std::str::from_utf8(&bytes).map_err(|error| error.to_string())?;
        toml::from_str::<RoadmapArtifact>(text)
            .map(|plan| Some(Arc::new(plan)))
            .map_err(|error| error.to_string())
    })();
    let (roadmap, source) = match result {
        Ok(Some(plan)) => (
            Some(plan),
            observation(Availability::Available, Coverage::Complete),
        ),
        Ok(None) => (None, observation(Availability::Absent, Coverage::Complete)),
        Err(error) => (
            None,
            observation(Availability::Unavailable(error), Coverage::Unknown),
        ),
    };
    PlanningInput {
        path,
        roadmap,
        source,
    }
}

/// Bounded regular-file read; Unix opens also refuse links and cannot wait on a device.
pub(super) fn read_regular_bounded(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    if !std::fs::symlink_metadata(path)
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Planning input must be a regular file, not a link or device".into());
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Planning input is not a regular file".into());
    }
    if metadata.len() > MAX_PLAN_BYTES {
        return Err("Planning input exceeds 1 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PLAN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PLAN_BYTES {
        return Err("Planning input exceeds 1 MiB".into());
    }
    Ok(bytes)
}
