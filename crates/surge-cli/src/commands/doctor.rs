//! `surge doctor` — runtime diagnostics for ACP agents and sandbox matrix.
//!
//! Surfaces:
//! - detected agents on `PATH` (via `surge-acp` registry + discovery);
//! - their version vs. the declared
//!   [`surge_core::runtime::RuntimeVersionPolicy`];
//! - bundled sandbox matrix as JSON / TOML / text.
//!
//! Build the `DoctorReport` data shape from `surge_core::doctor` so every
//! consumer (CLI text output, JSON for tooling, UI surface) agrees on the
//! contract.

use anyhow::Result;
use clap::{Subcommand, ValueEnum};
use std::path::PathBuf;
use surge_acp::Registry;
use surge_acp::bridge::error::{OpenSessionError, SendMessageError};
use surge_acp::bridge::{AcpBridge, AgentKind, AlwaysAllowSandbox, MessageContent, SessionConfig};
use surge_core::OutcomeKey;
use surge_core::doctor::{DoctorEntry, DoctorReport, MatrixCell, MatrixCellStatus, VersionStatus};
use surge_core::runtime::{RuntimeKind, RuntimeVersionPolicy, version_policy};
use surge_core::sandbox::SandboxMode;
use surge_core::sandbox_matrix::{RuntimeSandboxMatrix, RuntimeSandboxRow};
use surge_orchestrator::engine::version_probe::{ProbeError, probe_version_with_args};
use tracing::{debug, info, warn};

/// The lifecycle stage a `surge doctor agent` smoke session reached (or
/// failed at). Lets the operator see *where* a runtime breaks — the single
/// most useful signal when "the agent won't work".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmokeStage {
    /// Spawning the agent subprocess (binary missing / not on PATH).
    Spawn,
    /// ACP handshake — `initialize` / `new_session` (protocol mismatch, agent
    /// not speaking ACP, crash before ready).
    Handshake,
    /// Authentication (agent process is up but not logged in / no API key).
    Auth,
    /// Sending the first prompt (dispatch failed for a non-auth reason).
    Prompt,
}

impl std::fmt::Display for SmokeStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Spawn => "spawn",
            Self::Handshake => "handshake",
            Self::Auth => "auth",
            Self::Prompt => "prompt",
        };
        f.write_str(s)
    }
}

/// Map an `open_session` failure to the stage it broke at.
fn classify_open_error(e: &OpenSessionError) -> SmokeStage {
    match e {
        OpenSessionError::AgentSpawnFailed { .. } => SmokeStage::Spawn,
        // Everything else from open is the handshake leg (initialize /
        // new_session / config validation surfaced before a session exists).
        _ => SmokeStage::Handshake,
    }
}

/// Map a `send_message` failure to the stage it broke at. Auth failures get
/// their own stage (reusing the `AgentAuthenticationFailed` classification
/// from the prompt-dispatch path) so the operator is told to log in.
fn classify_send_error(e: &SendMessageError) -> SmokeStage {
    match e {
        SendMessageError::AgentAuthenticationFailed { .. } => SmokeStage::Auth,
        _ => SmokeStage::Prompt,
    }
}

