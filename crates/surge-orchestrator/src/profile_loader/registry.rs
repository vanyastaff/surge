//! `ProfileRegistry` — the orchestrator's resolver over project + home + bundled.
//!
//! Three lanes, in precedence order ([`surge_core::Layer`]):
//!
//! - **Project:** `<repo>/.surge/profiles/` — the run's own
//!   [`surge_core::ProjectLayer`]. Scoped **per run** via
//!   [`ProfileRegistry::for_run`], so a daemon serving two repositories
//!   never leaks one repository's profiles into the other's runs.
//! - **Home:** `${SURGE_HOME}/profiles/` — the operator's profiles, shared
//!   by every run of the process.
//! - **Bundled:** [`surge_core::profile::BundledRegistry`], compiled in.
//!
//! Within the two disk lanes the order is **versioned → latest**: an exact
//! `[role] version` match when the reference names one, else the highest
//! semver for the name. A hit in a higher lane wins outright — a project
//! `implementer@1.0` shadows the bundled one — and every resolution logs
//! its [`Provenance`] (`ProfileResolved`), so shadowing is recorded, never
//! silent. Trust pinning of project files is a separate, later check; this
//! registry only decides *which* file wins.
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
use surge_core::project_layer::{Layer, ProjectLayer};

use super::DiskProfileSet;
use crate::prompt::PromptRenderer;

/// Registry combining the project, home and bundled profile stores.
///
/// Construct via [`ProfileRegistry::load`] (reads `${SURGE_HOME}/profiles`
/// and, when given a [`ProjectLayer`], its `profiles_dir`) or
/// [`ProfileRegistry::new`] (caller-supplied home set, e.g. for tests).
/// Rebind the project lane for one run with [`ProfileRegistry::for_run`].
#[derive(Debug, Clone)]
pub struct ProfileRegistry {
    /// The run's `.surge/profiles/` lane; empty when no project layer is bound.
    project: DiskProfileSet,
    /// The `${SURGE_HOME}/profiles/` lane — shared by every run of the
    /// process, hence behind an `Arc` like the bundled set.
    home: Arc<DiskProfileSet>,
    bundled: Arc<Vec<Profile>>,
}

/// One entry in [`ProfileRegistry::list`] output: profile + provenance.
#[derive(Debug, Clone)]
pub struct ProfileListEntry {
    /// The listed profile.
    pub profile: Profile,
    /// Which lane listed it.
    pub provenance: Provenance,
    /// For a project entry, the highest lane below it that also carries
    /// this profile's *name* (any version) — an unversioned reference to
    /// the name now lands in the project lane instead of there. `None`
    /// for home and bundled entries, and for a project name nothing else
    /// knows.
    pub shadows: Option<Layer>,
}

impl ProfileRegistry {
    /// Construct a registry by scanning `${SURGE_HOME}/profiles`, the
    /// project layer's `profiles_dir` when one is given, and pulling the
    /// bundled set.
    ///
    /// A missing directory in either disk lane is **not** an error —
    /// bundled profiles still resolve. This matches the fresh-install
    /// experience and a repository without `.surge/`.
    ///
    /// `project` is the process-wide default lane: right for an
    /// in-process CLI engine that serves one repository, wrong for a
    /// daemon that serves many — the daemon passes `None` and binds the
    /// lane per run with [`Self::for_run`].
    ///
    /// Every loaded profile's `prompt.system` is run through
    /// [`PromptRenderer::validate_template`] at this point so a broken
    /// template fails the load rather than the agent launch.
    ///
    /// # Errors
    /// Propagates [`SurgeError`] from the path resolver or directory walker.
    /// Per-file parse failures inside a directory are logged at WARN and
    /// skipped, not returned. Per-profile template-compile failures abort
    /// the load with [`SurgeError::Config`].
    pub fn load(project: Option<&ProjectLayer>) -> Result<Self, SurgeError> {
        let home_dir = super::paths::profiles_dir()?;
        let home = DiskProfileSet::scan(&home_dir)?;
        let bundled = Arc::new(BundledRegistry::all());
        validate_prompts(&home, &bundled)?;
        let registry = Self {
            project: DiskProfileSet::empty(),
            home: Arc::new(home),
            bundled,
        };
        let registry = match project {
            Some(layer) => registry.for_run(layer)?,
            None => registry,
        };
        tracing::info!(
            target: "profile::registry",
            project_count = registry.project.entries().len(),
            project_dir = project.map(|layer| layer.profiles_dir().display().to_string()),
            home_count = registry.home.entries().len(),
            home_dir = %home_dir.display(),
            bundled_count = registry.bundled.len(),
            "ProfileRegistry loaded"
        );
        Ok(registry)
    }

