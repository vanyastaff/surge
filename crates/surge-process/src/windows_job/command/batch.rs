//! Adapted from Rust's `library/std/src/sys/args/windows.rs` batch encoder.
//! Copyright The Rust Project Contributors. Licensed MIT OR Apache-2.0;
//! pinned algorithm: Rust 1.96.0. The repository's dual license applies.
//! https://github.com/rust-lang/rust/blob/1.96.0/library/std/src/sys/args/windows.rs
//! Only structured arguments are supported; no raw shell fragments are accepted.
use super::{invalid, wide};
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use windows::Win32::Storage::FileSystem::GetFullPathNameW;
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::core::PCWSTR;

pub(super) fn command_prompt() -> io::Result<PathBuf> {
    let mut buffer = vec![0_u16; 32768];
    // SAFETY: SDK receives a live UTF-16 output slice of sufficient native path size.
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length >= buffer.len() {
        return Err(invalid("system directory exceeds native limit"));
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])).join("cmd.exe"))
}

pub(super) fn command_line(program: &Path, args: &[OsString]) -> io::Result<Vec<u16>> {
    let script = user_path(program)?;
    let mut result: Vec<u16> = "cmd.exe /e:ON /v:OFF /d /c \"\"".encode_utf16().collect();
    if script.contains(&u16::from(b'"')) || script.last() == Some(&u16::from(b'\\')) {
        return Err(invalid(
            "batch file path contains quotes or a trailing separator",
        ));
    }
    result.extend(script);
    result.push(u16::from(b'"'));
    for arg in args {
        result.push(u16::from(b' '));
        append_arg(&mut result, arg)?;
    }
    result.extend([u16::from(b'"'), 0]);
    if result.len() > 32767 {
        return Err(invalid("batch command line exceeds native limit"));
    }
    Ok(result)
}

fn user_path(program: &Path) -> io::Result<Vec<u16>> {
    let units = wide(program.as_os_str())?;
    let units = &units[..units.len() - 1];
    let prefix: Vec<_> = r"\\?\".encode_utf16().collect();
    let unc: Vec<_> = r"\\?\UNC\".encode_utf16().collect();
    let result = if units.starts_with(&unc) {
        [
            r"\\".encode_utf16().collect::<Vec<_>>(),
            units[unc.len()..].to_vec(),
        ]
        .concat()
    } else if units.starts_with(&prefix) {
        units[prefix.len()..].to_vec()
    } else {
        units.to_vec()
    };
    if result.len() >= 260 && units.starts_with(&prefix) {
        return Err(invalid("batch executable needs a non-verbatim native path"));
    }
    if units.starts_with(&prefix) {
        // A verbatim path must not acquire DOS normalization semantics when cmd
        // receives it. Reject ambiguous components and require exact full-path
        // equivalence, following std's from_wide_to_user_path check.
        let separator = |unit: &u16| *unit == u16::from(b'\\') || *unit == u16::from(b'/');
        if result.split(separator).any(|part| {
            part.last()
                .is_some_and(|last| *last == u16::from(b'.') || *last == u16::from(b' '))
        }) {
            return Err(invalid("batch verbatim path has ambiguous DOS components"));
        }
        let mut terminated = result.clone();
        terminated.push(0);
        let mut normalized = vec![0_u16; 32768];
        // SAFETY: terminated input and output slice remain live for this query.
        let length =
            unsafe { GetFullPathNameW(PCWSTR(terminated.as_ptr()), Some(&mut normalized), None) }
                as usize;
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        if length >= normalized.len() || normalized[..length] != result {
            return Err(invalid(
                "batch verbatim path changes under DOS normalization",
            ));
        }
    }
    Ok(result)
}

fn append_arg(result: &mut Vec<u16>, value: &OsStr) -> io::Result<()> {
    let units = wide(value)?;
    let units = &units[..units.len() - 1];
    if units.contains(&u16::from(b'\r')) || units.contains(&u16::from(b'\n')) {
        return Err(invalid("batch file arguments may not contain line breaks"));
    }
    let quote = units.is_empty()
        || units.last() == Some(&u16::from(b'\\'))
        || char::decode_utf16(units.iter().copied())
            .filter_map(Result::ok)
            .any(|character| {
                (character.is_ascii()
                    && !(character.is_ascii_alphanumeric() || r"#$*+-./:?@\_".contains(character)))
                    || character.is_control()
            });
    if quote {
        result.push(u16::from(b'"'));
    }
    let mut slashes = 0;
    for &unit in units {
        if unit == u16::from(b'\\') {
            slashes += 1;
        } else {
            if unit == u16::from(b'"') {
                result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes));
                result.push(u16::from(b'"'));
            } else if unit == u16::from(b'%') {
                // Empty dynamic-variable substring prevents argument variable expansion.
                result.extend("%%cd:~,".encode_utf16());
            }
            slashes = 0;
        }
        result.push(unit);
    }
    if quote {
        result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes));
        result.push(u16::from(b'"'));
    }
    Ok(())
}
