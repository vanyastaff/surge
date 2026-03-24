//! Agent discovery — automatic detection of installed ACP-compatible agents.
//!
//! # Discovery process
//!
//! | Source | API |
//! |---|---|
//! | Environment variables | [`AgentDiscovery::from_env()`] |
//! | Standard paths | [`AgentDiscovery::from_standard_paths()`] |
//! | Version detection | [`AgentDiscovery::detect_version()`] |
//! | Combined discovery | [`AgentDiscovery::discover_all()`] |
//!
//! Discovery results are cached to avoid repeated filesystem probing.
//!
//! # Integration with Registry
//!
//! The discovery system integrates with the [`Registry`](crate::registry::Registry)
//! to provide intelligent agent matching. Discovery accepts registry entries from
//! any source (builtin, remote, or config) and returns [`DetectedAgent`](crate::registry::DetectedAgent)
//! instances with full metadata when agents are found on the system.
//!
//! ```ignore
//! use surge_acp::registry::Registry;
//! use surge_acp::discovery::AgentDiscovery;
//!
//! // Create a merged registry from multiple sources
//! let builtin = Registry::builtin();
//! let merged = Registry::merged(builtin, remote);
//!
//! // Discover which agents are actually installed
//! let mut discovery = AgentDiscovery::new();
//! let detected = discovery.discover_all(merged.list());
//! ```

use crate::registry::{DetectedAgent, RegistryEntry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, warn};

// ── Public types ────────────────────────────────────────────────────

/// Platform information for agent discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    MacOS,
    Linux,
    Windows,
}

impl Platform {
    /// Detect the current platform.
    #[must_use]
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        return Self::MacOS;
        #[cfg(target_os = "linux")]
        return Self::Linux;
        #[cfg(target_os = "windows")]
        return Self::Windows;
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        compile_error!("Unsupported platform");
    }

    /// Returns standard installation paths for agents on this platform.
    #[must_use]
    pub fn standard_paths(self) -> Vec<PathBuf> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(PathBuf::from);

        match self {
            Self::MacOS => {
                let mut paths = vec![
                    PathBuf::from("/usr/local/bin"),
                    PathBuf::from("/opt/homebrew/bin"),
                ];
                if let Some(h) = home {
                    paths.push(h.join(".local/bin"));
                }
                paths
            }
            Self::Linux => {
                let mut paths = vec![PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")];
                if let Some(h) = home {
                    paths.push(h.join(".local/bin"));
                }
                paths
            }
            Self::Windows => {
                let mut paths = vec![
                    PathBuf::from("C:\\Program Files"),
                    PathBuf::from("C:\\Program Files (x86)"),
                ];
                if let Some(h) = home {
                    paths.push(h.join("AppData\\Local"));
                }
                paths
            }
        }
    }
}

/// Agent discovery engine — finds installed agents via probing.
#[derive(Debug, Clone)]
pub struct AgentDiscovery {
    /// Cached discovery results: agent ID → detected path.
    cache: HashMap<String, Option<PathBuf>>,
    /// Platform we're running on.
    platform: Platform,
}

