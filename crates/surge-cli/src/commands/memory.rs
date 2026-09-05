use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use surge_core::MemoryClaimId;
use surge_persistence::memory::{AuditReport, MemoryStore, StaleReason, models::*, run_audit};
use surge_persistence::runs::SystemClock;
use surge_persistence::runs::registry::open_registry_pool;

use super::common::surge_home_dir;
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

/// Output format for `surge memory audit`.
#[derive(Debug, Clone, ValueEnum, Default)]
pub(crate) enum AuditFormat {
    /// Human-readable text output
    #[default]
    Text,
    /// JSON output
    Json,
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

    /// Show what has gone stale and what hindered past runs. Proposes
    /// pruning candidates — deletes nothing; pruning is `surge memory
    /// forget`, invoked explicitly by the operator.
    Audit {
        /// Output format
        #[arg(long, value_enum, default_value = "text")]
        format: AuditFormat,
    },

    /// Delete the named memory claim(s) by id. This is the only way a
    /// claim is ever removed — `surge memory audit` only proposes;
    /// this command, invoked explicitly with explicit ids, is what
    /// actually deletes.
    Forget {
        /// One or more claim ids to delete (the `claim_id` field from
        /// `surge memory audit`'s output). Accepts either the prefixed
        /// form (`claim-01ARZ...`) or the bare ULID.
        #[arg(required = true, value_name = "CLAIM_ID")]
        ids: Vec<String>,
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
        MemoryCommands::Audit { format } => audit_memory(format),
        MemoryCommands::Forget { ids } => forget_claims(ids),
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
        },

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
        },

        MemoryCategory::Gotcha => {
            let title = title.unwrap_or_else(|| "Gotcha".to_string());
            let mut gotcha = Gotcha::new(title, content.clone(), String::new(), timestamp_ms);

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
        },

        MemoryCategory::File => {
            let file_path = file_path.ok_or_else(|| {
                anyhow::anyhow!("--file-path is required for file category entries")
            })?;

            let mut file_context =
                FileContext::new(file_path.clone(), content.clone(), timestamp_ms);

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
        },
    }

    Ok(())
}

fn search_memory(
    query: String,
    spec: Option<String>,
    tags: Option<String>,
    limit: usize,
) -> Result<()> {
    // Open the memory store
    let store_path = MemoryStore::default_path()?;

    if !store_path.exists() {
        println!("⚠️  No memory data available yet.");
        println!("   Add entries using 'surge memory add' to build your knowledge base.");
        return Ok(());
    }

    let store = MemoryStore::open(&store_path)?;

    // Parse spec ID if provided
    let spec_id = if let Some(spec_str) = spec {
        let spec_file = load_spec_by_id(&spec_str)?;
        Some(spec_file.spec.id)
    } else {
        None
    };

    // Parse tags if provided
    let tags_filter: Vec<String> = tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    // Execute FTS5 search
    // Note: If the query contains FTS5 special characters and causes an error,
    // try wrapping it in quotes for exact phrase search
    let mut results = match store.search_all(&query, Some(limit)) {
        Ok(results) => results,
        Err(e) => {
            // If FTS5 query fails, try again with quoted query for exact phrase match
            let quoted_query = format!("\"{}\"", query);
            match store.search_all(&quoted_query, Some(limit)) {
                Ok(results) => results,
                Err(_) => {
                    eprintln!("⚠️  Search error: {}", e);
                    eprintln!("   Try quoting your search query or using simpler terms.");
                    return Err(e.into());
                },
            }
        },
    };

    // Apply spec_id filter if provided
    if let Some(sid) = spec_id {
        results
            .discoveries
            .retain(|d| d.spec_id.as_ref() == Some(&sid));
        results
            .patterns
            .retain(|p| p.spec_id.as_ref() == Some(&sid));
        results.gotchas.retain(|g| g.spec_id.as_ref() == Some(&sid));
        results
            .file_contexts
            .retain(|f| f.spec_id.as_ref() == Some(&sid));
    }

    // Apply tags filter if provided
    if !tags_filter.is_empty() {
        results
            .discoveries
            .retain(|d| tags_filter.iter().any(|tag| d.tags.contains(tag)));
        results
            .patterns
            .retain(|p| tags_filter.iter().any(|tag| p.tags.contains(tag)));
        results
            .gotchas
            .retain(|g| tags_filter.iter().any(|tag| g.tags.contains(tag)));
        results
            .file_contexts
            .retain(|f| tags_filter.iter().any(|tag| f.tags.contains(tag)));
    }

    // Display results
    println!("⚡ Memory Search Results");
    println!("Query: \"{}\"", query);
    println!();

    if results.is_empty() {
        println!("⚠️  No results found");
        return Ok(());
    }

    println!("Found {} total results\n", results.total_count());

    // Display discoveries
    if !results.discoveries.is_empty() {
        println!("═══ Discoveries ({}) ═══", results.discoveries.len());
        for discovery in &results.discoveries {
            println!("  📋 {}", discovery.title);
            println!("     ID: {}", discovery.id);
            if let Some(category) = &discovery.category {
                println!("     Category: {}", category);
            }
            if !discovery.tags.is_empty() {
                println!("     Tags: {}", discovery.tags.join(", "));
            }
            // Show preview of content (first 100 chars)
            let content_preview = if discovery.content.len() > 100 {
                format!("{}...", &discovery.content[..100])
            } else {
                discovery.content.clone()
            };
            println!("     {}", content_preview);
            println!();
        }
    }

    // Display patterns
    if !results.patterns.is_empty() {
        println!("═══ Patterns ({}) ═══", results.patterns.len());
        for pattern in &results.patterns {
            println!("  🔧 {}", pattern.name);
            println!("     ID: {}", pattern.id);
            if let Some(language) = &pattern.language {
                println!("     Language: {}", language);
            }
            if let Some(category) = &pattern.category {
                println!("     Category: {}", category);
            }
            if !pattern.tags.is_empty() {
                println!("     Tags: {}", pattern.tags.join(", "));
            }
            // Show preview of description (first 100 chars)
            let desc_preview = if pattern.description.len() > 100 {
                format!("{}...", &pattern.description[..100])
            } else {
                pattern.description.clone()
            };
            println!("     {}", desc_preview);
            println!();
        }
    }

    // Display gotchas
    if !results.gotchas.is_empty() {
        println!("═══ Gotchas ({}) ═══", results.gotchas.len());
        for gotcha in &results.gotchas {
            println!("  ⚠️  {}", gotcha.title);
            println!("     ID: {}", gotcha.id);
            if let Some(severity) = &gotcha.severity {
                println!("     Severity: {}", severity);
            }
            if let Some(category) = &gotcha.category {
                println!("     Category: {}", category);
            }
            if !gotcha.tags.is_empty() {
                println!("     Tags: {}", gotcha.tags.join(", "));
            }
            // Show preview of description (first 100 chars)
            let desc_preview = if gotcha.description.len() > 100 {
                format!("{}...", &gotcha.description[..100])
            } else {
                gotcha.description.clone()
            };
            println!("     {}", desc_preview);
            println!();
        }
    }

    // Display file contexts
    if !results.file_contexts.is_empty() {
        println!("═══ File Contexts ({}) ═══", results.file_contexts.len());
        for context in &results.file_contexts {
            println!("  📄 {}", context.file_path);
            println!("     ID: {}", context.id);
            if let Some(language) = &context.language {
                println!("     Language: {}", language);
            }
            if !context.key_apis.is_empty() {
                println!("     Key APIs: {}", context.key_apis.join(", "));
            }
            if !context.tags.is_empty() {
                println!("     Tags: {}", context.tags.join(", "));
            }
            // Show summary
            println!("     {}", context.summary);
            println!();
        }
    }

    Ok(())
}

