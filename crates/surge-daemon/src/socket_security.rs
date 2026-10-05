//! Unix socket publication and cleanup within the user's private namespace.

use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) struct SocketDirectory {
    path: PathBuf,
    directory: File,
}

#[cfg(target_os = "macos")]
use mac_acl::rejects_grants;

fn denied(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}

impl SocketDirectory {
    pub(crate) fn prepare(socket: &Path) -> io::Result<Self> {
        let parent = socket
            .parent()
            .ok_or_else(|| denied("socket has no parent directory"))?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
            .open(parent)?;
        let metadata = directory.metadata()?;
        #[cfg(target_os = "macos")]
        rejects_grants(&directory).map_err(|()| denied("socket directory has an unsafe ACL"))?;
        if metadata.uid() != nix::unistd::geteuid().as_raw() {
            return Err(denied("socket directory belongs to another user"));
        }
        // Check resolved ancestors before publishing: a writable ancestor can
        // let another user replace the private directory's namespace.
        let canonical = parent.canonicalize()?;
        for ancestor in canonical.ancestors().skip(1) {
            let ancestor_file = OpenOptions::new()
                .read(true)
                .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
                .open(ancestor)?;
            let metadata = ancestor_file.metadata()?;
            #[cfg(target_os = "macos")]
            rejects_grants(&ancestor_file)
                .map_err(|()| denied("socket ancestor has an unsafe ACL"))?;
            let uid = nix::unistd::geteuid().as_raw();
            let protected_sticky =
                metadata.mode() & 0o1000 != 0 && (metadata.uid() == 0 || metadata.uid() == uid);
            if (metadata.uid() != 0 && metadata.uid() != uid)
                || (metadata.mode() & 0o022 != 0 && !protected_sticky)
            {
                return Err(denied("socket directory has an insecure ancestor"));
            }
        }
        // File::set_permissions changes the opened descriptor, never a
        // substituted symlink at the pathname.
        directory.set_permissions(std::fs::Permissions::from_mode(0o700))?;
        let secured = Self {
            path: parent.to_path_buf(),
            directory,
        };
        secured.validate()?;
        match std::fs::symlink_metadata(socket) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && metadata.uid() == nix::unistd::geteuid().as_raw() =>
            {
                std::fs::remove_file(socket)?;
            },
            Ok(_) => return Err(denied("existing socket path is not an owned socket")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {},
            Err(error) => return Err(error),
        }
        Ok(secured)
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        let current = std::fs::symlink_metadata(&self.path)?;
        let held = self.directory.metadata()?;
        #[cfg(target_os = "macos")]
        rejects_grants(&self.directory)
            .map_err(|()| denied("socket directory has an unsafe ACL"))?;
        if !current.is_dir() || !same_file(&current, &held) || held.mode() & 0o777 != 0o700 {
            return Err(denied("socket directory changed during publication"));
        }
        Ok(())
    }

    pub(crate) fn publish(self, socket: &Path) -> io::Result<BoundSocket> {
        self.validate()?;
        let identity = std::fs::symlink_metadata(socket)?;
        if !identity.file_type().is_socket() || identity.uid() != nix::unistd::geteuid().as_raw() {
            return Err(denied("bound socket identity is not owned"));
        }
        let bound = BoundSocket {
            path: socket.to_path_buf(),
            identity,
            directory: self,
        };
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
        bound.directory.validate()?;
        let current = std::fs::symlink_metadata(socket)?;
        if !same_file(&bound.identity, &current) || current.mode() & 0o777 != 0o600 {
            return Err(denied("bound socket changed during publication"));
        }
        Ok(bound)
    }
}

pub(crate) struct BoundSocket {
    path: PathBuf,
    identity: Metadata,
    directory: SocketDirectory,
}

impl Drop for BoundSocket {
    fn drop(&mut self) {
        if self.directory.validate().is_ok()
            && std::fs::symlink_metadata(&self.path).is_ok_and(|current| {
                same_file(&self.identity, &current) && current.file_type().is_socket()
            })
            && let Err(error) = std::fs::remove_file(&self.path)
        {
            tracing::warn!(%error, "failed to remove owned daemon socket");
        }
    }
}

