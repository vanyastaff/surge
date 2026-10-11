//! Cross-crate pool ownership oracle, kept next to the event tap implementation.
#[path = "../src/engine/event_tap/ownership_tests.rs"]
mod ownership_tests;

use ownership_tests::runtime_home as runtime_home_fixture;