    /// Construct a registry from an explicit home set and no project
    /// lane. Useful for tests that want to supply a `tempdir`-scoped
    /// store without exporting `SURGE_HOME` into the process env.
    ///
    /// Skips the load-time prompt validation step — tests that need it
    /// can call [`Self::load`] with `SURGE_HOME` set, or call
    /// [`validate_prompts`] explicitly.
    #[must_use]
    pub fn new(home: DiskProfileSet) -> Self {
        let bundled = Arc::new(BundledRegistry::all());
        Self {
            project: DiskProfileSet::empty(),
            home: Arc::new(home),
            bundled,
        }
    }

    /// Replace the project lane with an explicit set (no validation, no
    /// I/O) — the test-side twin of [`Self::for_run`].
    #[must_use]
    pub fn with_project(mut self, project: DiskProfileSet) -> Self {
        self.project = project;
        self
    }

    /// The registry one run resolves against: this registry's home and
    /// bundled lanes plus a **fresh scan** of `layer.profiles_dir` as the
    /// project lane, replacing any lane already bound. The run-scoped
    /// copy is what keeps two repositories served by one daemon from
    /// seeing each other's `.surge/profiles/`.
    ///
    /// Home and bundled are shared, not re-read: both sit behind an `Arc`,
    /// so a per-task dispatcher can call this on every dispatch for the
    /// cost of one directory scan.
    ///
    /// # Errors
    /// - [`SurgeError::Io`] when `layer.profiles_dir` exists but cannot be
    ///   read (a missing directory is an empty lane, not an error).
    /// - [`SurgeError::Config`] when a project profile's `prompt.system`
    ///   fails [`PromptRenderer::validate_template`].
    pub fn for_run(&self, layer: &ProjectLayer) -> Result<Self, SurgeError> {
        let project = DiskProfileSet::scan(layer.profiles_dir())?;
        validate_prompts(&project, &[])?;
        tracing::debug!(
            target: "profile::registry",
            project_dir = %layer.profiles_dir().display(),
            project_count = project.entries().len(),
            "project profile lane bound"
        );
        Ok(Self {
            project,
            home: Arc::clone(&self.home),
            bundled: Arc::clone(&self.bundled),
        })
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
        let Leaf {
            profile: leaf,
            provenance: leaf_provenance,
            path: leaf_path,
        } = self.find_leaf(key_ref)?;

        // 2. Walk the extends chain using the same lookup as `find_leaf`,
        //    but for parent references (which themselves are ProfileKey
        //    forms like "implementer@1.0").
        //
        //    Crucially, only `ProfileNotFound` is collapsed into `Ok(None)`
        //    so the walker can attribute it to the *parent* reference;
        //    every other resolution error (`ProfileVersionMismatch`,
        //    `InvalidProfileKey`, etc.) is propagated unchanged so the
        //    caller sees the actionable failure instead of a generic
        //    "not found".
        let chain = collect_chain(leaf.clone(), |parent_key: &ProfileKey| {
            let parsed = parse_key_ref(parent_key.as_str())
                .map_err(|e| SurgeError::InvalidProfileKey(e.to_string()))?;
            match self.find_leaf(&parsed) {
                Ok(parent) => Ok(Some(parent.profile)),
                Err(SurgeError::ProfileNotFound(_)) => Ok(None),
                Err(other) => Err(other),
            }
        })?;

        let chain_keys: Vec<ProfileKey> = chain.iter().map(|p| p.role.id.clone()).collect();
        let merged = merge_chain(&chain)?;

        // `ProfileResolved` always carries the provenance. A project file
        // anywhere in the chain — the leaf *or* a parent a bundled child
        // `extends` — is named together with what it shadows, so a
        // repository taking over `implementer@1.0` underneath every bundled
        // implementer is on the record for every resolve, not hidden
        // behind the leaf's `bundled` provenance.
        let project_members: Vec<(&str, Option<Layer>)> = chain
            .iter()
            .filter(|member| self.is_project_profile(member))
            .map(|member| {
                let name = member.role.id.as_str();
                (name, self.shadowed_layer(name))
            })
            .collect();
        let project_members_field: Vec<String> = project_members
            .iter()
            .map(|(name, shadowed)| match shadowed {
                Some(shadowed) => format!("{name} shadows {shadowed}"),
                None => (*name).to_string(),
            })
            .collect();
        tracing::debug!(
            target: "profile::registry",
            requested = key_ref.name.as_str(),
            requested_version = ?key_ref.version,
            provenance = %leaf_provenance,
            layer = %leaf_provenance.layer(),
            path = leaf_path.as_ref().map(|p| p.display().to_string()),
            project_members = ?project_members_field,
            chain_len = chain_keys.len(),
            "ProfileResolved"
        );
        if project_members
            .iter()
            .any(|(_, shadowed)| shadowed.is_some())
        {
            tracing::info!(
                target: "profile::registry",
                requested = key_ref.name.as_str(),
                provenance = %leaf_provenance,
                project_members = ?project_members_field,
                "project profile shadows a home or bundled profile in this chain"
            );
        }
        log_profile_artifact_contracts(&merged);

        Ok(ResolvedProfile {
            profile: merged,
            provenance: leaf_provenance,
            chain: chain_keys,
        })
    }

