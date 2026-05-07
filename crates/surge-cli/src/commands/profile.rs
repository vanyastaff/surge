//! `surge profile {list,show,validate,new}` — CLI surface for the
//! profile registry shipped in the `Profile registry & bundled roles`
//! milestone.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use surge_core::profile::Profile;
use surge_core::profile::keyref::parse_key_ref;
use surge_core::profile::registry::Provenance;
use surge_orchestrator::profile_loader::ProfileRegistry;
use surge_orchestrator::prompt::PromptRenderer;

/// `surge profile <command>`.
#[derive(Debug, Subcommand)]
pub enum ProfileCommands {
    /// List every visible profile with its provenance (disk vs bundled).
    List(ListArgs),

    /// Render a profile after `extends` resolution and merge.
    Show(ShowArgs),

    /// Validate a profile file against the schema and Handlebars syntax.
    Validate(ValidateArgs),

    /// Scaffold a new profile under `${SURGE_HOME}/profiles/`.
    New(NewArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Output format. Defaults to a human-readable table.
    #[arg(long, value_enum, default_value_t = ListFormat::Table)]
    pub format: ListFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ListFormat {
    Table,
    Json,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Profile name (e.g. `implementer`). Use `--version` to pin a
    /// specific semver; otherwise the highest available version wins.
    pub name: String,

    /// Pin to an exact version (e.g. `1.0.0` or `1.0`).
    #[arg(long)]
    pub version: Option<String>,

    /// Show the raw (un-merged) profile instead of the resolved /
    /// inheritance-merged form.
    #[arg(long)]
    pub raw: bool,
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Path to the profile TOML file.
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// Profile name (must satisfy `ProfileKey` rules: ASCII letter start,
    /// alphanumeric or `_-.@`, max 64 chars).
    pub name: String,

    /// Optional base profile to inherit from (e.g. `implementer@1.0`).
    /// Without this the new profile starts from scratch.
    #[arg(long)]
    pub base: Option<String>,

    /// Override the destination directory. Defaults to
    /// `${SURGE_HOME}/profiles/`.
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

/// Dispatch a `surge profile <command>`.
pub async fn run(command: ProfileCommands) -> Result<()> {
    match command {
        ProfileCommands::List(args) => run_list(args),
        ProfileCommands::Show(args) => run_show(args),
        ProfileCommands::Validate(args) => run_validate(args),
        ProfileCommands::New(args) => run_new(args),
    }
}

fn run_list(args: ListArgs) -> Result<()> {
    let registry = ProfileRegistry::load().context("load profile registry")?;
    let entries = registry.list();

    match args.format {
        ListFormat::Table => print_table(&entries),
        ListFormat::Json => print_json(&entries)?,
    }
    Ok(())
}

#[derive(Serialize)]
struct ProfileJson<'a> {
    id: &'a str,
    version: String,
    display_name: &'a str,
    category: String,
    provenance: &'static str,
    agent_id: &'a str,
}

fn print_table(entries: &[surge_orchestrator::profile_loader::registry::ProfileListEntry]) {
    if entries.is_empty() {
        println!("(no profiles)");
        return;
    }
    println!("NAME                         VERSION    WHERE      AGENT                    DISPLAY NAME");
    for e in entries {
        let id = e.profile.role.id.as_str();
        let version = e.profile.role.version.to_string();
        let where_label = provenance_label(e.provenance);
        let agent = &e.profile.runtime.agent_id;
        let display = &e.profile.role.display_name;
        println!("{id:<28} {version:<10} {where_label:<10} {agent:<24} {display}");
    }
}

fn print_json(
    entries: &[surge_orchestrator::profile_loader::registry::ProfileListEntry],
) -> Result<()> {
    let json: Vec<ProfileJson<'_>> = entries
        .iter()
        .map(|e| ProfileJson {
            id: e.profile.role.id.as_str(),
            version: e.profile.role.version.to_string(),
            display_name: &e.profile.role.display_name,
            category: format!("{:?}", e.profile.role.category).to_lowercase(),
            provenance: provenance_label(e.profile.role.extends.as_ref().map_or(
                e.provenance,
                |_| e.provenance,
            )),
            agent_id: &e.profile.runtime.agent_id,
        })
        .collect();
    let s = serde_json::to_string_pretty(&json).context("serialize profiles to JSON")?;
    println!("{s}");
    Ok(())
}

fn provenance_label(p: Provenance) -> &'static str {
    match p {
        Provenance::Versioned => "disk@ver",
        Provenance::Latest => "disk",
        Provenance::Bundled => "bundled",
    }
}

fn run_show(args: ShowArgs) -> Result<()> {
    let registry = ProfileRegistry::load().context("load profile registry")?;

    let key_input = match &args.version {
        Some(v) => format!("{}@{}", args.name, v),
        None => args.name.clone(),
    };
    let key_ref = parse_key_ref(&key_input)
        .with_context(|| format!("parse profile reference {key_input:?}"))?;

    if args.raw {
        // For --raw we want the un-merged profile from its original source.
        let profile = if let Some(ref version) = key_ref.version {
            registry
                .disk()
                .by_name_version(args.name.as_str(), version)
                .map(|e| e.profile.clone())
                .or_else(|| {
                    surge_core::profile::bundled::BundledRegistry::by_name_version(
                        args.name.as_str(),
                        version,
                    )
                })
        } else {
            registry
                .disk()
                .by_name_latest(args.name.as_str())
                .map(|e| e.profile.clone())
                .or_else(|| {
                    surge_core::profile::bundled::BundledRegistry::by_name_latest(args.name.as_str())
                })
        }
        .with_context(|| format!("profile not found: {key_input}"))?;
        print_profile_toml(&profile)?;
    } else {
        let resolved = registry
            .resolve(&key_ref)
            .with_context(|| format!("resolve profile {key_input}"))?;
        eprintln!(
            "# resolved {} (provenance: {:?}, chain: [{}])",
            args.name,
            resolved.provenance,
            resolved
                .chain
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        print_profile_toml(&resolved.profile)?;
    }
    Ok(())
}

fn print_profile_toml(profile: &Profile) -> Result<()> {
    let s = toml::to_string_pretty(profile).context("serialize profile to TOML")?;
    print!("{s}");
    Ok(())
}

fn run_validate(args: ValidateArgs) -> Result<()> {
    if !args.path.exists() {
        bail!("file not found: {}", args.path.display());
    }
    let raw = std::fs::read_to_string(&args.path)
        .with_context(|| format!("read {}", args.path.display()))?;

    // Schema check.
    let profile: Profile = toml::from_str(&raw)
        .with_context(|| format!("parse profile TOML at {}", args.path.display()))?;

    // Handlebars syntax check.
    let renderer = PromptRenderer::strict();
    renderer
        .validate_template(&profile.prompt.system)
        .with_context(|| {
            format!(
                "prompt.system template invalid in {}",
                args.path.display()
            )
        })?;

    // Extends parent existence check (best-effort: load the registry; if
    // we cannot, skip — the profile is still considered valid in
    // isolation).
    if let Some(parent) = &profile.role.extends {
        match ProfileRegistry::load() {
            Ok(registry) => {
                let parent_ref = parse_key_ref(parent.as_str())
                    .with_context(|| format!("parse extends parent {parent:?}"))?;
                if registry.resolve(&parent_ref).is_err() {
                    eprintln!(
                        "WARN: extends parent {parent:?} not found in current registry"
                    );
                }
            },
            Err(e) => {
                eprintln!("WARN: skipped extends-parent check (registry load failed: {e})");
            },
        }
    }

    println!(
        "✅ {} ({}@{}): schema OK, prompt OK",
        args.path.display(),
        profile.role.id.as_str(),
        profile.role.version
    );
    Ok(())
}

fn run_new(args: NewArgs) -> Result<()> {
    use surge_core::keys::ProfileKey;

    // Validate the name now so we don't write a file the registry cannot
    // load.
    let key = ProfileKey::try_new(&args.name).with_context(|| {
        format!("invalid profile name {:?}", args.name)
    })?;

    let dir = match args.dir {
        Some(d) => d,
        None => surge_orchestrator::profile_loader::profiles_dir()
            .context("resolve ${SURGE_HOME}/profiles directory")?,
    };
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create profiles dir {}", dir.display()))?;
    }

    let file_name = format!("{}-1.0.toml", key.as_str());
    let dest = dir.join(&file_name);
    if dest.exists() {
        bail!(
            "refusing to overwrite existing profile {} (delete it first if intentional)",
            dest.display()
        );
    }

    let body = scaffold_body(key.as_str(), args.base.as_deref());
    std::fs::write(&dest, body)
        .with_context(|| format!("write {}", dest.display()))?;
    println!("✅ created {}", dest.display());
    println!("   Validate: surge profile validate {}", dest.display());
    Ok(())
}

fn scaffold_body(name: &str, base: Option<&str>) -> String {
    let extends_line = match base {
        Some(b) => format!("extends = \"{b}\"\n"),
        None => String::new(),
    };
    format!(
        r#"# {name}@1.0 — generated by `surge profile new`.
#
# Replace the placeholder fields and run:
#   surge profile validate ${{SURGE_HOME:-~/.surge}}/profiles/{name}-1.0.toml
#
# Inheritance: when `extends` is set this profile inherits everything from
# the named parent and only overrides the fields it explicitly sets.

schema_version = 1

[role]
id = "{name}"
version = "1.0.0"
display_name = "{display}"
category = "agents"
description = "TODO: one-line description of what this profile does."
when_to_use = "TODO: when an operator should pick this profile."
{extends_line}
[runtime]
recommended_model = "claude-opus-4-7"
default_temperature = 0.2
default_max_tokens = 200000
agent_id = "claude-code"

[sandbox]
mode = "workspace-write"

[[outcomes]]
id = "done"
description = "Replace with a real outcome name and description."
edge_kind_hint = "forward"

[prompt]
system = """
TODO: write the system prompt for this profile.
Use {{{{var}}}} placeholders for bindings; declare them in
`[[bindings.expected]]` when this profile depends on artifacts from
other nodes.
"""
"#,
        name = name,
        display = name,
        extends_line = extends_line,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scaffold_body_includes_name_and_no_extends_when_none() {
        let body = scaffold_body("my-impl", None);
        assert!(body.contains("id = \"my-impl\""));
        assert!(!body.contains("extends ="));
    }

    #[test]
    fn scaffold_body_includes_extends_line_when_base_provided() {
        let body = scaffold_body("my-impl", Some("implementer@1.0"));
        assert!(body.contains("extends = \"implementer@1.0\""));
    }

    #[test]
    fn run_new_writes_scaffold_file() {
        let tmp = TempDir::new().unwrap();
        let args = NewArgs {
            name: "demo".into(),
            base: None,
            dir: Some(tmp.path().to_path_buf()),
        };
        run_new(args).unwrap();
        let written = tmp.path().join("demo-1.0.toml");
        assert!(written.exists());
        let body = std::fs::read_to_string(written).unwrap();
        // Round-trip check: scaffolded body must parse as a Profile.
        let _: Profile = toml::from_str(&body).expect("scaffold parses");
    }

    #[test]
    fn run_new_refuses_to_overwrite() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("dup-1.0.toml"), "existing").unwrap();
        let args = NewArgs {
            name: "dup".into(),
            base: None,
            dir: Some(tmp.path().to_path_buf()),
        };
        let err = run_new(args).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"));
    }