// Descriptor ACL contract copied from persistence start_preparation/secure_lock.rs.
// Darwin chmod does not revoke extended grants, so deny-only ACLs are required.
#[cfg(target_os = "macos")]
mod mac_acl {
    use std::{ffi::c_void, fs::File, os::fd::AsRawFd};
    // Signatures/constants from the platform SDK sys/acl.h. These are opaque libc objects.
    unsafe extern "C" {
        fn acl_get_fd_np(fd: nix::libc::c_int, kind: nix::libc::c_int) -> *mut c_void;
        fn acl_get_entry(
            acl: *mut c_void,
            id: nix::libc::c_int,
            entry: *mut *mut c_void,
        ) -> nix::libc::c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut nix::libc::c_int) -> nix::libc::c_int;
        fn acl_free(acl: *mut c_void) -> nix::libc::c_int;
    }
    struct Acl(*mut c_void);
    impl Drop for Acl {
        fn drop(&mut self) {
            // SAFETY: this pointer is a nonnull ACL returned by libc, freed exactly once.
            unsafe {
                acl_free(self.0);
            }
        }
    }
    pub(super) fn rejects_grants(file: &File) -> Result<(), ()> {
        // SAFETY: file owns a live descriptor; ACL_TYPE_EXTENDED=0x100 on macOS.
        let pointer = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
        if pointer.is_null() {
            // Darwin returns ENOENT for an absent extended ACL on a live descriptor.
            return (std::io::Error::last_os_error().raw_os_error() == Some(nix::libc::ENOENT))
                .then_some(())
                .ok_or(());
        }
        let acl = Acl(pointer);
        let mut id = 0; // ACL_FIRST_ENTRY
        // The SDK caps extended ACLs at 169 entries; a larger result fails closed.
        for _ in 0..170 {
            let mut entry = std::ptr::null_mut();
            // SAFETY: ACL is live and owned; entry output is valid writable storage.
            let result = unsafe { acl_get_entry(acl.0, id, &raw mut entry) };
            if result != 0 {
                // Darwin returns EINVAL when no next entry exists, including an empty ACL.
                return (std::io::Error::last_os_error().raw_os_error() == Some(nix::libc::EINVAL))
                    .then_some(())
                    .ok_or(());
            }
            if entry.is_null() {
                return Err(());
            }
            let mut tag = 0;
            // SAFETY: entry belongs to live ACL and tag output is valid writable storage.
            if unsafe { acl_get_tag_type(entry, &raw mut tag) } != 0 || tag != 2 {
                // Only ACL_EXTENDED_DENY entries are safe; grant and unknown tags fail closed.
                return Err(());
            }
            id = -1; // ACL_NEXT_ENTRY
        }
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn publication_is_private_and_cleanup_preserves_replacements() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let socket = root.path().join("daemon.sock");
        let directory = SocketDirectory::prepare(&socket).unwrap();
        assert_eq!(root.path().metadata().unwrap().mode() & 0o777, 0o700);
        let listener = UnixListener::bind(&socket).unwrap();
        let bound = directory.publish(&socket).unwrap();
        assert_eq!(socket.metadata().unwrap().mode() & 0o777, 0o600);
        std::fs::remove_file(&socket).unwrap();
        std::fs::write(&socket, "replacement").unwrap();
        drop(bound);
        drop(listener);
        assert_eq!(std::fs::read_to_string(socket).unwrap(), "replacement");
    }

    #[test]
    fn rejects_symlink_directory_without_changing_target() {
        let root = tempfile::tempdir().unwrap();
        let actual = root.path().join("actual");
        std::fs::create_dir(&actual).unwrap();
        std::fs::set_permissions(&actual, std::fs::Permissions::from_mode(0o755)).unwrap();
        let linked = root.path().join("linked");
        std::os::unix::fs::symlink(&actual, &linked).unwrap();
        assert!(SocketDirectory::prepare(&linked.join("daemon.sock")).is_err());
        assert_eq!(actual.metadata().unwrap().mode() & 0o777, 0o755);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn extended_acl_grant_refuses_publication() {
        let root = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow list,search"])
            .arg(root.path())
            .status()
            .unwrap();
        assert!(
            status.success(),
            "test filesystem must support extended ACLs"
        );
        let Err(error) = SocketDirectory::prepare(&root.path().join("daemon.sock")) else {
            panic!("extended grant ACL must refuse socket publication");
        };
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn replaced_directory_refuses_publication() {
        let root = tempfile::tempdir().unwrap();
        let parent = root.path().join("daemon");
        std::fs::create_dir(&parent).unwrap();
        let directory = SocketDirectory::prepare(&parent.join("daemon.sock")).unwrap();
        std::fs::rename(&parent, root.path().join("original")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        assert!(directory.validate().is_err());
    }

    #[test]
    fn owned_stale_socket_is_removed_and_bound_socket_is_cleaned() {
        let root = tempfile::tempdir().unwrap();
        let socket = root.path().join("daemon.sock");
        drop(UnixListener::bind(&socket).unwrap());
        let directory = SocketDirectory::prepare(&socket).unwrap();
        assert!(!socket.exists());
        let listener = UnixListener::bind(&socket).unwrap();
        let bound = directory.publish(&socket).unwrap();
        drop(bound);
        assert!(!socket.exists());
        drop(listener);
    }
}