    /// List every visible profile with its provenance.
    ///
    /// Order: project entries (sorted by name + descending version), then
    /// home entries, then bundled entries — each lane skipping any
    /// `(name, version)` a higher lane already listed. Each profile
    /// appears once.
    #[must_use]
    pub fn list(&self) -> Vec<ProfileListEntry> {
        let mut out: Vec<ProfileListEntry> = Vec::new();
        let mut seen: std::collections::HashSet<(String, semver::Version)> =
            std::collections::HashSet::new();

        // Disk lanes, highest precedence first. Home entries are
        // provisionally tagged `Latest`; the provenance `resolve` assigns
        // depends on whether the caller asked for a specific version, and
        // `list` is for inventory only.
        for (lane, provenance) in [
            (&self.project, Provenance::Project),
            (&self.home, Provenance::Latest),
        ] {
            let mut entries: Vec<&super::disk::DiskEntry> = lane.entries().iter().collect();
            entries.sort_by(|a, b| {
                a.profile
                    .role
                    .id
                    .as_str()
                    .cmp(b.profile.role.id.as_str())
                    .then(b.profile.role.version.cmp(&a.profile.role.version))
            });
            for entry in entries {
                let key = (
                    entry.profile.role.id.as_str().to_string(),
                    entry.profile.role.version.clone(),
                );
                if !seen.insert(key) {
                    continue;
                }
                let shadows = match provenance {
                    Provenance::Project => self.shadowed_layer(entry.profile.role.id.as_str()),
                    _ => None,
                };
                out.push(ProfileListEntry {
                    profile: entry.profile.clone(),
                    provenance,
                    shadows,
                });
            }
        }

        // Bundled entries that don't shadow a disk match.
        for p in self.bundled.iter() {
            let key = (p.role.id.as_str().to_string(), p.role.version.clone());
            if seen.contains(&key) {
                continue;
            }
            out.push(ProfileListEntry {
                profile: p.clone(),
                provenance: Provenance::Bundled,
                shadows: None,
            });
        }

        out
    }

