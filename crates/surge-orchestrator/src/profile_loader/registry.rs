//! `ProfileRegistry` — the orchestrator's resolver over disk + bundled.
//!
//! Resolution order: **versioned → latest → bundled**.
//!
//! - **Versioned hit:** disk file whose body's `[role] version` exactly
//!   matches the requested semver.
//! - **Latest hit:** disk profile by name with the highest semver, used
//!   when the reference omits a version.
//! - **Bundled fallback:** the matching profile from
//!   [`surge_core::profile::BundledRegistry`].
//!
//! Version match is **canonical against `Profile.role.version`** in the
//! TOML body, not the filename. The filename is just a hint to humans
//! (and a duplicate-detection key).

use std::sync::Arc;

use surge_core::error::SurgeError;
use surge_core::keys::ProfileKey;
use surge_core::profile::Profile;
use surge_core::profile::bundled::BundledRegistry;
use surge_core::profile::keyref::{ProfileKeyRef, parse_key_ref};
use surge_core::profile::registry::{Provenance, ResolvedProfile, collect_chain, merge_chain};

use super::DiskProfileSet;
use crate::prompt::PromptRenderer;

/// Registry combining disk + bundled profile stores.
///
/// Construct via [`ProfileRegistry::load`] (reads `${SURGE_HOME}/profiles`)
/// or [`ProfileRegistry::new`] (caller-supplied disk set, e.g. for tests).
#[derive(Debug, Clone)]
pub struct ProfileRegistry {
    disk: DiskProfileSet,
    bundled: Arc<Vec<Profile>>,
}

/// One entry in [`ProfileRegistry::list`] output: profile + provenance.
#[derive(Debug, Clone)]
pub struct ProfileListEntry {
    pub profile: Profile,
    pub provenance: Provenance,
}

impl ProfileRegistry {
    /// Construct a registry by scanning the configured profiles directory
    /// and pulling the bundled set.
    ///
    /// A missing `${SURGE_HOME}/profiles/` directory is **not** an error
    /// — bundled profiles still resolve. This matches the fresh-install
    /// experience.
    ///
    /// Every loaded profile's `prompt.system` is run through
    /// [`PromptRenderer::validate_template`] at this point. Per Task 18
    /// of the milestone plan we fail-fast on broken templates rather
    /// than letting the engine discover them at agent-launch time.
    ///
    /// # Errors
    /// Propagates [`SurgeError`] from the path resolver or directory walker.
    /// Per-file parse failures inside the directory are logged at WARN and
    /// skipped, not returned. Per-profile template-compile failures abort
    /// the load with [`SurgeError::Config`].
    pub fn load() -> Result<Self, SurgeError> {
        let dir = super::paths::profiles_dir()?;
        let disk = DiskProfileSet::scan(&dir)?;
        let bundled = Arc::new(BundledRegistry::all());
        validate_prompts(&disk, &bundled)?;
        tracing::info!(
            target: "profile::registry",
            disk_count = disk.entries().len(),
            bundled_count = bundled.len(),
            dir = %dir.display(),
            "ProfileRegistry loaded"
        );
        Ok(Self { disk, bundled })
    }

    /// Construct a registry from an explicit disk set. Useful for tests
    /// that want to supply a `tempdir`-scoped store without exporting
    /// `SURGE_HOME` into the process env.
    ///
    /// Skips the load-time prompt validation step — tests that need it
    /// can call [`Self::load`] with `SURGE_HOME` set, or call
    /// [`validate_prompts`] explicitly.
    #[must_use]
    pub fn new(disk: DiskProfileSet) -> Self {
        let bundled = Arc::new(BundledRegistry::all());
        Self { disk, bundled }
    }

    /// Resolve a profile reference into a fully merged [`ResolvedProfile`].
    ///
    /// Walks the `extends` chain via
    /// [`surge_core::profile::registry::collect_chain`] using this registry
    /// as the lookup, then folds via
    /// [`surge_core::profile::registry::merge_chain`].
    ///
    /// # Errors
    /// - [`SurgeError::ProfileNotFound`] if neither disk nor bundled stores
    ///   contain a matching name.
    /// - [`SurgeError::ProfileVersionMismatch`] if a specific version was
    ///   requested but no matching profile carries that exact semver.
    /// - Any error from the merge / chain walker (cycle, depth, etc.).
    pub fn resolve(&self, key_ref: &ProfileKeyRef) -> Result<ResolvedProfile, SurgeError> {
        // 1. Find the leaf profile + its provenance.
        let (leaf, leaf_provenance) = self.find_leaf(key_ref)?;

        // 2. Walk the extends chain using the same lookup as `find_leaf`,
        //    but for parent references (which themselves are ProfileKey
        //    forms like "implementer@1.0").
        let chain = collect_chain(leaf.clone(), |parent_key: &ProfileKey| {
            let parsed = parse_key_ref(parent_key.as_str())
                .map_err(|e| SurgeError::InvalidProfileKey(e.to_string()))?;
            Ok(self.find_leaf(&parsed).ok().map(|(p, _)| p))
        })?;

        let chain_keys: Vec<ProfileKey> = chain.iter().map(|p| p.role.id.clone()).collect();
        let merged = merge_chain(&chain)?;

        tracing::debug!(
            target: "profile::registry",
            requested = key_ref.name.as_str(),
            requested_version = ?key_ref.version,
            provenance = ?leaf_provenance,
            chain_len = chain_keys.len(),
            "profile resolved"
        );

        Ok(ResolvedProfile {
            profile: merged,
            provenance: leaf_provenance,
            chain: chain_keys,
        })
    }

