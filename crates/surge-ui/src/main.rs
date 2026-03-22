mod actions;
mod app;
mod command_palette;
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

fn main() {
    let app = Application::new();

    app.run(move |cx| {
        gpui_component::init(cx);
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
                let view = cx.new(SurgeApp::new);
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });
}