/// `surge memory audit`: flags what has drifted stale and what claims
/// trace back to a failed run. Read-only — it proposes candidates for
/// pruning but never deletes anything itself; deletion is a separate,
/// explicit command an operator issues.
fn audit_memory(format: AuditFormat) -> Result<()> {
    let store_path = MemoryStore::default_path()?;

    if !store_path.exists() {
        match format {
            AuditFormat::Json => {
                let empty = AuditReport {
                    stale: Vec::new(),
                    unverifiable: Vec::new(),
                    run_correlated: Vec::new(),
                    caveats: vec![
                        surge_persistence::memory::audit::LOOPING_CORRELATION_UNAVAILABLE
                            .to_string(),
                    ],
                };
                println!("{}", serde_json::to_string_pretty(&empty)?);
            },
            AuditFormat::Text => {
                println!("⚠️  No memory data available yet.");
                println!("   Add entries using 'surge memory add' to build your knowledge base.");
            },
        }
        return Ok(());
    }

    let store = MemoryStore::open(&store_path)?;
    let claims = store.list_claims()?;

    let home = surge_home_dir()?;
    let pool = open_registry_pool(&home, &SystemClock)
        .map_err(|e| anyhow::anyhow!("open run registry: {e}"))?;
    let report = run_audit(&claims, &pool)?;

    match format {
        AuditFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        AuditFormat::Text => print_audit_report(&report),
    }

    Ok(())
}

