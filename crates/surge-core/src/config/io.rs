//! Loading, discovering, saving and validating a [`SurgeConfig`], and the
//! environment overrides applied on top.

use super::*;

/// Overwrites `slot` with the parsed value of env var `key`. An unset variable
/// keeps the value; an unparsable one is reported and ignored rather than
/// silently swallowed.
fn override_from_env<T: std::str::FromStr>(key: &str, slot: &mut T) {
    let Ok(raw) = std::env::var(key) else { return };
    match raw.parse::<T>() {
        Ok(parsed) => *slot = parsed,
        Err(_) => tracing::warn!(key, value = %raw, "ignoring unparsable environment override"),
    }
}

impl SurgeConfig {
    /// Load config from a TOML file at the given path.
    pub fn load(path: &Path) -> Result<Self, crate::SurgeError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::SurgeError::Config(format!("Failed to read {}: {e}", path.display()))
        })?;
        let config: Self = toml::from_str(&content).map_err(|e| {
            // `toml`'s Display echoes source snippets, which could carry an inline
            // credential; report only the message and position.
            let (line, col) = e.span().map_or((0, 0), |sp| {
                let before = &content[..sp.start.min(content.len())];
                let line = before.matches('\n').count() + 1;
                (line, before.len() - before.rfind('\n').map_or(0, |n| n + 1) + 1)
            });
            crate::SurgeError::Config(format!(
                "Failed to parse {} at line {line}, column {col}: {} (credentials must be referenced via *_env, never inlined)",
                path.display(),
                e.message()
            ))
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate the configuration and return helpful error messages.
    pub fn validate(&self) -> Result<(), crate::SurgeError> {
        // Schema gate first: a config from a newer surge must not be
        // silently misread by an older binary.
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(crate::SurgeError::Config(format!(
                "unsupported surge.toml schema_version {} (this build supports {}). \
                 Upgrade surge, or migrate the config — see docs/schema-versioning.md.",
                self.schema_version, CONFIG_SCHEMA_VERSION
            )));
        }

        // Validate default_agent exists in agents map (when agents are configured)
        if !self.agents.is_empty() && !self.agents.contains_key(&self.default_agent) {
            return Err(crate::SurgeError::Config(format!(
                "default_agent '{}' not found in agents. Available agents: {}",
                self.default_agent,
                self.agents
                    .keys()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        // Validate each agent configuration
        for (name, agent) in &self.agents {
            agent.validate(name)?;
        }

        // Validate pipeline configuration
        self.pipeline.validate()?;
        self.init.validate()?;
        self.capacity.validate()?;

        // Validate routing agent_preferences reference existing agents
        if !self.agents.is_empty() {
            for (complexity, agent_name) in &self.routing.agent_preferences {
                if !self.agents.contains_key(agent_name) {
                    return Err(crate::SurgeError::Config(format!(
                        "routing.agent_preferences['{complexity}'] references unknown agent '{agent_name}'. \
                         Available agents: {}",
                        self.agents
                            .keys()
                            .map(|k| k.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            }
        }

        // Validate log level
        const VALID_LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];
        if !VALID_LEVELS.contains(&self.log.level.as_str()) {
            return Err(crate::SurgeError::Config(format!(
                "log.level '{}' is invalid. Must be one of: {}",
                self.log.level,
                VALID_LEVELS.join(", ")
            )));
        }

        Ok(())
    }

    /// Discover surge.toml by searching current directory and parent directories.
    /// Returns a default configuration if no file is found.
    pub fn discover() -> Result<Self, crate::SurgeError> {
        let start_dir = std::env::current_dir().map_err(|e| {
            crate::SurgeError::Config(format!("Failed to get current directory: {e}"))
        })?;
        Self::discover_from(&start_dir)
    }

    /// Discover surge.toml by searching `start_dir` and parent directories.
    /// Returns a default configuration if no file is found.
    pub fn discover_from(start_dir: &Path) -> Result<Self, crate::SurgeError> {
        match Self::find_config_file(start_dir) {
            Ok(config_path) => Self::load(&config_path),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Apply environment variable overrides to the configuration.
    /// Environment variables with the SURGE_* prefix override config values:
    /// - SURGE_DEFAULT_AGENT
    /// - SURGE_MAX_QA_ITERATIONS
    /// - SURGE_MAX_PARALLEL
    /// - SURGE_GATE_AFTER_SPEC
    /// - SURGE_GATE_AFTER_PLAN
    /// - SURGE_GATE_AFTER_EACH_SUBTASK
    /// - SURGE_GATE_AFTER_QA
    pub fn apply_env_overrides(&mut self) {
        if let Ok(value) = std::env::var("SURGE_DEFAULT_AGENT") {
            self.default_agent = value;
        }
        override_from_env(
            "SURGE_MAX_QA_ITERATIONS",
            &mut self.pipeline.max_qa_iterations,
        );
        override_from_env("SURGE_MAX_PARALLEL", &mut self.pipeline.max_parallel);
        let gates = &mut self.pipeline.gates;
        override_from_env("SURGE_GATE_AFTER_SPEC", &mut gates.after_spec);
        override_from_env("SURGE_GATE_AFTER_PLAN", &mut gates.after_plan);
        override_from_env(
            "SURGE_GATE_AFTER_EACH_SUBTASK",
            &mut gates.after_each_subtask,
        );
        override_from_env("SURGE_GATE_AFTER_QA", &mut gates.after_qa);
    }

    /// Save config to a TOML file at the given path.
    ///
    /// Validates the config before writing, creates parent directories
    /// if needed, and writes atomically via a same-directory temp
    /// file + `rename` so a process crash mid-write can never leave
    /// a partially-written / corrupted TOML on disk.
    #[must_use = "save returns a Result that should be checked"]
    pub fn save(&self, path: &Path) -> Result<(), crate::SurgeError> {
        self.validate()?;
        let content = toml::to_string_pretty(self)
            .map_err(|e| crate::SurgeError::Config(format!("Failed to serialize config: {e}")))?;
        let parent = path.parent().ok_or_else(|| {
            crate::SurgeError::Config(format!(
                "Cannot save config to {}: path has no parent directory",
                path.display()
            ))
        })?;
        std::fs::create_dir_all(parent).map_err(|e| {
            crate::SurgeError::Config(format!(
                "Failed to create directory {}: {e}",
                parent.display()
            ))
        })?;

        // Write through an exclusively created descriptor: concurrent saves do not
        // share a path, and an old predictable-name symlink cannot redirect writes.
        // The same-directory persist atomically replaces the destination. This does
        // not promise power-loss durability of the directory entry.
        use std::io::Write;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
            crate::SurgeError::Config(format!(
                "Failed to create temp config in {}: {e}",
                parent.display()
            ))
        })?;
        temporary
            .write_all(content.as_bytes())
            .map_err(|e| crate::SurgeError::Config(format!("Failed to write temp config: {e}")))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|e| crate::SurgeError::Config(format!("Failed to sync temp config: {e}")))?;
        temporary.persist(path).map_err(|e| {
            crate::SurgeError::Config(format!(
                "Failed to replace config {}: {}",
                path.display(),
                e.error
            ))
        })?;
        Ok(())
    }

    /// Find surge.toml by walking up from the given directory.
    pub(super) fn find_config_file(start_dir: &Path) -> Result<PathBuf, crate::SurgeError> {
        let mut current = start_dir;

        loop {
            let candidate = current.join("surge.toml");
            if candidate.exists() {
                return Ok(candidate);
            }

            // Move to parent directory
            match current.parent() {
                Some(parent) => current = parent,
                None => {
                    return Err(crate::SurgeError::Config(format!(
                        "surge.toml not found in {} or any parent directory",
                        start_dir.display()
                    )));
                },
            }
        }
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn save_ignores_predictable_temporary_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("surge.toml");
        let victim = directory.path().join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        let legacy_temp = directory
            .path()
            .join(format!(".surge.toml.tmp.{}", std::process::id()));
        std::os::unix::fs::symlink(&victim, &legacy_temp).unwrap();
        SurgeConfig::default().save(&destination).unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        assert!(
            std::fs::symlink_metadata(&legacy_temp)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        SurgeConfig::load(&destination).unwrap();
    }

    #[test]
    fn concurrent_saves_publish_complete_configs() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("surge.toml");
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let mut writers = Vec::new();
            for writer in 0..8 {
                let destination = &destination;
                let barrier = &barrier;
                writers.push(scope.spawn(move || {
                    let config = SurgeConfig {
                        default_agent: format!("writer-{writer}"),
                        ..SurgeConfig::default()
                    };
                    barrier.wait();
                    repeatedly_save_and_read(&config, destination);
                }));
            }
            for writer in writers {
                writer.join().unwrap();
            }
        });
        let config = SurgeConfig::load(&destination).unwrap();
        assert!(config.default_agent.starts_with("writer-"));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    fn repeatedly_save_and_read(config: &SurgeConfig, destination: &Path) {
        for _ in 0..20 {
            config.save(destination).unwrap();
            SurgeConfig::load(destination).unwrap();
        }
    }

    #[test]
    fn failed_persist_preserves_destination_and_removes_owned_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("surge.toml");
        std::fs::create_dir(&destination).unwrap();
        let marker = destination.join("keep");
        std::fs::write(&marker, "original").unwrap();
        assert!(SurgeConfig::default().save(&destination).is_err());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "original");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("surge.toml");
        SurgeConfig::default().save(&destination).unwrap();
        let permissions = std::fs::metadata(destination).unwrap().permissions().mode();
        assert_eq!(permissions & 0o777, 0o600);
    }
}