/// Run a real ACP smoke session against `entry`: spawn → handshake →
/// [→ new_session → one prompt], then close. Returns the first stage that
/// failed (with a detail string), or `Ok(())` when every reachable stage
/// passed.
///
/// `handshake_only = true` stops after the ACP handshake and never sends a
/// prompt — the handshake needs only the runtime installed, while sending a
/// prompt additionally needs the runtime's own model credentials configured.
/// Distinguishing the two lets a CI canary fail on a genuine wiring/protocol
/// break while a missing model credential stays a soft, expected gap (see
/// `.github/workflows/dsh-canary.yml`).
///
/// Requires the runtime's binary installed (and, unless `handshake_only`,
/// authenticated) — hence the `SURGE_DOCTOR_REAL` gate in [`run_agent_smoke`].
async fn run_real_smoke(
    entry: &surge_acp::RegistryEntry,
    handshake_only: bool,
) -> std::result::Result<(), (SmokeStage, String)> {
    // The builtin registry launches via npx with the full invocation in
    // `default_args`, so `Custom` spawns `command + args` verbatim (same as
    // the engine's resolution; see `derive_agent_kind_from_id`).
    let agent_kind = AgentKind::Custom {
        binary: PathBuf::from(&entry.command),
        args: entry.default_args.clone(),
    };
    let workdir = std::env::temp_dir().join(format!("surge-doctor-smoke-{}", entry.id));
    let _ = std::fs::create_dir_all(&workdir);
    let outcome = OutcomeKey::try_from("done").expect("static outcome key");
    let config = SessionConfig {
        agent_kind,
        working_dir: workdir,
        system_prompt: "Surge doctor smoke. Touch no files.".into(),
        declared_outcomes: vec![outcome],
        allows_escalation: false,
        tools: vec![],
        sandbox: Box::new(AlwaysAllowSandbox),
        permission_policy: surge_acp::client::PermissionPolicy::default(),
        bindings: Default::default(),
    };

    let bridge = AcpBridge::with_defaults().map_err(|e| (SmokeStage::Spawn, e.to_string()))?;
    let session = match bridge.open_session(config).await {
        Ok(s) => s,
        Err(e) => return Err((classify_open_error(&e), e.to_string())),
    };
    if handshake_only {
        let _ = bridge.close_session(session).await;
        return Ok(());
    }
    // Spawn + handshake passed. Send one prompt to exercise auth/dispatch.
    let send = bridge
        .send_message(
            session,
            MessageContent::Text("Report the `done` outcome immediately.".into()),
        )
        .await;
    let _ = bridge.close_session(session).await;
    match send {
        Ok(()) => Ok(()),
        Err(e) => Err((classify_send_error(&e), e.to_string())),
    }
}

/// `true` when `policy.min_version` is a single exact (`=`) comparator
/// rather than a floor (`>=`-style range).
///
/// Exact pins get different treatment throughout this module: a floor is
/// "not yet updated" (warn-only, per the module-level policy documented in
/// `versions.toml`), but an exact pin has no floor to be below — any
/// mismatch, older or newer, is protocol drift, and (in [`run_agent_smoke`])
/// "no data to compare" is refused rather than silently skipped.
fn is_exact_pin(policy: &RuntimeVersionPolicy) -> bool {
    matches!(
        policy.min_version.comparators.as_slice(),
        [comparator] if comparator.op == semver::Op::Exact
    )
}

/// `surge doctor` subcommand surface.
#[derive(Subcommand, Debug)]
pub enum DoctorCommands {
    /// Full diagnostic report: detected agents, version policy compliance,
    /// per-runtime sandbox-matrix status.
    Report {
        /// Output format.
        #[arg(short, long, default_value = "text")]
        format: DoctorFormat,
    },
    /// Print the bundled sandbox matrix without probing.
    Matrix {
        /// Output format.
        #[arg(short, long, default_value = "text")]
        format: DoctorFormat,
    },
    /// Run a smoke session against a named agent. The mock smoke runs in
    /// CI; the real smoke is gated by `SURGE_DOCTOR_REAL=1`.
    Agent {
        /// Agent id from `surge.toml` or the builtin registry.
        name: String,
        /// Stop the real smoke after spawn + ACP handshake; skip sending a
        /// prompt. The handshake needs only the runtime installed; sending
        /// a prompt additionally needs the runtime's own model credentials
        /// configured. A CI canary that must fail on a genuine wiring or
        /// protocol break (but not on "no credentials configured yet")
        /// wants this flag. Has no effect without `SURGE_DOCTOR_REAL=1`.
        #[arg(long)]
        handshake_only: bool,
    },
}

/// Output format selector for `report` and `matrix` subcommands.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum DoctorFormat {
    /// Human-friendly aligned text (default).
    Text,
    /// Machine-readable JSON.
    Json,
    /// TOML mirror of the bundled matrix shape.
    Toml,
}

