//! Atomic Windows process launch into an authenticated, retained job.
//!
//! The job handle is never inherited and breakaway is never enabled. A native
//! job-list creation attribute installs containment before the child executes.
//! Missing or mismatched recovery evidence must remain unknown at the caller.
mod command;
mod pipes;
mod security;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus};
use windows::Win32::Foundation::{
    BOOL, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INVALID_PARAMETER, FILETIME,
    GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
};
use windows::Win32::System::JobObjects::{
    CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation, OpenJobObjectW,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::SystemServices::{JOB_OBJECT_QUERY, JOB_OBJECT_TERMINATE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows::core::PCWSTR;

const JOB_PREFIX: &str = "Global\\SurgeMcp-";

/// Complete native launch input. The environment is explicit, never inherited.
///
/// Batch programs use the standard library's protected command-line encoding;
/// NUL and batch arguments containing line breaks are rejected before launch.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Program path or executable name resolved using the supplied environment.
    pub program: OsString,
    /// Structured arguments, preserving empty values and Windows UTF-16.
    pub args: Vec<OsString>,
    /// Complete child environment, including any required `SystemRoot`/`PATH`.
    pub environment: Vec<(OsString, OsString)>,
    /// Child working directory; absence uses the host's current directory.
    pub current_dir: Option<PathBuf>,
}

/// A job handle whose owner and protected DACL match the process-owner SID.
pub struct Job {
    handle: OwnedHandle,
    name: String,
}

impl Job {
    fn fresh() -> io::Result<Self> {
        let mut nonce = [0_u8; 32];
        // SAFETY: the OS RNG writes exactly the provided live output slice.
        unsafe {
            BCryptGenRandom(
                BCRYPT_ALG_HANDLE::default(),
                &mut nonce,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
            .ok()
            .map_err(win_error)?;
        }
        let name = format!(
            "{JOB_PREFIX}{}",
            nonce.map(|byte| format!("{byte:02x}")).concat()
        );
        Self::create_fresh(name)
    }

    fn create_fresh(name: String) -> io::Result<Self> {
        validate_name(&name)?;
        let security = security::Descriptor::current_user()?;
        let name_wide = command::wide(std::ffi::OsStr::new(&name))?;
        let attributes = security.attributes(false);
        // SAFETY: descriptor and terminated name remain live through creation.
        let raw = unsafe { CreateJobObjectW(Some(&attributes), PCWSTR(name_wide.as_ptr())) }
            .map_err(win_error)?;
        // GetLastError must be read immediately, before any other Windows call.
        let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let handle = owned(raw)?;
        if existing {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "MCP job name collision",
            ));
        }
        let job = Self { handle, name };
        security::verify(job.raw())?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: valid retained job and correctly sized SDK information buffer.
        unsafe {
            SetInformationJobObject(
                job.raw(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_u32::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()?,
            )
        }
        .map_err(win_error)?;
        Ok(job)
    }

    /// Open a captured job, validating its name, owner and protected DACL.
    ///
    /// Returns `None` only when the kernel reports `ERROR_FILE_NOT_FOUND`.
    /// # Errors
    /// Rejects invalid names, inaccessible jobs and untrusted security descriptors.
    pub fn open(name: &str) -> io::Result<Option<Self>> {
        validate_name(name)?;
        let name_wide = command::wide(std::ffi::OsStr::new(name))?;
        // SAFETY: terminated name is live; returned handle is non-inheritable.
        let result = unsafe {
            OpenJobObjectW(
                JOB_OBJECT_QUERY | JOB_OBJECT_TERMINATE | 0x0002_0000,
                false,
                PCWSTR(name_wide.as_ptr()),
            )
        };
        let raw = match result {
            Ok(raw) => raw,
            Err(error)
                if error.code() == windows::core::HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) =>
            {
                return Ok(None);
            },
            Err(error) => return Err(win_error(error)),
        };
        let job = Self {
            handle: owned(raw)?,
            name: name.to_owned(),
        };
        security::verify(job.raw())?;
        Ok(Some(job))
    }

    /// Count actual processes still associated with this retained job.
    /// # Errors
    /// Returns native query errors; callers must not infer emptiness from errors.
    pub fn active_processes(&self) -> io::Result<u32> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: SDK buffer is valid for the specified class and exact size.
        unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast(),
                size_u32::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>()?,
                None,
            )
        }
        .map_err(win_error)?;
        Ok(accounting.ActiveProcesses)
    }

    /// Match a live process creation identity and membership using retained handles.
    /// # Errors
    /// Returns process access, identity query or membership query failures.
    pub fn contains_process(&self, pid: u32, creation_filetime: u64) -> io::Result<bool> {
        // SAFETY: opening does not mutate the process; ownership is transferred once.
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        };
        let process = match process {
            Ok(handle) => owned(handle)?,
            Err(error)
                if error.code()
                    == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
            {
                return Ok(false);
            },
            Err(error) => return Err(win_error(error)),
        };
        let process = raw(&process);
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: retained process handle and live SDK output buffers.
        unsafe {
            GetProcessTimes(
                process,
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        }
        .map_err(win_error)?;
        let observed =
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        if observed != creation_filetime || wait_state(process)? {
            return Ok(false);
        }
        let mut member = BOOL::default();
        // SAFETY: both process and job handles remain retained during the query.
        unsafe { IsProcessInJob(process, self.raw(), &raw mut member) }.map_err(win_error)?;
        Ok(member.as_bool())
    }

    /// Request termination of every process in this authenticated job.
    /// # Errors
    /// Returns the native termination error; success is a request, not settled exit.
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: handle identifies the retained, authenticated job, never a PID.
        unsafe { TerminateJobObject(self.raw(), 1) }.map_err(win_error)
    }

    fn raw(&self) -> HANDLE {
        raw(&self.handle)
    }
}

