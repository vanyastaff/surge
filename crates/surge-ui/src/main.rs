// UI code under development - suppress dead code warnings temporarily
#![allow(dead_code)]
#![allow(unused_variables)]
// Pre-existing legacy code; M5 does not modify surge-ui.  Suppress pedantic
// lints that activate because -D clippy::pedantic is now applied workspace-wide.
#![allow(clippy::excessive_nesting)]
#![allow(clippy::ptr_arg)]

mod actions;
mod agent_usage;
mod app;
mod assets;
mod app_state;
mod backlog_source;
mod command_palette;
mod config_edit;
mod daemon_link;
mod decisions;
mod dismissed;
mod flow_diagram;
mod flow_levels;
mod flow_review;
mod markdown;
mod memory_vault;
mod mission;
mod notifications;
mod project;
mod project_init;
mod roadmap_source;
mod router;
mod run_stream;
mod screens;
mod sidebar;
mod theme;
mod top_bar;
mod ui;

use gpui_kit::*;

use app::SurgeApp;
use app_state::AppState;

fn main() {
    // `from_default_env()` with no `RUST_LOG` set builds an *empty* filter:
    // the app then logs nothing at all, errors included, and a user whose
    // run misbehaves has not one line to look at. Fall back to the same
    // default `surge-cli` uses so the desktop app is not the silent one.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "surge=info".into()),
        )
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

    let app = gpui_kit::application().with_assets(assets::AppAssets);

    app.run(move |cx| {
        gpui_kit::init(cx);
        // The persisted appearance, pushed into gpui-component too so its
        // chrome (TitleBar, buttons, inputs) matches instead of following
        // the OS appearance.
        theme::init();
        theme::sync_component_theme(cx);
        SurgeApp::bind_actions(cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.0), px(100.0)),
                    size(px(1280.0), px(800.0)),
                ))),
                // Client-side decorations: we draw our own TitleBar (drag +
                // min/max/close) and gpui-component's Root renders the resize
                // border + shadow via window_border(). Server decorations
                // aren't shown by this Wayland compositor, so the window was
                // unmanageable without this.
                titlebar: Some(gpui_kit::component::TitleBar::title_bar_options()),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("surge".into()),
                window_min_size: Some(size(px(960.0), px(640.0))),
                ..Default::default()
            };

            if let Err(err) = gpui_kit::open_window(options, cx, |_, cx| {
                    let state = cx.new(|_| AppState::new());
                    cx.new(|cx| SurgeApp::new(state, cx))
            }) {
                // Reproducible on a compositor whose renderer gpui can't
                // use (observed: "Failed to create surface:
                // PlatformNotSupported"). No retry, no fallback renderer —
                // just tell the user what happened and exit instead of a
                // raw panic + backtrace.
                tracing::error!("failed to open window: {err:#}");
                eprintln!(
                    "surge: could not open a window ({err}) — likely no usable GPU/compositor surface; try a different compositor or update your GPU driver"
                );
                std::process::exit(1);
            }
    });
}