    /// List every visible profile with its provenance.
    ///
    /// Order: disk versioned entries first (sorted by name + descending
    /// version), then bundled entries that don't shadow a disk match.
    /// Each profile appears once.
    #[must_use]
    pub fn list(&self) -> Vec<ProfileListEntry> {
        let mut out: Vec<ProfileListEntry> = Vec::new();
        let mut seen: std::collections::HashSet<(String, semver::Version)> =
            std::collections::HashSet::new();

        // Disk entries first; sort by (name, desc version).
        let mut disk_entries: Vec<&super::disk::DiskEntry> = self.disk.entries().iter().collect();
        disk_entries.sort_by(|a, b| {
            a.profile
                .role
                .id
                .as_str()
                .cmp(b.profile.role.id.as_str())
                .then(b.profile.role.version.cmp(&a.profile.role.version))
        });
        for e in disk_entries {
            let key = (
                e.profile.role.id.as_str().to_string(),
                e.profile.role.version.clone(),
            );
            seen.insert(key);
            // We provisionally tag every disk entry as `Latest`; the actual
            // provenance assigned by `resolve` depends on whether the user
            // asked for a specific version. `list` is for inventory only.
            out.push(ProfileListEntry {
                profile: e.profile.clone(),
                provenance: Provenance::Latest,
            });
        }

        // Bundled entries that don't shadow a disk match.
        for p in self.bundled.iter() {
            let key = (
                p.role.id.as_str().to_string(),
                p.role.version.clone(),
            );
            if seen.contains(&key) {
                continue;
            }
            out.push(ProfileListEntry {
                profile: p.clone(),
                provenance: Provenance::Bundled,
            });
        }

        out
    }

    /// 3-way leaf lookup: versioned disk → latest disk → bundled.
    fn find_leaf(&self, key_ref: &ProfileKeyRef) -> Result<(Profile, Provenance), SurgeError> {
        let name = key_ref.name.as_str();
        if let Some(ref requested_version) = key_ref.version {
            // Versioned ref: disk first, then bundled. Only an exact match
            // counts. If neither contains it, surface a *version mismatch*
            // (showing what we did find for that name) rather than a flat
            // "not found".
            if let Some(entry) = self.disk.by_name_version(name, requested_version) {
                return Ok((entry.profile.clone(), Provenance::Versioned));
            }
            if let Some(profile) = BundledRegistry::by_name_version(name, requested_version) {
                return Ok((profile, Provenance::Bundled));
            }
            // No match: collect what versions DO exist for this name to
            // make the error actionable.
            let mut available: Vec<String> = Vec::new();
            for e in self.disk.entries().iter().filter(|e| e.profile.role.id.as_str() == name) {
                available.push(e.profile.role.version.to_string());
            }
            for p in self.bundled.iter().filter(|p| p.role.id.as_str() == name) {
                available.push(p.role.version.to_string());
            }
            available.sort();
            available.dedup();
            if available.is_empty() {
                return Err(SurgeError::ProfileNotFound(format!(
                    "{name}@{requested_version}"
                )));
            }
            return Err(SurgeError::ProfileVersionMismatch {
                name: name.to_string(),
                requested: requested_version.to_string(),
                available,
            });
        }

        // No version requested: latest disk wins, else latest bundled.
        if let Some(entry) = self.disk.by_name_latest(name) {
            return Ok((entry.profile.clone(), Provenance::Latest));
        }
        if let Some(profile) = BundledRegistry::by_name_latest(name) {
            return Ok((profile, Provenance::Bundled));
        }
        Err(SurgeError::ProfileNotFound(name.to_string()))
    }

    /// Borrow the underlying disk set (for diagnostics / `surge profile list`).
    #[must_use]
    pub fn disk(&self) -> &DiskProfileSet {
        &self.disk
    }

    /// Borrow the bundled set (for diagnostics / `surge profile list`).
    #[must_use]
    pub fn bundled(&self) -> &[Profile] {
        &self.bundled
    }
}

