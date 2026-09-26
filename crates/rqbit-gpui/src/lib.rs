//! Native desktop client for rqbit, built on [GPUI](https://gpui.rs).
//!
//! The GUI is a pure client of the rqbit HTTP API (the same API the web UI
//! uses), so it can run on another machine than the server. It never touches
//! the session or the filesystem directly.
//!
//! Layout:
//! - [`api`]: blocking HTTP client + serde types (mirrors the web UI's `api-types.ts`).
//! - [`format`]: human readable formatting shared by views.
//! - `ui`: GPUI views. Port further web UI features as new views there.

pub mod api;
pub mod format;
mod logging;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    App, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

/// Command line options for the GUI (`rqbit gui ...` or the standalone
/// `rqbit-gpui` binary).
#[derive(clap::Args, Debug, Clone)]
pub struct GuiOpts {
    /// rqbit HTTP API URL to connect to. Can be changed in the window.
    /// Accepts http(s)://[user:pass@]host:port[/prefix].
    #[arg(long, env = "RQBIT_GUI_URL", default_value = "http://127.0.0.1:3030")]
    pub url: String,
}

/// Opens the window and runs the GPUI event loop on the current thread
/// (must be the main thread, and must not be inside a tokio runtime).
pub fn run(opts: GuiOpts) -> anyhow::Result<()> {
    logging::init();
    let http = api::ApiClient::new_http()?;
    let open_error: Rc<RefCell<Option<anyhow::Error>>> = Default::default();
    let open_error_2 = open_error.clone();

    Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1180.), px(680.)), cx);
        let res = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("rqbit".into()),
                    ..Default::default()
                }),
                app_id: Some("rqbit".into()),
                window_min_size: Some(size(px(720.), px(360.))),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| ui::RqbitWindow::new(http, opts.url, window, cx)),
        );
        match res {
            Ok(_) => cx.activate(true),
            Err(e) => {
                *open_error_2.borrow_mut() = Some(e.context("error opening the rqbit window"));
                cx.quit();
            }
        }
    });

    match open_error.borrow_mut().take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
