//! Bounded fixture-only deletion through independently validated exact SDK handles.
use super::{objects, test_security};
use std::{
    fs::{File, OpenOptions},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
};
use windows::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{
        DELETE, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO,
        FileDispositionInfo, FileStandardInfo, GetFileInformationByHandleEx, READ_CONTROL,
        SetFileInformationByHandle,
    },
};

pub(super) struct Object {
    path: PathBuf,
    id: test_security::FileIdentity,
    descriptor: objects::Snapshot,
    directory: bool,
}
impl Object {
    pub(super) fn capture(path: &Path, directory: bool) -> Self {
        let reader = test_security::open_observer(path);
        check_type(&reader, directory);
        Self {
            path: path.to_owned(),
            id: test_security::file_identity(&reader),
            descriptor: objects::inspect(&reader),
            directory,
        }
    }
    pub(super) fn require_identity(&self, expected: test_security::FileIdentity) {
        assert_eq!(self.id, expected);
    }
    fn deleting_handle(&self) -> File {
        let file = OpenOptions::new()
            .access_mode((DELETE | READ_CONTROL | FILE_READ_ATTRIBUTES).0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
            .open(&self.path)
            .unwrap();
        self.validate(&file);
        file
    }
    fn validate(&self, file: &File) {
        check_type(file, self.directory);
        assert_eq!(test_security::file_identity(file), self.id);
        assert_eq!(objects::inspect(file), self.descriptor);
    }
    fn remove(&self, file: File) {
        self.validate(&file);
        if self.directory {
            assert!(names(&self.path).is_empty());
        }
        let disposition = FILE_DISPOSITION_INFO {
            DeleteFile: true.into(),
        };
        // SAFETY: exact retained DELETE handle, revalidated above, and matching live SDK input.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                (&raw const disposition).cast(),
                u32::try_from(std::mem::size_of::<FILE_DISPOSITION_INFO>()).unwrap(),
            )
        }
        .unwrap();
        drop(file);
        assert!(!self.path.try_exists().unwrap());
    }
    pub(super) fn remove_exact(self) {
        let file = self.deleting_handle();
        self.remove(file);
    }
}
fn check_type(file: &File, directory: bool) {
    let metadata = file.metadata().unwrap();
    assert_eq!(
        metadata.file_attributes() & 0x400,
        0,
        "reparse cleanup refused"
    );
    assert_eq!(metadata.is_dir(), directory);
    assert!(directory || metadata.is_file());
    let mut standard = FILE_STANDARD_INFO::default();
    // SAFETY: retained observer/delete handle and exact initialized SDK output buffer.
    unsafe {
        GetFileInformationByHandleEx(
            HANDLE(file.as_raw_handle()),
            FileStandardInfo,
            (&raw mut standard).cast(),
            u32::try_from(std::mem::size_of::<FILE_STANDARD_INFO>()).unwrap(),
        )
    }
    .unwrap();
    assert_eq!(standard.Directory.as_bool(), directory);
    assert!(!standard.DeletePending.as_bool());
    if !directory {
        assert_eq!(standard.NumberOfLinks, 1);
    }
}
fn names(path: &Path) -> Vec<String> {
    // Bound iteration before allocating an unexpected arbitrarily large inventory.
    let mut values = std::fs::read_dir(path)
        .unwrap()
        .take(5)
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    values.sort();
    values
}

pub(super) struct Parent {
    file: File,
    original: Object,
}
impl Parent {
    pub(super) fn capture(path: &Path) -> Self {
        let file = test_security::open_observer(path);
        let original = Object::capture(path, true);
        original.validate(&file);
        Self { file, original }
    }
    pub(super) fn verify(&self) {
        self.original.validate(&self.file);
    }
}

pub(super) struct Home {
    parent: Parent,
    objects: Vec<Object>,
    database_names: Vec<String>,
}
impl Home {
    pub(super) fn capture(
        parent: Parent,
        path: &Path,
        actor: &str,
        home_id: test_security::FileIdentity,
        database_id: test_security::FileIdentity,
    ) -> Self {
        parent.verify();
        let mut objects = Vec::new();
        for (relative, inherits) in [
            ("", true),
            ("db", true),
            ("runs", true),
            ("daemon", false),
            ("work-items", false),
            ("work-items/locks", false),
        ] {
            let object = Object::capture(&path.join(relative), true);
            assert_eq!(object.descriptor.owner, actor);
            let flags = if inherits { "OICI" } else { "" };
            assert_eq!(
                object.descriptor.dacl(),
                format!("P(A;{flags};FA;;;{actor})")
            );
            objects.push(object);
        }
        objects[0].require_identity(home_id);
        let database_names = names(&path.join("db"));
        assert!(database_names.iter().any(|name| name == "registry.sqlite"));
        for name in &database_names {
            assert!(
                [
                    "registry.sqlite",
                    "registry.sqlite-wal",
                    "registry.sqlite-shm",
                    "registry.sqlite-journal"
                ]
                .contains(&name.as_str())
            );
            let object = Object::capture(&path.join("db").join(name), false);
            if name == "registry.sqlite" {
                object.require_identity(database_id);
                assert_eq!(object.descriptor.owner, actor);
                assert_eq!(object.descriptor.dacl(), format!("P(A;;FA;;;{actor})"));
            } else {
                sidefile(&object, actor);
            }
            objects.push(object);
        }
        let result = Self {
            parent,
            objects,
            database_names,
        };
        result.inventory();
        result
    }
    fn inventory(&self) {
        let root = &self.objects[0].path;
        assert_eq!(names(root), ["daemon", "db", "runs", "work-items"]);
        assert_eq!(names(&root.join("db")), self.database_names);
        assert!(names(&root.join("runs")).is_empty());
        assert!(names(&root.join("daemon")).is_empty());
        assert_eq!(names(&root.join("work-items")), ["locks"]);
        assert!(names(&root.join("work-items/locks")).is_empty());
    }
    pub(super) fn remove(self) {
        self.parent.verify();
        let mut handles: Vec<_> = self
            .objects
            .iter()
            .map(|object| Some(object.deleting_handle()))
            .collect();
        self.inventory();
        // Files precede directories. Reverse fixed directory order removes locks before work-items.
        for index in (0..self.objects.len()).rev() {
            self.objects[index].remove(handles[index].take().unwrap());
        }
        self.parent.verify();
    }
}
fn sidefile(object: &Object, actor: &str) {
    let (_, default_owner) = test_security::token_principals_for_actor(actor);
    assert!(
        object.descriptor.owner == actor
            || (object.descriptor.owner == default_owner
                && ["S-1-5-18", "S-1-5-32-544"].contains(&default_owner.as_str()))
    );
    let dacl = object.descriptor.dacl();
    let (control, ace) = dacl.split_once('(').unwrap();
    assert!(["", "P", "AI", "AR", "PAI", "PAR", "ARAI", "PARAI"].contains(&control));
    assert!([format!("A;;FA;;;{actor})"), format!("A;ID;FA;;;{actor})")].contains(&ace.to_owned()));
}

pub(super) fn remove_singleton(home: Object, sentinel: Object) {
    let home_handle = home.deleting_handle();
    let sentinel_handle = sentinel.deleting_handle();
    assert_eq!(names(&home.path), ["sentinel"]);
    sentinel.remove(sentinel_handle);
    home.remove(home_handle);
}
