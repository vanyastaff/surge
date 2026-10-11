//! Test-only synchronous observation after real FILE_CREATE, before validation or I/O.
use std::{cell::RefCell, ffi::OsStr, fs::File, marker::PhantomData, rc::Rc};

type Callback = Box<dyn FnMut(&File, &OsStr, bool, &File)>;
thread_local! {
    static OBSERVER: RefCell<Option<Callback>> = const { RefCell::new(None) };
}

/// Cannot leave the registering thread; nested registrations are rejected.
pub(in crate::state_home) struct Registration(PhantomData<Rc<()>>);
impl Drop for Registration {
    fn drop(&mut self) {
        OBSERVER.with(|observer| {
            observer.borrow_mut().take();
        });
    }
}

pub(in crate::state_home) fn register(
    callback: impl FnMut(&File, &OsStr, bool, &File) + 'static,
) -> Registration {
    OBSERVER.with(|observer| {
        let mut observer = observer.borrow_mut();
        assert!(observer.is_none(), "nested creation observation");
        *observer = Some(Box::new(callback));
    });
    Registration(PhantomData)
}

pub(super) fn created(parent: &File, name: &OsStr, directory: bool, created: &File) {
    OBSERVER.with(|observer| {
        if let Some(callback) = observer.borrow_mut().as_mut() {
            callback(parent, name, directory, created);
        }
    });
}
