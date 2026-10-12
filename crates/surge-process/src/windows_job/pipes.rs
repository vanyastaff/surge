//! Overlapped parent pipe handles, synchronous inherited child handles.
use super::{command, owned, raw, security, win_error};
use std::io;
use std::os::windows::io::OwnedHandle;
use std::process::{ChildStderr, ChildStdin, ChildStdout};
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
    FILE_SHARE_MODE, OPEN_EXISTING, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND,
};
use windows::Win32::System::Pipes::{
    CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::core::PCWSTR;

struct Pair {
    parent: OwnedHandle,
    child: OwnedHandle,
}

pub(super) struct Pipes {
    stdin: Pair,
    stdout: Pair,
    stderr: Pair,
}

impl Pipes {
    pub(super) fn new(job_name: &str) -> io::Result<Self> {
        Ok(Self {
            stdin: pair(job_name, "stdin", true)?,
            stdout: pair(job_name, "stdout", false)?,
            stderr: pair(job_name, "stderr", false)?,
        })
    }
    pub(super) fn child_handles(&self) -> [HANDLE; 3] {
        [
            raw(&self.stdin.child),
            raw(&self.stdout.child),
            raw(&self.stderr.child),
        ]
    }
    pub(super) fn into_parent(self) -> (ChildStdin, ChildStdout, ChildStderr) {
        // Parent handles are asynchronous as required by std's conversion contract.
        (
            self.stdin.parent.into(),
            self.stdout.parent.into(),
            self.stderr.parent.into(),
        )
    }
}

fn pair(job: &str, stream: &str, input: bool) -> io::Result<Pair> {
    let name = format!(r"\\.\pipe\{}-{stream}", job.replace('\\', "-"));
    let name = command::wide(std::ffi::OsStr::new(&name))?;
    let descriptor = security::Descriptor::pipe()?;
    let parent_attributes = descriptor.attributes(false);
    let mode = if input {
        PIPE_ACCESS_OUTBOUND
    } else {
        PIPE_ACCESS_INBOUND
    };
    // SAFETY: terminated unique name and descriptor are live; only parent is overlapped.
    let parent = owned(unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            mode | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            0,
            Some(&parent_attributes),
        )
    })?;
    let child_attributes = descriptor.attributes(true);
    // SAFETY: the freshly created pipe remains retained; child endpoint is synchronous.
    let child = owned(
        unsafe {
            CreateFileW(
                PCWSTR(name.as_ptr()),
                if input {
                    GENERIC_READ.0
                } else {
                    GENERIC_WRITE.0
                },
                FILE_SHARE_MODE(0),
                Some(&child_attributes),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                HANDLE::default(),
            )
        }
        .map_err(win_error)?,
    )?;
    Ok(Pair { parent, child })
}
