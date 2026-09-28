//! `surge telegram` subcommand group: setup / revoke / list.
//!
//! Stores a token environment reference, mints target-bound pairing codes, manages
//! the paired-chat allowlist. The actual bot loop runs inside
//! `surge-daemon`; this CLI only configures the registry SQLite that the
//! daemon reads.

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
    /// Configure a token environment reference and mint a target-bound pairing code.
    Setup {
        /// Unsupported legacy credential input; always rejected without echoing its value.
        #[arg(long, hide = true)]
        token: Option<String>,
        /// Environment variable holding the bot token in the daemon environment.
        #[arg(long)]
        token_env: Option<String>,
        /// Explicit delivery and pairing target.
        #[arg(long, allow_hyphen_values = true)]
        chat_id: Option<i64>,

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
            token_env,
            chat_id,
            label,
            ttl_secs,
        } => setup(token, token_env, chat_id, label, ttl_secs),
        TelegramCommands::Revoke { chat_id } => revoke(chat_id),
        TelegramCommands::List => list(),
    }
}

/// Store references only. Never read a bot token in the setup process.
fn setup(
    token: Option<String>,
    token_env: Option<String>,
    chat_id: Option<i64>,
    label: String,
    ttl_secs: u64,
) -> Result<()> {
    if token.is_some() {
        return Err(anyhow!(
            "--token is no longer supported; use --token-env NAME --chat-id ID"
        ));
    }
    let reference = token_env.ok_or_else(|| {
        anyhow!("--token-env NAME is required; token input on stdin is not supported")
    })?;
    let mut chars = reference.bytes();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(anyhow!(
            "--token-env must name an environment variable, not contain a token"
        ));
    }
    let chat_id = chat_id
        .filter(|id| *id != 0)
        .ok_or_else(|| anyhow!("--chat-id must be an explicit nonzero Telegram chat id"))?;
    let path = std::env::current_dir()?.join("surge.toml");
    let mut document = if path.exists() {
        std::fs::read_to_string(&path)?
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| {
                anyhow!("invalid surge.toml; repair configuration before Telegram setup")
            })?
    } else {
        toml_edit::DocumentMut::new()
    };
    let mut telegram = toml_edit::Table::new();
    telegram["bot_token_env"] = toml_edit::value(reference);
    telegram["chat_id"] = toml_edit::value(chat_id);
    document["telegram"] = toml_edit::Item::Table(telegram);
    // The replaced table removes unsupported inline credentials during migration.
    std::fs::write(&path, document.to_string()).context("save Telegram environment reference")?;
    let conn = open_registry_connection()?;
    secrets::delete_secret(&conn, TELEGRAM_BOT_TOKEN_KEY)
        .context("remove legacy plaintext Telegram credential")?;
    let pairing_token = mint_pairing_token(
        &conn,
        &label,
        chat_id,
        Duration::from_secs(ttl_secs),
        now_ms(),
    )
    .context("mint pairing code")?;
    println!(
        "Telegram configured for chat {chat_id}; supply the token environment variable to the daemon."
    );
    println!("Send /pair {pairing_token} from that chat within {ttl_secs} seconds.");
    println!("Start the daemon with this surge.toml configuration to receive cards.");
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

/// `$SURGE_HOME` if set and non-empty, otherwise `~/.surge/`.
///
/// Mirrors the resolution helper in `commands/bootstrap.rs` so isolated
/// installs and tests that point `SURGE_HOME` at a temp dir see the
/// same registry the daemon writes to.
fn surge_home_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var("SURGE_HOME")
        && !custom.is_empty()
    {
        return Ok(PathBuf::from(custom));
    }
    let base = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home directory"))?;
    Ok(base.join(".surge"))
}

/// Unix epoch ms now. Used as the timestamp for secrets and pairings rows.
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
