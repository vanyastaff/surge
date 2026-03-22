mod app;
mod panels;
mod theme;

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "surge_ui=info".into()),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 600.0])
            .with_title("Surge — Agent Orchestrator"),
        ..Default::default()
    };

    eframe::run_native(
        "Surge",
        options,
        Box::new(|cc| Ok(Box::new(app::SurgeApp::new(cc)))),
    )
}
