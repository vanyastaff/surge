//! Structured UTF-16 command lines and atomic job-list process creation.
use super::{LaunchSpec, invalid, owned, pipes::Pipes, win_error};
use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Globalization::{
    CSTR_EQUAL, CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringOrdinal,
};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
};
use windows::core::{PCWSTR, PWSTR};

impl LaunchSpec {
    /// Snapshot inherited host environment, then apply a command's overrides/removals.
    ///
    /// The caller must know that this command uses environment inheritance.
    /// `std::process::Command` cannot reveal an earlier `env_clear`; commands with
    /// complete explicit environments must construct `LaunchSpec` directly.
    /// Windows' own command environment map handles case-insensitive key aliases.
    /// # Errors
    /// Rejects native command/environment inputs containing NUL or invalid keys.
    pub fn from_inherited_command(command: &std::process::Command) -> io::Result<Self> {
        let mut snapshot = std::process::Command::new(command.get_program());
        snapshot.env_clear().envs(std::env::vars_os());
        for (key, value) in command.get_envs() {
            if let Some(value) = value {
                snapshot.env(key, value);
            } else {
                snapshot.env_remove(key);
            }
        }
        let child_environment = snapshot
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect::<Vec<_>>();
        wide(command.get_program())?;
        for arg in command.get_args() {
            wide(arg)?;
        }
        environment(&child_environment)?;
        Ok(Self {
            program: command.get_program().to_owned(),
            args: command.get_args().map(OsStr::to_owned).collect(),
            environment: child_environment,
            current_dir: command.get_current_dir().map(Path::to_owned),
        })
    }
}

#[path = "command/batch.rs"]
mod batch;

pub(super) struct Spawned {
    pub(super) process: OwnedHandle,
    pub(super) pid: u32,
}

struct Attributes {
    storage: Vec<usize>,
}
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: first discovery call receives only the required allocation size.
        let _ = unsafe {
            InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST::default(),
                2,
                0,
                &raw mut size,
            )
        };
        if size == 0 || size > 65536 {
            return Err(invalid("unexpected process attribute size"));
        }
        let mut storage = vec![0; size.div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: aligned allocation has the exact size requested by the SDK.
        unsafe {
            InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast()),
                2,
                0,
                &raw mut size,
            )
        }
        .map_err(win_error)?;
        Ok(Self { storage })
    }
    fn raw(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        LPPROC_THREAD_ATTRIBUTE_LIST(self.storage.as_mut_ptr().cast())
    }
    fn handles(&mut self, key: u32, handles: &[HANDLE]) -> io::Result<()> {
        // SAFETY: caller keeps this slice live until after CreateProcess and attribute deletion.
        unsafe {
            UpdateProcThreadAttribute(
                self.raw(),
                0,
                key as usize,
                Some(handles.as_ptr().cast()),
                std::mem::size_of_val(handles),
                None,
                None,
            )
        }
        .map_err(win_error)
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: only successfully initialized lists reach this destructor.
        unsafe { DeleteProcThreadAttributeList(self.raw()) };
    }
}

pub(super) fn spawn(
    spec: &LaunchSpec,
    job: HANDLE,
    pipes: &Pipes,
    admit: impl FnOnce() -> io::Result<()>,
) -> io::Result<Spawned> {
    let program = resolve(spec)?;
    let (executable, mut command_line) = if is_batch(&program) {
        (
            batch::command_prompt()?,
            batch::command_line(&program, &spec.args)?,
        )
    } else {
        (
            program.clone(),
            command_line(program.as_os_str(), &spec.args)?,
        )
    };
    let executable_wide = wide(executable.as_os_str())?;
    let environment = environment(&spec.environment)?;
    let current_dir = spec
        .current_dir
        .as_deref()
        .map(|path| wide(path.as_os_str()))
        .transpose()?;
    let job_handles = [job];
    let std_handles = pipes.child_handles();
    // Values outlive attribute list destruction; only these handles are inherited.
    let mut attributes = Attributes::new()?;
    attributes.handles(PROC_THREAD_ATTRIBUTE_JOB_LIST, &job_handles)?;
    attributes.handles(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &std_handles)?;
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = super::size_u32::<STARTUPINFOEXW>()?;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = std_handles[0];
    startup.StartupInfo.hStdOutput = std_handles[1];
    startup.StartupInfo.hStdError = std_handles[2];
    startup.lpAttributeList = attributes.raw();
    let mut process = PROCESS_INFORMATION::default();
    admit()?;
    // SAFETY: every input allocation and attribute value remains live through this
    // call. JOB_LIST assignment is atomic with creation; no child runs uncontained.
    unsafe {
        CreateProcessW(
            PCWSTR(executable_wide.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            true,
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            Some(environment.as_ptr().cast()),
            PCWSTR(
                current_dir
                    .as_ref()
                    .map_or(std::ptr::null(), |path| path.as_ptr()),
            ),
            &raw const startup.StartupInfo,
            &raw mut process,
        )
    }
    .map_err(win_error)?;
    // Both successful handles are immediately owned. Job ownership in the caller
    // kills all assigned processes if any subsequent preparation fails.
    let process_handle = owned(process.hProcess)?;
    let _thread = owned(process.hThread)?;
    Ok(Spawned {
        process: process_handle,
        pid: process.dwProcessId,
    })
}

pub(super) fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut units: Vec<_> = value.encode_wide().collect();
    if units.contains(&0) {
        return Err(invalid("NUL in Windows process input"));
    }
    units.push(0);
    Ok(units)
}