/// Dispatch entry point — wired from `main.rs`'s `Commands::Doctor` arm.
pub async fn run(command: DoctorCommands) -> Result<()> {
    info!(target: "surge_cli.doctor", ?command, "running surge doctor");
    match command {
        DoctorCommands::Report { format } => run_report(format).await,
        DoctorCommands::Matrix { format } => run_matrix(format),
        DoctorCommands::Agent {
            name,
            handshake_only,
        } => run_agent_smoke(name, handshake_only).await,
    }
}

async fn run_report(format: DoctorFormat) -> Result<()> {
    debug!(target: "surge_cli.doctor", ?format, "building DoctorReport");
    let registry = Registry::builtin();
    let detected = registry.detect_installed_with_paths();
    let matrix = surge_core::default_matrix();
    info!(
        target: "surge_cli.doctor",
        registry_count = registry.list().len(),
        detected_count = detected.len(),
        "registry detection complete"
    );

    // Index detections by agent id so every registry entry can find its
    // command_path (when present). Reports are stable across machines:
    // every declared agent gets a row, with `binary_path = None` and
    // `version_status = ProbeFailed` for the ones not on PATH.
    let detected_by_id: std::collections::HashMap<String, surge_acp::DetectedAgent> = detected
        .into_iter()
        .map(|d| (d.entry.id.clone(), d))
        .collect();

    let mut report = DoctorReport::new();
    for entry in registry.list() {
        let command_path = detected_by_id
            .get(&entry.id)
            .and_then(|d| d.command_path.clone());
        let doctor_entry = build_doctor_entry(entry.clone(), command_path, &matrix).await;
        report.entries.push(doctor_entry);
    }

    render(&report, format)?;

    if matches!(format, DoctorFormat::Text) {
        render_telegram_section();
    }
    Ok(())
}

/// Render the `telegram-cockpit` block of the doctor report. Text-only —
/// JSON/TOML render paths read from the central `DoctorReport` struct and
/// would need a typed field on it; the structured surface is left for a
/// follow-up.
///
/// Errors from registry access are logged and surfaced as a single
/// "(unavailable)" line so a missing `~/.surge/` directory does not
/// abort the rest of the report.
fn render_telegram_section() {
    println!();
    println!("─── telegram-cockpit ───");
    match collect_telegram_health() {
        Ok(health) => {
            let bot = if health.bot_token_configured {
                "✅ yes"
            } else {
                "❌ no"
            };
            println!("  bot token configured:        {bot}");
            println!(
                "  active pairings:             {n}",
                n = health.active_pairings
            );
            println!("  open cards:                  {n}", n = health.open_cards);
            let last_api = match health.last_bot_api_at_ms {
                Some(ms) => format_unix_ms(ms),
                None => "(never)".to_owned(),
            };
            println!("  last Bot API call (approx):  {last_api}");
        },
        Err(err) => {
            warn!(
                target: "surge_cli.doctor",
                error = %err,
                "telegram health probe failed"
            );
            println!("  (unavailable — open ~/.surge/db/registry.sqlite failed)");
        },
    }
}

/// Aggregate counts the doctor report cares about. No bot-token value is
/// captured here — only presence.
struct TelegramHealth {
    bot_token_configured: bool,
    active_pairings: i64,
    open_cards: i64,
    last_bot_api_at_ms: Option<i64>,
}

fn collect_telegram_health() -> Result<TelegramHealth> {
    use surge_persistence::secrets::{TELEGRAM_BOT_TOKEN_KEY, has_secret};
    use surge_persistence::telegram::{cards, pairings};

    let home = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not resolve home directory"))?
        .join(".surge");
    let db_path = home.join("db").join("registry.sqlite");
    if !db_path.exists() {
        return Ok(TelegramHealth {
            bot_token_configured: false,
            active_pairings: 0,
            open_cards: 0,
            last_bot_api_at_ms: None,
        });
    }
    let conn =
        rusqlite::Connection::open(&db_path).map_err(|e| anyhow::anyhow!("open registry: {e}"))?;

    let bot_token_configured = has_secret(&conn, TELEGRAM_BOT_TOKEN_KEY)
        .map_err(|e| anyhow::anyhow!("query secret: {e}"))?;
    let active_pairings =
        pairings::count_active(&conn).map_err(|e| anyhow::anyhow!("count active pairings: {e}"))?;
    let open_cards =
        cards::count_open(&conn).map_err(|e| anyhow::anyhow!("count open cards: {e}"))?;
    let last_bot_api_at_ms = cards::latest_updated_at_ms(&conn)
        .map_err(|e| anyhow::anyhow!("latest card updated_at: {e}"))?;

    Ok(TelegramHealth {
        bot_token_configured,
        active_pairings,
        open_cards,
        last_bot_api_at_ms,
    })
}

