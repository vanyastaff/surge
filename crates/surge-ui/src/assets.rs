//! Asset source for the desktop app.
//!
//! gpui-kit embeds only the ~100 icons its own components use. Surge's
//! semantic vocabulary needs a handful more from the same Lucide catalog;
//! [`icon_assets!`] embeds exactly those, and [`AppAssets`] serves ours
//! and falls back to the component set. An icon used through
//! [`gpui_kit::assets::IconName`] but missing from the list below renders
//! as nothing — add it here.

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::{AssetSource, Result, SharedString};

icon_assets!(SurgeIcons, [
    Activity,
    AppWindow,
    ArrowRight,
    Brain,
    CircleDot,
    Clock,
    Command,
    CornerDownLeft,
    FolderPlus,
    GitBranch,
    GitCompare,
    GitMerge,
    Hourglass,
    Kanban,
    Pin,
    PinOff,
    Repeat,
    ScrollText,
    ShieldAlert,
    ShieldCheck,
    Sparkles,
    UserCheck,
    Waypoints,
    Workflow,
    X,
    Zap,
]);

/// Surge's additions, then gpui-kit's default icons.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        // `Assets` reports an unknown path as an error, not `None`, so ask
        // ours first: it answers `None` for anything it does not hold.
        match SurgeIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(SurgeIcons.list(path)?);
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconName;

    #[test]
    fn serves_surge_icons_and_component_defaults() {
        for icon in [IconName::ShieldCheck, IconName::FolderPlus, IconName::Workflow] {
            let path = icon.path();
            assert!(AppAssets.load(&path).unwrap().is_some(), "{path} missing");
        }
        // A gpui-kit default still comes through the fallback.
        let path = IconName::Settings.path();
        assert!(AppAssets.load(&path).unwrap().is_some(), "{path} missing");
    }
}
