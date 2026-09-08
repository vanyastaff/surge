//! `surge skill list|show|verify` — operator-facing inspection of the skill
//! packs `surge_core::skill::SkillCatalog` can discover (R16).
//!
//! This module owns rendering only: locating roots on disk, picking which
//! provider a bare `--name` belongs to when it is ambiguous, and formatting
//! text/JSON. Identity, discovery, and content hashing all stay in
//! `surge_core::skill` — this command never re-derives them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use surge_core::ContentHash;
use surge_core::skill::{ResolvedSkill, SkillCatalog, SkillProvider, SkillRef};

/// `surge skill` subcommand surface.
#[derive(Subcommand, Debug)]
pub enum SkillCommands {
    /// List every skill pack discovered under the project and user roots:
    /// name, provider, version, and a short content hash.
    List {
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: SkillOutputFormat,
    },
    /// Show a resolved skill's instructions and file list.
    Show {
        /// The skill's declared name (`SKILL.md` frontmatter's `name` key).
        name: String,
        /// Narrow the search to one provider (needed only when the same
        /// name exists under more than one).
        #[arg(long, value_enum)]
        provider: Option<SkillProviderArg>,
        /// Narrow the search to one declared version.
        #[arg(long)]
        version: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: SkillOutputFormat,
    },
    /// Verify a skill's current on-disk content against a previously pinned
    /// hash. Exits non-zero when the content has drifted since the pin was
    /// taken.
    Verify {
        /// The skill's declared name.
        name: String,
        /// The pinned content hash to check against (`sha256:<hex>` or bare
        /// 64-char hex).
        #[arg(long)]
        hash: String,
        /// Narrow the search to one provider (needed only when the same
        /// name exists under more than one).
        #[arg(long)]
        provider: Option<SkillProviderArg>,
        /// Narrow the search to one declared version.
        #[arg(long)]
        version: Option<String>,
        /// Output format.
        #[arg(long, value_enum, default_value = "text")]
        format: SkillOutputFormat,
    },
}

/// Output format shared by every `surge skill` subcommand — the same
/// `--format text|json` convention as `surge doctor report`/`surge memory
/// audit`.
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum SkillOutputFormat {
    /// Human-readable text output.
    #[default]
    Text,
    /// Machine-readable JSON.
    Json,
}

/// CLI-facing provider selector. `surge_core::skill::SkillProvider` has no
/// `ValueEnum` impl of its own — `surge-core` does not depend on `clap` — so
/// this crate owns the mapping.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SkillProviderArg {
    /// A project-local skill root (`.claude/skills/` in the repo).
    Project,
    /// A user-level skill root (`~/.claude/skills/`, `~/.claude/plugins/`).
    User,
    /// A configured registry root.
    Registry,
}

impl From<SkillProviderArg> for SkillProvider {
    fn from(value: SkillProviderArg) -> Self {
        match value {
            SkillProviderArg::Project => Self::ProjectDir,
            SkillProviderArg::User => Self::UserDir,
            SkillProviderArg::Registry => Self::Registry,
        }
    }
}

/// Dispatch entry point — wired from `main.rs`'s `Commands::Skill` arm.
///
/// # Errors
/// Returns an error when the catalog cannot be built (home directory
/// unresolvable), a name/provider/version cannot be resolved to a unique
/// pack, or the matched pack fails to parse.
pub fn run(command: SkillCommands) -> Result<()> {
    match command {
        SkillCommands::List { format } => list_skills(format),
        SkillCommands::Show {
            name,
            provider,
            version,
            format,
        } => show_skill(&name, provider.map(Into::into), version.as_deref(), format),
        SkillCommands::Verify {
            name,
            hash,
            provider,
            version,
            format,
        } => verify_skill(
            &name,
            &hash,
            provider.map(Into::into),
            version.as_deref(),
            format,
        ),
    }
}

/// Where Surge looks for skill packs by default: the project's own
/// `.claude/skills/` (the Agent Skills convention `surge_core::skill`'s own
/// module doc uses as its example root) plus the user-level
/// `~/.claude/skills/` and `~/.claude/plugins/` directories Claude Code,
/// Cursor, and Codex already populate
/// (`.autopilot/competitive-waves/interfaces.md`'s measured corpus: 352
/// `SKILL.md`, 47 plugin manifests on the reference machine). No `Registry`
/// root is wired: `surge.toml` carries no registry-root key today, and this
/// task does not invent one.
fn default_skill_roots(project_root: &Path, home_dir: &Path) -> Vec<surge_core::skill::SkillRoot> {
    vec![
        surge_core::skill::SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: project_root.join(".claude").join("skills"),
        },
        surge_core::skill::SkillRoot {
            provider: SkillProvider::UserDir,
            path: home_dir.join(".claude").join("skills"),
        },
        surge_core::skill::SkillRoot {
            provider: SkillProvider::UserDir,
            path: home_dir.join(".claude").join("plugins"),
        },
    ]
}

