//! Helpers for constructing ACP `ContentBlock`s used by both legacy and bridge
//! code paths. Kept in `shared/` so a future ACP-SDK upgrade touches one place.

use agent_client_protocol::ContentBlock;

/// Build a single text `ContentBlock` from an owned string.
pub(crate) fn text(s: impl Into<String>) -> ContentBlock {
    ContentBlock::Text(agent_client_protocol::TextContent::new(s))
}

/// Build a single-element `Vec<ContentBlock>` from a string.
pub(crate) fn text_vec(s: impl Into<String>) -> Vec<ContentBlock> {
    vec![text(s)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_block_round_trip() {
        let b = text("hello");
        match b {
            ContentBlock::Text(t) => assert_eq!(t.text, "hello"),
            _ => panic!("expected Text block"),
        }
    }

    #[test]
    fn text_vec_yields_single_element() {
        let v = text_vec("x");
        assert_eq!(v.len(), 1);
    }
}
