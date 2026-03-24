use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use surge_persistence::memory::{models::*, MemoryStore};

use super::load_spec_by_id;

/// Memory entry category
#[derive(Debug, Clone, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum MemoryCategory {
    /// Architectural decision or reasoning
    Discovery,
    /// Coding pattern or convention
    Pattern,
    /// Known pitfall or gotcha
    Gotcha,
    /// File-level context
    File,
}

#[derive(Subcommand)]
pub enum MemoryCommands {
    /// Add a new memory entry
    Add {
        /// Category of memory entry
        #[arg(long, value_enum)]
        category: MemoryCategory,

        /// Content to store
        #[arg(long)]
        content: String,

        /// Optional tags (comma-separated)
        #[arg(long)]
        tags: Option<String>,

        /// Optional spec ID to associate with
        #[arg(long)]
        spec: Option<String>,

        /// File path (required for file category)
        #[arg(long)]
        file_path: Option<String>,

        /// Title (for discovery/gotcha categories)
        #[arg(long)]
        title: Option<String>,

        /// Name (for pattern category)
        #[arg(long)]
        name: Option<String>,
    },

    /// Search memory entries
    Search {
        /// Search query
        query: String,

        /// Filter by spec ID
        #[arg(long)]
        spec: Option<String>,

        /// Filter by tags (comma-separated)
        #[arg(long)]
        tags: Option<String>,

        /// Maximum number of results
        #[arg(long, default_value = "10")]
        limit: usize,
    },
}

pub fn run(command: MemoryCommands) -> Result<()> {
    match command {
        MemoryCommands::Add {
            category,
            content,
            tags,
            spec,
            file_path,
            title,
            name,
        } => add_memory(category, content, tags, spec, file_path, title, name),
        MemoryCommands::Search {
            query,
            spec,
            tags,
            limit,
        } => search_memory(query, spec, tags, limit),
    }
}

fn add_memory(
    category: MemoryCategory,
    content: String,
    tags: Option<String>,
    spec: Option<String>,
    file_path: Option<String>,
    title: Option<String>,
    name: Option<String>,
) -> Result<()> {
    // Open the memory store
    let store_path = MemoryStore::default_path()?;
    let store = MemoryStore::open(&store_path)?;

    // Parse tags if provided
    let tags_vec: Vec<String> = tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    // Parse spec ID if provided
    let spec_id = if let Some(spec_str) = spec {
        let spec_file = load_spec_by_id(&spec_str)?;
        Some(spec_file.spec.id)
    } else {
        None
    };

    // Get current timestamp
    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;

    // Create and add the appropriate entry type
    match category {
        MemoryCategory::Discovery => {
            let title = title.unwrap_or_else(|| "Discovery".to_string());
            let mut discovery = Discovery::new(title, content.clone(), timestamp_ms);

            if let Some(sid) = spec_id {
                discovery = discovery.with_spec_id(sid);
            }
            if !tags_vec.is_empty() {
                discovery = discovery.with_tags(tags_vec.clone());
            }

            store.add_discovery(&discovery)?;

            println!("✅ Discovery added successfully");
            println!("   ID: {}", discovery.id);
            println!("   Title: {}", discovery.title);
            if !tags_vec.is_empty() {
                println!("   Tags: {}", tags_vec.join(", "));
            }
        }

        MemoryCategory::Pattern => {
            let name = name.unwrap_or_else(|| "Pattern".to_string());
            let mut pattern = Pattern::new(name, content.clone(), timestamp_ms);

            if let Some(sid) = spec_id {
                pattern = pattern.with_spec_id(sid);
            }
            if !tags_vec.is_empty() {
                pattern = pattern.with_tags(tags_vec.clone());
            }

            store.add_pattern(&pattern)?;

            println!("✅ Pattern added successfully");
            println!("   ID: {}", pattern.id);
            println!("   Name: {}", pattern.name);
            if !tags_vec.is_empty() {
                println!("   Tags: {}", tags_vec.join(", "));
            }
        }

        MemoryCategory::Gotcha => {
            let title = title.unwrap_or_else(|| "Gotcha".to_string());
            let mut gotcha = Gotcha::new(title, content.clone(), content.clone(), timestamp_ms);

            if let Some(sid) = spec_id {
                gotcha = gotcha.with_spec_id(sid);
            }
            if !tags_vec.is_empty() {
                gotcha = gotcha.with_tags(tags_vec.clone());
            }

            store.add_gotcha(&gotcha)?;

            println!("✅ Gotcha added successfully");
            println!("   ID: {}", gotcha.id);
            println!("   Title: {}", gotcha.title);
            if !tags_vec.is_empty() {
                println!("   Tags: {}", tags_vec.join(", "));
            }
        }

        MemoryCategory::File => {
            let file_path = file_path.ok_or_else(|| {
                anyhow::anyhow!("--file-path is required for file category entries")
            })?;

            let mut file_context = FileContext::new(file_path.clone(), content.clone(), timestamp_ms);

            if let Some(sid) = spec_id {
                file_context = file_context.with_spec_id(sid);
            }
            if !tags_vec.is_empty() {
                file_context = file_context.with_tags(tags_vec.clone());
            }

            store.add_file_context(&file_context)?;

            println!("✅ File context added successfully");
            println!("   ID: {}", file_context.id);
            println!("   File: {}", file_context.file_path);
            if !tags_vec.is_empty() {
                println!("   Tags: {}", tags_vec.join(", "));
            }
        }
    }

    Ok(())
}

fn search_memory(
    query: String,
    spec: Option<String>,
    tags: Option<String>,
    limit: usize,
) -> Result<()> {
    // TODO: Implementation will be added in subsequent subtasks
    println!("⚡ Memory Search");
    println!("Query: {}", query);
    if let Some(spec) = spec {
        println!("Spec filter: {}", spec);
    }
    if let Some(tags) = tags {
        println!("Tags filter: {}", tags);
    }
    println!("Limit: {}", limit);
    println!("\n⚠️  Memory system not yet implemented");
    Ok(())
}