/// The project root: the enclosing git repo's top level, or the current
/// directory when none is found — same fallback `surge project describe`
/// uses (`commands/project.rs`), so `surge skill` finds the same
/// `.claude/skills/` a git-less checkout would still expect.
fn project_root(cwd: &Path) -> PathBuf {
    surge_git::GitManager::discover()
        .map(|manager| manager.repo_path().to_path_buf())
        .unwrap_or_else(|_| cwd.to_path_buf())
}

fn discover_catalog() -> Result<SkillCatalog> {
    let cwd = std::env::current_dir().context("resolve current directory")?;
    let root = project_root(&cwd);
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    let roots = default_skill_roots(&root, &home);
    Ok(SkillCatalog::discover(&roots))
}

fn provider_label(provider: SkillProvider) -> &'static str {
    match provider {
        SkillProvider::ProjectDir => "project",
        SkillProvider::UserDir => "user",
        SkillProvider::Registry => "registry",
    }
}

/// Stable display order for providers in error messages and tables — the
/// three variants have no `Ord` impl of their own (`surge_core::skill`
/// exposes only `PartialEq`/`Hash`).
fn provider_rank(provider: SkillProvider) -> u8 {
    match provider {
        SkillProvider::ProjectDir => 0,
        SkillProvider::UserDir => 1,
        SkillProvider::Registry => 2,
    }
}

/// First 12 hex characters of a [`ContentHash`] — enough to disambiguate in
/// a `list` table without a 64-char wall of hex. `show`/`verify` print the
/// full hash, where a pin needs the exact value.
fn short_hash(hash: ContentHash) -> String {
    format!("{}…", &hash.to_hex()[..12])
}

/// Resolve which provider `name` (and `version`, when given) belongs to,
/// choosing automatically when only one provider in the catalog has a
/// matching pack. `provider`, when given, is used directly without
/// searching — except `SkillProvider::Registry`, which this command rejects
/// outright (see below).
///
/// This is a CLI-level disambiguation (which *provider* is `name` under?),
/// distinct from `SkillCatalog::resolve`'s own `SkillError::Ambiguous`
/// (differing content under the *same* provider), which is left to bubble
/// unchanged from the caller's later `catalog.resolve` call.
///
/// # Errors
/// - `--provider registry` is given: no registry root is ever configured
///   today (`default_skill_roots` wires only `ProjectDir`/`UserDir` —
///   Решение §22 defers registry loading, and `surge.toml` carries no
///   registry-root key to configure one). Resolving against it would always
///   land on `SkillError::NotFound`, which reads as "this skill isn't in the
///   registry" rather than "no registry is configured at all" — so this
///   rejects up front with the actual reason instead of a misleading empty
///   result.
/// - No pack matches `name`/`version` under any provider, or more than one
///   provider has a matching pack and none was given.
fn locate_provider(
    catalog: &SkillCatalog,
    name: &str,
    provider: Option<SkillProvider>,
    version: Option<&str>,
) -> Result<SkillProvider> {
    if let Some(provider) = provider {
        if provider == SkillProvider::Registry {
            bail!(
                "no registry root is configured: `surge.toml` has no registry-root key yet. \
                 Use --provider project or --provider user instead."
            );
        }
        return Ok(provider);
    }

    let mut found: Vec<SkillProvider> = catalog
        .skills()
        .filter(|s| s.name == name && version.is_none_or(|v| s.version.as_deref() == Some(v)))
        .map(|s| s.provider)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    found.sort_by_key(|p| provider_rank(*p));

    match found.as_slice() {
        [] => {
            let version_suffix = version.map_or_else(String::new, |v| format!(" at version {v:?}"));
            bail!("no skill named {name:?} found{version_suffix}")
        },
        [only] => Ok(*only),
        many => {
            let providers = many
                .iter()
                .map(|p| provider_label(*p))
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "skill {name:?} exists under multiple providers ({providers}); \
                 pass --provider to disambiguate"
            )
        },
    }
}

