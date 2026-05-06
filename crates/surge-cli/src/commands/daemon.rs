//! `surge daemon` subtree — start / stop / status / restart for the
//! long-running surge-daemon process.

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use std::path::PathBuf;
use std::time::Duration;
use surge_orchestrator::engine::daemon_facade::DaemonEngineFacade;

/// Subcommands under `surge daemon`.
#[derive(Subcommand, Debug)]
pub enum DaemonCommands {
    /// Start the daemon.
    Start {
        /// Detach from the controlling terminal.
        #[arg(long)]
        detached: bool,
        /// Maximum concurrent runs.
        #[arg(long, default_value_t = 8)]
        max_active: usize,
    },
    /// Stop the daemon (graceful drain).
    Stop {
        /// Skip the graceful drain and terminate the daemon
        /// immediately (Unix: SIGKILL; Windows: `taskkill /F`).
        #[arg(long)]
        force: bool,
    },
    /// Print daemon status (pid, socket, ping ok/err).
    Status,
    /// Restart the daemon (stop + start).
    Restart,
}

/// Top-level dispatcher for `surge daemon` invocations.
pub async fn run(cmd: DaemonCommands) -> Result<()> {
    match cmd {
        DaemonCommands::Start {
            detached,
            max_active,
        } => start(detached, max_active).await,
        DaemonCommands::Stop { force } => stop(force).await,
        DaemonCommands::Status => status().await,
        DaemonCommands::Restart => {
            if let Err(e) = stop(false).await {
                eprintln!("note: stop failed during restart: {e}");
            }
            // Wait for the old daemon to actually exit before spawning the new one.
            wait_for_daemon_exit(Duration::from_secs(10)).await?;
            start(true, 8).await
        },
    }
}

async fn start(detached: bool, max_active: usize) -> Result<()> {
    use surge_daemon::pidfile;

    if let Some(pid) = pidfile::read_pid(&pidfile::pid_path()?)? {
        if pidfile::is_alive(pid) {
            return Err(anyhow!("daemon already running (pid {pid})"));
        }
        eprintln!("note: stale pid file (pid {pid} not alive); will overwrite");
    }

    let mut cmd = std::process::Command::new(daemon_binary_path()?);
    cmd.arg("--max-active").arg(max_active.to_string());
    if detached {
        cmd.arg("--detached");
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                cmd.pre_exec(|| {
                    nix::unistd::setsid().ok();
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
            cmd.creation_flags(0x0000_0008 | 0x0000_0200);
        }
    }
    let child = cmd.spawn().context("spawn surge-daemon")?;
    println!("started surge-daemon (pid {})", child.id());

    // Poll for daemon readiness via connect attempt.
    // On Windows the named pipe lives in \\.\pipe\ namespace and has no
    // filesystem entry, so socket_path.exists() would always be false.
    // A successful DaemonEngineFacade::connect proves the listener is bound.
    let socket_path = pidfile::socket_path()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "daemon at {} did not become ready within 5s",
                socket_path.display()
            ));
        }
        match DaemonEngineFacade::connect(socket_path.clone()).await {
            Ok(_) => {
                println!("daemon ready: {}", socket_path.display());
                return Ok(());
            },
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            },
        }
    }
}

async fn stop(force: bool) -> Result<()> {
    use surge_daemon::pidfile;
    let pid = pidfile::read_pid(&pidfile::pid_path()?)?
        .ok_or_else(|| anyhow!("no daemon pid file; daemon not running?"))?;

    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;
        let signal = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        kill(Pid::from_raw(pid as i32), signal).context("kill daemon")?;
    }
    #[cfg(windows)]
    {
        let mut tk = std::process::Command::new("taskkill");
        tk.arg("/pid").arg(pid.to_string());
        if force {
            tk.arg("/F");
        }
        let status = tk.status().context("taskkill")?;
        if !status.success() {
            return Err(anyhow!("taskkill exited with {status}"));
        }
    }

    println!("requested daemon stop (pid {pid})");
    Ok(())
}

async fn status() -> Result<()> {
    use surge_daemon::pidfile;
    let pid_path = pidfile::pid_path()?;
    let socket_path = pidfile::socket_path()?;
    let pid = pidfile::read_pid(&pid_path)?;
    match pid {
        Some(p) if pidfile::is_alive(p) => {
            println!("status: running");
            println!("pid:    {p}");
            println!("socket: {}", socket_path.display());
            // Try Ping for live confirmation by opening the facade.
            match DaemonEngineFacade::connect(socket_path).await {
                Ok(_) => println!("ping:   ok"),
                Err(e) => println!("ping:   error ({e})"),
            }
        },
        Some(p) => {
            println!("status: stopped (stale pid file: {p})");
        },
        None => {
            println!("status: not running");
        },
    }
    Ok(())
}

/// Wait for the daemon's pid to no longer be alive. Used by
/// `restart` to ensure the old daemon has exited before spawning
/// a new one. Returns `Ok(())` when the pid is dead; `Err` after
/// `max_wait` elapses.
async fn wait_for_daemon_exit(max_wait: Duration) -> Result<()> {
    use surge_daemon::pidfile;
    let deadline = std::time::Instant::now() + max_wait;
    loop {
        let pid = pidfile::read_pid(&pidfile::pid_path()?)?;
        match pid {
            None => return Ok(()), // pid file gone — daemon exited cleanly
            Some(p) if !pidfile::is_alive(p) => return Ok(()), // stale pid
            Some(_) => {
                if std::time::Instant::now() >= deadline {
                    return Err(anyhow!("daemon did not exit within {max_wait:?}"));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            },
        }
    }
}

fn daemon_binary_path() -> Result<PathBuf> {
    if let Ok(my_exe) = std::env::current_exe()
        && let Some(parent) = my_exe.parent()
    {
        let candidate = parent.join(if cfg!(windows) {
            "surge-daemon.exe"
        } else {
            "surge-daemon"
        });
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    which::which("surge-daemon").context("surge-daemon binary not found in PATH or alongside surge")
}