    /// Leaf lookup across the three lanes, highest precedence first:
    /// project → home → bundled, each lane trying an exact version match
    /// when one was requested, else its highest version for the name.
    ///
    /// Resolves the bundled half against `self.bundled` (the cached
    /// `Arc<Vec<Profile>>` populated once at `load`/`new`) rather than via
    /// `BundledRegistry::by_name_*` static methods. The static methods
    /// re-parse all embedded TOMLs on every call; consulting the cached
    /// vec keeps `resolve` allocation-free for the bundled path.
    fn find_leaf(&self, key_ref: &ProfileKeyRef) -> Result<Leaf, SurgeError> {
        let name = key_ref.name.as_str();
        if let Some(ref requested_version) = key_ref.version {
            // Versioned ref: only an exact match counts, in lane order. If
            // no lane contains it, surface a *version mismatch* (showing
            // what we did find for that name) rather than a flat "not
            // found".
            if let Some(entry) = self.project.by_name_version(name, requested_version) {
                return Ok(Leaf::from_disk(entry, Provenance::Project));
            }
            if let Some(entry) = self.home.by_name_version(name, requested_version) {
                return Ok(Leaf::from_disk(entry, Provenance::Versioned));
            }
            if let Some(profile) = self
                .bundled
                .iter()
                .find(|p| p.role.id.as_str() == name && &p.role.version == requested_version)
            {
                return Ok(Leaf::bundled(profile));
            }
            // No match: collect what versions DO exist for this name across
            // every lane to make the error actionable.
            let mut available: Vec<String> = self
                .project
                .entries()
                .iter()
                .chain(self.home.entries())
                .filter(|e| e.profile.role.id.as_str() == name)
                .map(|e| e.profile.role.version.to_string())
                .chain(
                    self.bundled
                        .iter()
                        .filter(|p| p.role.id.as_str() == name)
                        .map(|p| p.role.version.to_string()),
                )
                .collect();
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

        // No version requested: the highest-precedence lane that knows the
        // name wins with its latest version.
        if let Some(entry) = self.project.by_name_latest(name) {
            return Ok(Leaf::from_disk(entry, Provenance::Project));
        }
        if let Some(entry) = self.home.by_name_latest(name) {
            return Ok(Leaf::from_disk(entry, Provenance::Latest));
        }
        if let Some(profile) = self
            .bundled
            .iter()
            .filter(|p| p.role.id.as_str() == name)
            .max_by(|a, b| a.role.version.cmp(&b.role.version))
        {
            return Ok(Leaf::bundled(profile));
        }
        Err(SurgeError::ProfileNotFound(name.to_string()))
    }

    /// Whether this exact `(id, version)` came from the project lane.
    fn is_project_profile(&self, profile: &Profile) -> bool {
        self.project
            .by_name_version(profile.role.id.as_str(), &profile.role.version)
            .is_some()
    }

    /// The highest-precedence lane *below* the project lane that also
    /// knows `name` — what a project profile of that name shadows, if
    /// anything.
    fn shadowed_layer(&self, name: &str) -> Option<Layer> {
        if self.home.by_name_latest(name).is_some() {
            return Some(Layer::Home);
        }
        self.bundled
            .iter()
            .any(|p| p.role.id.as_str() == name)
            .then_some(Layer::Bundled)
    }

    /// Borrow the project lane (for diagnostics / `surge profile list`).
    #[must_use]
    pub fn project(&self) -> &DiskProfileSet {
        &self.project
    }

    /// Borrow the home lane (for diagnostics / `surge profile list`).
    #[must_use]
    pub fn home(&self) -> &DiskProfileSet {
        &self.home
    }

    /// Borrow the bundled set (for diagnostics / `surge profile list`).
    #[must_use]
    pub fn bundled(&self) -> &[Profile] {
        &self.bundled
    }
}

/// A leaf hit from [`ProfileRegistry::find_leaf`]: the profile, where it
/// came from, and the file it was read from (bundled hits have no path).
struct Leaf {
    profile: Profile,
    provenance: Provenance,
    path: Option<std::path::PathBuf>,
}

impl Leaf {
    fn from_disk(entry: &super::disk::DiskEntry, provenance: Provenance) -> Self {
        Self {
            profile: entry.profile.clone(),
            provenance,
            path: Some(entry.path.clone()),
        }
    }

