//! `surge-core::skill` — discovery and resolution against on-disk fixture
//! packs (both the Agent Skills and Agent Plugins layouts), per Истории 8-9,
//! 15, 42 and Решения §3-4, §22 of the competitive-waves spec.

use std::path::{Path, PathBuf};

use surge_core::skill::{SkillCatalog, SkillError, SkillProvider, SkillRef, SkillRoot};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/skills")
}

#[test]
fn discover_resolves_agent_skills_and_agent_plugins_packs_unmodified() {
    let roots = [
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: fixtures_dir().join("project"),
        },
        SkillRoot {
            provider: SkillProvider::UserDir,
            path: fixtures_dir().join("user"),
        },
    ];
    let catalog = SkillCatalog::discover(&roots);

    let mut names: Vec<&str> = catalog.skills().map(|s| s.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["code-reviewer", "commit-helper"]);

    // Agent Skills pack: `.../project/code-reviewer/SKILL.md`.
    let code_reviewer = catalog
        .skills()
        .find(|s| s.name == "code-reviewer")
        .expect("code-reviewer discovered from the ProjectDir root");
    assert_eq!(code_reviewer.provider, SkillProvider::ProjectDir);
    assert_eq!(code_reviewer.version.as_deref(), Some("1.0.0"));
    let resolved = catalog
        .resolve(code_reviewer)
        .expect("code-reviewer resolves");
    assert_eq!(
        resolved.instructions,
        "# Code Reviewer\n\nRead the diff, flag correctness bugs first, then style nits."
    );
    assert_eq!(resolved.files.len(), 2);
    assert!(resolved.files.iter().any(|f| f.ends_with("SKILL.md")));
    assert!(resolved.files.iter().any(|f| f.ends_with("reference.md")));

    // Agent Plugins package: `.../user/acme-toolkit/skills/commit-helper/SKILL.md`,
    // version falls back to the plugin manifest's `version` since the nested
    // SKILL.md declares none of its own.
    let commit_helper = catalog
        .skills()
        .find(|s| s.name == "commit-helper")
        .expect("commit-helper discovered from the UserDir root's nested plugin package");
    assert_eq!(commit_helper.provider, SkillProvider::UserDir);
    assert_eq!(commit_helper.version.as_deref(), Some("2.3.1"));
    let resolved = catalog
        .resolve(commit_helper)
        .expect("commit-helper resolves");
    assert_eq!(
        resolved.instructions,
        "# Commit Helper\n\nSummarize the diff into a single conventional-commit subject line."
    );
    assert_eq!(resolved.files.len(), 2);
}

#[test]
fn registry_provider_resolves_against_configured_root_on_disk() {
    // §22: `SkillProvider::Registry` is just another caller-tagged root — no
    // network client exists anywhere in this crate to fetch a pack over.
    let roots = [SkillRoot {
        provider: SkillProvider::Registry,
        path: fixtures_dir().join("project"),
    }];
    let catalog = SkillCatalog::discover(&roots);

    let skill_ref = catalog
        .skills()
        .find(|s| s.name == "code-reviewer")
        .expect("code-reviewer discovered from the Registry root");
    assert_eq!(skill_ref.provider, SkillProvider::Registry);
    assert!(catalog.resolve(skill_ref).is_ok());
}

#[test]
fn malformed_skill_md_yields_typed_error_naming_file_and_reason() {
    let roots = [SkillRoot {
        provider: SkillProvider::ProjectDir,
        path: fixtures_dir().join("broken"),
    }];
    let catalog = SkillCatalog::discover(&roots);

    // The broken pack never parses into a `SkillRef` (no frontmatter `name`
    // was readable), so a caller resolving it by the directory name — the
    // same string a flow node's `skills = ["broken-pack"]` declaration would
    // use — gets the typed reason, not a silent `NotFound`.
    let probe = SkillRef {
        name: "broken-pack".to_string(),
        provider: SkillProvider::ProjectDir,
        version: None,
        hash: None, // unpinned: match by name/provider(/version) only
    };

    match catalog.resolve(&probe) {
        Err(SkillError::MalformedFrontmatter { file, reason }) => {
            assert!(
                file.ends_with("broken-pack/SKILL.md"),
                "expected the offending file to be named, got {file:?}"
            );
            assert!(
                reason.contains("closing"),
                "expected the reason to explain the missing closing delimiter, got {reason:?}"
            );
        },
        other => panic!("expected MalformedFrontmatter, got {other:?}"),
    }
}

