//! Every bundled profile carries display metadata the UI can rely on.

use std::path::Path;

use surge_core::profile::Profile;

const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

#[test]
fn bundled_profiles_have_valid_icon_color_and_effort_floor() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("bundled/profiles");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("bundled profiles dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).expect("read profile");
        let profile: Profile = toml::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} failed to parse: {e}", path.display()));
        let name = path.file_name().unwrap().to_string_lossy();

        assert!(profile.role.icon.is_some(), "{name}: missing icon");
        let color = profile
            .role
            .color
            .as_deref()
            .unwrap_or_else(|| panic!("{name}: missing color"));
        assert!(
            color.len() == 7
                && color.starts_with('#')
                && color[1..].chars().all(|c| c.is_ascii_hexdigit()),
            "{name}: color {color:?} is not #RRGGBB"
        );
        if let Some(effort) = profile.role.min_effort.as_deref() {
            assert!(
                EFFORTS.contains(&effort),
                "{name}: unknown min_effort {effort:?}"
            );
        }
        checked += 1;
    }
    assert!(
        checked >= 15,
        "expected the bundled profile set, found {checked}"
    );
}
