//! Fixed actor bindings and retained fixture directories; no privilege changes.
use super::test_security;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::windows::{
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
};
use windows::Win32::{
    Foundation::{ERROR_NO_TOKEN, HANDLE},
    Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
    System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken},
};

#[derive(Clone, Copy)]
pub(super) enum Role {
    AdministrativeCreation,
    ForeignSetup,
    StandardCreation,
    ForeignRefusal,
}
impl Role {
    pub(super) fn filter(self) -> &'static str {
        match self {
            Self::AdministrativeCreation => {
                "state_home::privileged_fixture_tests::admin_default_owner_creation"
            },
            Self::ForeignSetup => {
                "state_home::privileged_fixture_tests::provision_foreign_owner_fixture"
            },
            Self::StandardCreation => {
                "state_home::privileged_fixture_tests::standard_user_below_admin_ancestor"
            },
            Self::ForeignRefusal => {
                "state_home::privileged_fixture_tests::foreign_owner_refuses_before_initialization"
            },
        }
    }
    fn elevated(self) -> bool {
        matches!(self, Self::AdministrativeCreation | Self::ForeignSetup)
    }
}

pub(super) struct Context {
    pub(super) actor: String,
    pub(super) standard: String,
    pub(super) foreign: String,
    pub(super) data: PathBuf,
    pub(super) standard_data: PathBuf,
    receipt: PathBuf,
    hash: String,
    nonce: String,
    role: Role,
    _routes: Vec<File>,
}

fn env(key: &str) -> String {
    std::env::var(format!("SURGE_FIXTURE_{key}")).unwrap()
}

fn lower_hex(value: &str, size: usize) {
    assert_eq!(value.len(), size);
    assert!(
        value
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
    );
}

/// Reject aliases and hold every actual component against delete/rename through actor exit.
fn retain_directory(path: &Path) -> Vec<File> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        panic!("absolute drive path required")
    };
    assert!(matches!(prefix.kind(), Prefix::Disk(_)));
    assert_eq!(components.next(), Some(Component::RootDir));
    let mut root = prefix.as_os_str().to_os_string();
    root.push("\\");
    let mut current = PathBuf::from(root);
    let mut paths = vec![current.clone()];
    for component in components {
        let Component::Normal(name) = component else {
            panic!("noncanonical fixture path")
        };
        let text = name.to_str().unwrap();
        assert!(!text.ends_with(['.', ' ']) && !text.contains(':'));
        current.push(name);
        paths.push(current.clone());
    }
    paths
        .into_iter()
        .map(|path| {
            let file = test_security::open_observer(&path);
            let metadata = file.metadata().unwrap();
            assert!(metadata.is_dir());
            assert_eq!(
                metadata.file_attributes() & 0x400,
                0,
                "fixture route reparse point"
            );
            assert_eq!(
                test_security::file_identity(&file),
                test_security::path_identity(&path)
            );
            file
        })
        .collect()
}

impl Context {
    pub(super) fn read(role: Role) -> Self {
        assert_eq!(
            std::env::var_os("SURGE_FIXTURE_BOOTSTRAP").is_some(),
            role.elevated()
        );
        assert_eq!(env("FILTER"), role.filter());
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        assert_eq!(
            arguments,
            [
                "--ignored",
                "--exact",
                role.filter(),
                "--nocapture",
                "--test-threads=1"
            ]
            .map(std::ffi::OsString::from)
        );
        let data = PathBuf::from(env("DATA"));
        let standard_data = PathBuf::from(env("STANDARD_DATA"));
        let receipt = PathBuf::from(env("RECEIPT"));
        assert_eq!(receipt.file_name().unwrap(), "complete.json");
        let mut routes = retain_directory(&data);
        routes.extend(retain_directory(&standard_data));
        routes.extend(retain_directory(receipt.parent().unwrap()));
        let context = Self {
            actor: env("ACTOR_SID"),
            standard: env("STANDARD_SID"),
            foreign: env("FOREIGN_SID"),
            data,
            standard_data,
            receipt,
            hash: env("HASH"),
            nonce: env("NONCE"),
            role,
            _routes: routes,
        };
        lower_hex(&context.hash, 64);
        lower_hex(&context.nonce, 32);
        assert_ne!(context.foreign, context.standard);
        assert_ne!(context.foreign, context.actor);
        assert!(!["S-1-5-18", "S-1-5-32-544"].contains(&context.foreign.as_str()));
        test_security::token_principals_for_actor(&context.actor);
        if role.elevated() {
            assert_ne!(context.actor, context.standard);
        } else {
            assert_eq!(context.actor, context.standard);
            assert_eq!(
                context.actor,
                std::env::var("SURGE_NATIVE_PROBE_SID").unwrap()
            );
            assert_standard_token();
        }
        if matches!(role, Role::AdministrativeCreation) {
            assert_ne!(
                test_security::path_identity(&context.data),
                test_security::path_identity(&context.standard_data)
            );
        } else {
            assert_eq!(context.data, context.standard_data);
        }
        context
    }

    pub(super) fn complete(&self, observation: &str, facts: serde_json::Value) {
        let receipt = serde_json::json!({
            "schema_version":1, "phase":"complete", "candidate_hash":self.hash,
            "filter":self.role.filter(), "nonce":self.nonce, "actor_sid":self.actor,
            "pid":std::process::id(), "observation":observation, "facts":facts,
        });
        let bytes = serde_json::to_vec(&receipt).unwrap();
        assert!(bytes.len() <= 64 * 1024);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.receipt)
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.sync_all().unwrap();
        println!("{observation}");
    }
}

fn assert_standard_token() {
    let mut token = HANDLE::default();
    // SAFETY: query-only effective thread token attempt and initialized output.
    let thread = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &raw mut token) };
    match thread {
        Ok(()) => {
            // SAFETY: even unexpected token success owns one handle; close it before failing.
            let unexpected = unsafe { OwnedHandle::from_raw_handle(token.0) };
            drop(unexpected);
            panic!("standard actor must not impersonate");
        },
        Err(error) => assert_eq!(error.code(), ERROR_NO_TOKEN.to_hresult()),
    }
    // SAFETY: process pseudo handle and initialized query-token output.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) }.unwrap();
    // SAFETY: one successful fresh token handle transfers uniquely into RAII ownership.
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    // SAFETY: exact class-specific initialized buffer and retained token handle.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenElevation,
            Some((&raw mut elevation).cast()),
            u32::try_from(std::mem::size_of::<TOKEN_ELEVATION>()).unwrap(),
            &raw mut returned,
        )
    }
    .unwrap();
    assert_eq!(
        usize::try_from(returned).unwrap(),
        std::mem::size_of::<TOKEN_ELEVATION>()
    );
    assert_eq!(
        elevation.TokenIsElevated, 0,
        "runtime actor must be non-elevated"
    );
}