/// Pretty-print a Unix epoch ms value as a UTC RFC 3339 timestamp.
fn format_unix_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map_or_else(|| format!("{ms} ms"), |dt| dt.to_rfc3339())
}

fn run_matrix(format: DoctorFormat) -> Result<()> {
    debug!(target: "surge_cli.doctor", ?format, "rendering bundled matrix");
    let matrix = surge_core::default_matrix();
    match format {
        DoctorFormat::Text => render_matrix_text(&matrix),
        DoctorFormat::Json => {
            let v = serde_json::to_string_pretty(&matrix)
                .map_err(|e| anyhow::anyhow!("json render failed: {e}"))?;
            println!("{v}");
        },
        DoctorFormat::Toml => {
            #[derive(serde::Serialize)]
            struct Doc<'a> {
                rows: &'a [RuntimeSandboxRow],
            }
            let v = toml::to_string(&Doc {
                rows: matrix.rows(),
            })
            .map_err(|e| anyhow::anyhow!("toml render failed: {e}"))?;
            println!("{v}");
        },
    }
    Ok(())
}

/// Task 11 entry point. The mock smoke path is verified in CI; the real
/// smoke path is gated behind `SURGE_DOCTOR_REAL=1`. Until the agent-stage
/// integration lands, the command surface is wired so users see the
/// intended behavior and the test suite can probe it.
///
/// `handshake_only` stops the real smoke after spawn + ACP handshake,
/// skipping the prompt dispatch that needs the runtime's own model
/// credentials — see [`run_real_smoke`].
async fn run_agent_smoke(name: String, handshake_only: bool) -> Result<()> {
    let real = std::env::var("SURGE_DOCTOR_REAL").is_ok();
    let registry = Registry::builtin();
    let entry = registry
        .list()
        .iter()
        .find(|e| e.id == name)
        .ok_or_else(|| anyhow::anyhow!("agent `{name}` not in builtin registry"))?
        .clone();
    let matrix = surge_core::default_matrix();
    let runtime = entry.runtime;

    println!("agent: {} ({})", entry.id, entry.display_name);
    if let Some(rt) = runtime {
        println!("runtime: {rt}");
        if let Some(policy) = version_policy(rt) {
            let label = if is_exact_pin(&policy) {
                "pinned version (exact)"
            } else {
                "declared minimum"
            };
            println!("{label}: {} ({})", policy.min_version, policy.note);
        }
        let matrix_dry_run = matrix_dry_run(rt, &matrix);
        println!("matrix dry-run:");
        for cell in matrix_dry_run {
            println!(
                "  {:?} -> {}{}",
                cell.mode,
                format_status(cell.status),
                if cell.flags.is_empty() {
                    String::new()
                } else {
                    format!(" flags={:?}", cell.flags)
                }
            );
        }
    } else {
        warn!(target: "surge_cli.doctor", agent = %name, "no runtime mapping in registry entry");
        println!("runtime: <unmapped; matrix lookup skipped>");
    }

    if real {
        // Version check first: a mismatched protocol version can hang or
        // half-succeed at the ACP layer instead of failing cleanly, which
        // reads as a flaky agent rather than a stale/drifted install. Reuses
        // the same probe_one/VersionStatus the passive report/matrix
        // surfaces use — the only new decision here is refuse-vs-warn:
        // an exact-pin runtime (developer preview, e.g. `dsh-acp`) refuses
        // on mismatch *and* on "no data to compare", because an unverifiable
        // pin is exactly the silent-degradation case R06.1 exists to catch.
        // A floor-style policy (`>=`, the other builtin runtimes) stays
        // warn-only here, unchanged.
        if let Some(rt) = runtime
            && let Some(policy) = version_policy(rt)
            && is_exact_pin(&policy)
        {
            let command_path = registry
                .detect_installed_with_paths()
                .into_iter()
                .find(|d| d.entry.id == entry.id)
                .and_then(|d| d.command_path);
            let refusal = match command_path.as_deref() {
                None => Some(format!(
                    "no local artifact found to verify the pinned version `{}` ({}) — refusing rather than silently proceeding",
                    policy.min_version, policy.note
                )),
                Some(path) => {
                    let (detected, status) =
                        probe_one(path, &entry.version_probe_args, Some(&policy)).await;
                    match status {
                        VersionStatus::Mismatched => Some(format!(
                            "{} is pinned to `{}` ({}); found `{}`",
                            policy.runtime,
                            policy.min_version,
                            policy.note,
                            detected.as_deref().unwrap_or("<unparseable>"),
                        )),
                        VersionStatus::ProbeFailed => Some(format!(
                            "could not verify the pinned version `{}` ({}) — the version probe itself failed; refusing rather than silently proceeding",
                            policy.min_version, policy.note
                        )),
                        // Ok / NotApplicable / BelowMinimum (the last is
                        // unreachable here since `policy` is an exact pin,
                        // never a floor) all mean nothing to refuse.
                        _ => None,
                    }
                },
            };
            if let Some(detail) = refusal {
                warn!(
                    target: "surge_cli.doctor",
                    agent = %name,
                    detail = %detail,
                    "doctor smoke refused: runtime version drift"
                );
                println!("real smoke session: FAIL at stage `version`");
                println!("  detail: {detail}");
                return Err(anyhow::anyhow!(
                    "doctor smoke for `{name}` refused: runtime version drift ({detail})"
                ));
            }
        }

        // Real smoke: spawn → handshake [→ new_session → prompt] → close.
        // Requires the runtime installed (and, unless `handshake_only`,
        // logged in).
        match run_real_smoke(&entry, handshake_only).await {
            Ok(()) => {
                if handshake_only {
                    println!(
                        "real smoke session: PASS (spawn + handshake only; prompt not attempted)"
                    );
                } else {
                    println!("real smoke session: PASS (spawn + handshake + prompt dispatched)");
                }
            },
            Err((stage, detail)) => {
                warn!(
                    target: "surge_cli.doctor",
                    agent = %name,
                    stage = %stage,
                    detail = %detail,
                    "doctor smoke failed"
                );
                println!("real smoke session: FAIL at stage `{stage}`");
                println!("  detail: {detail}");
                if stage == SmokeStage::Auth {
                    println!("  hint: log the runtime in (run its own login, or set its API key).");
                }
                return Err(anyhow::anyhow!(
                    "doctor smoke for `{name}` failed at stage `{stage}`"
                ));
            },
        }
    } else {
        println!("real smoke session: skipped (set SURGE_DOCTOR_REAL=1 to enable)");
    }
    Ok(())
}