#[test]
fn frontmatter_outside_supported_subset_yields_typed_error_naming_file_and_reason() {
    // The frontmatter parser only supports flat `key: value` pairs and inline
    // `[a, b]` lists (Решение constraint: no external YAML parser). A nested
    // map under `metadata:` is outside that subset and must be rejected with
    // a typed reason naming the file, not silently half-parsed.
    let roots = [SkillRoot {
        provider: SkillProvider::ProjectDir,
        path: fixtures_dir().join("broken"),
    }];
    let catalog = SkillCatalog::discover(&roots);

    let probe = SkillRef {
        name: "nested-structure-pack".to_string(),
        provider: SkillProvider::ProjectDir,
        version: None,
        hash: None, // unpinned: match by name/provider(/version) only
    };

    match catalog.resolve(&probe) {
        Err(SkillError::MalformedFrontmatter { file, reason }) => {
            assert!(
                file.ends_with("nested-structure-pack/SKILL.md"),
                "expected the offending file to be named, got {file:?}"
            );
            assert!(
                reason.contains("indented"),
                "expected the reason to explain the unsupported nested construct, got {reason:?}"
            );
        },
        other => panic!("expected MalformedFrontmatter, got {other:?}"),
    }
}

#[test]
fn ambiguous_name_provider_across_two_roots_is_a_typed_error() {
    // Two ProjectDir roots each contribute a pack named `dup-skill`. Silently
    // picking whichever sorts first would let a name collision shadow a
    // caller's intended pack without a trace — resolve() must refuse.
    let roots = [
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: fixtures_dir().join("ambiguous/root-a"),
        },
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: fixtures_dir().join("ambiguous/root-b"),
        },
    ];
    let catalog = SkillCatalog::discover(&roots);

    let probe = SkillRef {
        name: "dup-skill".to_string(),
        provider: SkillProvider::ProjectDir,
        version: None,
        hash: None, // unpinned: match by name/provider(/version) only
    };

    match catalog.resolve(&probe) {
        Err(SkillError::Ambiguous { name, provider }) => {
            assert_eq!(name, "dup-skill");
            assert_eq!(provider, SkillProvider::ProjectDir);
        },
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

#[test]
fn identical_content_across_two_roots_is_not_ambiguous() {
    // Решение §4: identity is the content hash, not name/version. Two
    // physically separate directories that happen to hold byte-identical
    // content (a source checkout next to its installed cache, e.g.
    // `plugins/cache/vanya/rust-studio/0.45.0` and
    // `plugins/marketplaces/vanya/plugins/rust-studio` on the real corpus
    // above) are the same pack found twice, not a genuine collision —
    // resolve() must pick one, not refuse as `Ambiguous`.
    let scratch = unique_scratch_dir("identical-content-not-ambiguous");
    let skill_md = "---\nname: same-pack\n---\n\nIdentical content in both copies.\n";
    for root_name in ["copy-a", "copy-b"] {
        let pack_dir = scratch.join(root_name).join("same-pack");
        std::fs::create_dir_all(&pack_dir).expect("create scratch pack dir");
        std::fs::write(pack_dir.join("SKILL.md"), skill_md).expect("write SKILL.md");
    }

    let roots = [
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: scratch.join("copy-a"),
        },
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: scratch.join("copy-b"),
        },
    ];
    let catalog = SkillCatalog::discover(&roots);
    assert_eq!(
        catalog.skills().filter(|s| s.name == "same-pack").count(),
        2,
        "both copies must still be discovered individually"
    );

    let probe = SkillRef {
        name: "same-pack".to_string(),
        provider: SkillProvider::ProjectDir,
        version: None,
        hash: None, // unpinned: match by name/provider only
    };
    let resolved = catalog
        .resolve(&probe)
        .expect("byte-identical duplicates must resolve, not be Ambiguous");
    assert_eq!(resolved.instructions, "Identical content in both copies.");

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn pinned_hash_selects_exact_pack_among_differing_candidates() {
    // The complementary case: when the two name/provider-matching candidates
    // genuinely differ in content, a caller that already knows which exact
    // one it wants (a real, non-placeholder `hash`) resolves straight to it
    // instead of hitting `Ambiguous`.
    let roots = [
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: fixtures_dir().join("ambiguous/root-a"),
        },
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: fixtures_dir().join("ambiguous/root-b"),
        },
    ];
    let catalog = SkillCatalog::discover(&roots);

    let candidates: Vec<SkillRef> = catalog
        .skills()
        .filter(|s| s.name == "dup-skill")
        .cloned()
        .collect();
    assert_eq!(
        candidates.len(),
        2,
        "expected both root-a's and root-b's dup-skill to be discovered"
    );
    assert_ne!(
        candidates[0].hash, candidates[1].hash,
        "the two copies must hash differently for this test to be meaningful"
    );

    let pinned = candidates[1].clone();
    let resolved = catalog
        .resolve(&pinned)
        .expect("a pinned hash must resolve to its exact pack, not Ambiguous");
    assert_eq!(Some(resolved.hash), pinned.hash);
    assert_eq!(
        resolved.instructions,
        "# Dup Skill (root B)\n\nCopy from root B."
    );
}

