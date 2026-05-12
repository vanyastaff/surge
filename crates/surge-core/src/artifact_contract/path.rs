//! Path helpers for artifact contract path matching.

use std::path::Path;

pub(super) fn normalize_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn is_adr_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("docs/adr/") else {
        return false;
    };
    if !rest.ends_with(".md") {
        return false;
    }
    let stem = rest.trim_end_matches(".md");
    let Some((number, slug)) = stem.split_once('-') else {
        return false;
    };
    number.len() == 4 && number.chars().all(|ch| ch.is_ascii_digit()) && !slug.is_empty()
}

pub(super) fn is_story_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("stories/story-") else {
        return false;
    };
    let Some(number) = rest.strip_suffix(".md") else {
        return false;
    };
    number.len() == 3 && number.chars().all(|ch| ch.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::super::contract::{ArtifactKind, contract_for};
    use std::path::Path;

    #[test]
    fn path_patterns_accept_expected_locations() {
        assert!(contract_for(ArtifactKind::Adr).accepts_path(Path::new("docs/adr/0001-choice.md")));
        assert!(contract_for(ArtifactKind::Story).accepts_path(Path::new("stories/story-001.md")));
        assert!(!contract_for(ArtifactKind::Story).accepts_path(Path::new("stories/story-1.md")));
    }
}