async fn build_doctor_entry(
    entry: surge_acp::RegistryEntry,
    command_path: Option<String>,
    matrix: &RuntimeSandboxMatrix,
) -> DoctorEntry {
    let runtime = entry.runtime;
    let policy = runtime.and_then(version_policy);

    // Probe the binary (if available); fold into VersionStatus.
    let (detected_version, version_status) = match command_path.as_deref() {
        None => (None, VersionStatus::ProbeFailed),
        Some(path) => probe_one(path, &entry.version_probe_args, policy.as_ref()).await,
    };

    let matrix_cells = match runtime {
        Some(rt) => matrix_dry_run(rt, matrix),
        None => Vec::new(),
    };

    let mut e = DoctorEntry::new(entry.id.clone());
    e.runtime = runtime;
    e.binary_path = command_path;
    e.detected_version = detected_version;
    e.policy = policy;
    e.version_status = version_status;
    e.matrix = matrix_cells;
    e
}

/// Probe `binary`'s version and compare it against `policy`.
///
/// `leading_args` are inserted before the trailing `--version` — empty for a
/// real, standalone binary; non-empty for a wrapper-launched runtime with no
/// such binary (e.g. `dsh-acp`'s `["-y", "@deepseek-ai/dsh"]`, so this probes
/// `npx -y @deepseek-ai/dsh --version` — the actually-launched artifact —
/// rather than a bare `npx --version`, which would report npx's own
/// version). See [`surge_acp::RegistryEntry::version_probe_args`].
async fn probe_one(
    binary: &str,
    leading_args: &[String],
    policy: Option<&RuntimeVersionPolicy>,
) -> (Option<String>, VersionStatus) {
    let path = PathBuf::from(binary);
    match probe_version_with_args(&path, leading_args).await {
        Ok(version) => {
            let detected_str = version.to_string();
            match policy {
                Some(p) if !p.min_version.matches(&version) => {
                    let exact_pin = is_exact_pin(p);
                    warn!(
                        target: "surge_cli.doctor",
                        binary,
                        found = %detected_str,
                        expected = %p.min_version,
                        exact_pin,
                        "runtime version does not satisfy declared policy"
                    );
                    let status = if exact_pin {
                        VersionStatus::Mismatched
                    } else {
                        VersionStatus::BelowMinimum
                    };
                    (Some(detected_str), status)
                },
                Some(_) => (Some(detected_str), VersionStatus::Ok),
                None => (Some(detected_str), VersionStatus::NotApplicable),
            }
        },
        Err(err) => {
            warn!(target: "surge_cli.doctor", binary, error = %err, "version probe failed");
            (None, probe_error_to_status(&err))
        },
    }
}

