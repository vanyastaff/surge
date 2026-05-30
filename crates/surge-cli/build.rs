//! Build script: embed the git short SHA and commit date into the
//! binary so `surge --version` can report exactly which build is running.
//!
//! Both values fall back to `"unknown"` when git is unavailable (e.g. a
//! crates.io tarball build with no `.git`), so the `env!` lookups in
//! `main.rs` always succeed. The commit date (not wall-clock build time)
//! is used so the version string is reproducible for a given commit.

use std::process::Command;

fn main() {
    // Re-run when HEAD moves so the embedded SHA stays fresh. `--git-path`
    // resolves correctly inside linked worktrees (where `.git` is a file).
    if let Some(head) = git(&["rev-parse", "--git-path", "HEAD"]) {
        println!("cargo:rerun-if-changed={head}");
    }
    // Also watch the resolved branch ref. HEAD is usually symbolic
    // (`ref: refs/heads/<branch>`) and its file content does NOT change on a
    // new commit — the branch ref file does. Without this, incremental
    // builds keep a stale SHA/date after HEAD advances on the same branch.
    if let Some(sym) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        if let Some(ref_path) = git(&["rev-parse", "--git-path", sym.as_str()]) {
            println!("cargo:rerun-if-changed={ref_path}");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");

    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SURGE_GIT_SHA={sha}");

    let commit_date = git(&["log", "-1", "--format=%cd", "--date=short"])
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SURGE_BUILD_DATE={commit_date}");
}

/// Run a git command, returning trimmed stdout on success, `None` on any
/// failure (git missing, non-zero exit, empty output, non-UTF-8).
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
