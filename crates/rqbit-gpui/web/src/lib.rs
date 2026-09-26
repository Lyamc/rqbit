//! wasm32 entry point: runs the rqbit GPUI client in the browser on GPUI's web
//! platform (canvas + WebGPU/WebGL2), talking to the rqbit API with `fetch`.
//!
//! When served by rqbit under `/gpui/`, the API is the same origin, so no CORS
//! or extra credentials are needed.

#[cfg(target_family = "wasm")]
mod web {
    use std::borrow::Cow;
    use std::rc::Rc;
    use std::sync::Arc;

    use gpui::{App, Application};
    use wasm_bindgen::prelude::*;

    /// API base URL: the page URL up to (excluding) the `/gpui` path segment.
    fn api_base_url() -> String {
        let Some(location) = web_sys::window().map(|w| w.location()) else {
            return "http://127.0.0.1:3030".into();
        };
        let origin = location.origin().unwrap_or_default();
        let path = location.pathname().unwrap_or_default();
        let prefix = match path.find("/gpui") {
            Some(idx) => &path[..idx],
            None => "",
        };
        format!("{origin}{prefix}")
    }

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        gpui_web::init_logging();

        // Single threaded: no SharedArrayBuffer, so no COOP/COEP headers and no
        // nightly `build-std` are needed.
        let platform = Rc::new(gpui_web::WebPlatform::new(false));
        let http = Arc::new(platform.fetch_http_client());
        Application::with_platform(platform)
            .with_http_client(http)
            .run(|cx: &mut App| {
                // Browsers don't expose system fonts to the canvas renderer, so
                // bundle the UI font. GPUI maps its default UI font (.ZedSans)
                // to IBM Plex Sans (SIL OFL 1.1, see fonts/).
                if let Err(e) = cx.text_system().add_fonts(vec![
                    Cow::Borrowed(include_bytes!("../fonts/IBMPlexSans-Regular.ttf")),
                    Cow::Borrowed(include_bytes!("../fonts/IBMPlexSans-SemiBold.ttf")),
                ]) {
                    log::error!("error loading fonts: {e:#}");
                }
                let transport = rqbit_gpui::Transport::new(cx.http_client());
                let url = api_base_url();
                log::info!("rqbit gpui web: API at {url}");
                if let Err(e) = rqbit_gpui::open_main_window(cx, transport, url) {
                    log::error!("error opening the rqbit window: {e:#}");
                }
            });
    }
}