#[test]
#[cfg(unix)]
fn symlink_cycle_does_not_recurse_forever_or_overcount() {
    // Reproduces the exact shape a review found on a second pass: a symlink
    // several levels below the configured root pointing straight back at
    // the root itself (`skills/deep/inner/loop -> skills`), alongside one
    // real pack. A depth cap or a naive "stop when the path gets too long"
    // behavior can make the walk *terminate* without actually guarding
    // against the cycle — the real assertion is that the one real pack is
    // discovered exactly once, not silently zero or (by re-entering the
    // loop's target and re-discovering it) more than once. Unix-only: relies
    // on a real symlink to construct the cycle deterministically.
    use std::os::unix::fs::symlink;

    let scratch = unique_scratch_dir("symlink-cycle");
    let skills_root = scratch.join("skills");
    std::fs::create_dir_all(skills_root.join("real-pack")).expect("create real pack dir");
    std::fs::write(
        skills_root.join("real-pack").join("SKILL.md"),
        "---\nname: real-pack\n---\n\nBody.\n",
    )
    .expect("write SKILL.md");
    std::fs::create_dir_all(skills_root.join("deep/inner")).expect("create nested dirs");
    symlink(&skills_root, skills_root.join("deep/inner/loop"))
        .expect("create symlink back to the root");

    let roots = [SkillRoot {
        provider: SkillProvider::ProjectDir,
        path: skills_root,
    }];

    let catalog = SkillCatalog::discover(&roots);
    let names: Vec<&str> = catalog.skills().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["real-pack"],
        "the one real pack must be discovered exactly once through the symlink loop, not \
         zero or multiple times"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn two_providers_at_the_same_physical_path_each_contribute_their_own_pack() {
    // The symlink-cycle guard's `visited` set must key on `(provider,
    // canonical path)`, not the canonical path alone — a real project root
    // can be symlinked into (or simply reused as) a user root, and
    // `SkillProvider` exists precisely to keep those apart (История 42). A
    // key missing `provider` would let the *second* root's scan find every
    // candidate already marked visited by the first and silently discover
    // nothing.
    let same_path = fixtures_dir().join("project");
    let roots = [
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: same_path.clone(),
        },
        SkillRoot {
            provider: SkillProvider::UserDir,
            path: same_path,
        },
    ];
    let catalog = SkillCatalog::discover(&roots);

    let project_hits = catalog
        .skills()
        .filter(|s| s.name == "code-reviewer" && s.provider == SkillProvider::ProjectDir)
        .count();
    let user_hits = catalog
        .skills()
        .filter(|s| s.name == "code-reviewer" && s.provider == SkillProvider::UserDir)
        .count();
    assert_eq!(project_hits, 1, "the ProjectDir root must find the pack");
    assert_eq!(
        user_hits, 1,
        "the UserDir root pointed at the same physical path must ALSO find the pack — a \
         provider-blind visited set would silently collapse it to zero"
    );
}

