//! Internal macros.

/// Delegate read-method calls from `RunWriter` to its embedded `RunReader`.
///
/// Avoids ~40 lines of forwarding boilerplate while staying explicit (no Deref magic).
///
/// Usage:
/// ```ignore
/// delegate_to_reader! {
///     async {
///         pub current_seq() -> Result<EventSeq, StorageError>;
///         pub stage_executions() -> Result<Vec<StageExecution>, StorageError>;
///     }
/// }
/// ```
macro_rules! delegate_to_reader {
    (
        async {
            $(
                $vis:vis $name:ident( $( $arg:ident : $ty:ty ),* $(,)? ) -> $ret:ty;
            )*
        }
    ) => {
        $(
            $vis async fn $name(&self, $( $arg : $ty ),*) -> $ret {
                self.reader.$name( $( $arg ),* ).await
            }
        )*
    };
}

pub(crate) use delegate_to_reader;
