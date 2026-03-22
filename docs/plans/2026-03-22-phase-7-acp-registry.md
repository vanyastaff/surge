# ACP Registry — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build an ACP agent registry — a browsable catalog of known ACP-compatible agents with metadata, capabilities, install instructions, and one-click configuration.

**Architecture:** New `registry.rs` module in surge-acp with a built-in catalog of known agents (hardcoded JSON). `RegistryEntry` contains agent metadata (name, command, capabilities, install instructions). CLI commands provide `surge registry list/search/info/add`. `add` command writes the agent config directly into `surge.toml`. Future: remote registry fetching.

**Tech Stack:** Rust 2024, serde_json (catalog), surge-core config types, clap CLI

---

### Task 1: RegistryEntry type and built-in catalog

**Files:**
- Create: `crates/surge-acp/src/registry.rs`
- Modify: `crates/surge-acp/src/lib.rs`

**Step 1: Write registry.rs**

```rust
//! ACP Agent Registry — catalog of known ACP-compatible agents.

use serde::{Deserialize, Serialize};
use surge_core::config::{AgentConfig, Transport};

/// Capability that an agent supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentCapability {
    /// Can write/modify code.
    Code,
    /// Can create plans and break down tasks.
    Plan,
    /// Can review code and find issues.
    Review,
    /// Can run tests and validate.
    Test,
    /// Can refactor existing code.
    Refactor,
    /// Can explain code and answer questions.
    Chat,
}

impl std::fmt::Display for AgentCapability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Code => write!(f, "code"),
            Self::Plan => write!(f, "plan"),
            Self::Review => write!(f, "review"),
            Self::Test => write!(f, "test"),
            Self::Refactor => write!(f, "refactor"),
            Self::Chat => write!(f, "chat"),
        }
    }
}

/// A registry entry describing an ACP-compatible agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryEntry {
    /// Unique identifier (used as key in surge.toml).
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Short description.
    pub description: String,
    /// Command to launch the agent.
    pub command: String,
    /// Default arguments.
    #[serde(default)]
    pub default_args: Vec<String>,
    /// Transport type.
    #[serde(default)]
    pub transport: Transport,
    /// Agent capabilities.
    pub capabilities: Vec<AgentCapability>,
    /// Installation instructions.
    pub install_instructions: String,
    /// Website or documentation URL.
    #[serde(default)]
    pub website: Option<String>,
    /// Tags for search/filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl RegistryEntry {
    /// Convert to AgentConfig for surge.toml.
    pub fn to_agent_config(&self) -> AgentConfig {
        AgentConfig {
            command: self.command.clone(),
            args: self.default_args.clone(),
            transport: self.transport.clone(),
        }
    }

    /// Check if this entry matches a search query.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        self.id.to_lowercase().contains(&q)
            || self.display_name.to_lowercase().contains(&q)
            || self.description.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
    }
}

/// The built-in registry of known ACP agents.
pub struct Registry {
    entries: Vec<RegistryEntry>,
}

impl Registry {
    /// Load the built-in registry.
    pub fn builtin() -> Self {
        Self {
            entries: builtin_catalog(),
        }
    }

    /// List all entries.
    pub fn list(&self) -> &[RegistryEntry] {
        &self.entries
    }

    /// Search entries by query.
    pub fn search(&self, query: &str) -> Vec<&RegistryEntry> {
        self.entries.iter().filter(|e| e.matches(query)).collect()
    }

    /// Find entry by ID.
    pub fn find(&self, id: &str) -> Option<&RegistryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// List entries filtered by capability.
    pub fn by_capability(&self, cap: &AgentCapability) -> Vec<&RegistryEntry> {
        self.entries
            .iter()
            .filter(|e| e.capabilities.contains(cap))
            .collect()
    }
}

/// Built-in catalog of known ACP-compatible agents.
fn builtin_catalog() -> Vec<RegistryEntry> {
    vec![
        RegistryEntry {
            id: "claude-code".to_string(),
            display_name: "Claude Code".to_string(),
            description: "Anthropic's Claude coding agent — autonomous coding with terminal, file editing, and search".to_string(),
            command: "claude".to_string(),
            default_args: vec!["--print".to_string(), "--output-format".to_string(), "stream-json".to_string()],
            transport: Transport::Stdio,
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Plan,
                AgentCapability::Review,
                AgentCapability::Test,
                AgentCapability::Refactor,
                AgentCapability::Chat,
            ],
            install_instructions: "npm install -g @anthropic-ai/claude-code".to_string(),
            website: Some("https://claude.ai/claude-code".to_string()),
            tags: vec!["anthropic".to_string(), "claude".to_string(), "ai".to_string(), "full-featured".to_string()],
        },
        RegistryEntry {
            id: "copilot-cli".to_string(),
            display_name: "GitHub Copilot CLI".to_string(),
            description: "GitHub's Copilot coding assistant — code suggestions and completions".to_string(),
            command: "gh".to_string(),
            default_args: vec!["copilot".to_string()],
            transport: Transport::Stdio,
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Chat,
            ],
            install_instructions: "gh extension install github/gh-copilot".to_string(),
            website: Some("https://github.com/features/copilot".to_string()),
            tags: vec!["github".to_string(), "copilot".to_string(), "ai".to_string()],
        },
        RegistryEntry {
            id: "zed-agent".to_string(),
            display_name: "Zed Agent".to_string(),
            description: "Zed editor's built-in AI agent — fast, integrated coding assistant".to_string(),
            command: "zed".to_string(),
            default_args: vec!["--agent".to_string()],
            transport: Transport::Stdio,
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Refactor,
                AgentCapability::Chat,
            ],
            install_instructions: "Install Zed editor from https://zed.dev".to_string(),
            website: Some("https://zed.dev".to_string()),
            tags: vec!["zed".to_string(), "editor".to_string(), "ai".to_string()],
        },
        RegistryEntry {
            id: "aider".to_string(),
            display_name: "Aider".to_string(),
            description: "AI pair programming in your terminal — works with many LLM providers".to_string(),
            command: "aider".to_string(),
            default_args: vec!["--no-auto-commits".to_string()],
            transport: Transport::Stdio,
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Refactor,
                AgentCapability::Chat,
            ],
            install_instructions: "pip install aider-chat".to_string(),
            website: Some("https://aider.chat".to_string()),
            tags: vec!["aider".to_string(), "open-source".to_string(), "multi-llm".to_string()],
        },
        RegistryEntry {
            id: "codex-cli".to_string(),
            display_name: "OpenAI Codex CLI".to_string(),
            description: "OpenAI's coding agent — terminal-based code generation".to_string(),
            command: "codex".to_string(),
            default_args: vec![],
            transport: Transport::Stdio,
            capabilities: vec![
                AgentCapability::Code,
                AgentCapability::Chat,
            ],
            install_instructions: "npm install -g @openai/codex".to_string(),
            website: Some("https://openai.com".to_string()),
            tags: vec!["openai".to_string(), "codex".to_string(), "ai".to_string()],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_catalog_not_empty() {
        let reg = Registry::builtin();
        assert!(!reg.list().is_empty());
        assert!(reg.list().len() >= 5);
    }

    #[test]
    fn test_find_by_id() {
        let reg = Registry::builtin();
        let claude = reg.find("claude-code");
        assert!(claude.is_some());
        assert_eq!(claude.unwrap().command, "claude");
    }

    #[test]
    fn test_search() {
        let reg = Registry::builtin();
        let results = reg.search("claude");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "claude-code");
    }

    #[test]
    fn test_search_by_tag() {
        let reg = Registry::builtin();
        let results = reg.search("open-source");
        assert!(results.iter().any(|e| e.id == "aider"));
    }

    #[test]
    fn test_by_capability() {
        let reg = Registry::builtin();
        let reviewers = reg.by_capability(&AgentCapability::Review);
        assert!(reviewers.iter().any(|e| e.id == "claude-code"));
        // copilot doesn't have review capability
        assert!(!reviewers.iter().any(|e| e.id == "copilot-cli"));
    }

    #[test]
    fn test_to_agent_config() {
        let reg = Registry::builtin();
        let entry = reg.find("claude-code").unwrap();
        let config = entry.to_agent_config();
        assert_eq!(config.command, "claude");
        assert!(!config.args.is_empty());
    }

    #[test]
    fn test_search_case_insensitive() {
        let reg = Registry::builtin();
        let results = reg.search("CLAUDE");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_capability_display() {
        assert_eq!(AgentCapability::Code.to_string(), "code");
        assert_eq!(AgentCapability::Review.to_string(), "review");
    }
}
```

