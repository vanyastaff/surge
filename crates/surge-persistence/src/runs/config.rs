//! Storage configuration loaded from `~/.surge/config.toml`.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Storage-level config (loaded from `[storage]` section of `~/.surge/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// How often to issue an explicit `wal_checkpoint(TRUNCATE)` from the writer.
    #[serde(default = "default_checkpoint_interval")]
    pub checkpoint_interval_seconds: u64,

    /// r2d2 pool max_size for per-run reader connections.
    #[serde(default = "default_reader_pool_size")]
    pub reader_pool_size: u32,

    /// Bound on the writer's WriterCommand mpsc channel.
    #[serde(default = "default_writer_capacity")]
    pub writer_channel_capacity: usize,
}

fn default_checkpoint_interval() -> u64 {
    300
}
fn default_reader_pool_size() -> u32 {
    4
}
fn default_writer_capacity() -> usize {
    64
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            checkpoint_interval_seconds: default_checkpoint_interval(),
            reader_pool_size: default_reader_pool_size(),
            writer_channel_capacity: default_writer_capacity(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    storage: StorageConfig,
}

/// Load `~/.surge/config.toml` from `home`, returning defaults if absent or unparseable.
#[must_use]
pub fn load_or_default(home: &Path) -> StorageConfig {
    let path = home.join("config.toml");
    let Ok(s) = std::fs::read_to_string(&path) else {
        return StorageConfig::default();
    };
    toml::from_str::<ConfigFile>(&s)
        .map(|c| c.storage)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn defaults_when_file_absent() {
        let tmp = TempDir::new().unwrap();
        let cfg = load_or_default(tmp.path());
        assert_eq!(cfg.reader_pool_size, 4);
        assert_eq!(cfg.checkpoint_interval_seconds, 300);
        assert_eq!(cfg.writer_channel_capacity, 64);
    }

    #[test]
    fn parses_present_overrides() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("config.toml"),
            "[storage]\ncheckpoint_interval_seconds = 60\nreader_pool_size = 16\n",
        )
        .unwrap();
        let cfg = load_or_default(tmp.path());
        assert_eq!(cfg.checkpoint_interval_seconds, 60);
        assert_eq!(cfg.reader_pool_size, 16);
        assert_eq!(cfg.writer_channel_capacity, 64);
    }
}
