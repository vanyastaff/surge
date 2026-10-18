// Test-only discovery; select the binary before committing an immutable launch recipe.
use std::path::{Path, PathBuf};

pub fn path() -> PathBuf {
    let binary_name = format!("mock_acp_agent{}", std::env::consts::EXE_SUFFIX);
    let explicit = std::env::var_os("SURGE_TEST_MOCK_ACP_BINARY")
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_mock_acp_agent"))
        .or_else(|| option_env!("CARGO_BIN_EXE_mock_acp_agent").map(std::ffi::OsString::from));
    let binary = if let Some(explicit) = explicit {
        PathBuf::from(explicit)
    } else if let Some(profile) = std::env::current_exe().ok().and_then(|executable| {
        let directory = executable.parent()?;
        (directory.file_name()? == "deps")
            .then(|| directory.parent().map(Path::to_path_buf))
            .flatten()
    }) {
        profile.join(binary_name)
    } else {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .map_or_else(|| workspace.join("target"), |target| {
                if target.is_absolute() { target } else { workspace.join(target) }
            });
        target.join("debug").join(binary_name)
    };
    assert!(binary.is_file(), "build the selected mock_acp_agent prerequisite: {}", binary.display());
    binary
}
