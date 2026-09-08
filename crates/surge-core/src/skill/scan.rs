//! Directory traversal, file collection, and content hashing that turn a
//! [`SkillRoot`] into catalog entries.
//!
//! Two on-disk shapes are recognized, at any depth under a root:
//! - an **Agent Skills** pack: a directory containing `SKILL.md`;
//! - an **Agent Plugins** package: a directory containing a manifest — at
//!   `.claude-plugin/plugin.json` (the position real installs use) or the
//!   flatter `plugin.json` (checked as a fallback) — and a `skills/`
//!   subdirectory, itself holding Agent Skills packs.
//!
//! A directory matching neither shape is walked into (its immediate
//! subdirectories are the next candidates) rather than skipped: real
//! installs nest a package several organizational levels below a configured
//! root (vendor/name/version, or marketplace/plugins/name — measured on this
//! machine's `~/.claude/plugins`), not as the root's immediate children.
//! Once a directory *is* recognized as one of the two shapes above, the walk
//! stops there — it does not also descend into the package's other
//! subdirectories (`node_modules`, `.git`, source trees, ...).
//!
//! Real roots can themselves be symlinks (`~/.claude/skills/*` on this
//! machine), and nothing on disk stops one from pointing at its own
//! ancestor, so the walk tracks canonicalized paths it has already visited
//! and refuses to revisit one — turning a symlink cycle into a single
//! redundant step instead of unbounded recursion.

use std::path::{Path, PathBuf};

use crate::content_hash::ContentHash;

use super::error::SkillError;
use super::frontmatter::split_frontmatter;
use super::plugin::parse_plugin_manifest;
use super::{ResolvedSkill, SkillProvider, SkillRef, SkillRoot};

/// A successfully parsed pack, as it stood at `discover()` time.
///
/// Deliberately does **not** cache a [`ResolvedSkill`]: `resolve()` re-reads
/// `dir` from disk on every call so its `hash`/`instructions` reflect the
/// pack's content *now*, not at discovery time — a skill edited or swapped
/// between `discover()` and `resolve()` must be visible, not served stale.
#[derive(Debug)]
pub(super) struct CatalogEntry {
    pub(super) skill_ref: SkillRef,
    /// The pack's own directory (containing its `SKILL.md`), reread on
    /// every `resolve()`.
    pub(super) dir: PathBuf,
    /// The containing plugin's manifest `version`, if any — the fallback
    /// `resolve()` re-applies when the pack's own frontmatter, on rereading,
    /// still declares none.
    pub(super) plugin_version: Option<String>,
}

/// The ways a recognized pack shape can fail to become a usable [`SkillRef`].
#[derive(Debug)]
enum BrokenKind {
    Frontmatter,
    Plugin,
    /// A file read failed after the pack shape was already recognized (e.g.
    /// `SKILL.md` or one of the pack's other files became unreadable between
    /// the two `read_dir` calls). Reconstructed as a fresh [`std::io::Error`]
    /// carrying the original message — the original [`std::io::Error`] isn't
    /// `Clone`, so its `kind()` is not preserved, only its text.
    Io,
}

/// A pack shape was recognized on disk but its manifest failed to parse.
///
/// Kept (rather than silently dropped) so that a later `resolve()` for this
/// exact name/provider returns the typed reason instead of `NotFound` —
/// R09.1's "a broken `SKILL.md` does not fail the run silently".
#[derive(Debug)]
pub(super) struct BrokenEntry {
    pub(super) name: String,
    pub(super) provider: SkillProvider,
    file: PathBuf,
    reason: String,
    kind: BrokenKind,
}

impl BrokenEntry {
    /// Reconstruct the typed parse error for this pack.
    pub(super) fn error(&self) -> SkillError {
        match self.kind {
            BrokenKind::Frontmatter => SkillError::MalformedFrontmatter {
                file: self.file.clone(),
                reason: self.reason.clone(),
            },
            BrokenKind::Plugin => SkillError::MalformedPlugin {
                file: self.file.clone(),
                reason: self.reason.clone(),
            },
            BrokenKind::Io => SkillError::Io {
                path: self.file.clone(),
                source: std::io::Error::other(self.reason.clone()),
            },
        }
    }
}

/// Walk every configured root and collect both the good and the broken packs
/// found under it.
pub(super) fn scan_roots(roots: &[SkillRoot]) -> (Vec<CatalogEntry>, Vec<BrokenEntry>) {
    let mut entries = Vec::new();
    let mut broken = Vec::new();
    // `(provider, canonical path)` pairs already visited by
    // `scan_candidate_dir`'s recursion, shared across every root scanned in
    // this call — the guard against a symlink cycle (see
    // `scan_candidate_dir`). Keyed on `provider` too, not just the path:
    // two configured roots can legitimately point at the same physical
    // directory under *different* providers (e.g. a project root symlinked
    // into a user root) — that must contribute a pack per provider, not
    // collapse into whichever root happened to scan it first.
    let mut visited: std::collections::HashSet<(SkillProvider, PathBuf)> =
        std::collections::HashSet::new();
    for root in roots {
        scan_root(root, &mut entries, &mut broken, &mut visited);
    }
    entries.sort_by(|a: &CatalogEntry, b: &CatalogEntry| a.skill_ref.name.cmp(&b.skill_ref.name));
    (entries, broken)
}

