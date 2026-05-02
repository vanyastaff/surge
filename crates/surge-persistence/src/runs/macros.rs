//! Internal macros.

/// Delegate read-method calls from `RunWriter` to its embedded `RunReader`.
///
/// Avoids ~40 lines of forwarding boilerplate while staying explicit (no Deref magic).
/// Each method may carry its own doc attributes (`#[doc = "..."]` or `///`)
/// so generated forwarders are documented for `cargo doc -D warnings`.
///
/// Usage:
/// ```ignore
/// delegate_to_reader! {
///     async {
///         /// Doc for current_seq.
///         pub current_seq() -> Result<EventSeq, StorageError>;
///         /// Doc for stage_executions.
///         pub stage_executions() -> Result<Vec<StageExecution>, StorageError>;
///     }
/// }
/// ```
macro_rules! delegate_to_reader {
    (
        async {
            $(
                $(#[$attr:meta])*
                $vis:vis $name:ident( $( $arg:ident : $ty:ty ),* $(,)? ) -> $ret:ty;
            )*
        }
    ) => {
        $(
            $(#[$attr])*
            $vis async fn $name(&self, $( $arg : $ty ),*) -> $ret {
                self.reader.$name( $( $arg ),* ).await
            }
        )*
    };
}

pub(crate) use delegate_to_reader;