    fn bundled(profile: &Profile) -> Self {
        Self {
            profile: profile.clone(),
            provenance: Provenance::Bundled,
            path: None,
        }
    }
}

fn log_profile_artifact_contracts(profile: &Profile) {
    for outcome in &profile.outcomes {
        for artifact in &outcome.produced_artifacts {
            tracing::debug!(
                target: "profile::registry",
                profile = profile.role.id.as_str(),
                outcome = outcome.id.as_ref(),
                artifact = artifact.path.as_str(),
                artifact_kind = artifact.contract.kind.as_str(),
                schema_version = artifact.contract.schema_version,
                "profile artifact contract resolved"
            );
        }
    }
}

/// Run [`PromptRenderer::validate_template`] over every disk and bundled
/// profile's `prompt.system`.
///
/// # Errors
/// Returns [`SurgeError::Config`] on the first broken template, naming
/// the offending profile so the operator knows which file to fix.
pub fn validate_prompts(disk: &DiskProfileSet, bundled: &[Profile]) -> Result<(), SurgeError> {
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

    /// A project lane: `<root>/.surge/profiles` holding the given files.
    fn project_root_with(files: &[(&str, &str)]) -> (TempDir, surge_core::ProjectLayer) {
        let repo = TempDir::new().unwrap();
        let layer = surge_core::ProjectLayer::for_project(repo.path());
        std::fs::create_dir_all(layer.profiles_dir()).unwrap();
        for (file, body) in files {
            write(layer.profiles_dir(), file, body);
        }
        (repo, layer)
    }

    // ── Acceptance: ".surge/profiles/x-1.0.toml in repo, none in home" ──

    #[test]
    fn project_profile_resolves_with_project_provenance_when_home_has_none() {
        let home = TempDir::new().unwrap();
        let (_repo, layer) =
            project_root_with(&[("x-1.0.toml", &minimal_toml("x", "1.0.0", "PROJECT X"))]);
        let reg = registry_with_disk(home.path()).for_run(&layer).unwrap();

        let versioned = reg.resolve(&parse_key_ref("x@1").unwrap()).unwrap();
        assert_eq!(versioned.provenance, Provenance::Project);
        assert_eq!(versioned.profile.prompt.system, "PROJECT X");

        let latest = reg.resolve(&parse_key_ref("x").unwrap()).unwrap();
        assert_eq!(latest.provenance, Provenance::Project);
    }

    #[test]
    fn project_profile_wins_over_home_with_the_same_name() {
        let home = TempDir::new().unwrap();
        write(
            home.path(),
            "x-1.0.toml",
            &minimal_toml("x", "1.0.0", "HOME X"),
        );
        let (_repo, layer) =
            project_root_with(&[("x-1.0.toml", &minimal_toml("x", "1.0.0", "PROJECT X"))]);
        let reg = registry_with_disk(home.path()).for_run(&layer).unwrap();

        let resolved = reg.resolve(&parse_key_ref("x@1").unwrap()).unwrap();
        assert_eq!(resolved.provenance, Provenance::Project);
        assert_eq!(resolved.profile.prompt.system, "PROJECT X");

        // The shadowed home copy is still there — the project lane wins by
        // precedence, not by hiding the loser.
        assert_eq!(reg.home().entries().len(), 1);
        assert_eq!(reg.project().entries().len(), 1);
    }

    #[test]
    fn project_profile_wins_over_bundled_with_the_same_name() {
        let home = TempDir::new().unwrap();
        let (_repo, layer) = project_root_with(&[(
            "implementer-1.0.toml",
            &minimal_toml("implementer", "1.0.0", "PROJECT IMPLEMENTER"),
        )]);
        let reg = registry_with_disk(home.path()).for_run(&layer).unwrap();
        let resolved = reg.resolve(&parse_key_ref("implementer").unwrap()).unwrap();
        assert_eq!(resolved.provenance, Provenance::Project);
        assert_eq!(resolved.profile.prompt.system, "PROJECT IMPLEMENTER");
    }

    #[test]
    fn two_runs_from_one_home_registry_never_see_each_others_project_profiles() {
        let home = TempDir::new().unwrap();
        let base = registry_with_disk(home.path());
        let (_repo_a, layer_a) =
            project_root_with(&[("a-only-1.0.toml", &minimal_toml("a-only", "1.0.0", "A"))]);
        let (_repo_b, layer_b) =
            project_root_with(&[("b-only-1.0.toml", &minimal_toml("b-only", "1.0.0", "B"))]);

        let run_a = base.for_run(&layer_a).unwrap();
        let run_b = base.for_run(&layer_b).unwrap();
        let a_only = parse_key_ref("a-only").unwrap();
        let b_only = parse_key_ref("b-only").unwrap();

        assert_eq!(
            run_a.resolve(&a_only).unwrap().provenance,
            Provenance::Project
        );
        assert!(matches!(
            run_a.resolve(&b_only),
            Err(SurgeError::ProfileNotFound(_))
        ));
        assert_eq!(
            run_b.resolve(&b_only).unwrap().provenance,
            Provenance::Project
        );
        assert!(matches!(
            run_b.resolve(&a_only),
            Err(SurgeError::ProfileNotFound(_))
        ));
        // The shared base is untouched by either run's lane.
        assert!(matches!(
            base.resolve(&a_only),
            Err(SurgeError::ProfileNotFound(_))
        ));
        assert!(matches!(
            base.resolve(&b_only),
            Err(SurgeError::ProfileNotFound(_))
        ));
    }

    #[test]
    fn for_run_replaces_an_existing_project_lane_rather_than_merging() {
        let home = TempDir::new().unwrap();
        let (_repo_a, layer_a) =
            project_root_with(&[("a-only-1.0.toml", &minimal_toml("a-only", "1.0.0", "A"))]);
        let (_repo_b, layer_b) =
            project_root_with(&[("b-only-1.0.toml", &minimal_toml("b-only", "1.0.0", "B"))]);
        let with_a = registry_with_disk(home.path()).for_run(&layer_a).unwrap();
        let rebound_to_b = with_a.for_run(&layer_b).unwrap();
        assert!(matches!(
            rebound_to_b.resolve(&parse_key_ref("a-only").unwrap()),
            Err(SurgeError::ProfileNotFound(_))
        ));
        assert_eq!(
            rebound_to_b
                .resolve(&parse_key_ref("b-only").unwrap())
                .unwrap()
                .provenance,
            Provenance::Project
        );
    }

    #[test]
    fn for_run_with_a_missing_project_dir_is_an_empty_lane() {
        let home = TempDir::new().unwrap();
        let repo = TempDir::new().unwrap();
        let layer = surge_core::ProjectLayer::for_project(repo.path());
        let reg = registry_with_disk(home.path()).for_run(&layer).unwrap();
        assert!(reg.project().entries().is_empty());
        assert_eq!(
            reg.resolve(&parse_key_ref("implementer").unwrap())
                .unwrap()
                .provenance,
            Provenance::Bundled
        );
    }

    #[test]
    fn for_run_rejects_a_project_profile_with_a_broken_prompt_template() {
        let home = TempDir::new().unwrap();
        let (_repo, layer) = project_root_with(&[(
            "broken-1.0.toml",
            &minimal_toml("broken", "1.0.0", "{{unclosed"),
        )]);
        let err = registry_with_disk(home.path()).for_run(&layer).unwrap_err();
        assert!(matches!(err, SurgeError::Config(_)), "{err:?}");
    }

    #[test]
    fn versioned_mismatch_lists_project_versions_too() {
        let home = TempDir::new().unwrap();
        let (_repo, layer) =
            project_root_with(&[("x-2.0.toml", &minimal_toml("x", "2.0.0", "PROJECT X2"))]);
        let reg = registry_with_disk(home.path()).for_run(&layer).unwrap();
        let err = reg.resolve(&parse_key_ref("x@1.0").unwrap()).unwrap_err();
        match err {
            SurgeError::ProfileVersionMismatch { available, .. } => {
                assert_eq!(available, vec!["2.0.0".to_string()]);
            },
            other => panic!("expected version mismatch, got {other:?}"),
        }
    }

    #[test]
    fn list_puts_project_entries_first_with_project_provenance() {
        let home = TempDir::new().unwrap();
        write(
            home.path(),
            "team-impl-1.0.toml",
            &minimal_toml("team-impl", "1.0.0", "home"),
        );
        let (_repo, layer) = project_root_with(&[
            ("x-1.0.toml", &minimal_toml("x", "1.0.0", "px")),
            (
                "implementer-1.0.toml",
                &minimal_toml("implementer", "1.0.0", "shadow bundled"),
            ),
        ]);
        let reg = registry_with_disk(home.path()).for_run(&layer).unwrap();
        let entries = reg.list();
        // Bundled implementer@1.0.0 is shadowed by the project copy.
        assert_eq!(entries.len(), surge_core::BUNDLED_COUNT + 2);
        assert_eq!(entries[0].provenance, Provenance::Project);
        assert_eq!(entries[1].provenance, Provenance::Project);
        // Sorted by name: `implementer` (shadows bundled) before `x` (new name).
        assert_eq!(entries[0].profile.role.id.as_str(), "implementer");
        assert_eq!(entries[0].shadows, Some(Layer::Bundled));
        assert_eq!(entries[1].profile.role.id.as_str(), "x");
        assert_eq!(entries[1].shadows, None);
        assert!(entries[2..].iter().all(|e| e.shadows.is_none()));
        // Exactly one `implementer@1.0.0` — the project copy; the bundled
        // one at that version is hidden, other bundled versions stay.
        let implementer_1_0: Vec<&ProfileListEntry> = entries
            .iter()
            .filter(|e| {
                e.profile.role.id.as_str() == "implementer"
                    && e.profile.role.version == semver::Version::new(1, 0, 0)
            })
            .collect();
        assert_eq!(implementer_1_0.len(), 1);
        assert_eq!(implementer_1_0[0].provenance, Provenance::Project);
        assert!(entries.iter().any(
            |e| e.profile.role.id.as_str() == "team-impl" && e.provenance == Provenance::Latest
        ));
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
            SurgeError::ProfileVersionMismatch {
                name, available, ..
            } => {
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
        assert!(
            resolved
                .profile
                .tools
                .default_skills
                .iter()
                .any(|s| s == "aif-implement")
        );
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
        // Every bundled profile plus the one disk profile — no shadow
        // collision. Derived from BUNDLED_COUNT so adding a bundled profile
        // does not fail an unrelated test with a literal to hand-bump.
        assert_eq!(entries.len(), surge_core::BUNDLED_COUNT + 1);
        assert!(entries.iter().any(
            |e| e.profile.role.id.as_str() == "team-impl" && e.provenance == Provenance::Latest
        ));
        assert!(
            entries
                .iter()
                .any(|e| e.profile.role.id.as_str() == "implementer"
                    && e.provenance == Provenance::Bundled)
        );
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
        // Bundled implementer at 1.0.0 is shadowed by the disk override, so
        // the total stays at the bundled count rather than growing.
        assert_eq!(entries.len(), surge_core::BUNDLED_COUNT);
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
        assert!(
            resolved
                .chain
                .iter()
                .any(|k| k.as_str() == "bug-fix-implementer")
        );
    }
}