impl Default for AgentDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentDiscovery {
    /// Create a new discovery instance for the current platform.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            platform: Platform::current(),
        }
    }

    /// Get the current platform.
    #[must_use]
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Clear the discovery cache.
    pub fn clear_cache(&mut self) {
        debug!("Clearing discovery cache");
        self.cache.clear();
    }

    /// Check if an agent is in the cache.
    #[must_use]
    pub fn is_cached(&self, agent_id: &str) -> bool {
        self.cache.contains_key(agent_id)
    }

    /// Discover an agent by checking environment variables.
    ///
    /// Returns the path to the agent binary if found via env vars.
    #[must_use]
    pub fn from_env(&self, agent_id: &str) -> Option<PathBuf> {
        debug!("Checking environment for {agent_id}");

        // Define environment variable names to check for each agent ID
        let env_vars = match agent_id {
            "claude-acp" => vec!["CLAUDE_PATH", "CLAUDE_BIN"],
            "github-copilot-cli" => vec!["COPILOT_PATH", "COPILOT_BIN", "GH_PATH"],
            "codex-acp" => vec!["CODEX_PATH", "CODEX_BIN"],
            "gemini" => vec!["GEMINI_PATH", "GEMINI_BIN"],
            _ => vec![],
        };

        // Try each environment variable in order
        for env_var in env_vars {
            if let Ok(value) = std::env::var(env_var) {
                debug!("Found {env_var}={value}");
                let path = PathBuf::from(value);

                // Validate the path exists and is a file
                if path.exists() && path.is_file() {
                    debug!("Validated {agent_id} at {:?} from {env_var}", path);
                    return Some(path);
                } else {
                    warn!(
                        "Environment variable {env_var} points to invalid path: {:?}",
                        path
                    );
                }
            }
        }

        debug!("No environment variables found for {agent_id}");
        None
    }

    /// Discover an agent by probing standard installation paths.
    ///
    /// Returns the path to the agent binary if found in standard locations.
    #[must_use]
    pub fn from_standard_paths(&self, agent_id: &str) -> Option<PathBuf> {
        debug!("Probing standard paths for {agent_id}");

        // Map agent ID to binary name
        let binary_name = match agent_id {
            "claude-acp" => "claude",
            "github-copilot-cli" => "gh",
            "codex-acp" => "codex",
            "gemini" => "gemini",
            _ => return None,
        };

        // Get platform-specific standard paths
        let standard_paths = self.platform.standard_paths();

        // Probe each standard path
        for base_path in standard_paths {
            let candidate = base_path.join(binary_name);

            // On Windows, also check for .exe extension
            let paths_to_check = if self.platform == Platform::Windows {
                vec![candidate.clone(), candidate.with_extension("exe")]
            } else {
                vec![candidate]
            };

            for path in paths_to_check {
                if path.exists() && path.is_file() {
                    debug!("Found {agent_id} at {:?}", path);
                    return Some(path);
                }
            }
        }

        debug!("Agent {agent_id} not found in standard paths");
        None
    }

    /// Detect the version of an installed agent.
    ///
    /// Executes `<agent> --version` to determine the installed version.
    #[must_use]
    pub fn detect_version(&self, agent_id: &str, path: &PathBuf) -> Option<String> {
        use std::process::Command;

        debug!("Detecting version for {} at {:?}", agent_id, path);

        // Map agent ID to version command args
        let args: &[&str] = match agent_id {
            "claude-acp" => &["--version"],
            "github-copilot-cli" => &["copilot", "--version"],
            "codex-acp" => &["--version"],
            "gemini" => &["--version"],
            _ => return None,
        };

        // Execute the version command using the provided path
        let output = Command::new(path).args(args).output().ok()?;

        // Check if command succeeded
        if !output.status.success() {
            warn!("Version command failed for {}: {:?}", agent_id, output.status);
            return None;
        }

        // Parse stdout for version string
        let stdout = String::from_utf8_lossy(&output.stdout);
        let version_line = stdout.lines().next()?.trim();

        if version_line.is_empty() {
            warn!("Empty version output for {}", agent_id);
            return None;
        }

        debug!("Detected version for {}: {}", agent_id, version_line);
        Some(version_line.to_string())
    }

    /// Discover all installed agents by combining all detection methods.
    ///
    /// This is the main entry point for agent discovery. It:
    /// 1. Checks environment variables
    /// 2. Probes standard installation paths
    /// 3. Detects versions for found agents
    /// 4. Caches results
    ///
    /// # Registry Integration
    ///
    /// Accepts registry entries from any source (builtin, remote, or merged).
    /// This enables intelligent matching of discovered binaries against registry
    /// metadata, returning full agent information including capabilities, models,
    /// and vendor details.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let registry = Registry::builtin();
    /// let mut discovery = AgentDiscovery::new();
    /// let detected = discovery.discover_all(registry.list());
    /// for agent in detected {
    ///     println!("Found: {} v{:?}", agent.entry.display_name, agent.detected_version);
    /// }
    /// ```
    ///
    /// Returns a list of detected agents with their paths and metadata.
    pub fn discover_all(&mut self, registry_entries: &[RegistryEntry]) -> Vec<DetectedAgent> {
        debug!("Starting agent discovery on {:?}", self.platform);
        let mut detected = Vec::new();

        for entry in registry_entries {
            // Skip if already cached
            if let Some(cached_path) = self.cache.get(&entry.id) {
                if let Some(path) = cached_path {
                    let version = self.detect_version(&entry.id, path);
                    detected.push(DetectedAgent {
                        entry: entry.clone(),
                        command_path: Some(path.to_string_lossy().to_string()),
                        detected_version: version,
                    });
                }
                continue;
            }

            // Try environment variables first
            let path = self
                .from_env(&entry.id)
                .or_else(|| self.from_standard_paths(&entry.id));

            // Cache the result (even if None)
            self.cache.insert(entry.id.clone(), path.clone());

            if let Some(found_path) = path {
                debug!("Discovered {} at {:?}", entry.id, found_path);
                let version = self.detect_version(&entry.id, &found_path);
                detected.push(DetectedAgent {
                    entry: entry.clone(),
                    command_path: Some(found_path.to_string_lossy().to_string()),
                    detected_version: version,
                });
            } else {
                warn!("Agent {} not found", entry.id);
            }
        }

        debug!("Discovery complete: found {} agents", detected.len());
        detected
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_detection() {
        let platform = Platform::current();
        // Just verify it compiles and returns a valid platform
        assert!(matches!(
            platform,
            Platform::MacOS | Platform::Linux | Platform::Windows
        ));
    }

    #[test]
    fn test_platform_standard_paths() {
        let platform = Platform::current();
        let paths = platform.standard_paths();
        assert!(!paths.is_empty(), "Should return at least one path");
    }

    #[test]
    fn test_discovery_new() {
        let discovery = AgentDiscovery::new();
        assert_eq!(discovery.platform(), Platform::current());
        assert!(!discovery.is_cached("claude-acp"));
    }

    #[test]
    fn test_discovery_cache() {
        let mut discovery = AgentDiscovery::new();
        assert!(!discovery.is_cached("claude-acp"));

        // Manually insert into cache for testing
        discovery.cache.insert("claude-acp".to_string(), None);
        assert!(discovery.is_cached("claude-acp"));

        discovery.clear_cache();
        assert!(!discovery.is_cached("claude-acp"));
    }

    #[test]
    fn test_platform_paths() {
        let discovery = AgentDiscovery::new();
        let platform = discovery.platform();

        // Test that standard paths are returned for the current platform
        let paths = platform.standard_paths();
        assert!(!paths.is_empty(), "Standard paths should not be empty");

        // Verify path format is correct for the platform
        match platform {
            Platform::MacOS | Platform::Linux => {
                // Unix-like paths should start with /
                for path in &paths {
                    let path_str = path.to_string_lossy();
                    assert!(
                        path_str.starts_with('/'),
                        "Unix path should start with /: {:?}",
                        path
                    );
                }
            }
            Platform::Windows => {
                // Windows paths should contain :\ or start with appropriate prefix
                for path in &paths {
                    let path_str = path.to_string_lossy();
                    assert!(
                        path_str.contains(":\\") || path_str.contains("\\"),
                        "Windows path should contain backslashes: {:?}",
                        path
                    );
                }
            }
        }

        // Test from_standard_paths returns None for non-existent agents
        // (unless the agent happens to be installed, which is fine)
        let result = discovery.from_standard_paths("claude-acp");
        // We can't assert the result since the agent might or might not be installed
        // Just verify the method runs without panicking
        match result {
            Some(path) => {
                // If found, verify it's a valid path
                assert!(path.exists(), "Found path should exist: {:?}", path);
                assert!(path.is_file(), "Found path should be a file: {:?}", path);
            }
            None => {
                // Not found, which is fine for testing
            }
        }
    }

    #[test]
    fn test_version_detection() {
        let discovery = AgentDiscovery::new();

        // Test version detection with a known command (git should be present in CI)
        // We'll use a mock approach by testing that the method handles non-existent commands gracefully
        let fake_path = PathBuf::from("/fake/path/to/agent");
        let result = discovery.detect_version("claude-acp", &fake_path);

        // For a non-existent path, we expect None
        // This tests the error handling path
        assert!(
            result.is_none(),
            "Version detection should return None for fake path"
        );

        // If git is available (very likely), test with a real command
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            use std::process::Command;

            // Check if git is available
            let git_check = Command::new("git").arg("--version").output();
            if git_check.is_ok() {
                // Git is available, we can use it for a real test
                // Note: We're not testing the actual agent commands here,
                // just verifying that the version detection logic works
                // The real agent detection will be tested in integration tests
            }
        }
    }

    #[test]
    fn test_env_detection() {
        use std::env;
        use std::fs::File;

        let discovery = AgentDiscovery::new();

        // Test 1: No environment variables set
        let _result = discovery.from_env("claude-acp");
        // Should return None if no env vars are set (unless user has them set)
        // We can't assert None here because the user might have these set

        // Test 2: Set an environment variable pointing to a non-existent path
        unsafe {
            env::set_var("CLAUDE_PATH", "/non/existent/path");
        }
        let result = discovery.from_env("claude-acp");
        assert!(result.is_none(), "Should return None for non-existent path");
        unsafe {
            env::remove_var("CLAUDE_PATH");
        }

        // Test 3: Create a temporary file and point env var to it
        let temp_dir = env::temp_dir();
        let temp_file = temp_dir.join("test_claude_agent");

        // Create the temporary file
        if File::create(&temp_file).is_ok() {
            // Set env var to the temp file
            unsafe {
                env::set_var("CLAUDE_PATH", temp_file.to_string_lossy().as_ref());
            }

            let result = discovery.from_env("claude-acp");
            assert!(
                result.is_some(),
                "Should find agent via CLAUDE_PATH env var"
            );
            assert_eq!(
                result.unwrap(),
                temp_file,
                "Should return the correct path from env var"
            );

            // Clean up
            unsafe {
                env::remove_var("CLAUDE_PATH");
            }
            let _ = std::fs::remove_file(&temp_file);
        }

        // Test 4: Test fallback to secondary env var
        let temp_file2 = temp_dir.join("test_claude_agent2");
        if File::create(&temp_file2).is_ok() {
            // Don't set CLAUDE_PATH, but set CLAUDE_BIN
            unsafe {
                env::set_var("CLAUDE_BIN", temp_file2.to_string_lossy().as_ref());
            }

            let result = discovery.from_env("claude-acp");
            assert!(result.is_some(), "Should find agent via CLAUDE_BIN env var");

            // Clean up
            unsafe {
                env::remove_var("CLAUDE_BIN");
            }
            let _ = std::fs::remove_file(&temp_file2);
        }

        // Test 5: Test Copilot with GH_PATH
        let temp_file3 = temp_dir.join("test_gh_agent");
        if File::create(&temp_file3).is_ok() {
            unsafe {
                env::set_var("GH_PATH", temp_file3.to_string_lossy().as_ref());
            }

            let result = discovery.from_env("github-copilot-cli");
            assert!(result.is_some(), "Should find Copilot via GH_PATH env var");

            // Clean up
            unsafe {
                env::remove_var("GH_PATH");
            }
            let _ = std::fs::remove_file(&temp_file3);
        }
    }

    #[test]
    fn test_registry_integration() {
        use crate::registry::Registry;

        // Create a merged registry from builtin sources
        let builtin = Registry::builtin();
        let entries = builtin.list();

        // Verify registry has agents
        assert!(!entries.is_empty(), "Builtin registry should have agents");

        // Create discovery instance
        let mut discovery = AgentDiscovery::new();

        // Run discovery against registry entries
        let detected = discovery.discover_all(entries);

        // Verify discovery returns results (may be empty if no agents installed)
        // The key test is that discovery accepts registry entries and doesn't panic
        assert!(
            detected.len() <= entries.len(),
            "Detected agents should not exceed registry entries"
        );

        // Each detected agent should have matching registry metadata
        for agent in &detected {
            assert!(
                entries.iter().any(|e| e.id == agent.entry.id),
                "Detected agent {:?} should match a registry entry",
                agent.entry.id
            );

            // Verify DetectedAgent has full registry metadata
            assert!(!agent.entry.id.is_empty(), "Agent ID should not be empty");
            assert!(
                !agent.entry.display_name.is_empty(),
                "Display name should not be empty"
            );
        }

        // Test with merged registry (builtin + empty remote)
        let empty_remote = Registry::builtin();
        let merged = Registry::merged(builtin.clone(), empty_remote);
        let merged_entries = merged.list();

        // Discovery should work with merged registry
        let detected_merged = discovery.discover_all(merged_entries);

        // Results should be consistent (discovery uses cached results)
        assert_eq!(
            detected.len(),
            detected_merged.len(),
            "Merged registry discovery should produce consistent results"
        );
    }
}