**Step 2: Update lib.rs**

Add `pub mod registry;` and `pub use registry::{Registry, RegistryEntry, AgentCapability};`

**Step 3: Run tests, commit**

```bash
cargo test -p surge-acp -- registry
git add crates/surge-acp/src/registry.rs crates/surge-acp/src/lib.rs
git commit -m "feat(acp): add ACP Registry — built-in catalog of known agents"
```

---

### Task 2: CLI registry commands

**Files:**
- Modify: `crates/surge-cli/src/main.rs`

**Step 1: Add Registry subcommand**

Add to `Commands` enum:
```rust
    /// Browse the ACP agent registry
    Registry {
        #[command(subcommand)]
        command: RegistryCommands,
    },
```

Add enum:
```rust
#[derive(Subcommand)]
enum RegistryCommands {
    /// List all known agents
    List,
    /// Search agents by name, description, or tag
    Search {
        /// Search query
        query: String,
    },
    /// Show detailed info about an agent
    Info {
        /// Agent ID from registry
        id: String,
    },
    /// Add an agent from registry to surge.toml
    Add {
        /// Agent ID from registry
        id: String,
        /// Custom name (default: use registry ID)
        #[arg(short, long)]
        name: Option<String>,
    },
}
```

**Step 2: Add handlers**

