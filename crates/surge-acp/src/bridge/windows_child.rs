//! Native job ownership behind the bridge's asynchronous child lifecycle.
use std::io;
use std::process::ExitStatus;
use std::time::Duration;
use surge_core::execution_recovery::process::{WriterContainer, WriterCoverage};
use surge_process::windows_job::{LaunchSpec, NativeChild};

type Pipes = (
    tokio::process::ChildStdin,
    tokio::process::ChildStdout,
    tokio::process::ChildStderr,
);

/// Original process/job handles remain owned while wait futures are cancelled.
pub(crate) struct WindowsChild {
    native: NativeChild,
}

impl WindowsChild {
    pub(super) fn spawn(
        launch: &LaunchSpec,
        admit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<Self> {
        NativeChild::spawn_admitted(launch, admit).map(|native| Self { native })
    }

    pub(super) fn take_pipes(&mut self) -> io::Result<Pipes> {
        // Conversion may fail after any transfer. The actual child/job stay in
        // self and the worker kills and awaits them before surfacing that error.
        let stdin = self
            .native
            .take_stdin()
            .ok_or_else(|| io::Error::other("agent stdin unavailable"))?;
        let stdout = self
            .native
            .take_stdout()
            .ok_or_else(|| io::Error::other("agent stdout unavailable"))?;
        let stderr = self
            .native
            .take_stderr()
            .ok_or_else(|| io::Error::other("agent stderr unavailable"))?;
        Ok((
            tokio::process::ChildStdin::from_std(stdin)?,
            tokio::process::ChildStdout::from_std(stdout)?,
            tokio::process::ChildStderr::from_std(stderr)?,
        ))
    }

    pub(super) fn container(
        &self,
    ) -> Result<WriterContainer, crate::process_evidence::ObservationError> {
        crate::process_evidence::observe_named_container(
            self.native.id(),
            self.native.job_name(),
            WriterCoverage::GroupOnly,
        )
    }

    pub(super) fn start_kill(&mut self) -> io::Result<()> {
        self.native.kill()
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.native.try_wait()
    }

    pub(super) async fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
