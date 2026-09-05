//! Skill pack identity, discovery, and resolution.
//!
//! Surge adopts the [Agent Skills](https://agentclientprotocol.com) directory
//! layout (`SKILL.md` with YAML frontmatter) and the Agent Plugins package
//! shape (`plugin.json` + a `skills/` subdirectory of Agent Skills packs) —
//! the format is theirs, loading it is ours. This module owns skill
//! *identity* and *resolution*; it does not run, sandbox, or interpret a
//! skill's instructions, and it does not decide whether an unpinned or
//! changed skill needs operator approval (a caller's concern, layered on top
//! of [`SkillCatalog::resolve`]).
//!
//! ```no_run
//! use surge_core::skill::{SkillCatalog, SkillProvider, SkillRoot};
//! use std::path::PathBuf;
//!
//! let roots = [SkillRoot {
//!     provider: SkillProvider::ProjectDir,
//!     path: PathBuf::from(".claude/skills"),
//! }];
//! let catalog = SkillCatalog::discover(&roots);
//! for skill_ref in catalog.skills() {
//!     if let Ok(resolved) = catalog.resolve(skill_ref) {
//!         println!("{}: {}", skill_ref.name, resolved.instructions);
//!     }
//! }
//! ```

mod error;
mod frontmatter;
mod plugin;
mod scan;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::content_hash::ContentHash;

pub use error::SkillError;

/// Where a skill pack was found, i.e. which configured root it resolved
/// against — not the on-disk packaging shape (Agent Skills vs. Agent
/// Plugins), which [`SkillCatalog`] hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillProvider {
    /// A project-local skill root (e.g. `.claude/skills/` in the repo).
    ProjectDir,
    /// A user-level skill root (e.g. `~/.claude/skills/`).
    UserDir,
    /// A configured registry root, resolved on disk only — no network fetch
    /// (that is a separate, out-of-scope class of risk).
    Registry,
}

/// A root directory to scan for skill packs, tagged with the provider it
/// represents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    /// Which provider this root represents.
    pub provider: SkillProvider,
    /// The directory to scan; its immediate subdirectories are candidate
    /// packs.
    pub path: PathBuf,
}

/// Identity of a skill, as referenced from a flow node or a run event.
///
/// `hash` is the durable identity (Решение §4): a name or version can be
/// forged by re-publishing a pack under the same label, the content hash
/// cannot. [`SkillCatalog::resolve`] matches on `name` and `provider` (and
/// `version`, when given) to *locate* a pack, and additionally on `hash`
/// itself whenever the caller has pinned one — so a caller that already
/// knows which exact content it wants can resolve straight to it even when
/// several physical copies share the same name/provider/version (a source
/// checkout next to its installed cache, say). Comparing the returned
/// [`ResolvedSkill::hash`] against a previously pinned `hash` to detect
/// drift the caller didn't already know about is still the caller's job
/// (bind-time approval), not this type's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillRef {
    /// The skill's declared name (`SKILL.md` frontmatter's `name` key).
    pub name: String,
    /// Which root this skill was found under.
    pub provider: SkillProvider,
    /// Declared version, if any. Not required by the Agent Skills format
    /// (kept as an unconstrained string, not `semver::Version`, so a
    /// third-party pack with a non-semver version string still resolves).
    ///
    /// **Deliberate inheritance, not an accident of parse order:** for a
    /// skill nested in an Agent Plugins package (`plugin.json` + `skills/`),
    /// a `SKILL.md` that declares no `version` of its own inherits the
    /// containing plugin manifest's `version`. Only a skill with no
    /// `version` anywhere — neither its own frontmatter nor a containing
    /// plugin's manifest — resolves to `None`. A caller pinning against this
    /// field (drift/approval checks) is pinning against the plugin's release
    /// version in that case, not a per-skill one; the Agent Skills format has
    /// no other version signal to fall back to.
    pub version: Option<String>,
    /// SHA-256 over the pack's file set, when pinned to a specific one.
    ///
    /// Every [`SkillRef`] returned by [`SkillCatalog::discover`] carries
    /// `Some` — discovery always knows the pack's current content hash.
    /// `None` is a caller's own construction, meaning "resolve by
    /// name/provider/version only, I don't know or care which exact
    /// content" (e.g. a flow node's `skills = ["code-reviewer"]` declaration,
    /// which names no hash at all) — [`SkillCatalog::resolve`] treats it
    /// that way, never as a request for a pack that hashes to nothing.
    /// `Option` rather than a sentinel value on principle: "no value" is
    /// exactly what the type exists for, and a sentinel (the hash of an
    /// empty byte string, say) is indistinguishable from a pack that
    /// genuinely hashes to it.
    pub hash: Option<ContentHash>,
}