fn probe_error_to_status(err: &ProbeError) -> VersionStatus {
    let _ = err; // Errors all fold to the same surface for the report.
    VersionStatus::ProbeFailed
}

/// Render every `(runtime, mode)` matrix cell for a specific runtime.
fn matrix_dry_run(runtime: RuntimeKind, matrix: &RuntimeSandboxMatrix) -> Vec<MatrixCell> {
    let modes = [
        SandboxMode::ReadOnly,
        SandboxMode::WorkspaceWrite,
        SandboxMode::WorkspaceNetwork,
        SandboxMode::FullAccess,
    ];
    modes
        .iter()
        .map(|&mode| match matrix.lookup(runtime, mode) {
            None => MatrixCell::new(mode, MatrixCellStatus::Unsupported),
            Some(row) => {
                let status = if row.verified {
                    MatrixCellStatus::Verified
                } else {
                    MatrixCellStatus::DeclaredUnverified
                };
                MatrixCell::new(mode, status)
                    .with_flags(row.flags.iter().cloned())
                    .with_note(row.note.clone())
            },
        })
        .collect()
}

fn render(report: &DoctorReport, format: DoctorFormat) -> Result<()> {
    match format {
        DoctorFormat::Text => render_report_text(report),
        DoctorFormat::Json => {
            let s = serde_json::to_string_pretty(report)
                .map_err(|e| anyhow::anyhow!("json render failed: {e}"))?;
            println!("{s}");
        },
        DoctorFormat::Toml => {
            let s =
                toml::to_string(report).map_err(|e| anyhow::anyhow!("toml render failed: {e}"))?;
            println!("{s}");
        },
    }
    Ok(())
}

