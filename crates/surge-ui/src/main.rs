// UI code under development - suppress dead code warnings temporarily
#![allow(dead_code)]
#![allow(unused_variables)]
// Pre-existing legacy code; M5 does not modify surge-ui.  Suppress pedantic
// lints that activate because -D clippy::pedantic is now applied workspace-wide.
#![allow(clippy::excessive_nesting)]
#![allow(clippy::ptr_arg)]

mod actions;
mod app;
mod app_state;
mod command_palette;
mod markdown;
mod notifications;
mod project;
mod router;
mod screens;
mod sidebar;
mod theme;
mod top_bar;

use gpui::*;
use gpui_component::Root;

use app::SurgeApp;
use app_state::AppState;

fn main() {
    // Initialize tracing for debug logs
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // Start a background tokio runtime for ACP pool operations.
    // gpui uses its own async executor, but AgentPool needs tokio channels.
    let tokio_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");
    let _guard = tokio_rt.enter();

    let app = Application::new().with_assets(gpui_component_assets::Assets);

    app.run(move |cx| {
        gpui_component::init(cx);
        theme::init();
        SurgeApp::bind_actions(cx);

        cx.spawn(async move |cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.0), px(100.0)),
                    size(px(1280.0), px(800.0)),
                ))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Surge".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let state = cx.new(|_| AppState::new());
                let view = cx.new(|cx| SurgeApp::new(state, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });
}