    #[test]
    fn run_new_rejects_invalid_name() {
        let tmp = TempDir::new().unwrap();
        let args = NewArgs {
            name: "1bad".into(),
            base: None,
            dir: Some(tmp.path().to_path_buf()),
        };
        assert!(run_new(args).is_err());
    }

    #[test]
    fn run_validate_accepts_minimal_profile() {
        let tmp = TempDir::new().unwrap();
        let body = r#"
schema_version = 1

[role]
id = "ok"
version = "1.0.0"
display_name = "OK"
category = "agents"
description = "ok"
when_to_use = "ok"

[runtime]
recommended_model = "claude-opus-4-7"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "Minimal prompt."
"#;
        let path = tmp.path().join("ok-1.0.toml");
        std::fs::write(&path, body).unwrap();
        let args = ValidateArgs { path };
        run_validate(args).unwrap();
    }

    #[test]
    fn run_validate_rejects_missing_file() {
        let args = ValidateArgs {
            path: PathBuf::from("/path/that/does/not/exist.toml"),
        };
        assert!(run_validate(args).is_err());
    }

    #[test]
    fn run_validate_rejects_broken_handlebars() {
        let tmp = TempDir::new().unwrap();
        let body = r#"
schema_version = 1

[role]
id = "bad"
version = "1.0.0"
display_name = "Bad"
category = "agents"
description = "bad"
when_to_use = "bad"

[runtime]
recommended_model = "claude-opus-4-7"

[[outcomes]]
id = "done"
description = "Success"
edge_kind_hint = "forward"

[prompt]
system = "Hello {{ unmatched"
"#;
        let path = tmp.path().join("bad-1.0.toml");
        std::fs::write(&path, body).unwrap();
        let args = ValidateArgs { path };
        let err = run_validate(args).unwrap_err();
        assert!(err.to_string().contains("template invalid"));
    }
}
