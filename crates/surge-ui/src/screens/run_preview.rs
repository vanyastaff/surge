//! Embedded native preview for static applications.

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod native {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use gpui_kit::component::button::{Button, ButtonVariants};
    use gpui_kit::component::{Disableable, StyledExt};
    use gpui_kit::*;
    use surge_core::RunId;
    use wry::http::{HeaderValue, Request, Response, StatusCode};

    use crate::screens::preview_assets::PreviewAssets;
    use crate::theme;

    const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; object-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'self'";

    pub(super) fn navigation_allowed(url: &str, origin: &str) -> bool {
        if url == "about:blank" {
            return true;
        }
        same_origin(url, origin)
    }

    fn same_origin(url: &str, origin: &str) -> bool {
        let Ok(uri) = url.parse::<wry::http::Uri>() else {
            return false;
        };
        let Ok(expected) = origin.parse::<wry::http::Uri>() else {
            return false;
        };
        uri.scheme() == expected.scheme() && uri.authority() == expected.authority()
    }

    fn response(status: u16, mime: &'static str, body: Vec<u8>, head: bool) -> Response<Vec<u8>> {
        let length = body.len();
        let mut response = Response::new(if head { Vec::new() } else { body });
        *response.status_mut() =
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let headers = response.headers_mut();
        headers.insert("content-type", HeaderValue::from_static(mime));
        headers.insert("content-security-policy", HeaderValue::from_static(CSP));
        headers.insert(
            "x-content-type-options",
            HeaderValue::from_static("nosniff"),
        );
        headers.insert("cache-control", HeaderValue::from_static("no-store"));
        headers.insert("content-length", HeaderValue::from(length));
        response
    }

    fn asset_response(
        assets: &PreviewAssets,
        request: &Request<Vec<u8>>,
        origin: &str,
    ) -> Response<Vec<u8>> {
        let head = request.method() == wry::http::Method::HEAD;
        if !same_origin(&request.uri().to_string(), origin) {
            return response(403, "text/plain", b"Preview origin refused".to_vec(), head);
        }
        if request.method() != wry::http::Method::GET && !head {
            return response(
                405,
                "text/plain",
                b"Only GET and HEAD are supported".to_vec(),
                false,
            );
        }
        match assets.load(request.uri().path()) {
            Ok(asset) => response(200, asset.mime, asset.bytes, head),
            Err(error) => response(
                error.status,
                "text/plain",
                error.message.as_bytes().to_vec(),
                head,
            ),
        }
    }

    fn build_webview(
        assets: Arc<PreviewAssets>,
        origin: String,
        closed: Arc<AtomicBool>,
        window: &Window,
    ) -> Result<wry::WebView, String> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|error| error.to_string())?;
        let permits = Arc::new(tokio::sync::Semaphore::new(8));
        #[cfg(target_os = "windows")]
        let navigation_origin = origin.replacen("surge-preview://", "http://surge-preview.", 1);
        #[cfg(target_os = "macos")]
        let navigation_origin = origin.clone();
        wry::WebViewBuilder::new()
            .with_incognito(true)
            .with_devtools(false)
            .with_navigation_handler(move |url| navigation_allowed(&url, &navigation_origin))
            .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
            .with_download_started_handler(|_, _| false)
            .with_drag_drop_handler(|_| true)
            .with_asynchronous_custom_protocol(
                "surge-preview".into(),
                move |_, request, responder| {
                    let Ok(permit) = permits.clone().try_acquire_owned() else {
                        responder.respond(response(
                            503,
                            "text/plain",
                            b"Preview request limit reached".to_vec(),
                            false,
                        ));
                        return;
                    };
                    let assets = assets.clone();
                    let origin = origin.clone();
                    let closed = closed.clone();
                    runtime.spawn_blocking(move || {
                        let _permit = permit;
                        let result = if closed.load(Ordering::Acquire) {
                            response(410, "text/plain", b"Preview closed".to_vec(), false)
                        } else {
                            asset_response(&assets, &request, &origin)
                        };
                        if !closed.load(Ordering::Acquire) {
                            responder.respond(result);
                        }
                    });
                },
            )
            .build_as_child(window)
            .map_err(|error| error.to_string())
    }

    pub(in super::super) struct PreviewView {
        origin: String,
        status: String,
        pending: Option<Arc<PreviewAssets>>,
        webview: Option<Entity<gpui_wry::WebView>>,
        navigation: InitialNavigation,
        closed: Arc<AtomicBool>,
    }

    #[derive(Default)]
    enum InitialNavigation {
        #[default]
        WaitingForLayout,
        Started,
        Closed,
    }

    impl InitialNavigation {
        fn start(&mut self, bounds: Bounds<Pixels>) -> bool {
            if !matches!(self, Self::WaitingForLayout)
                || bounds.size.width <= px(0.0)
                || bounds.size.height <= px(0.0)
            {
                return false;
            }
            *self = Self::Started;
            true
        }
    }

    impl PreviewView {
        pub(in super::super) fn new(
            run_id: RunId,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let host = run_id.to_string().to_lowercase();
            let origin = format!("surge-preview://{host}");
            let weak = cx.entity().downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                let _ = weak.update(cx, |view, cx| view.close(cx));
                true
            });
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let result = load_assets(run_id).await;
                cx.update(|cx| { let _ = this.update(cx, |view, cx| {
                    if view.closed.load(Ordering::Acquire) { return; }
                    match result {
                        Ok(assets) => { view.pending = Some(Arc::new(assets)); view.status = "Opening static application…".into(); },
                        Err(error) => view.status = format!("Static preview unavailable: {error}. Reopen Preview after index.html is ready."),
                    }
                    cx.notify();
                }); });
            }).detach();
            Self {
                origin,
                status: "Locating static application…".into(),
                pending: None,
                webview: None,
                navigation: InitialNavigation::default(),
                closed: Arc::new(AtomicBool::new(false)),
            }
        }

        pub(in super::super) fn close(&mut self, cx: &mut Context<Self>) {
            self.closed.store(true, Ordering::Release);
            self.navigation = InitialNavigation::Closed;
            self.pending = None;
            if let Some(webview) = self.webview.take() {
                webview.update(cx, |view, _| view.hide());
            }
        }
    }

    impl Drop for PreviewView {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Release);
        }
    }

    async fn load_assets(run_id: RunId) -> Result<PreviewAssets, String> {
        let home = surge_core::home::surge_home_dir().ok_or("Surge home unavailable")?;
        let path = super::super::load_result_folder(home, run_id).await?;
        tokio::task::spawn_blocking(move || {
            let assets = PreviewAssets::new(path)?;
            assets
                .load("/index.html")
                .map_err(|error| error.message.to_string())?;
            Ok(assets)
        })
        .await
        .map_err(|error| error.to_string())?
    }

    impl Render for PreviewView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if let Some(assets) = self.pending.take() {
                match build_webview(assets, self.origin.clone(), self.closed.clone(), window) {
                    Ok(view) => {
                        self.webview = Some(cx.new(|cx| gpui_wry::WebView::new(view, window, cx)));
                        // Wry starts a builder URL before attaching its native child. The
                        // GPUI wrapper then sets zero bounds until prepaint. Navigate only
                        // after that first layout, rather than loading into a zero-size view.
                        let weak = cx.entity().downgrade();
                        window.on_next_frame(move |_, cx| {
                            let _ = weak.update(cx, |view, cx| {
                                if view.closed.load(Ordering::Acquire) {
                                    return;
                                }
                                let Some(webview) = &view.webview else { return; };
                                let bounds = webview.read(cx).bounds();
                                if !view.navigation.start(bounds) {
                                    view.status = "Cannot open embedded preview: preview layout has no visible area. Reopen Preview to try again.".into();
                                } else {
                                    match webview.read(cx).raw().load_url(&format!("{}/index.html", view.origin)) {
                                        Ok(()) => view.status = "Static application · local files · backend services are not started".into(),
                                        Err(error) => view.status = format!("Cannot load embedded preview: {error}"),
                                    }
                                }
                                cx.notify();
                            });
                        });
                    },
                    Err(error) => self.status = format!("Cannot open embedded preview: {error}"),
                }
            }
            div()
                .flex_1()
                .min_h_0()
                .v_flex()
                .gap(px(8.0))
                .p(px(12.0))
                .child(
                    div()
                        .h_flex()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(11.0))
                                .text_color(theme::text_muted())
                                .child(self.status.clone()),
                        )
                        .child(
                            Button::new("reload-preview")
                                .ghost()
                                .label("Reload preview")
                                .disabled(self.webview.is_none())
                                .on_click(cx.listener(|view, _, _, cx| {
                                    if let Some(webview) = &view.webview
                                        && let Err(error) = webview.read(cx).raw().reload()
                                    {
                                        view.status = format!("Preview reload failed: {error}");
                                        cx.notify();
                                    }
                                })),
                        ),
                )
                .child(if let Some(webview) = &self.webview {
                    div().flex_1().min_h_0().child(webview.clone())
                } else {
                    div().flex_1().min_h_0()
                })
        }
    }
    #[cfg(test)]
    mod protocol_tests {
        use super::{
            Arc, AtomicBool, CSP, Ordering, PreviewAssets, PreviewView, Request, StatusCode,
            asset_response, navigation_allowed, same_origin,
        };

        #[test]
        fn assets_apply_origin_method_head_and_security_headers() {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("index.html"), "<p>app</p>").unwrap();
            let assets = PreviewAssets::new(root.path().into()).unwrap();
            let origin = "surge-preview://run-one";
            let request = |method, uri| {
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(Vec::new())
                    .unwrap()
            };
            let get = asset_response(
                &assets,
                &request("GET", "surge-preview://run-one/index.html"),
                origin,
            );
            assert_eq!(get.status(), StatusCode::OK);
            assert_eq!(get.body(), b"<p>app</p>");
            assert_eq!(get.headers()["x-content-type-options"], "nosniff");
            assert_eq!(get.headers()["content-security-policy"], CSP);
            assert_eq!(get.headers()["cache-control"], "no-store");
            let head = asset_response(
                &assets,
                &request("HEAD", "surge-preview://run-one/index.html"),
                origin,
            );
            assert_eq!(head.status(), StatusCode::OK);
            assert!(head.body().is_empty());
            assert_eq!(head.headers()["content-length"], "10");
            assert_eq!(
                asset_response(
                    &assets,
                    &request("POST", "surge-preview://run-one/index.html"),
                    origin
                )
                .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
            assert_eq!(
                asset_response(
                    &assets,
                    &request("GET", "surge-preview://run-two/index.html"),
                    origin
                )
                .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                asset_response(
                    &assets,
                    &request("GET", "surge-preview://run-one/missing.js"),
                    origin
                )
                .status(),
                StatusCode::NOT_FOUND
            );
        }

        #[test]
        fn relative_styles_and_scripts_are_served_at_the_document_origin() {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(
                root.path().join("index.html"),
                "<link rel=stylesheet href=styles.css><script src=app.js></script>",
            )
            .unwrap();
            std::fs::write(root.path().join("styles.css"), "body { color: red; }").unwrap();
            std::fs::write(root.path().join("app.js"), "document.title = 'Ready';").unwrap();
            let assets = PreviewAssets::new(root.path().into()).unwrap();
            let origin = "surge-preview://run-one";
            for (path, mime, expected) in [
                ("index.html", "text/html; charset=utf-8", "<link"),
                ("styles.css", "text/css; charset=utf-8", "body"),
                ("app.js", "text/javascript; charset=utf-8", "document.title"),
            ] {
                let request = Request::builder()
                    .uri(format!("{origin}/{path}"))
                    .body(Vec::new())
                    .unwrap();
                let result = asset_response(&assets, &request, origin);
                assert_eq!(result.status(), StatusCode::OK);
                assert_eq!(result.headers()["content-type"], mime);
                assert!(
                    std::str::from_utf8(result.body())
                        .unwrap()
                        .starts_with(expected)
                );
            }
        }

        #[test]
        fn first_navigation_requires_layout_runs_once_and_cannot_outlive_close() {
            use super::InitialNavigation;
            use gpui_kit::{Bounds, point, px, size};
            let bounds =
                |width, height| Bounds::new(point(px(12.0), px(40.0)), size(px(width), px(height)));
            let mut navigation = InitialNavigation::default();
            for (width, height) in [(0.0, 0.0), (600.0, 0.0), (0.0, 400.0)] {
                assert!(!navigation.start(bounds(width, height)));
            }
            assert!(navigation.start(bounds(600.0, 400.0)));
            assert!(!navigation.start(bounds(600.0, 400.0)));
            let mut closed = InitialNavigation::Closed;
            assert!(!closed.start(bounds(600.0, 400.0)));
        }

        #[test]
        fn windows_navigation_origin_is_separate_from_restored_protocol_origin() {
            let origin = "http://surge-preview.run-one";
            assert!(navigation_allowed(
                "http://surge-preview.run-one/index.html",
                origin
            ));
            assert!(!navigation_allowed(
                "http://surge-preview.run-two/index.html",
                origin
            ));
            assert!(!navigation_allowed(
                "http://surge-preview.run-one.evil/index.html",
                origin
            ));
            assert!(!same_origin(
                "http://surge-preview.run-one/index.html",
                "surge-preview://run-one"
            ));
            assert!(same_origin(
                "surge-preview://run-one/index.html",
                "surge-preview://run-one"
            ));
        }

        #[test]
        fn close_marks_pending_preview_cancelled_without_creating_native_view() {
            use gpui_kit::AppContext as _;
            let mut cx = gpui_kit::TestAppContext::single();
            let closed = Arc::new(AtomicBool::new(false));
            let view = cx.new(|_| PreviewView {
                origin: "surge-preview://test".into(),
                status: "Loading".into(),
                pending: None,
                webview: None,
                navigation: super::InitialNavigation::default(),
                closed: closed.clone(),
            });
            view.update(&mut cx, |view, cx| view.close(cx));
            assert!(closed.load(Ordering::Acquire));
            view.update(&mut cx, |view, _| {
                assert!(!view.navigation.start(gpui_kit::Bounds::new(
                    gpui_kit::point(gpui_kit::px(0.0), gpui_kit::px(0.0)),
                    gpui_kit::size(gpui_kit::px(600.0), gpui_kit::px(400.0)),
                )));
            });
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) use native::PreviewView;
#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
use native::navigation_allowed;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use gpui_kit::*;
    pub(in super::super) struct PreviewView;
    impl PreviewView {
        pub(in super::super) fn new(
            _: surge_core::RunId,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> Self {
            Self
        }
        pub(in super::super) fn close(&mut self, _: &mut Context<Self>) {}
    }
    impl Render for PreviewView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child("Embedded preview is currently supported on macOS and Windows.")
        }
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(super) use unsupported::PreviewView;

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    #[test]
    fn navigation_is_confined_to_exact_preview_origin() {
        let origin = "surge-preview://run-one";
        assert!(super::navigation_allowed(
            "surge-preview://run-one/index.html",
            origin
        ));
        assert!(super::navigation_allowed("about:blank", origin));
        for url in [
            "https://example.com",
            "file:///etc/passwd",
            "surge-preview://run-two/",
            "surge-preview://run-one.evil/",
            "surge-preview://run-one@evil/",
        ] {
            assert!(!super::navigation_allowed(url, origin), "{url}");
        }
    }
}
