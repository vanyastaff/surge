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

        // Same-directory temp file + atomic rename. On every platform
        // we support, a successful `rename` within a single directory
        // is atomic at the filesystem level: readers see either the
        // old `path` contents or the new ones, never a partial mix.
        let file_name = path
            .file_name()
            .ok_or_else(|| {
                crate::SurgeError::Config(format!(
                    "Cannot save config to {}: path has no file name",
                    path.display()
                ))
            })?
            .to_string_lossy();
        let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));
        std::fs::write(&tmp, content).map_err(|e| {
            crate::SurgeError::Config(format!(
                "Failed to write temp config {}: {e}",
                tmp.display()
            ))
        })?;
        if let Err(e) = std::fs::rename(&tmp, path) {
            // Best-effort: clean up the temp file on rename failure.
            let _ = std::fs::remove_file(&tmp);
            return Err(crate::SurgeError::Config(format!(
                "Failed to rename {} -> {}: {e}",
                tmp.display(),
                path.display()
            )));
        }
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
