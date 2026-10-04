//! Terminal runtime owner: cancel async work before joining actual child owners.
pub(crate) struct HostRuntime {
    runtime: Option<tokio::runtime::Runtime>,
    background: bool,
}
impl HostRuntime {
    pub(crate) fn new(runtime: tokio::runtime::Runtime, background: bool) -> Self {
        Self {
            runtime: Some(runtime),
            background,
        }
    }
    pub(crate) fn runtime(&self) -> &tokio::runtime::Runtime {
        match &self.runtime {
            Some(runtime) => runtime,
            None => std::process::abort(),
        }
    }
}
impl Drop for HostRuntime {
    fn drop(&mut self) {
        surge_persistence::work_items::close_owned_flow_refusal_admission();
        if let Some(runtime) = self.runtime.take() {
            if self.background {
                // Internal stdio helper may retain an uncancellable stdin read.
                runtime.shutdown_background();
            } else {
                drop(runtime);
            }
        }
        surge_mcp::shutdown_children_and_join();
        surge_persistence::work_items::join_owned_flow_refusal_owners();
    }
}
