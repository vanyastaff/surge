use anyhow::Result;
use clap::Subcommand;

use super::load_spec_by_id;

#[derive(Subcommand)]
pub enum SpecCommands {
    /// Create a new spec from a template
    Create {
        /// Description of the spec
        description: String,
        /// Template to use (feature, bugfix, refactor)
        #[arg(short, long)]
        template: Option<String>,
    },
    /// List all specs
    List,
    /// Show spec details
    Show {
        /// Spec ID or filename
        id: String,
    },
    /// Validate a spec
    Validate {
        /// Spec ID or filename
        id: String,
    },
}

pub fn run(command: SpecCommands) -> Result<()> {
    match command {
        SpecCommands::Create {
            description,
            template,
        } => {
            let kind = template.as_deref().unwrap_or("feature");
            let template_kind = surge_spec::TemplateKind::parse(kind)?;
            let spec_file = surge_spec::generate_template(template_kind, &description)?;

            let path = spec_file.save_to_specs_dir()?;
            println!("⚡ Created spec: {}", spec_file.spec.title);
            println!("   ID: {}", spec_file.spec.id);
            println!("   File: {}", path.display());
            println!("   Subtasks: {}", spec_file.spec.subtasks.len());
        },
        SpecCommands::List => {
            let specs = surge_spec::SpecFile::list_all()?;
            if specs.is_empty() {
                println!("No specs found. Create one with: surge spec create \"description\"");
            } else {
                println!("⚡ Specs:\n");
                for (path, sf) in &specs {
                    let filename = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();
                    println!(
                        "  {} — {} ({} subtasks)",
                        filename,
                        sf.spec.title,
                        sf.spec.subtasks.len()
                    );
                }
            }
        },
        SpecCommands::Show { id } => {
            let spec_file = load_spec_by_id(&id)?;
            let spec = &spec_file.spec;

            println!("⚡ Spec: {}\n", spec.title);
            println!("ID: {}", spec.id);
            println!("Complexity: {:?}", spec.complexity);
            println!("Description: {}", spec.description);
            println!("\nSubtasks ({}):", spec.subtasks.len());

            for (i, sub) in spec.subtasks.iter().enumerate() {
                println!("  {}. {} [{:?}]", i + 1, sub.title, sub.complexity);
                if !sub.acceptance_criteria.is_empty() {
                    for ac in &sub.acceptance_criteria {
                        let mark = if ac.met { "✅" } else { "⬜" };
                        println!("     {mark} {}", ac.description);
                    }
                }
            }

            if !spec.subtasks.is_empty() {
                match surge_spec::DependencyGraph::from_spec(spec) {
                    Ok(graph) => {
                        println!("\nDependency Graph:");
                        println!("{}", graph.to_ascii(spec));
                    },
                    Err(e) => println!("\nGraph error: {e}"),
                }
            }
        },
        SpecCommands::Validate { id } => {
            let spec_file = load_spec_by_id(&id)?;
            let result = surge_spec::validate_spec(&spec_file.spec);

            if result.is_ok() {
                println!("✅ Spec '{}' is valid", spec_file.spec.title);
                for w in &result.warnings {
                    println!("   ⚠️  {w}");
                }
            } else {
                println!("❌ Spec '{}' has errors:", spec_file.spec.title);
                for e in &result.errors {
                    println!("   ❌ {e}");
                }
                for w in &result.warnings {
                    println!("   ⚠️  {w}");
                }
                std::process::exit(1);
            }
        },
    }
    Ok(())
}