fn scan_root(
    root: &SkillRoot,
    entries: &mut Vec<CatalogEntry>,
    broken: &mut Vec<BrokenEntry>,
    visited: &mut std::collections::HashSet<(SkillProvider, PathBuf)>,
) {
    let Ok(read_dir) = std::fs::read_dir(&root.path) else {
        tracing::debug!(
            target: "skill::discover",
            path = %root.path.display(),
            "skill root missing or unreadable; skipping"
        );
        return;
    };

    // Seed the root itself as visited (under its own provider) before
    // recursing into its children: a symlink several levels below `root`
    // can point straight back at `root` (not merely at some already-visited
    // child), and that cycle is only caught if `root`'s own canonical path
    // is already in the set.
    if let Ok(canonical_root) = root.path.canonicalize() {
        visited.insert((root.provider, canonical_root));
    }

    for dir in sorted_subdirs(read_dir) {
        scan_candidate_dir(&dir, root.provider, entries, broken, visited);
    }
}

/// Recognize `dir` as an Agent Skills pack or an Agent Plugins package;
/// if it is neither, walk into its own subdirectories looking for a pack
/// nested arbitrarily deeper (see the module doc for why that depth is
/// real, not defensive over-engineering).
///
/// Guards against a symlink cycle before doing anything else: real skill
/// roots on this machine (`~/.claude/skills/*`) are themselves symlinks, and
/// `sorted_subdirs`'s `is_dir()` follows them, so nothing stops one from
/// pointing at an ancestor — recursing into it forever. Canonicalizing `dir`
/// and refusing to revisit an already-seen `(provider, canonical path)` pair
/// turns that cycle into a single redundant visit instead of unbounded
/// recursion, while still letting a *different* provider visit the same
/// physical directory independently; a directory that cannot be
/// canonicalized (broken symlink, permission denied) is simply not a usable
/// candidate.
fn scan_candidate_dir(
    dir: &Path,
    provider: SkillProvider,
    entries: &mut Vec<CatalogEntry>,
    broken: &mut Vec<BrokenEntry>,
    visited: &mut std::collections::HashSet<(SkillProvider, PathBuf)>,
) {
    let Ok(canonical) = dir.canonicalize() else {
        return;
    };
    if !visited.insert((provider, canonical)) {
        return;
    }

    if dir.join("SKILL.md").is_file() {
        scan_skill_dir(dir, provider, None, entries, broken);
        return;
    }
    if let Some(manifest_path) = plugin_manifest_path(dir) {
        scan_plugin_dir(dir, &manifest_path, provider, entries, broken);
        return;
    }

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for child in sorted_subdirs(read_dir) {
        if is_scan_noise(&child) {
            continue;
        }
        scan_candidate_dir(&child, provider, entries, broken, visited);
    }
}

/// The Agent Plugins manifest, at whichever of its two recognized positions
/// exists: `.claude-plugin/plugin.json` (measured as the position real
/// installs use — 47 packs on this machine) checked first, the flatter
/// `plugin.json` (0 real packs measured, but part of the documented format)
/// as a fallback.
fn plugin_manifest_path(dir: &Path) -> Option<PathBuf> {
    let nested = dir.join(".claude-plugin").join("plugin.json");
    if nested.is_file() {
        return Some(nested);
    }
    let flat = dir.join("plugin.json");
    flat.is_file().then_some(flat)
}

/// Directory names that are never themselves a pack and are never worth
/// descending into while looking for one — dependency trees and VCS
/// metadata that can otherwise make the walk both slow and noisy for no
/// discovery benefit. Every hidden (`.`-prefixed) directory is skipped too:
/// a recognized package's own dot-directories (`.claude-plugin`, alternate
/// per-agent projections such as `.codex-plugin`) are read by path directly
/// where relevant, never by walking into them as traversal candidates.
fn is_scan_noise(dir: &Path) -> bool {
    dir.file_name()
        .and_then(|n| n.to_str())
        .is_none_or(|name| name.starts_with('.') || matches!(name, "node_modules" | "target"))
}

fn scan_plugin_dir(
    dir: &Path,
    manifest_path: &Path,
    provider: SkillProvider,
    entries: &mut Vec<CatalogEntry>,
    broken: &mut Vec<BrokenEntry>,
) {
    let plugin_version = match parse_plugin_manifest(manifest_path) {
        Ok(version) => version,
        Err(reason) => {
            broken.push(BrokenEntry {
                name: dir_name(dir),
                provider,
                file: manifest_path.to_path_buf(),
                reason,
                kind: BrokenKind::Plugin,
            });
            return;
        },
    };

    let skills_dir = dir.join("skills");
    let Ok(read_dir) = std::fs::read_dir(&skills_dir) else {
        tracing::debug!(
            target: "skill::discover",
            path = %skills_dir.display(),
            "plugin has no readable `skills/` directory; skipping"
        );
        return;
    };

    for skill_dir in sorted_subdirs(read_dir) {
        if skill_dir.join("SKILL.md").is_file() {
            scan_skill_dir(
                &skill_dir,
                provider,
                plugin_version.clone(),
                entries,
                broken,
            );
        }
    }
}

