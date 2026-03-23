use anyhow::Result;

pub fn diff(spec_id: String) -> Result<()> {
    let mgr = surge_git::GitManager::discover()?;
    let diff = mgr.diff(&spec_id)?;
    if diff.is_empty() {
        println!("No changes in worktree for '{spec_id}'");
    } else {
        println!("{diff}");
    }
    Ok(())
}

pub fn merge(spec_id: String, yes: bool) -> Result<()> {
    if !yes {
        println!("⚡ Merge worktree for '{spec_id}' into current branch?");
        println!("   Run with -y to skip confirmation.");
        return Ok(());
    }

    let mgr = surge_git::GitManager::discover()?;
    mgr.merge(&spec_id, None)?;
    println!("✅ Merged '{spec_id}' into current branch");
    Ok(())
}

pub fn discard(spec_id: String, yes: bool) -> Result<()> {
    if !yes {
        println!("⚡ Discard worktree and branch for '{spec_id}'?");
        println!("   This is irreversible. Run with -y to confirm.");
        return Ok(());
    }

    let mgr = surge_git::GitManager::discover()?;
    mgr.discard(&spec_id)?;
    println!("✅ Discarded worktree for '{spec_id}'");
    Ok(())
}

pub fn clean(yes: bool) -> Result<()> {
    let mgr = surge_git::GitManager::discover()?;
    let lifecycle = surge_git::LifecycleManager::new(mgr);

    if !yes {
        println!("⚡ Cleanup preview (run with -y to execute):");
        return Ok(());
    }

    let report = lifecycle.full_cleanup()?;

    if report.removed_worktrees.is_empty() && report.removed_branches.is_empty() {
        println!("✅ Nothing to clean up");
    } else {
        for wt in &report.removed_worktrees {
            println!("  Removed worktree: {wt}");
        }
        for br in &report.removed_branches {
            println!("  Deleted branch: {br}");
        }
        println!("\n✅ Cleanup complete");
    }
    Ok(())
}

pub fn worktrees() -> Result<()> {
    let mgr = surge_git::GitManager::discover()?;
    let worktrees = mgr.list_worktrees()?;

    if worktrees.is_empty() {
        println!("No active worktrees.");
    } else {
        println!("⚡ Active worktrees:\n");
        for wt in &worktrees {
            let status = if wt.exists_on_disk { "✅" } else { "❌ (missing)" };
            println!("  {status} {} — {}", wt.spec_id, wt.branch);
            println!("       {}", wt.path.display());
        }
    }
    Ok(())
}