/// Native child whose original process handle and containing job remain owned.
pub struct NativeChild {
    process: OwnedHandle,
    job: Job,
    pid: u32,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
}

impl NativeChild {
    /// Launch atomically into a fresh job, with only three explicit stdio handles inherited.
    /// # Errors
    /// Rejects invalid command/environment input and all failed native preparation or launch.
    pub fn spawn(spec: &LaunchSpec) -> io::Result<Self> {
        let job = Job::fresh()?;
        let pipes = pipes::Pipes::new(&job.name)?;
        let spawned = command::spawn(spec, job.raw(), &pipes)?;
        let (stdin, stdout, stderr) = pipes.into_parent();
        Ok(Self {
            process: spawned.process,
            pid: spawned.pid,
            job,
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr: Some(stderr),
        })
    }

    /// Actual process identifier; recovery must additionally validate creation identity.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.pid
    }
    /// Captured random job name, suitable for validated durable recovery evidence.
    #[must_use]
    pub fn job_name(&self) -> &str {
        &self.job.name
    }
    /// Transfer the parent's asynchronous stdin pipe once.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }
    /// Transfer the parent's asynchronous stdout pipe once.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }
    /// Transfer the parent's asynchronous stderr pipe once.
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.stderr.take()
    }
    /// Request termination of the whole retained job.
    /// # Errors
    /// Returns native termination failures, without claiming completion.
    pub fn kill(&mut self) -> io::Result<()> {
        self.job.terminate()
    }
    /// Observe direct process exit and actual job emptiness before returning its status.
    ///
    /// Leader exit initiates descendant termination. Query errors preserve ownership.
    /// # Errors
    /// Returns native wait, exit-code, termination or occupancy-query errors.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if !wait_state(raw(&self.process))? {
            return Ok(None);
        }
        if self.job.active_processes()? != 0 {
            self.job.terminate()?;
            return Ok(None);
        }
        let mut status = 0;
        // SAFETY: original process handle is retained through the exit-code query.
        unsafe { GetExitCodeProcess(raw(&self.process), &raw mut status) }.map_err(win_error)?;
        Ok(Some(ExitStatus::from_raw(status)))
    }
}

fn validate_name(name: &str) -> io::Result<()> {
    let Some(nonce) = name.strip_prefix(JOB_PREFIX) else {
        return Err(invalid("invalid MCP job prefix"));
    };
    if nonce.len() != 64
        || nonce.bytes().all(|byte| byte == b'0')
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("invalid MCP job nonce"));
    }
    Ok(())
}

fn wait_state(handle: HANDLE) -> io::Result<bool> {
    // SAFETY: callers retain the original process handle for this zero-time query.
    match unsafe { WaitForSingleObject(handle, 0) } {
        WAIT_OBJECT_0 => Ok(true),
        WAIT_TIMEOUT => Ok(false),
        _ => Err(io::Error::last_os_error()),
    }
}
fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}
fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful native creation/open transfers one fresh handle here.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}
fn size_u32<T>() -> io::Result<u32> {
    u32::try_from(std::mem::size_of::<T>()).map_err(|_| invalid("SDK buffer exceeds DWORD"))
}
fn win_error(error: windows::core::Error) -> io::Error {
    let bits = error.code().0.to_ne_bytes();
    let unsigned = u32::from_ne_bytes(bits);
    if unsigned & 0x7fff_0000 == 0x0007_0000 {
        return io::Error::from_raw_os_error(i32::from(u16::from_ne_bytes([bits[0], bits[1]])));
    }
    io::Error::other(error)
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
