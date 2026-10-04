//! One dedicated process owns the global subscriber before runtime construction.
use std::sync::{Arc, Mutex};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        self.0
            .lock()
            .unwrap()
            .push(event.metadata().target().into());
    }

    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        _: &tracing::span::Id,
        _: Context<'_, S>,
    ) {
        self.0
            .lock()
            .unwrap()
            .push(attributes.metadata().target().into());
    }
}

#[test]
fn mandatory_global_veto_covers_spans_background_tasks_and_all_output_layers() {
    let first = Arc::new(Mutex::new(Vec::new()));
    let second = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::filter::filter_fn(|metadata| {
            surge_mcp::diagnostics::permits_target(metadata.target())
        }))
        .with(tracing_subscriber::EnvFilter::new(
            "trace,rmcp=trace,process_wrap=trace",
        ))
        .with(Capture(first.clone()))
        .with(Capture(second.clone()));
    tracing::subscriber::set_global_default(subscriber).unwrap();
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap().block_on(async {
        tokio::spawn(async {
            tracing::trace!(target:"rmcp", private="root diagnostic payload");
            tracing::warn!(target:"rmcp::service", private="background notification payload");
            let span=tracing::info_span!(target:"process_wrap::tokio", "external child", private="formatted command");
            let _entered=span.enter();
            tracing::trace!(target:"process_wrap", private="root wrapper payload");
            tracing::info!(target:"surge_mcp", "safe owned category");
            tracing::trace!(target:"rmcp_extra", "unrelated public target");
        }).await.unwrap();
    });
    for capture in [first, second] {
        let targets = capture.lock().unwrap();
        assert_eq!(*targets, vec!["surge_mcp", "rmcp_extra"]);
    }
}
