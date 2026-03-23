use anyhow::Result;
use clap::Subcommand;

use super::load_spec_by_id;

#[derive(Subcommand)]
pub enum InsightsCommands {
    /// Show cost breakdown by subtask
    Cost {
        /// Spec ID or filename
        id: String,
    },
}

pub fn run(command: InsightsCommands) -> Result<()> {
    match command {
        InsightsCommands::Cost { id } => {
            let spec_file = load_spec_by_id(&id)?;
            let spec = &spec_file.spec;

            println!("⚡ Cost Insights: {}\n", spec.title);
            println!("ID: {}", spec.id);
            println!("\nSubtask Breakdown:");

            for (i, sub) in spec.subtasks.iter().enumerate() {
                println!("  {}. {} [{:?}]", i + 1, sub.title, sub.complexity);
                // TODO: Show actual cost metrics once tracking is implemented
                println!("     Cost tracking: Not yet implemented");
            }

            // Summary
            println!("\n📊 Summary:");
            println!("   Total subtasks: {}", spec.subtasks.len());
            println!("   Cost tracking: Coming soon");
            println!("\n💡 Tip: Cost tracking will be available once metrics are integrated.");
        }
    }
    Ok(())
}