fn command_line(program: &OsStr, args: &[OsString]) -> io::Result<Vec<u16>> {
    let mut result = quoted(program)?;
    for arg in args {
        result.push(u16::from(b' '));
        result.extend(quoted(arg)?);
    }
    result.push(0);
    if result.len() > 32767 {
        return Err(invalid("Windows command line exceeds native limit"));
    }
    Ok(result)
}

fn quoted(value: &OsStr) -> io::Result<Vec<u16>> {
    let units = wide(value)?;
    let mut result = vec![u16::from(b'"')];
    let mut slashes = 0;
    for &unit in &units[..units.len() - 1] {
        if unit == u16::from(b'\\') {
            slashes += 1;
            continue;
        }
        if unit == u16::from(b'"') {
            result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes));
        }
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
    result.push(u16::from(b'"'));
    Ok(result)
}

fn compare(left: &OsStr, right: &OsStr) -> io::Result<Ordering> {
    let left: Vec<_> = left.encode_wide().collect();
    let right: Vec<_> = right.encode_wide().collect();
    // SAFETY: slices describe complete live UTF-16 inputs with explicit lengths.
    match unsafe { CompareStringOrdinal(&left, &right, true) } {
        CSTR_LESS_THAN => Ok(Ordering::Less),
        CSTR_EQUAL => Ok(Ordering::Equal),
        CSTR_GREATER_THAN => Ok(Ordering::Greater),
        _ => Err(io::Error::last_os_error()),
    }
}

fn environment(values: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
    let mut sorted: Vec<&(OsString, OsString)> = Vec::new();
    for pair in values {
        let key = wide(&pair.0)?;
        if key.len() <= 1 || key[..key.len() - 1].contains(&u16::from(b'=')) {
            return Err(invalid("invalid Windows environment key"));
        }
        wide(&pair.1)?;
        let mut position = sorted.len();
        for (index, old) in sorted.iter().enumerate() {
            match compare(&pair.0, &old.0)? {
                Ordering::Less => {
                    position = index;
                    break;
                },
                Ordering::Equal => {
                    return Err(invalid("duplicate Windows environment key"));
                },
                Ordering::Greater => {},
            }
        }
        sorted.insert(position, pair);
    }
    let mut block = Vec::new();
    for (key, value) in sorted {
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(wide(value)?);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn env_value<'a>(spec: &'a LaunchSpec, name: &str) -> io::Result<Option<&'a OsStr>> {
    for (key, value) in &spec.environment {
        if compare(key, OsStr::new(name))? == Ordering::Equal {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn resolve(spec: &LaunchSpec) -> io::Result<PathBuf> {
    wide(&spec.program)?;
    if spec.program.is_empty() {
        return Err(invalid("empty Windows program"));
    }
    let program = Path::new(&spec.program);
    let cwd = spec
        .current_dir
        .clone()
        .map_or_else(std::env::current_dir, Ok)?;
    let cwd = if cwd.is_absolute() {
        cwd
    } else {
        std::env::current_dir()?.join(cwd)
    };
    let mut candidates = vec![cwd.join(program)];
    if program.is_absolute() {
        candidates = vec![program.to_owned()];
    } else if program.components().count() == 1
        && let Some(paths) = env_value(spec, "PATH")?
    {
        candidates.extend(
            std::env::split_paths(paths)
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| {
                    if path.is_absolute() {
                        path.join(program)
                    } else {
                        cwd.join(path).join(program)
                    }
                }),
        );
    }
    let extensions = env_value(spec, "PATHEXT")?
        .map_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"), OsStr::to_owned);
    let extensions: Vec<_> = extensions
        .to_string_lossy()
        .split(';')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    for candidate in candidates {
        if candidate.extension().is_some() && candidate.is_file() {
            return Ok(candidate);
        }
        if candidate.extension().is_none() {
            if let Some(executable) = with_extension(&candidate, &extensions) {
                return Ok(executable);
            }
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Windows executable absent from configured PATH",
    ))
}

fn with_extension(candidate: &Path, extensions: &[String]) -> Option<PathBuf> {
    extensions
        .iter()
        .filter_map(|extension| extension.strip_prefix('.'))
        .filter(|extension| !extension.is_empty() && extension.chars().all(char::is_alphanumeric))
        .map(|extension| candidate.with_extension(extension))
        .find(|path| path.is_file())
}

fn is_batch(program: &Path) -> bool {
    program.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt_encoding_keeps_empty_and_trailing_slashes() {
        let line = command_line(
            OsStr::new("fixture.exe"),
            &["".into(), "slash\\".into(), "a\"b".into()],
        )
        .unwrap();
        assert_eq!(
            String::from_utf16(&line[..line.len() - 1]).unwrap(),
            "\"fixture.exe\" \"\" \"slash\\\\\" \"a\\\"b\""
        );
    }

    #[test]
    fn explicit_environment_is_sorted_and_rejects_alias_duplicates_and_nuls() {
        let block =
            environment(&[("z".into(), "last".into()), ("A".into(), "first".into())]).unwrap();
        assert_eq!(String::from_utf16(&block).unwrap(), "A=first\0z=last\0\0");
        assert!(environment(&[("PATH".into(), "a".into()), ("Path".into(), "b".into())]).is_err());
        assert!(environment(&[("a=b".into(), "value".into())]).is_err());
        assert!(environment(&[("a".into(), "nul\0value".into())]).is_err());
        assert_eq!(environment(&[]).unwrap(), vec![0, 0]);
    }
}