/// A resolved skill's content, ready to hand to an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    /// The instructions body — `SKILL.md` with its frontmatter stripped.
    pub instructions: String,
    /// Every file belonging to the pack (absolute paths, sorted).
    pub files: Vec<PathBuf>,
    /// SHA-256 over `files`' current on-disk content (see [`SkillRef::hash`]).
    pub hash: ContentHash,
}

/// Skill packs discovered under a set of [`SkillRoot`]s.
///
/// Hides how packs are found and parsed: frontmatter parsing, `plugin.json`
/// parsing, directory traversal, and hashing are all internal to
/// [`SkillCatalog::discover`].
#[derive(Debug)]
pub struct SkillCatalog {
    entries: Vec<scan::CatalogEntry>,
    broken: Vec<scan::BrokenEntry>,
}

impl SkillCatalog {
    /// Discover every Agent Skills pack and Agent Plugins package under
    /// `roots`.
    ///
    /// A root that does not exist or cannot be read contributes no packs
    /// (rather than failing the whole discovery); a pack whose manifest
    /// fails to parse is recorded so that resolving it by name later returns
    /// the typed reason instead of [`SkillError::NotFound`].
    #[must_use]
    pub fn discover(roots: &[SkillRoot]) -> Self {
        let (entries, broken) = scan::scan_roots(roots);
        Self { entries, broken }
    }

    /// Skills successfully discovered, in name-sorted order.
    pub fn skills(&self) -> impl Iterator<Item = &SkillRef> {
        self.entries.iter().map(|entry| &entry.skill_ref)
    }

    /// Resolve a skill reference into its instructions and file set.
    ///
    /// Matches by `name` and `provider` (and `version`, when
    /// `skill_ref.version` is `Some`) to *locate* candidate packs on disk,
    /// then re-reads and re-hashes the matching one from disk **at this
    /// call**, not from whatever `discover()` last saw — so the returned
    /// [`ResolvedSkill::hash`] is always current; comparing it against
    /// `skill_ref.hash` to detect drift the caller didn't already know about
    /// is still the caller's job (bind-time approval), not this method's.
    ///
    /// When `skill_ref.hash` is `Some`, it additionally narrows the match to
    /// that exact content — letting a caller resolve straight to a known
    /// pack even among several name/provider/version-identical candidates.
    /// `None` matches by name/provider/version alone.
    ///
    /// A pack whose `SKILL.md` failed to parse at `discover()` time has no
    /// usable frontmatter `name` — it is tracked internally under its
    /// **directory name** instead (the convention `name` is expected to
    /// match anyway). Resolving such a pack by that directory name returns
    /// the typed parse error below; resolving it by any other name returns
    /// [`SkillError::NotFound`], since no other name for it is known.
    ///
    /// # Errors
    /// - [`SkillError::Ambiguous`] when more than one matching pack has a
    ///   **different** content hash — Решение §4: identity is the hash, not
    ///   the name or version, so candidates that hash identically are the
    ///   same pack found at more than one path (a source checkout next to
    ///   its installed cache, say) and either is a correct answer, not a
    ///   collision to refuse. Only genuinely differing content is ambiguous.
    /// - [`SkillError::MalformedFrontmatter`] / [`SkillError::MalformedPlugin`]
    ///   when the matching pack's manifest failed to parse (at `discover()`
    ///   time, or freshly on this call), naming the file and reason.
    /// - [`SkillError::NotFound`] when no matching pack, broken or otherwise,
    ///   is known.
    #[must_use = "a resolution failure (Err) must be handled, not discarded"]
    pub fn resolve(&self, skill_ref: &SkillRef) -> Result<ResolvedSkill, SkillError> {
        let matching: Vec<&scan::CatalogEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                entry.skill_ref.name == skill_ref.name
                    && entry.skill_ref.provider == skill_ref.provider
                    && match &skill_ref.version {
                        Some(requested) => {
                            entry.skill_ref.version.as_deref() == Some(requested.as_str())
                        },
                        None => true,
                    }
                    && skill_ref
                        .hash
                        .is_none_or(|hash| entry.skill_ref.hash == Some(hash))
            })
            .collect();

        if let Some(&first) = matching.first() {
            let differs_in_content = matching
                .iter()
                .any(|entry| entry.skill_ref.hash != first.skill_ref.hash);
            if differs_in_content {
                return Err(SkillError::Ambiguous {
                    name: skill_ref.name.clone(),
                    provider: skill_ref.provider,
                });
            }
            let (_fresh_ref, resolved) = scan::load_skill_pack(
                &first.dir,
                first.skill_ref.provider,
                first.plugin_version.clone(),
            )?;
            return Ok(resolved);
        }

        if let Some(broken) = self
            .broken
            .iter()
            .find(|b| b.name == skill_ref.name && b.provider == skill_ref.provider)
        {
            return Err(broken.error());
        }

        Err(SkillError::NotFound {
            name: skill_ref.name.clone(),
            provider: skill_ref.provider,
        })
    }
}
