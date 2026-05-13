use anyhow::Result;

pub fn clean(yes: bool) -> Result<()> {
    let mgr = surge_git::GitManager::discover()?;
    let repo_path = mgr.repo_path().to_path_buf();

    // Create audit logger in .surge/cleanup.log
    let audit_path = repo_path.join(".surge").join("cleanup.log");
    let audit = surge_git::CleanupAudit::new(audit_path)?;
    let lifecycle = surge_git::LifecycleManager::with_audit(mgr, audit);

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
            let status = if wt.exists_on_disk {
                "✅"
            } else {
                "❌ (missing)"
            };
            println!("  {status} {} — {}", wt.spec_id, wt.branch);
            println!("       {}", wt.path.display());
        }
    }
    Ok(())
}