fn scan_skill_dir(
    dir: &Path,
    provider: SkillProvider,
    plugin_version: Option<String>,
    entries: &mut Vec<CatalogEntry>,
    broken: &mut Vec<BrokenEntry>,
) {
    match load_skill_pack(dir, provider, plugin_version.clone()) {
        Ok((skill_ref, _resolved)) => entries.push(CatalogEntry {
            skill_ref,
            dir: dir.to_path_buf(),
            plugin_version,
        }),
        Err(SkillError::MalformedFrontmatter { file, reason }) => {
            broken.push(BrokenEntry {
                name: dir_name(dir),
                provider,
                file,
                reason,
                kind: BrokenKind::Frontmatter,
            });
        },
        Err(SkillError::Io { path, source }) => {
            // A pack shape already recognized (its `SKILL.md` exists) that
            // then failed to read: track it the same way a malformed
            // frontmatter is tracked, so `resolve()` for this exact name
            // later returns the typed reason instead of `NotFound` — losing
            // it here contradicts this module's own doc.
            broken.push(BrokenEntry {
                name: dir_name(dir),
                provider,
                file: path,
                reason: source.to_string(),
                kind: BrokenKind::Io,
            });
        },
        Err(other) => {
            // `load_skill_pack` only ever constructs `MalformedFrontmatter`
            // or `Io` above; `MalformedPlugin`/`NotFound`/`Ambiguous` cannot
            // occur here. Logged rather than tracked as broken (no
            // `BrokenKind` fits) in case that contract is ever broken later.
            tracing::warn!(
                target: "skill::discover",
                dir = %dir.display(),
                err = %other,
                "unexpected error from load_skill_pack; not tracked as broken"
            );
        },
    }
}

/// Parse `dir`'s `SKILL.md` and hash its file set, fresh from disk.
///
/// Called both at `discover()` time (to compute the identity/hash the
/// catalog lists) and again by `resolve()` (to serve current, not cached,
/// content) — the two calls may observe different content if the pack
/// changed in between, by design.
pub(super) fn load_skill_pack(
    dir: &Path,
    provider: SkillProvider,
    plugin_version: Option<String>,
) -> Result<(SkillRef, ResolvedSkill), SkillError> {
    let skill_md = dir.join("SKILL.md");
    let raw = std::fs::read_to_string(&skill_md).map_err(|e| SkillError::Io {
        path: skill_md.clone(),
        source: e,
    })?;
    let (frontmatter, body) =
        split_frontmatter(&raw).map_err(|reason| SkillError::MalformedFrontmatter {
            file: skill_md.clone(),
            reason,
        })?;

    let files = collect_files(dir)?;
    let hash = hash_files(dir, &files)?;

    let skill_ref = SkillRef {
        name: frontmatter.name,
        provider,
        version: frontmatter.version.or(plugin_version),
        hash: Some(hash),
    };
    let resolved = ResolvedSkill {
        instructions: body.trim().to_string(),
        files,
        hash,
    };
    Ok((skill_ref, resolved))
}

fn sorted_subdirs(read_dir: std::fs::ReadDir) -> Vec<PathBuf> {
    let mut children: Vec<PathBuf> = read_dir
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    children.sort();
    children
}

fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Recursively collect every regular file under `dir`, sorted for
/// deterministic hashing.
fn collect_files(dir: &Path) -> Result<Vec<PathBuf>, SkillError> {
    let mut files = Vec::new();
    collect_files_into(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_into(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), SkillError> {
    let read_dir = std::fs::read_dir(dir).map_err(|e| SkillError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|e| SkillError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        // `DirEntry::file_type()` (unlike `Path::is_dir()`/`is_file()`) does
        // not follow a symlink — a subdirectory symlinked back to one of its
        // own ancestors would otherwise recurse without end, the same class
        // of bug the discovery walk in `scan_candidate_dir` guards against.
        // A pack's own file tree has no legitimate reason to contain such a
        // symlink, so it is simply excluded from the hashed file set rather
        // than tracked through a visited-paths set here too.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_files_into(&path, out)?;
        } else if file_type.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// SHA-256 over the pack's file set: each sorted file, identified by its path
/// relative to `root`, contributes `<relative-path>\0<content>\0` to the
/// hashed byte stream. Reuses [`ContentHash`] rather than a second hash
/// mechanism.
fn hash_files(root: &Path, files: &[PathBuf]) -> Result<ContentHash, SkillError> {
    let mut buf = Vec::new();
    for path in files {
        let rel = path.strip_prefix(root).unwrap_or(path);
        buf.extend_from_slice(rel.to_string_lossy().as_bytes());
        buf.push(0);
        let content = std::fs::read(path).map_err(|e| SkillError::Io {
            path: path.clone(),
            source: e,
        })?;
        buf.extend_from_slice(&content);
        buf.push(0);
    }
    Ok(ContentHash::compute(&buf))
}
