//! Windows startup owns runtime files until the exact child is ready or settled.
use anyhow::{Context, Result, anyhow};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use surge_persistence::{RuntimeAppendFile, RuntimeDirectory, RuntimeHomeOwner};
use surge_process::owner_panic::abort_on_owner_panic as protected;

struct Startup {
    child: Option<Child>,
    logs: Vec<RuntimeAppendFile>,
    _home: RuntimeHomeOwner,
}

impl Startup {
    fn spawn(
        mut command: Command,
        home: RuntimeHomeOwner,
        logs: Vec<RuntimeAppendFile>,
    ) -> Result<Self> {
        let child = command.spawn().context("spawn surge-daemon")?;
        Ok(Self {
            child: Some(child),
            logs,
            _home: home,
        })
    }

    fn child(&mut self) -> &mut Child {
        match self.child.as_mut() {
            Some(child) => child,
            None => std::process::abort(),
        }
    }

    fn handoff(mut self) -> Result<()> {
        for log in &self.logs {
            log.flush()?;
        }
        // The successful readiness probe bound the listener to this live child.
        // The daemon now independently retains its PID and Storage namespace.
        self.child.take();
        Ok(())
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        settle_child(child);
        for log in &self.logs {
            if let Err(error) = log.flush() {
                tracing::error!(%error, "daemon startup log flush failed after child settlement");
            }
        }
    }
}

// Same direct-child terminal policy as the MCP owner: errors never mean exited.
// The retained child and namespaces stay outside every protected operation.
fn settle_child(child: &mut Child) {
    let mut warned = None;
    match protected(|| child.try_wait()) {
        Ok(Some(_)) => return,
        Ok(None) => {
            if let Err(error) = protected(|| child.kill()) {
                tracing::warn!(%error, "daemon startup termination uncertain; retaining ownership");
            }
        },
        Err(error) => {
            tracing::warn!(%error, "daemon startup wait uncertain; retaining ownership");
            warned = Some(Instant::now());
        },
    }
    loop {
        match protected(|| child.try_wait()) {
            Ok(Some(_)) => return,
            Ok(None) => {},
            Err(error) => {
                if warned.is_none_or(|last: Instant| last.elapsed() >= Duration::from_secs(60)) {
                    tracing::warn!(%error, "daemon startup wait uncertain; retaining ownership");
                    warned = Some(Instant::now());
                }
            },
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub(super) async fn start(detached: bool, max_active: usize) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let home = super::super::common::surge_home_dir()?;
    let owner = RuntimeHomeOwner::prepare(&home)?;
    let socket = surge_daemon::pidfile::socket_path_in(&home);
    let mut command = Command::new(super::daemon_binary_path()?);
    command.arg("--max-active").arg(max_active.to_string());
    let mut logs = Vec::new();
    if detached {
        let directory = owner.directory(RuntimeDirectory::Daemon)?;
        let log = directory.open_append("daemon.log".as_ref())?;
        let (stdout, stdout_lease) = log.stdio_clone()?;
        let (stderr, stderr_lease) = log.stdio_clone()?;
        logs.extend([stdout_lease, stderr_lease]);
        command.stdin(Stdio::null()).stdout(stdout).stderr(stderr);
        command
            .arg("--detached")
            .creation_flags(0x0000_0008 | 0x0000_0200);
    }
    let mut startup = Startup::spawn(command, owner, logs)?;
    println!("started surge-daemon (pid {})", startup.child().id());
    if detached {
        println!("daemon log: {}", home.join("daemon/daemon.log").display());
    }
    let deadline = tokio::time::Instant::now() + super::DAEMON_READY_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!(
                "daemon at {} did not become ready within {}s",
                socket.display(),
                super::DAEMON_READY_TIMEOUT.as_secs()
            ));
        }
        if let Some(status) = startup.child().try_wait().context("check daemon startup")? {
            return Err(anyhow!("surge-daemon exited during startup ({status})"));
        }
        match tokio::time::timeout_at(deadline, child_listener_ready(&socket, startup.child()))
            .await
        {
            Ok(Ok(true)) => {
                startup.handoff()?;
                println!("daemon ready: {}", socket.display());
                return Ok(());
            },
            Ok(Ok(false)) => {},
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(anyhow!(
                    "daemon at {} did not become ready within {}s",
                    socket.display(),
                    super::DAEMON_READY_TIMEOUT.as_secs()
                ));
            },
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn child_listener_ready(socket: &Path, child: &mut Child) -> Result<bool> {
    use interprocess::local_socket::tokio::prelude::*;
    let name = surge_orchestrator::engine::ipc::local_socket_name_from_path(socket)?;
    let stream = match LocalSocketStream::connect(name).await {
        Ok(stream) => stream,
        Err(_) => return Ok(false),
    };
    let peer = stream
        .peer_creds()
        .context("read daemon listener peer identity")?
        .pid();
    if peer != Some(child.id()) {
        return Err(anyhow!(
            "daemon listener does not belong to the spawned child"
        ));
    }
    if child
        .try_wait()
        .context("verify daemon child remains alive")?
        .is_some()
    {
        return Err(anyhow!("daemon exited before readiness handoff"));
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