/// Resolve `name` (and `version`/`provider`, when given) to its `SkillRef`
/// and freshly re-read content, sharing the provider-disambiguation and
/// error context between `show` and `verify`.
///
/// The `version` filter passed to `SkillCatalog::resolve` deliberately stays
/// exactly what the caller gave (`None` in the common case) so the library
/// keeps deciding ambiguity across differing versions on its own terms.
/// `SkillCatalog::resolve` itself now returns the resolved pack's own fresh
/// identity alongside its content, so this command hands that straight back
/// rather than reconstructing it by scanning `catalog.skills()` for an entry
/// whose hash happens to match — that reconstruction used to race a pack
/// changing on disk between `discover()` and `resolve()`: when it changed,
/// no catalog entry carried the freshly-resolved hash any longer, `find`
/// came back empty, and `show --format json` printed `"version": null` with
/// no explanation. The library's own fresh read can't lose that race.
fn resolve_named(
    catalog: &SkillCatalog,
    name: &str,
    provider: Option<SkillProvider>,
    version: Option<&str>,
) -> Result<(SkillRef, ResolvedSkill)> {
    let provider = locate_provider(catalog, name, provider, version)?;
    let lookup_ref = SkillRef {
        name: name.to_string(),
        provider,
        version: version.map(str::to_string),
        hash: None,
    };
    catalog
        .resolve(&lookup_ref)
        .with_context(|| format!("resolve skill {name:?} ({})", provider_label(provider)))
}

fn list_skills(format: SkillOutputFormat) -> Result<()> {
    let catalog = discover_catalog()?;
    let refs: Vec<&SkillRef> = catalog.skills().collect();

    match format {
        SkillOutputFormat::Json => println!("{}", serde_json::to_string_pretty(&refs)?),
        SkillOutputFormat::Text => print_list_table(&refs),
    }
    Ok(())
}

fn print_list_table(refs: &[&SkillRef]) {
    if refs.is_empty() {
        println!("No skill packs found under the project or user roots.");
        return;
    }
    println!("{:<28} {:<10} {:<10} HASH", "NAME", "PROVIDER", "VERSION");
    for r in refs {
        println!(
            "{:<28} {:<10} {:<10} {}",
            r.name,
            provider_label(r.provider),
            r.version.as_deref().unwrap_or("-"),
            r.hash.map_or_else(|| "-".to_string(), short_hash),
        );
    }
    println!("\n{} skill(s).", refs.len());
}

#[derive(Serialize)]
struct SkillShowJson<'a> {
    name: &'a str,
    provider: SkillProvider,
    version: Option<&'a str>,
    hash: String,
    instructions: &'a str,
    files: Vec<String>,
}

fn show_skill(
    name: &str,
    provider: Option<SkillProvider>,
    version: Option<&str>,
    format: SkillOutputFormat,
) -> Result<()> {
    let catalog = discover_catalog()?;
    let (skill_ref, resolved) = resolve_named(&catalog, name, provider, version)?;

    match format {
        SkillOutputFormat::Json => {
            let payload = SkillShowJson {
                name: &skill_ref.name,
                provider: skill_ref.provider,
                version: skill_ref.version.as_deref(),
                hash: resolved.hash.to_string(),
                instructions: &resolved.instructions,
                files: resolved
                    .files
                    .iter()
                    .map(|f| f.display().to_string())
                    .collect(),
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        },
        SkillOutputFormat::Text => {
            let version_suffix = skill_ref
                .version
                .as_deref()
                .map_or_else(String::new, |v| format!(", version {v}"));
            println!(
                "skill:    {} ({}{version_suffix})",
                skill_ref.name,
                provider_label(skill_ref.provider),
            );
            println!("hash:     {}", resolved.hash);
            println!("files ({}):", resolved.files.len());
            for f in &resolved.files {
                println!("  {}", f.display());
            }
            println!();
            println!("{}", resolved.instructions);
        },
    }
    Ok(())
}

#[derive(Serialize)]
struct SkillVerifyJson<'a> {
    name: &'a str,
    provider: SkillProvider,
    version: Option<&'a str>,
    expected: String,
    actual: String,
    matches: bool,
}

fn verify_skill(
    name: &str,
    pin: &str,
    provider: Option<SkillProvider>,
    version: Option<&str>,
    format: SkillOutputFormat,
) -> Result<()> {
    let expected: ContentHash = pin
        .parse()
        .with_context(|| format!("parse --hash {pin:?}"))?;
    let catalog = discover_catalog()?;
    let (skill_ref, resolved) = resolve_named(&catalog, name, provider, version)?;
    let content_matches = resolved.hash == expected;

    match format {
        SkillOutputFormat::Json => {
            let payload = SkillVerifyJson {
                name: &skill_ref.name,
                provider: skill_ref.provider,
                version: skill_ref.version.as_deref(),
                expected: expected.to_string(),
                actual: resolved.hash.to_string(),
                matches: content_matches,
            };
            println!("{}", serde_json::to_string_pretty(&payload)?);
        },
        SkillOutputFormat::Text => {
            println!(
                "skill:    {} ({})",
                skill_ref.name,
                provider_label(skill_ref.provider)
            );
            println!("expected: {expected}");
            println!("actual:   {}", resolved.hash);
            println!(
                "{}",
                if content_matches {
                    "MATCH"
                } else {
                    "DRIFT DETECTED"
                }
            );
        },
    }

    if !content_matches {
        std::process::exit(1);
    }
    Ok(())
}
