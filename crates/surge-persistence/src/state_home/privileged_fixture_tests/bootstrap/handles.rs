use super::{Manifest, decimal, paths::Routes};
use std::{
    mem::{size_of, size_of_val},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows::{
    Wdk::Foundation::{NtQueryObject, ObjectBasicInformation, ObjectTypeInformation},
    Win32::{
        Foundation::{
            GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, STATUS_SUCCESS, WAIT_OBJECT_0,
        },
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle},
            LibraryLoader::{
                LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
                SetDefaultDllDirectories,
            },
            Threading::{SetEvent, WaitForMultipleObjects},
            WindowsProgramming::{PUBLIC_OBJECT_BASIC_INFORMATION, PUBLIC_OBJECT_TYPE_INFORMATION},
        },
    },
};

pub(super) struct Transferred {
    handles: [OwnedHandle; 5],
    previous: [Option<HANDLE>; 2],
}

impl Transferred {
    pub(super) fn take(manifest: &Manifest) -> Self {
        let values = [
            &manifest.stdout.handle,
            &manifest.stderr.handle,
            &manifest.ready_handle,
            &manifest.proceed_handle,
            &manifest.cancel_handle,
        ]
        .map(|text| {
            let value = usize::try_from(decimal(text)).unwrap();
            assert!(value != 0 && value <= isize::MAX as usize);
            value
        });
        for (index, value) in values.iter().enumerate() {
            assert!(
                !values[..index].contains(value),
                "duplicate transferred handle"
            );
            let mut flags = 0;
            // SAFETY: querying an untrusted numeric handle does not dereference it. A successful
            // query establishes validity before the distinct transferred handles enter RAII.
            unsafe { GetHandleInformation(HANDLE(*value as *mut _), &raw mut flags) }.unwrap();
        }
        // SAFETY: protected manifest is bound to this child PID and describes five distinct
        // valid handles explicitly transferred by its provisioner, with ownership transferred once.
        let handles = values.map(|value| unsafe { OwnedHandle::from_raw_handle(value as *mut _) });
        Self {
            handles,
            previous: [None; 2],
        }
    }

    fn raw(&self, index: usize) -> HANDLE {
        HANDLE(self.handles[index].as_raw_handle())
    }

    pub(super) fn validate(&self, manifest: &Manifest, routes: &mut Routes) {
        for index in 0..5 {
            let expected = match index {
                0 | 1 => ("File", 0x0012_0082),
                2 => ("Event", 2),
                _ => ("Event", 0x0010_0000),
            };
            validate_object(self.raw(index), expected.0, expected.1);
        }
        let stdout = routes.validate_output(self.raw(0), "stdout.log", &manifest.stdout);
        let stderr = routes.validate_output(self.raw(1), "stderr.log", &manifest.stderr);
        assert_ne!(stdout, stderr, "stdout and stderr must be distinct files");
    }

    pub(super) fn install(&mut self) {
        // SAFETY: fixed SDK search flags; protected executable and system routes are provisioner-owned.
        unsafe {
            SetDefaultDllDirectories(
                LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }
        .unwrap();
        for (index, slot) in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .into_iter()
            .enumerate()
        {
            // SAFETY: fixed SDK standard stream selector; returned handle is borrowed only.
            let previous = unsafe { GetStdHandle(slot) }.unwrap_or_default();
            assert!(!(0..5).any(|owned| self.raw(owned) == previous));
            self.previous[index] = Some(previous);
            // SAFETY: live transferred disk handle stays owned until restoration in Drop.
            unsafe { SetStdHandle(slot, self.raw(index)) }.unwrap();
        }
    }

    pub(super) fn signal_and_wait(&self) {
        // SAFETY: SDK-confirmed Event with exactly EVENT_MODIFY_STATE access.
        unsafe { SetEvent(self.raw(2)) }.unwrap();
        // SAFETY: both SDK-confirmed events have SYNCHRONIZE; cancel has lower index and wins ties.
        let result = unsafe { WaitForMultipleObjects(&[self.raw(4), self.raw(3)], false, 30_000) };
        assert_eq!(
            result.0,
            WAIT_OBJECT_0.0 + 1,
            "bootstrap cancelled, timed out, or wait failed"
        );
    }
}

impl Drop for Transferred {
    fn drop(&mut self) {
        for (index, slot) in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .into_iter()
            .enumerate()
        {
            if let Some(previous) = self.previous[index] {
                // SAFETY: restore the borrowed original before owned replacement handles drop.
                if unsafe { SetStdHandle(slot, previous) }.is_err() {
                    // Failure must not let execution continue with a dangling process standard handle.
                    std::process::abort();
                }
            }
        }
    }
}

fn validate_object(handle: HANDLE, kind: &str, access: u32) {
    let mut flags = 0;
    // SAFETY: transferred handle is retained and output points to initialized writable storage.
    unsafe { GetHandleInformation(handle, &raw mut flags) }.unwrap();
    assert_eq!(
        flags & HANDLE_FLAG_INHERIT.0,
        0,
        "inheritable transferred handle"
    );
    let mut basic = PUBLIC_OBJECT_BASIC_INFORMATION::default();
    let mut returned = 0;
    // SAFETY: exact SDK class/output layout and live handle; bounded initialized output.
    let status = unsafe {
        NtQueryObject(
            handle,
            ObjectBasicInformation,
            Some((&raw mut basic).cast()),
            u32::try_from(size_of::<PUBLIC_OBJECT_BASIC_INFORMATION>()).unwrap(),
            Some(&raw mut returned),
        )
    };
    assert_eq!(status, STATUS_SUCCESS);
    assert_eq!(
        usize::try_from(returned).unwrap(),
        size_of::<PUBLIC_OBJECT_BASIC_INFORMATION>()
    );
    assert_eq!(
        basic.GrantedAccess, access,
        "unexpected transferred handle rights"
    );
    assert_eq!(object_type(handle), kind);
}

fn object_type(handle: HANDLE) -> String {
    let mut words = [0usize; 256];
    let mut returned = 0;
    // SAFETY: aligned, initialized, bounded storage exceeds SDK type header; live handle.
    let status = unsafe {
        NtQueryObject(
            handle,
            ObjectTypeInformation,
            Some(words.as_mut_ptr().cast()),
            u32::try_from(size_of_val(&words)).unwrap(),
            Some(&raw mut returned),
        )
    };
    assert_eq!(status, STATUS_SUCCESS);
    let length = usize::try_from(returned).unwrap();
    assert!((size_of::<PUBLIC_OBJECT_TYPE_INFORMATION>()..=size_of_val(&words)).contains(&length));
    // SAFETY: successful query returned the complete aligned SDK header in retained storage.
    let name = unsafe { (*words.as_ptr().cast::<PUBLIC_OBJECT_TYPE_INFORMATION>()).TypeName };
    let start = words.as_ptr() as usize;
    let position = name.Buffer.0 as usize;
    assert!(position >= start && position.is_multiple_of(2));
    assert!(name.Length.is_multiple_of(2));
    assert!(
        position
            .checked_add(usize::from(name.Length))
            .is_some_and(|end| end <= start + length)
    );
    // SAFETY: complete UTF-16 extent and alignment checked against actual returned allocation.
    let text = unsafe { std::slice::from_raw_parts(name.Buffer.0, usize::from(name.Length) / 2) };
    String::from_utf16(text).unwrap()
}
