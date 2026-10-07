//! Windows publication through the retained temporary object, without pathname reopening.
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::mem::{offset_of, size_of};
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
use std::path::Path;
use windows::Wdk::Storage::FileSystem::{
    FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS,
};
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_NORMAL, FILE_RENAME_INFO, FILE_SHARE_READ, FileRenameInfoEx,
    SetFileInformationByHandle,
};

// Rust drops struct fields in declaration order. Keep FILE FIRST: its close
// releases the delete-sharing fence before TempPath attempts failure cleanup.
// NamedTempFile declares path first, so retaining it would leak error temporaries
// while our exclusively retained file still denies DELETE sharing.
struct OwnedTemporary {
    file: std::fs::File,
    path: tempfile::TempPath,
}

pub(super) fn publish(parent: &Path, destination: &Path, contents: &[u8]) -> io::Result<()> {
    // READ sharing permits readers of the new published object before this
    // descriptor closes. Denying WRITE/DELETE sharing fences its identity,
    // content, and armed temporary pathname from competing opens.
    let temporary = tempfile::Builder::new()
        .prefix(".surge-config-")
        .make_in(parent, |path| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
                .share_mode(FILE_SHARE_READ.0)
                .attributes(FILE_ATTRIBUTE_NORMAL.0)
                .open(path)
        })?;
    let (file, path) = temporary.into_parts();
    let mut temporary = OwnedTemporary { file, path };
    temporary.file.write_all(contents)?;
    temporary.file.sync_all()?;
    rename_retained(&temporary.file, destination)?;
    // Publication moved the retained object. Disarm cleanup before any other
    // operation: dropping an armed old pathname could delete a replacement.
    temporary.path.disable_cleanup(true);
    drop(temporary);
    Ok(())
}

fn rename_retained(file: &std::fs::File, destination: &Path) -> io::Result<()> {
    let absolute = std::path::absolute(destination)?;
    let name: Vec<u16> = absolute.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "configuration destination contains NUL",
        ));
    }
    let name_bytes = name
        .len()
        .checked_mul(size_of::<u16>())
        .ok_or_else(invalid_length)?;
    let buffer_bytes = offset_of!(FILE_RENAME_INFO, FileName)
        .checked_add(name_bytes)
        .ok_or_else(invalid_length)?;
    let native_size = u32::try_from(buffer_bytes).map_err(|_| invalid_length())?;
    let name_size = u32::try_from(name_bytes).map_err(|_| invalid_length())?;
    let records = buffer_bytes.div_ceil(size_of::<FILE_RENAME_INFO>()).max(1);
    let initialized_bytes = records
        .checked_mul(size_of::<FILE_RENAME_INFO>())
        .ok_or_else(invalid_length)?;
    let mut storage = vec![FILE_RENAME_INFO::default(); records];
    // SAFETY: storage owns `records` SDK structs, establishing the SDK's native
    // alignment and initialized_bytes allocation size. All-zero representation
    // is valid for every SDK field (also used by its Default impl); zeroing the
    // full allocation initializes padding as well as the flexible filename tail.
    unsafe {
        std::ptr::write_bytes(storage.as_mut_ptr().cast::<u8>(), 0, initialized_bytes);
    }
    storage[0].Anonymous.Flags = FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS;
    storage[0].FileNameLength = name_size;
    // SAFETY: FileName's SDK offset is u16-aligned. Checked buffer_bytes includes
    // the complete name, and the rounded allocation is at least that size.
    // The source Vec is separate and alive; copy does not overlap. No terminator
    // is required: FileNameLength carries the exact byte count.
    unsafe {
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            storage
                .as_mut_ptr()
                .cast::<u8>()
                .add(offset_of!(FILE_RENAME_INFO, FileName))
                .cast::<u16>(),
            name.len(),
        );
    }
    // SAFETY: the borrowed File owns a valid handle for this call with DELETE
    // access. storage remains aligned, fully initialized and live for native_size
    // bytes through the synchronous call. Null RootDirectory plus an absolute
    // UTF-16 name supplies the full destination; named SDK flags alone replace
    // atomically without deleting the destination first or overriding readonly.
    unsafe {
        SetFileInformationByHandle(
            HANDLE(file.as_raw_handle()),
            FileRenameInfoEx,
            storage.as_ptr().cast(),
            native_size,
        )
    }
    .map_err(io::Error::other)
}

fn invalid_length() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "configuration destination exceeds native rename buffer limits",
    )
}