fn render_report_text(report: &DoctorReport) {
    println!("# surge doctor — agent diagnostic report\n");
    if report.entries.is_empty() {
        println!("No ACP agents detected on PATH.");
        println!("Tip: install one of the bundled agents (claude, codex, gemini) and re-run.");
        return;
    }
    for (i, entry) in report.entries.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("## {}", entry.agent_name);
        match &entry.runtime {
            Some(rt) => println!("  runtime:           {rt}"),
            None => println!("  runtime:           <unmapped>"),
        }
        match &entry.binary_path {
            Some(path) => println!("  binary:            {path}"),
            None => println!("  binary:            <not detected on PATH>"),
        }
        match &entry.detected_version {
            Some(v) => println!("  detected version:  {v}"),
            None => println!("  detected version:  <unknown>"),
        }
        if let Some(p) = &entry.policy {
            let label = if is_exact_pin(p) {
                "pinned version (exact):"
            } else {
                "declared minimum:"
            };
            println!("  {label:<19} {}", p.min_version);
            if !p.note.is_empty() {
                // Surfaces developer-preview / rationale notes (e.g. `dsh`'s
                // exact-pin explanation) instead of leaving them only in
                // `versions.toml` — an operator reading `doctor report`
                // should see *why* a floor (or pin) is what it is, not just
                // the number.
                println!("  note:              {}", p.note);
            }
        }
        println!(
            "  status:            {}",
            format_version_status(entry.version_status)
        );
        if !entry.matrix.is_empty() {
            println!("  sandbox matrix:");
            for cell in &entry.matrix {
                println!(
                    "    {:18} {}{}",
                    format!("{:?}", cell.mode),
                    format_status(cell.status),
                    if cell.flags.is_empty() {
                        String::new()
                    } else {
                        format!("  flags={:?}", cell.flags)
                    },
                );
            }
        }
    }
}

fn render_matrix_text(matrix: &RuntimeSandboxMatrix) {
    println!("# surge sandbox delegation matrix\n");
    let header_runtime = "runtime";
    let header_mode = "mode";
    let header_status = "status";
    let header_flags = "flags";
    println!("{header_runtime:<14} {header_mode:<18} {header_status:<22} {header_flags}");
    let separator = "-".repeat(80);
    println!("{separator}");
    for row in matrix.rows() {
        let status = if row.verified {
            "verified"
        } else if row.flags.is_empty() {
            "declared-unverified"
        } else {
            "declared (flags set)"
        };
        let flags = if row.flags.is_empty() {
            "—".to_string()
        } else {
            format!("{:?}", row.flags)
        };
        println!(
            "{:<14} {:<18} {:<22} {}",
            format!("{}", row.runtime),
            format!("{:?}", row.mode),
            status,
            flags,
        );
    }
}

fn format_version_status(status: VersionStatus) -> &'static str {
    match status {
        VersionStatus::NotApplicable => "no policy declared",
        VersionStatus::Ok => "OK",
        VersionStatus::BelowMinimum => "BELOW MINIMUM (warn-only)",
        VersionStatus::Mismatched => "MISMATCHED (exact pin; not a floor)",
        VersionStatus::ProbeFailed => "probe failed",
        _ => "unknown",
    }
}

