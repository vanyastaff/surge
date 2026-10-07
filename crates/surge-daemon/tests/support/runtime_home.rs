#[cfg(windows)]
use surge_persistence::RuntimeHomeOwner;
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/test-support/runtime_home.rs"
));