/// Human-readable rendering of an [`AuditReport`].
fn print_audit_report(report: &AuditReport) {
    println!("⚡ Memory Audit");
    println!();

    let nothing_found = report.stale.is_empty()
        && report.unverifiable.is_empty()
        && report.run_correlated.is_empty();

    if nothing_found {
        println!("✅ Nothing stale, nothing correlated with a failed run.");
    } else {
        if !report.stale.is_empty() {
            println!("═══ Stale ({}) ═══", report.stale.len());
            for finding in &report.stale {
                let reason = match finding.reason {
                    StaleReason::ContentChanged => "source content changed",
                    StaleReason::SourceMissing => "source no longer exists",
                };
                println!("  {}  ({reason})", finding.claim_id);
                println!("     source: {}", finding.source);
            }
            println!();
        }

        if !report.unverifiable.is_empty() {
            println!("═══ Could not verify ({}) ═══", report.unverifiable.len());
            for finding in &report.unverifiable {
                println!("  {}  — {}", finding.claim_id, finding.reason);
                println!("     source: {}", finding.source);
            }
            println!();
        }

        if !report.run_correlated.is_empty() {
            println!(
                "═══ Correlated with failed runs ({}) ═══",
                report.run_correlated.len()
            );
            for finding in &report.run_correlated {
                println!(
                    "  {}  ← {} ({})",
                    finding.claim_id, finding.run_id, finding.run_status
                );
            }
            println!();
        }

        let proposed = count_proposed_claims(report);
        if proposed > 0 {
            println!(
                "🔎 Proposing {proposed} entr{} for pruning. Nothing was deleted — run \
                 `surge memory forget <CLAIM_ID>...` to remove one explicitly.",
                if proposed == 1 { "y" } else { "ies" }
            );
        }
    }

    if !report.caveats.is_empty() {
        println!();
        println!("ℹ️  Notes");
        for note in &report.caveats {
            println!("   - {note}");
        }
    }
}

/// Count of distinct claims proposed for pruning across both finding
/// lists — a claim flagged both stale and run-correlated is proposed
/// once, not twice.
fn count_proposed_claims(report: &AuditReport) -> usize {
    let mut ids = std::collections::HashSet::new();
    ids.extend(report.stale.iter().map(|finding| finding.claim_id));
    ids.extend(report.run_correlated.iter().map(|finding| finding.claim_id));
    ids.len()
}

/// `surge memory forget <CLAIM_ID>...`: delete exactly the named claim(s).
/// The only place a claim is ever removed — `audit_memory` above only ever
/// reads. Every id is parsed up front so a typo anywhere in the list
/// cannot cause a partial deletion of the ids that did parse.
fn forget_claims(ids: Vec<String>) -> Result<()> {
    let store_path = MemoryStore::default_path()?;
    forget_claims_at(&store_path, &ids)
}

/// Testable core of [`forget_claims`], parameterized on the store path so
/// the all-ids-parsed-before-anything-is-opened guarantee can be exercised
/// against an isolated store rather than the real `~/.surge/memory.db`.
///
/// Every id in `ids` is parsed before [`MemoryStore::open`] is ever
/// called: a single malformed id anywhere in the list fails the whole
/// call with zero claims touched, not a partial run that deleted
/// everything before the bad one. This function contains no `DELETE`
/// beyond the one `MemoryStore::delete_claim` call per already-parsed,
/// already-named id — nothing here can remove a claim the caller did not
/// explicitly name.
fn forget_claims_at(store_path: &std::path::Path, ids: &[String]) -> Result<()> {
    if !store_path.exists() {
        anyhow::bail!(
            "no memory store found at {}; nothing to forget",
            store_path.display()
        );
    }

    let parsed_ids = ids
        .iter()
        .map(|raw| {
            raw.parse::<MemoryClaimId>()
                .map(|id| (raw.as_str(), id))
                .map_err(|e| anyhow::anyhow!("invalid claim id {raw:?}: {e}"))
        })
        .collect::<Result<Vec<_>>>()?;

    let store = MemoryStore::open(store_path)?;
    for (raw, id) in parsed_ids {
        if store.delete_claim(id)? {
            println!("🗑️  forgot {raw}");
        } else {
            println!("⚠️  {raw} was not found; nothing to forget");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::ContentHash;
    use surge_core::memory::MemoryClaim;

    /// The exact property a reviewer flagged as code-shaped but
    /// test-unenforced: a garbage id anywhere in the request must leave
    /// every existing claim exactly as it was — zero deletions, not a
    /// partial run that stopped partway through. If a future edit
    /// reordered `MemoryStore::open` ahead of id parsing, this test would
    /// catch it (the valid claim would vanish).
    #[test]
    fn forget_leaves_every_claim_in_place_when_one_id_in_the_list_is_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("memory.db");

        let store = MemoryStore::open(&store_path).unwrap();
        let claim = MemoryClaim::from_transcript(
            "keep me",
            "transcript:run-01ARZ3NDEKTSV4RRFFQ69G5FAV#turn-1",
            ContentHash::compute(b"turn 1"),
        );
        store.add_claim(&claim).unwrap();
        drop(store);

        let ids = vec![claim.id().to_string(), "not-a-valid-id".to_string()];
        let result = forget_claims_at(&store_path, &ids);
        assert!(
            result.is_err(),
            "a garbage id anywhere in the list must fail the whole call"
        );

        let reopened = MemoryStore::open(&store_path).unwrap();
        let remaining = reopened.list_claims().unwrap();
        assert_eq!(
            remaining.len(),
            1,
            "the valid claim must still be present: zero deletions, not a partial one"
        );
        assert_eq!(remaining[0].id(), claim.id());
    }
}