/// Run [`PromptRenderer::validate_template`] over every disk and bundled
/// profile's `prompt.system`.
///
/// # Errors
/// Returns [`SurgeError::Config`] on the first broken template, naming
/// the offending profile so the operator knows which file to fix.
pub fn validate_prompts(
    disk: &DiskProfileSet,
    bundled: &[Profile],
) -> Result<(), SurgeError> {
    let renderer = PromptRenderer::strict();
    for entry in disk.entries() {
        renderer
            .validate_template(&entry.profile.prompt.system)
            .map_err(|e| {
                tracing::error!(
                    target: "profile::validate",
                    path = %entry.path.display(),
                    id = %entry.profile.role.id,
                    err = %e,
                    "disk profile prompt.system failed validation"
                );
                SurgeError::Config(format!(
                    "disk profile {:?} ({}): prompt template invalid: {}",
                    entry.profile.role.id.as_str(),
                    entry.path.display(),
                    e
                ))
            })?;
    }
    for profile in bundled {
        renderer
            .validate_template(&profile.prompt.system)
            .map_err(|e| {
                tracing::error!(
                    target: "profile::validate",
                    id = %profile.role.id,
                    err = %e,
                    "bundled profile prompt.system failed validation"
                );
                SurgeError::Config(format!(
                    "bundled profile {:?}: prompt template invalid: {}",
                    profile.role.id.as_str(),
                    e
                ))
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn minimal_toml(id: &str, version: &str, prompt: &str) -> String {
        format!(
            r#"
schema_version = 1

[role]
id = "{id}"
version = "{version}"
display_name = "{id}"
category = "agents"
description = "test"
when_to_use = "test"

[runtime]
recommended_model = "test-model"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "{prompt}"
"#
        )
    }

    fn write(dir: &std::path::Path, file: &str, body: &str) -> PathBuf {
        let p = dir.join(file);
        std::fs::write(&p, body).unwrap();
        p
    }

    fn registry_with_disk(dir: &std::path::Path) -> ProfileRegistry {
        let disk = DiskProfileSet::scan(dir).unwrap();
        ProfileRegistry::new(disk)
    }

    #[test]
    fn resolve_bundled_only_when_disk_empty() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let key_ref = parse_key_ref("implementer").unwrap();
        let resolved = reg.resolve(&key_ref).unwrap();
        assert_eq!(resolved.profile.role.id.as_str(), "implementer");
        assert_eq!(resolved.provenance, Provenance::Bundled);
    }

    #[test]
    fn resolve_disk_overrides_bundled_for_latest() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "implementer-1.0.toml",
            &minimal_toml("implementer", "1.0.0", "DISK PROMPT"),
        );
        let reg = registry_with_disk(tmp.path());
        let key_ref = parse_key_ref("implementer").unwrap();
        let resolved = reg.resolve(&key_ref).unwrap();
        assert_eq!(resolved.provenance, Provenance::Latest);
        assert_eq!(resolved.profile.prompt.system, "DISK PROMPT");
    }

    #[test]
    fn resolve_versioned_exact_match_on_disk() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "implementer-1.0.toml",
            &minimal_toml("implementer", "1.0.0", "v1"),
        );
        write(
            tmp.path(),
            "implementer-2.0.toml",
            &minimal_toml("implementer", "2.0.0", "v2"),
        );
        let reg = registry_with_disk(tmp.path());
        let key_ref = parse_key_ref("implementer@1.0").unwrap();
        let resolved = reg.resolve(&key_ref).unwrap();
        assert_eq!(resolved.provenance, Provenance::Versioned);
        assert_eq!(resolved.profile.prompt.system, "v1");
    }

    #[test]
    fn resolve_versioned_mismatch_lists_available() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "implementer-2.0.toml",
            &minimal_toml("implementer", "2.0.0", "v2"),
        );
        let reg = registry_with_disk(tmp.path());
        let key_ref = parse_key_ref("implementer@9.9.9").unwrap();
        let err = reg.resolve(&key_ref).unwrap_err();
        match err {
            SurgeError::ProfileVersionMismatch { name, available, .. } => {
                assert_eq!(name, "implementer");
                // Bundled also has implementer@1.0.0; both should appear.
                assert!(available.iter().any(|v| v == "2.0.0"));
                assert!(available.iter().any(|v| v == "1.0.0"));
            },
            other => panic!("expected ProfileVersionMismatch, got {other:?}"),
        }
    }

    #[test]
    fn resolve_unknown_name_is_not_found() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let key_ref = parse_key_ref("definitely-not-a-real-profile").unwrap();
        let err = reg.resolve(&key_ref).unwrap_err();
        assert!(matches!(err, SurgeError::ProfileNotFound(_)));
    }

    #[test]
    fn resolve_extends_chain_uses_disk_then_bundled() {
        let tmp = TempDir::new().unwrap();
        // Disk profile that extends the bundled implementer.
        let disk_body = r#"
schema_version = 1

[role]
id = "my-impl"
version = "1.0.0"
display_name = "My Implementer"
category = "agents"
description = "team-local impl"
when_to_use = "test"
extends = "implementer@1.0"

[runtime]
recommended_model = "test-model"

[[outcomes]]
id = "implemented"
description = "done"
edge_kind_hint = "forward"

[prompt]
system = "team-local override"
"#;
        write(tmp.path(), "my-impl-1.0.toml", disk_body);
        let reg = registry_with_disk(tmp.path());
        let key_ref = parse_key_ref("my-impl").unwrap();
        let resolved = reg.resolve(&key_ref).unwrap();
        assert_eq!(resolved.profile.role.id.as_str(), "my-impl");
        // Chain: bundled implementer -> my-impl
        assert_eq!(resolved.chain.len(), 2);
        assert_eq!(resolved.chain[0].as_str(), "implementer");
        assert_eq!(resolved.chain[1].as_str(), "my-impl");
        // Child prompt wins over parent.
        assert_eq!(resolved.profile.prompt.system, "team-local override");
        // Inherited tools from bundled implementer.
        assert!(resolved
            .profile
            .tools
            .default_skills
            .iter()
            .any(|s| s == "aif-implement"));
    }

    #[test]
    fn list_includes_disk_and_bundled() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "team-impl-1.0.toml",
            &minimal_toml("team-impl", "1.0.0", "team prompt"),
        );
        let reg = registry_with_disk(tmp.path());
        let entries = reg.list();
        // 17 bundled + 1 disk = 18 (no shadow collision)
        assert_eq!(entries.len(), 18);
        assert!(entries.iter().any(|e| e.profile.role.id.as_str() == "team-impl"
            && e.provenance == Provenance::Latest));
        assert!(entries.iter().any(|e| e.profile.role.id.as_str() == "implementer"
            && e.provenance == Provenance::Bundled));
    }

    #[test]
    fn list_disk_shadows_bundled_when_id_version_match() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "implementer-1.0.toml",
            &minimal_toml("implementer", "1.0.0", "shadowed"),
        );
        let reg = registry_with_disk(tmp.path());
        let entries = reg.list();
        // Bundled implementer at 1.0.0 is shadowed by the disk override;
        // total count drops to 17.
        assert_eq!(entries.len(), 17);
        let implementer = entries
            .iter()
            .find(|e| e.profile.role.id.as_str() == "implementer")
            .unwrap();
        assert_eq!(implementer.provenance, Provenance::Latest);
        assert_eq!(implementer.profile.prompt.system, "shadowed");
    }

    #[test]
    fn validate_prompts_passes_for_bundled_set() {
        // Every shipped profile must compile against the strict-mode probe.
        let disk = DiskProfileSet::empty();
        let bundled = BundledRegistry::all();
        validate_prompts(&disk, &bundled).unwrap();
    }

    #[test]
    fn validate_prompts_rejects_broken_disk_template() {
        let tmp = TempDir::new().unwrap();
        // Raw string (no format!) so the literal "{{" survives intact.
        // An unmatched "{{" is what trips Handlebars' compile pass.
        let body = r#"
schema_version = 1

[role]
id = "broken"
version = "1.0.0"
display_name = "Broken"
category = "agents"
description = "broken template"
when_to_use = "test"

[runtime]
recommended_model = "test"

[[outcomes]]
id = "done"
description = "done"
edge_kind_hint = "forward"

[prompt]
system = "Hello {{ unmatched"
"#;
        write(tmp.path(), "broken-1.0.toml", body);
        let disk = DiskProfileSet::scan(tmp.path()).unwrap();
        // The scan layer parses the TOML successfully (it's syntactically
        // valid); the prompt body only fails when handed to Handlebars.
        let bundled = BundledRegistry::all();
        let err = validate_prompts(&disk, &bundled).unwrap_err();
        match err {
            SurgeError::Config(msg) => assert!(msg.contains("broken")),
            other => panic!("expected Config error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_specialized_extends_through_bundled() {
        let tmp = TempDir::new().unwrap();
        let reg = registry_with_disk(tmp.path());
        let key_ref = parse_key_ref("bug-fix-implementer").unwrap();
        let resolved = reg.resolve(&key_ref).unwrap();
        assert_eq!(resolved.profile.role.id.as_str(), "bug-fix-implementer");
        // chain: bundled implementer -> bundled bug-fix-implementer
        assert!(resolved.chain.iter().any(|k| k.as_str() == "implementer"));
        assert!(resolved
            .chain
            .iter()
            .any(|k| k.as_str() == "bug-fix-implementer"));
    }
}
