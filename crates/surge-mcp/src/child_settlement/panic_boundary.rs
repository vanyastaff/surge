//! Shared single hook/TLS identity across host ownership subsystems.
pub(super) use surge_process::owner_panic::{
    abort_on_owner_panic as protected, install_owner_panic_protection as install,
};
