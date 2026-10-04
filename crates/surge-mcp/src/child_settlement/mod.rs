//! Runtime-free actual-child owners; terminal admission cannot be reopened.
mod panic_boundary;
#[cfg(all(test, unix))]
mod tests;
mod tracker;
mod worker;

use crate::writer_observer::HostWriterObserver;
use rmcp::{
    RoleClient,
    transport::{Transport, async_rw::AsyncRwTransport},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
    mpsc,
};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

struct ChildParcel {
    child: std::process::Child,
    _owner: Option<Arc<dyn HostWriterObserver>>,
}
type ChildSlot = Arc<Mutex<Option<ChildParcel>>>;
type Pipes = (ChildStdout, ChildStdin, Option<ChildStderr>);

#[derive(Debug, thiserror::Error)]
pub(crate) enum ChildStartError {
    #[error("MCP child owner unavailable")]
    OwnerUnavailable,
    #[error("MCP execution control refuses child spawn")]
    EffectRefused,
    #[error("MCP child preparation failed")]
    Preparation,
}
impl From<std::io::Error> for ChildStartError {
    fn from(_error: std::io::Error) -> Self {
        Self::Preparation
    }
}

struct Reservation {
    send: mpsc::Sender<tracker::Start>,
    ticket: tracker::Ticket,
    child_spawned: bool,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.child_spawned {
            let _ = self.send.send(tracker::Start::NoChild);
        }
    }
}

pub(crate) struct ObservedTransport {
    inner: AsyncRwTransport<RoleClient, ChildStdout, ChildStdin>,
    ticket: tracker::Ticket,
    pid: u32,
}
impl ObservedTransport {
    pub(crate) fn spawn(
        mut command: tokio::process::Command,
        owner: Option<Arc<dyn HostWriterObserver>>,
        server: &str,
    ) -> Result<(Self, Option<ChildStderr>), ChildStartError> {
        let (send, ticket) = tracker::reserve().map_err(|_| ChildStartError::OwnerUnavailable)?;
        let mut reservation = Reservation {
            send,
            ticket,
            child_spawned: false,
        };
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // Reservation/reaping can wait. Admit the physical spawn only afterward.
        if let Some(observer) = &owner {
            observer
                .before_effect(server)
                .map_err(|_| ChildStartError::EffectRefused)?;
        }
        // Own the standard child before any fallible runtime pipe registration.
        let child = command.as_std_mut().spawn()?;
        reservation.child_spawned = true;
        let pid = child.id();
        let slot = Arc::new(Mutex::new(Some(ChildParcel {
            child,
            _owner: owner,
        })));
        let Ok(pipes) = pipes(&slot) else {
            fallback(&slot, &reservation);
            return Err(ChildStartError::Preparation);
        };
        let (taken_send, taken_receive) = mpsc::channel();
        let transfer = panic_boundary::protected(|| {
            reservation
                .send
                .send(tracker::Start::Child(slot.clone(), taken_send))
        });
        if transfer.is_err() || panic_boundary::protected(|| taken_receive.recv()).is_err() {
            fallback(&slot, &reservation);
            return Err(ChildStartError::Preparation);
        }
        let ticket = tracker::Ticket {
            control: reservation.ticket.control.clone(),
            completion: reservation.ticket.completion.clone(),
        };
        let (stdout, stdin, stderr) = pipes;
        Ok((
            Self {
                inner: AsyncRwTransport::new(stdout, stdin),
                ticket,
                pid,
            },
            stderr,
        ))
    }
    pub(crate) fn id(&self) -> u32 {
        self.pid
    }
    pub(crate) async fn settle(&self, graceful: bool) {
        self.ticket.request(graceful);
        self.ticket.completion.notified().await;
    }
}

fn pipes(slot: &ChildSlot) -> std::io::Result<Pipes> {
    panic_boundary::protected(|| {
        let mut owner = tracker::lock(slot);
        let Some(parcel) = owner.as_mut() else {
            std::process::abort();
        };
        let stdin = parcel
            .child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("mcp_child_stdin_missing"))?;
        let stdout = parcel
            .child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("mcp_child_stdout_missing"))?;
        let stderr = parcel.child.stderr.take();
        Ok((
            ChildStdout::from_std(stdout)?,
            ChildStdin::from_std(stdin)?,
            stderr.map(ChildStderr::from_std).transpose()?,
        ))
    })
}
fn fallback(slot: &ChildSlot, reservation: &Reservation) {
    let mut owned = panic_boundary::protected(|| tracker::lock(slot).take());
    panic_boundary::protected(|| {
        if owned.is_some() {
            worker::settle_owned(&mut owned, &AtomicU8::new(tracker::IMMEDIATE));
            if reservation.send.send(tracker::Start::Settled).is_err() {
                // Receiver failure never replaces observed-exit settlement.
                reservation.ticket.completion.finish();
            }
        } else {
            reservation
                .ticket
                .control
                .store(tracker::IMMEDIATE, Ordering::SeqCst);
            reservation.ticket.completion.wait();
        }
    });
}
impl Drop for ObservedTransport {
    fn drop(&mut self) {
        self.ticket.request(false);
    }
}
impl Transport<RoleClient> for ObservedTransport {
    type Error = std::io::Error;
    fn send(
        &mut self,
        item: rmcp::service::TxJsonRpcMessage<RoleClient>,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(item)
    }
    fn receive(
        &mut self,
    ) -> impl std::future::Future<Output = Option<rmcp::service::RxJsonRpcMessage<RoleClient>>> + Send
    {
        self.inner.receive()
    }
    fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        self.ticket.request(true);
        let completion = self.ticket.completion.clone();
        let close = self.inner.close();
        async move {
            let result = close.await;
            completion.notified().await;
            result
        }
    }
}

/// Permanently close child admission, settle tracked leaders and join their owners.
///
/// Call after consuming the host runtime, outside any cleanup worker or held
/// ownership mutex. This can block on unknown child exit. It does not prove
/// descendant containment. Hosts must install their panic hook before the first
/// MCP connection and must not replace the protected dispatch hook afterward.
pub fn shutdown_children_and_join() {
    tracker::shutdown();
}
