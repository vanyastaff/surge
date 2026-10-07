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
    DELETE, FILE_ATTRIBUTE_NORMAL, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX,
    FILE_DISPOSITION_INFO_EX_FLAGS, FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FileDispositionInfoEx, FileRenameInfoEx, SetFileInformationByHandle,
};

// Cleanup belongs to this retained object, never to its replaceable pathname.
struct OwnedTemporary {
    file: std::fs::File,
    state: TemporaryState,
}

#[derive(Clone, Copy)]
enum TemporaryState {
    Unpublished,
    CleanupAttempted,
    Published,
}

impl OwnedTemporary {
    fn create(parent: &Path) -> io::Result<Self> {
        let temporary =
            tempfile::Builder::new()
                .prefix(".surge-config-")
                .make_in(parent, |path| {
                    OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
                    // Allow atomic replacement even before our publisher closes;
                    // deny competing content writers throughout our ownership.
                    .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0)
                    .attributes(FILE_ATTRIBUTE_NORMAL.0)
                    .open(path)
                })?;
        let (file, mut path) = temporary.into_parts();
        // DELETE sharing permits another actor to replace the pathname. Disarm
        // pathname cleanup immediately, before any fallible operation.
        path.disable_cleanup(true);
        Ok(Self {
            file,
            state: TemporaryState::Unpublished,
        })
    }

    fn cleanup(&mut self) -> io::Result<()> {
        // Latch before the call: failure must remain observable, never retried
        // implicitly by Drop (which could obscure the original failure).
        self.state = TemporaryState::CleanupAttempted;
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_INFO_EX_FLAGS(
                FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0,
            ),
        };
        // SAFETY: File retains the original valid DELETE-capable handle. The SDK
        // struct is initialized, naturally aligned and alive for this synchronous
        // call. No pathname lookup occurs; only our unpublished object is marked
        // for removal when this handle closes. Readonly protection is preserved.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(self.file.as_raw_handle()),
                FileDispositionInfoEx,
                std::ptr::from_ref(&disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        }
        .map_err(io::Error::other)
    }

    fn abort(&mut self, original: io::Error) -> io::Error {
        match self.cleanup() {
            Ok(()) => original,
            Err(cleanup) => io::Error::new(
                original.kind(),
                format!(
                    "configuration publication failed: {original}; retained temporary cleanup failed: {cleanup}"
                ),
            ),
        }
    }
}

impl Drop for OwnedTemporary {
    fn drop(&mut self) {
        if matches!(self.state, TemporaryState::Unpublished)
            && let Err(error) = self.cleanup()
        {
            tracing::error!(%error, "failed to clean unpublished configuration object during unwinding");
        }
        // File closes after Drop, completing descriptor-based POSIX deletion.
    }
}

pub(super) fn publish(parent: &Path, destination: &Path, contents: &[u8]) -> io::Result<()> {
    let mut temporary = OwnedTemporary::create(parent)?;
    let result = temporary
        .file
        .write_all(contents)
        .and_then(|()| temporary.file.sync_all())
        .and_then(|()| rename_retained(&temporary.file, destination));
    match result {
        Ok(()) => {
            // The very next operation disarms deletion of the published object.
            temporary.state = TemporaryState::Published;
            Ok(())
        },
        Err(error) => Err(temporary.abort(error)),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_follows_owned_object_and_preserves_replaced_temporary_name() {
        let directory = tempfile::tempdir().unwrap();
        let mut temporary = OwnedTemporary::create(directory.path()).unwrap();
        let original_name = std::fs::read_dir(directory.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let moved_name = directory.path().join("moved-owned-object");
        rename_retained(&temporary.file, &moved_name).unwrap();
        std::fs::write(&original_name, b"unowned sentinel").unwrap();
        let blocked_destination = directory.path().join("destination-directory");
        std::fs::create_dir(&blocked_destination).unwrap();
        let error = rename_retained(&temporary.file, &blocked_destination).unwrap_err();
        let _reported_error = temporary.abort(error);
        drop(temporary);
        assert!(
            !moved_name.exists(),
            "owned object must be cleaned by handle"
        );
        assert_eq!(std::fs::read(&original_name).unwrap(), b"unowned sentinel");
        assert!(blocked_destination.is_dir());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}