fn format_status(status: MatrixCellStatus) -> &'static str {
    match status {
        MatrixCellStatus::Verified => "verified",
        MatrixCellStatus::DeclaredUnverified => "declared-unverified",
        MatrixCellStatus::Unsupported => "unsupported",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_open_error_maps_spawn_vs_handshake() {
        let spawn = OpenSessionError::AgentSpawnFailed {
            kind: "codex".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no binary"),
        };
        assert_eq!(classify_open_error(&spawn), SmokeStage::Spawn);

        let handshake = OpenSessionError::HandshakeFailed {
            reason: "protocol mismatch".into(),
        };
        assert_eq!(classify_open_error(&handshake), SmokeStage::Handshake);

        // Non-spawn open failures all read as the handshake leg.
        assert_eq!(
            classify_open_error(&OpenSessionError::NoDeclaredOutcomes),
            SmokeStage::Handshake
        );
    }

    #[test]
    fn classify_send_error_maps_auth_vs_prompt() {
        let auth = SendMessageError::AgentAuthenticationFailed {
            details: "401 authentication_error".into(),
        };
        assert_eq!(classify_send_error(&auth), SmokeStage::Auth);

        let other = SendMessageError::Bridge(
            surge_acp::bridge::error::BridgeError::CommandSendFailed("reset".into()),
        );
        assert_eq!(classify_send_error(&other), SmokeStage::Prompt);
    }

    #[test]
    fn smoke_stage_display_is_stable() {
        assert_eq!(SmokeStage::Spawn.to_string(), "spawn");
        assert_eq!(SmokeStage::Handshake.to_string(), "handshake");
        assert_eq!(SmokeStage::Auth.to_string(), "auth");
        assert_eq!(SmokeStage::Prompt.to_string(), "prompt");
    }

    #[test]
    fn matrix_dry_run_covers_four_modes() {
        let matrix = surge_core::default_matrix();
        let cells = matrix_dry_run(RuntimeKind::ClaudeCode, &matrix);
        assert_eq!(cells.len(), 4);
        let modes: Vec<_> = cells.iter().map(|c| c.mode).collect();
        assert_eq!(
            modes,
            vec![
                SandboxMode::ReadOnly,
                SandboxMode::WorkspaceWrite,
                SandboxMode::WorkspaceNetwork,
                SandboxMode::FullAccess,
            ]
        );
    }

    #[test]
    fn matrix_dry_run_for_claude_marks_verified_cells() {
        let matrix = surge_core::default_matrix();
        let cells = matrix_dry_run(RuntimeKind::ClaudeCode, &matrix);
        // The bundled matrix has Claude Code verified across all four modes.
        for cell in cells {
            assert_eq!(cell.status, MatrixCellStatus::Verified, "{cell:?}");
            assert!(!cell.flags.is_empty());
        }
    }

    #[test]
    fn matrix_dry_run_for_gemini_marks_unverified_non_full_access() {
        let matrix = surge_core::default_matrix();
        let cells = matrix_dry_run(RuntimeKind::Gemini, &matrix);
        // Only full-access is verified for Gemini (others are docker-only gaps).
        for cell in cells {
            match cell.mode {
                SandboxMode::FullAccess => {
                    assert_eq!(cell.status, MatrixCellStatus::Verified);
                },
                _ => {
                    assert_eq!(cell.status, MatrixCellStatus::DeclaredUnverified);
                },
            }
        }
    }

    #[test]
    fn is_exact_pin_true_for_single_exact_comparator() {
        let policy = RuntimeVersionPolicy::new(
            RuntimeKind::DeepSeekHarness,
            semver::VersionReq::parse("=0.1.2-rc.1").expect("valid exact req"),
        );
        assert!(is_exact_pin(&policy));
    }

    #[test]
    fn is_exact_pin_false_for_floor_style_comparator() {
        let policy = RuntimeVersionPolicy::new(
            RuntimeKind::ClaudeCode,
            semver::VersionReq::parse(">=2.0.0").expect("valid req"),
        );
        assert!(!is_exact_pin(&policy));
    }

    #[tokio::test]
    async fn probe_one_reports_mismatched_not_below_minimum_for_exact_pin_drift() {
        // A version that does not satisfy an exact pin must be labeled
        // `Mismatched`, never `BelowMinimum` — the pin has no floor to be
        // below, and a newer-than-pinned version is not "deficient".
        let policy = RuntimeVersionPolicy::new(
            RuntimeKind::DeepSeekHarness,
            semver::VersionReq::parse("=0.1.2-rc.1").expect("valid exact req"),
        )
        .with_note("developer preview; exact pin");
        let cargo = which::which("cargo").ok();
        let Some(cargo) = cargo else {
            eprintln!("skipping: cargo not on PATH");
            return;
        };
        // `cargo --version` reports cargo's own (never `0.1.2-rc.1`) version,
        // so this deterministically drifts from the pin without needing a
        // network call.
        let (detected, status) =
            probe_one(cargo.to_str().expect("utf8 path"), &[], Some(&policy)).await;
        assert_eq!(status, VersionStatus::Mismatched);
        assert!(detected.is_some());
    }

    #[tokio::test]
    async fn probe_one_reports_below_minimum_for_floor_style_drift() {
        let policy = RuntimeVersionPolicy::new(
            RuntimeKind::ClaudeCode,
            semver::VersionReq::parse(">=999.0.0").expect("valid req"),
        );
        let cargo = which::which("cargo").ok();
        let Some(cargo) = cargo else {
            eprintln!("skipping: cargo not on PATH");
            return;
        };
        let (_, status) = probe_one(cargo.to_str().expect("utf8 path"), &[], Some(&policy)).await;
        assert_eq!(status, VersionStatus::BelowMinimum);
    }
}