#[test]
fn resolve_rereads_from_disk_and_does_not_serve_a_discover_time_cache() {
    // The trust-path guarantee: a pack edited (or swapped) on disk after
    // `discover()` ran must be visible to `resolve()` using the *same*
    // `SkillRef` — `resolve()` must never serve content cached at discovery
    // time. Uses a scratch directory under the OS temp dir (not a checked-in
    // fixture) because this test mutates the pack after discovering it.
    let scratch = unique_scratch_dir("resolve-rereads-fresh");
    let pack_dir = scratch.join("live-pack");
    std::fs::create_dir_all(&pack_dir).expect("create scratch pack dir");
    std::fs::write(
        pack_dir.join("SKILL.md"),
        "---\nname: live-pack\n---\n\nOriginal instructions.\n",
    )
    .expect("write initial SKILL.md");

    let roots = [SkillRoot {
        provider: SkillProvider::ProjectDir,
        path: scratch.clone(),
    }];
    let catalog = SkillCatalog::discover(&roots);
    let skill_ref = catalog
        .skills()
        .find(|s| s.name == "live-pack")
        .expect("live-pack discovered")
        .clone();

    let first = catalog
        .resolve(&skill_ref)
        .expect("resolves before the on-disk edit");
    assert_eq!(first.instructions, "Original instructions.");

    // Mutate the pack on disk *after* discover() ran, then resolve again
    // with the exact same `SkillRef` discover() produced.
    std::fs::write(
        pack_dir.join("SKILL.md"),
        "---\nname: live-pack\n---\n\nEdited instructions from a different pack now.\n",
    )
    .expect("write edited SKILL.md");

    let second = catalog
        .resolve(&skill_ref)
        .expect("resolves again after the on-disk edit");
    assert_eq!(
        second.instructions,
        "Edited instructions from a different pack now."
    );
    assert_ne!(
        second.hash, first.hash,
        "resolve() must reflect the edit's new hash, not a discover()-time cache"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
#[cfg(unix)]
fn broken_pack_io_error_is_reachable_via_resolve_not_notfound() {
    // R09.1's guarantee ("a broken SKILL.md does not fail the run silently")
    // extends past a frontmatter parse failure: a pack whose `SKILL.md`
    // exists (so its shape is recognized at `discover()` time) but cannot be
    // read must still be reachable through `resolve()` with the typed `Io`
    // reason — falling through to `NotFound` would look like the pack was
    // never there at all, instead of there but unreadable. Unix-only: relies
    // on POSIX permission bits to force the read failure deterministically.
    use std::os::unix::fs::PermissionsExt;

    let scratch = unique_scratch_dir("broken-pack-io");
    let pack_dir = scratch.join("unreadable-pack");
    std::fs::create_dir_all(&pack_dir).expect("create scratch pack dir");
    let skill_md = pack_dir.join("SKILL.md");
    std::fs::write(&skill_md, "---\nname: unreadable-pack\n---\n\nBody.\n")
        .expect("write SKILL.md");
    std::fs::set_permissions(&skill_md, std::fs::Permissions::from_mode(0o000))
        .expect("revoke read permission on SKILL.md");

    if std::fs::read_to_string(&skill_md).is_ok() {
        // Root (or another privilege that bypasses permission bits, e.g. a
        // rootless-but-capable test container) reads straight through
        // chmod 0 — this test's precondition (that revoking read access
        // actually forces a read failure) does not hold here, so the pack
        // would parse fine and the assertions below would wrongly fail.
        // Checking the *actual* effect rather than guessing at `geteuid()`
        // covers every reason permission bits might not be enforced, not
        // just literally running as uid 0.
        eprintln!(
            "broken_pack_io_error_is_reachable_via_resolve_not_notfound: chmod 0 did not \
             block reading this file (likely running with elevated privileges); skipping"
        );
        let _ = std::fs::set_permissions(&skill_md, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::remove_dir_all(&scratch);
        return;
    }

    let roots = [SkillRoot {
        provider: SkillProvider::ProjectDir,
        path: scratch.clone(),
    }];
    let catalog = SkillCatalog::discover(&roots);

    // Never became a usable `SkillRef` — its frontmatter was never read,
    // same as a malformed pack.
    assert!(
        !catalog.skills().any(|s| s.name == "unreadable-pack"),
        "an unreadable SKILL.md must not produce a discoverable SkillRef"
    );

    let probe = SkillRef {
        name: "unreadable-pack".to_string(),
        provider: SkillProvider::ProjectDir,
        version: None,
        hash: None, // unpinned: match by name/provider(/version) only
    };
    match catalog.resolve(&probe) {
        Err(SkillError::Io { path, .. }) => {
            assert!(
                path.ends_with("unreadable-pack/SKILL.md"),
                "expected the offending file to be named, got {path:?}"
            );
        },
        other => panic!(
            "expected SkillError::Io, got {other:?} — falling through to NotFound would hide \
             that this pack exists on disk but could not be read"
        ),
    }

    // Restore read permission so the scratch directory can be cleaned up.
    let _ = std::fs::set_permissions(&skill_md, std::fs::Permissions::from_mode(0o644));
    let _ = std::fs::remove_dir_all(&scratch);
}

fn unique_scratch_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "surge-core-skill-test-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create unique scratch dir");
    dir
}

#[test]
fn hash_is_sha256_over_sorted_relative_paths_and_content() {
    // Independently computed (Python `hashlib.sha256` over the same recipe
    // the module documents: for each sorted relative path, `<path>\0<bytes>\0`)
    // against the exact bytes committed at
    // `tests/fixtures/skills/project/code-reviewer/{SKILL.md,reference.md}` —
    // not derived by calling the code under test.
    const EXPECTED: &str =
        "sha256:628c565ca34b6dc83771df30c2be3712802b85a14e28de5806223009da9259bb";

    let roots = [SkillRoot {
        provider: SkillProvider::ProjectDir,
        path: fixtures_dir().join("project"),
    }];
    let catalog = SkillCatalog::discover(&roots);
    let code_reviewer = catalog
        .skills()
        .find(|s| s.name == "code-reviewer")
        .expect("code-reviewer discovered");

    assert_eq!(
        code_reviewer
            .hash
            .expect("a discovered SkillRef always carries a real hash")
            .to_string(),
        EXPECTED
    );
    let resolved = catalog.resolve(code_reviewer).unwrap();
    assert_eq!(resolved.hash.to_string(), EXPECTED);
}

/// Правило приёмки, добавленное после волны 1: «существующее продолжает
/// работать» is proved against the real Agent Skills/Agent Plugins corpus
/// on this machine, not against our own fixtures — a fixture proves the code
/// does what we intended, not that what we intended was right (D04 found
/// exactly that gap: the original fixture used the minority manifest
/// position and never exercised a real `SKILL.md`).
///
/// Self-disables — like `real_acp_smoke.rs`'s real-agent tests — when
/// neither `~/.claude/plugins` nor `~/.claude/skills` exists, rather than
/// inventing a result. Where they do exist, it also refuses to pass
/// vacuously on zero packs found: that would silently repeat the exact
/// mistake this test exists to catch.
///
/// Unix-only: the `~/.claude` convention and the independent oracle's `find`
/// shell-out (see `count_real_pack_dirs`) are both Unix-specific, and the
/// self-disable above makes this moot on any other platform regardless.
#[test]
#[cfg(unix)]
fn real_corpus_packs_resolve_without_rejection() {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("real_corpus_packs_resolve_without_rejection: no $HOME, skipping");
        return;
    };
    let plugins_root = home.join(".claude/plugins");
    let skills_root = home.join(".claude/skills");
    if !plugins_root.is_dir() && !skills_root.is_dir() {
        eprintln!(
            "real_corpus_packs_resolve_without_rejection: neither {} nor {} exists, skipping",
            plugins_root.display(),
            skills_root.display()
        );
        return;
    }

    let mut roots = Vec::new();
    let mut on_disk_count = 0usize;
    for candidate in [&plugins_root, &skills_root] {
        if candidate.is_dir() {
            on_disk_count += count_real_pack_dirs(candidate);
            roots.push(SkillRoot {
                provider: SkillProvider::UserDir,
                path: candidate.clone(),
            });
        }
    }

    let catalog = SkillCatalog::discover(&roots);
    let discovered: Vec<&SkillRef> = catalog.skills().collect();

    // `SkillError::Ambiguous` is counted, reported, and excluded from the
    // "rejected" tally below on purpose: on *this* machine `~/.claude/plugins`
    // caches several released versions of the `rust-studio` plugin (its own
    // authoring checkout included) side by side, so the same skill name,
    // provider, and version genuinely exist at more than one physical path —
    // that is exactly the collision `ambiguous_name_provider_across_two_roots_
    // is_a_typed_error` above already proves `resolve()` refuses to guess
    // through, not a parser/manifest-position defect. Counting it as
    // "rejected" here would make this test fail on this machine's specific
    // multi-version cache forever, for a reason unrelated to what it exists
    // to catch.
    let mut ambiguous_on_resolve = 0usize;
    let mut rejected_on_resolve = 0usize;
    for skill_ref in &discovered {
        if let Err(err) = catalog.resolve(skill_ref) {
            if matches!(err, SkillError::Ambiguous { .. }) {
                ambiguous_on_resolve += 1;
            } else {
                rejected_on_resolve += 1;
                eprintln!(
                    "real corpus: resolve() rejected {} ({:?}): {err}",
                    skill_ref.name, skill_ref.provider
                );
            }
        }
    }

    eprintln!(
        "real corpus: {on_disk_count} pack directories on disk, {} discovered, \
         {rejected_on_resolve} rejected on resolve, {ambiguous_on_resolve} ambiguous \
         (same name/provider/version installed at more than one path)",
        discovered.len()
    );

    assert!(
        on_disk_count > 0,
        "expected at least one real Agent Skills/Agent Plugins pack under {} or {} — found zero, \
         which would make the assertions below vacuously true instead of proving anything",
        plugins_root.display(),
        skills_root.display()
    );
    assert_eq!(
        discovered.len(),
        on_disk_count,
        "discover() found {} packs but {on_disk_count} real pack directories exist on disk — \
         some are being silently dropped (not surfaced as a discoverable ref nor a typed \
         resolve() error) rather than counted as rejected",
        discovered.len()
    );
    assert_eq!(
        rejected_on_resolve,
        0,
        "resolve() rejected {rejected_on_resolve} of {} discovered real packs for a reason other \
         than a same-name/provider/version collision (see `ambiguous_on_resolve` above)",
        discovered.len()
    );
}