```rust
Commands::Registry { command } => match command {
    RegistryCommands::List => {
        let registry = surge_acp::Registry::builtin();
        println!("⚡ ACP Agent Registry\n");
        for entry in registry.list() {
            let caps: Vec<String> = entry.capabilities.iter().map(|c| c.to_string()).collect();
            println!("  {} — {}", entry.id, entry.display_name);
            println!("    {}", entry.description);
            println!("    Capabilities: {}", caps.join(", "));
            println!();
        }
        println!("Use 'surge registry info <id>' for details.");
        println!("Use 'surge registry add <id>' to add to surge.toml.");
    }
    RegistryCommands::Search { query } => {
        let registry = surge_acp::Registry::builtin();
        let results = registry.search(&query);
        if results.is_empty() {
            println!("No agents found for '{query}'.");
        } else {
            println!("⚡ Search results for '{query}':\n");
            for entry in &results {
                println!("  {} — {}", entry.id, entry.display_name);
                println!("    {}", entry.description);
                println!();
            }
        }
    }
    RegistryCommands::Info { id } => {
        let registry = surge_acp::Registry::builtin();
        match registry.find(&id) {
            Some(entry) => {
                println!("⚡ {}\n", entry.display_name);
                println!("ID: {}", entry.id);
                println!("Description: {}", entry.description);
                println!("Command: {} {}", entry.command, entry.default_args.join(" "));
                println!("Transport: {:?}", entry.transport);
                let caps: Vec<String> = entry.capabilities.iter().map(|c| c.to_string()).collect();
                println!("Capabilities: {}", caps.join(", "));
                if let Some(url) = &entry.website {
                    println!("Website: {url}");
                }
                println!("\nInstall:");
                println!("  {}", entry.install_instructions);
                println!("\nAdd to project:");
                println!("  surge registry add {}", entry.id);
            }
            None => {
                println!("Agent '{id}' not found in registry.");
                println!("Use 'surge registry list' to see available agents.");
            }
        }
    }
    RegistryCommands::Add { id, name } => {
        let registry = surge_acp::Registry::builtin();
        match registry.find(&id) {
            Some(entry) => {
                let agent_name = name.as_deref().unwrap_or(&entry.id);

                // Load current config
                let config_path = std::env::current_dir()?.join("surge.toml");
                if !config_path.exists() {
                    anyhow::bail!("surge.toml not found. Run 'surge init' first.");
                }

                let content = std::fs::read_to_string(&config_path)?;
                let mut config: toml::Table = toml::from_str(&content)?;

                // Add agent to config
                let agents = config
                    .entry("agents")
                    .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                    .as_table_mut()
                    .ok_or_else(|| anyhow::anyhow!("agents is not a table"))?;

                if agents.contains_key(agent_name) {
                    anyhow::bail!("Agent '{agent_name}' already exists in surge.toml");
                }

                let agent_config = entry.to_agent_config();
                let agent_toml = toml::to_string(&agent_config)?;
                let agent_value: toml::Table = toml::from_str(&agent_toml)?;
                agents.insert(agent_name.to_string(), toml::Value::Table(agent_value));

                std::fs::write(&config_path, toml::to_string_pretty(&config)?)?;

                println!("✅ Added agent '{}' to surge.toml", agent_name);
                println!("   Command: {} {}", entry.command, entry.default_args.join(" "));
                println!("\n   Install the agent:");
                println!("   {}", entry.install_instructions);
            }
            None => {
                println!("Agent '{id}' not found in registry.");
                println!("Use 'surge registry list' to see available agents.");
            }
        }
    }
},
```

**Step 3: Verify compilation, commit**

```bash
cargo check -p surge-cli
git add crates/surge-cli/src/main.rs
git commit -m "feat(cli): add registry commands — list, search, info, add"
```

---

### Task 3: Final verification

**Step 1:** `cargo test --workspace`
**Step 2:** `cargo clippy --workspace`
**Step 3:** Commit fixes

---

## Dependency Graph

```
Task 1 (registry.rs) → Task 2 (CLI commands) → Task 3 (verify)
```

Linear chain.
