//! Platform-local endpoints; no TCP listener or remote pipe access.

#[cfg(unix)]
pub(super) type Stream = tokio::net::UnixStream;
#[cfg(windows)]
pub(super) type Stream = tokio::net::windows::named_pipe::NamedPipeServer;

#[cfg(unix)]
pub(super) struct Listener {
    socket: tokio::net::UnixListener,
    directory: std::path::PathBuf,
    address: String,
}

#[cfg(unix)]
impl Listener {
    pub fn bind() -> std::io::Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        // Short path also works with macOS's 104-byte sockaddr_un limit.
        let name = format!("sgm-{:016x}", rand::random::<u64>());
        let directory = std::path::Path::new("/tmp").join(name);
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let path = directory.join("s");
        let listener = match tokio::net::UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = std::fs::remove_dir(&directory);
                return Err(error);
            },
        };
        if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        {
            drop(listener);
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_dir(&directory);
            return Err(error);
        }
        Ok(Self {
            socket: listener,
            directory,
            address: path.to_string_lossy().into_owned(),
        })
    }
    pub fn address(&self) -> &str {
        &self.address
    }
    pub async fn accept(&self) -> std::io::Result<Stream> {
        self.socket.accept().await.map(|(stream, _)| stream)
    }
}

#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.address);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[cfg(windows)]
pub(super) struct Listener {
    next: tokio::sync::Mutex<Stream>,
    address: String,
}

#[cfg(windows)]
impl Listener {
    pub fn bind() -> std::io::Result<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let address = format!(r"\\.\pipe\surge-stage-{:032x}", rand::random::<u128>());
        let next = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&address)?;
        Ok(Self {
            next: tokio::sync::Mutex::new(next),
            address,
        })
    }
    pub fn address(&self) -> &str {
        &self.address
    }
    pub async fn accept(&self) -> std::io::Result<Stream> {
        use tokio::net::windows::named_pipe::ServerOptions;
        let mut next = self.next.lock().await;
        next.connect().await?;
        let replacement = ServerOptions::new()
            .reject_remote_clients(true)
            .create(&self.address)?;
        Ok(std::mem::replace(&mut *next, replacement))
    }
}

#[cfg(unix)]
pub(super) async fn connect(address: &str) -> std::io::Result<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(address).await
}

#[cfg(windows)]
pub(super) async fn connect(
    address: &str,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    open_with_busy_retry(
        || tokio::net::windows::named_pipe::ClientOptions::new().open(address),
        std::time::Duration::from_secs(2),
    )
    .await
}

#[cfg(any(windows, test))]
async fn open_with_busy_retry<T>(
    mut open: impl FnMut() -> std::io::Result<T>,
    budget: std::time::Duration,
) -> std::io::Result<T> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        match open() {
            Ok(value) => return Ok(value),
            Err(error) if error.raw_os_error() == Some(231) => {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "stage pipe remained busy",
                    ));
                }
                tokio::time::sleep(remaining.min(std::time::Duration::from_millis(10))).await;
            },
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod retry_tests {
    use super::open_with_busy_retry;
    use std::{cell::Cell, time::Duration};

    #[tokio::test]
    async fn busy_pipe_retries_then_connects() {
        let attempts = Cell::new(0);
        let value = open_with_busy_retry(
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() < 3 {
                    Err(std::io::Error::from_raw_os_error(231))
                } else {
                    Ok(42)
                }
            },
            Duration::from_millis(100),
        )
        .await
        .unwrap();
        assert_eq!(value, 42);
        assert_eq!(attempts.get(), 3);
    }

    #[tokio::test]
    async fn busy_pipe_has_elapsed_deadline() {
        let start = tokio::time::Instant::now();
        let error = open_with_busy_retry(
            || Err::<(), _>(std::io::Error::from_raw_os_error(231)),
            Duration::from_millis(20),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() >= Duration::from_millis(20));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn other_pipe_errors_fail_without_retry() {
        let attempts = Cell::new(0);
        let error = open_with_busy_retry(
            || {
                attempts.set(attempts.get() + 1);
                Err::<(), _>(std::io::Error::from_raw_os_error(5))
            },
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
        assert_eq!(attempts.get(), 1);
    }
    #[tokio::test]
    async fn pending_busy_retry_is_cancellable() {
        let attempts = Cell::new(0);
        let result = tokio::time::timeout(
            Duration::from_millis(1),
            open_with_busy_retry(
                || {
                    attempts.set(attempts.get() + 1);
                    Err::<(), _>(std::io::Error::from_raw_os_error(231))
                },
                Duration::from_secs(2),
            ),
        )
        .await;
        assert!(result.is_err());
        let stopped = attempts.get();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(attempts.get(), stopped);
    }
}