/// An oracle for the test above, independent of `surge_core::skill`'s own
/// discovery walk **by mechanism, not merely by being a separate function**:
/// shells out to `find(1)` rather than re-implementing a recursive
/// directory walk in Rust. A hand-written oracle that recognizes a pack
/// boundary and otherwise recurses into subdirectories — the first version
/// of this oracle did exactly that — shares `scan.rs`'s own traversal
/// *specification* even though the two never call each other, and a review
/// proved that sharing is real: a real pack placed behind a symlink loop
/// back to the scan root made *both* sides overcount identically ("41 found,
/// 41 discovered, 0 rejected"), because the hand-rolled oracle recursed into
/// the loop the same way `scan.rs` used to. `find(1)`'s own symlink-loop
/// detection is a separate, decades-old implementation with no shared code
/// path, so it cannot agree with a `scan.rs` regression by construction.
fn count_real_pack_dirs(root: &Path) -> usize {
    let output = std::process::Command::new("find")
        .arg("-L") // follow symlinks — real `~/.claude/skills/*` entries are.
        .arg(root)
        .args([
            "(", "-name", ".*", "-o", "-name", "node_modules", "-o", "-name", "target", ")",
            "-prune", "-o", "-type", "f", "-name", "SKILL.md", "-print",
        ])
        .output()
        .expect("`find` must be available to run this real-corpus oracle");
    // A detected filesystem loop makes `find` exit non-zero *after* still
    // printing every real match to stdout (verified against a synthetic
    // loop) — the diagnostic lands on stderr, so the exit status is not a
    // signal to check here.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .count()
}
