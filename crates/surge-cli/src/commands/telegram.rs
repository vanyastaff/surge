//! `surge telegram` subcommand group: setup / revoke / list.
//!
//! Persists the cockpit bot token, mints one-shot pairing tokens, manages
//! the paired-chat allowlist. The actual bot loop runs inside
//! `surge-daemon`; this CLI only configures the registry SQLite that the
//! daemon reads.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use surge_persistence::secrets::{self, TELEGRAM_BOT_TOKEN_KEY};
use surge_persistence::telegram::pairing::mint_pairing_token;
use surge_persistence::telegram::pairings;

/// Subcommands for `surge telegram`.
#[derive(Subcommand)]
pub enum TelegramCommands {
    /// Persist a Bot API token and mint a one-shot pairing token.
    ///
    /// In interactive mode the token is read from stdin; pass `--token` for
    /// a script-friendly path. The pairing token is printed once and
    /// expires after `--ttl-secs` seconds (default 10 minutes).
    Setup {
        /// Bot API token from BotFather. If omitted, prompts on stdin.
        #[arg(short, long)]
        token: Option<String>,

        /// Operator-supplied label attached to the resulting paired chat.
        #[arg(short, long, default_value = "operator")]
        label: String,

        /// Pairing token time-to-live, in seconds.
        #[arg(long, default_value_t = 600)]
        ttl_secs: u64,
    },

    /// Revoke a previously-paired chat. The chat will no longer pass the
    /// admission check.
    Revoke {
        /// Telegram chat id to revoke.
        chat_id: i64,
    },

    /// List all currently-active pairings.
    List,
}

/// Dispatch the subcommand.
///
/// # Errors
///
/// Returns any error surfaced by the underlying persistence helpers; the
/// caller's CLI binary prints the error chain.
pub async fn run(command: TelegramCommands) -> Result<()> {
    match command {
        TelegramCommands::Setup {
            token,
            label,
            ttl_secs,
        } => setup(token, label, ttl_secs),
        TelegramCommands::Revoke { chat_id } => revoke(chat_id),
        TelegramCommands::List => list(),
    }
}

/// `surge telegram setup` — write bot token, mint pairing token, print
/// instructions.
fn setup(token: Option<String>, label: String, ttl_secs: u64) -> Result<()> {
    let bot_token = resolve_bot_token(token)?;
    let trimmed = bot_token.trim();
    if trimmed.is_empty() {
        return Err(anyhow!(
            "bot token is empty — paste the token from BotFather"
        ));
    }
    let bot_token = trimmed.to_owned();

    let conn = open_registry_connection()?;
    let now_ms = now_ms();

    secrets::set_secret(&conn, TELEGRAM_BOT_TOKEN_KEY, &bot_token, now_ms)
        .context("persist telegram bot token")?;
    tracing::info!(
        target: "cli::telegram",
        "bot token stored under {TELEGRAM_BOT_TOKEN_KEY}"
    );

    let pairing_token = mint_pairing_token(&conn, &label, Duration::from_secs(ttl_secs), now_ms)
        .context("mint pairing token")?;
    tracing::info!(
        target: "cli::telegram",
        label = %label,
        ttl_secs = %ttl_secs,
        "pairing token minted"
    );

    println!("✅ Telegram cockpit configured.");
    println!();
    println!("Pairing token: {pairing_token}");
    println!();
    println!("Send the following to your bot from your personal chat within");
    println!("{ttl_secs} seconds to pair this chat:");
    println!();
    println!("    /pair {pairing_token}");
    println!();
    println!("Then start the daemon with `surge daemon start` to begin receiving cockpit cards.");
    Ok(())
}

/// `surge telegram revoke <chat_id>` — soft-delete the allowlist row.
fn revoke(chat_id: i64) -> Result<()> {
    let conn = open_registry_connection()?;
    pairings::revoke(&conn, chat_id, now_ms()).context("revoke pairing")?;
    tracing::info!(
        target: "cli::telegram",
        chat_id = %chat_id,
        "chat revoked"
    );
    println!("✅ Chat {chat_id} revoked.");
    Ok(())
}

/// `surge telegram list` — print every active pairing.
fn list() -> Result<()> {
    let conn = open_registry_connection()?;
    let rows = pairings::list_active(&conn).context("list active pairings")?;
    if rows.is_empty() {
        println!("No active pairings.");
        return Ok(());
    }
    println!("Active pairings ({n}):", n = rows.len());
    for p in rows {
        println!(
            "  chat_id={chat_id}  label={label}  paired_at_ms={paired_at}",
            chat_id = p.chat_id,
            label = p.user_label,
            paired_at = p.paired_at,
        );
    }
    Ok(())
}

/// Resolve the bot token from `--token` or stdin.
fn resolve_bot_token(token: Option<String>) -> Result<String> {
    if let Some(t) = token {
        return Ok(t);
    }
    print!("Bot token (paste from BotFather): ");
    std::io::stdout().flush().context("flush stdout")?;
    let mut buf = String::new();
    std::io::stdin()
        .read_line(&mut buf)
        .context("read bot token from stdin")?;
    Ok(buf)
}

/// Open a single connection on the registry SQLite. Applies migrations as
/// a side-effect via the existing `Storage`-less path so this command
/// works on a fresh install where the daemon has never run.
fn open_registry_connection() -> Result<rusqlite::Connection> {
    let home = surge_home_dir()?;
    let clock = surge_persistence::runs::SystemClock;
    let pool = surge_persistence::runs::registry::open_registry_pool(&home, &clock)
        .map_err(|e| anyhow!("open registry pool: {e}"))?;
    let conn = pool.get().context("acquire registry connection")?;
    // The pool returns a managed connection that drops back into the pool
    // on `Drop`. Detaching to an owned rusqlite::Connection requires
    // opening the underlying file directly — simpler than threading a
    // pool through every callsite for a one-shot CLI command.
    let db_path = home.join("db").join("registry.sqlite");
    drop(conn);
    drop(pool);
    rusqlite::Connection::open(&db_path).map_err(|e| anyhow!("open registry: {e}"))
}

/// `~/.surge/` — same convention every other CLI command uses.
fn surge_home_dir() -> Result<PathBuf> {
    let base = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    Ok(base.join(".surge"))
}

/// Unix epoch ms now. Used as the timestamp for secrets and pairings rows.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
