//! Desktop (and browser) client for rqbit, built on [GPUI](https://gpui.rs).
//!
//! The GUI is a pure client of the rqbit HTTP API (the same API the web UI
//! uses), so it can run on another machine than the server. It never touches
//! the session or the filesystem directly.
//!
//! Layout:
//! - [`api`]: HTTP client (native reqwest / browser fetch) + serde types
//!   mirroring the web UI's `api-types.ts`.
//! - [`format`]: human readable formatting shared by views.
//! - `ui`: GPUI views. Port further web UI features as new views there.
//!
//! Native entry point: [`run`]. Browser entry point: `crates/rqbit-gpui/web`,
//! which calls [`open_main_window`] on GPUI's web platform.

pub mod api;
pub mod format;
#[cfg(not(target_family = "wasm"))]
mod logging;
pub mod sources;
mod time;
mod ui;

use gpui::{App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

pub use api::Transport;

/// Opens the main rqbit window connected to `url`.
pub fn open_main_window(cx: &mut App, transport: Transport, url: String) -> anyhow::Result<()> {
    let bounds = Bounds::centered(None, size(px(1180.), px(680.)), cx);
    cx.open_window(
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
        move |window, cx| cx.new(|cx| ui::RqbitWindow::new(transport, url, window, cx)),
    )?;
    Ok(())
}

/// Command line options for the GUI (`rqbit gui ...` or the standalone
/// `rqbit-gpui` binary).
#[cfg(not(target_family = "wasm"))]
#[derive(clap::Args, Debug, Clone)]
pub struct GuiOpts {
    /// rqbit HTTP API URL to connect to. Can be changed in the window.
    /// Accepts http(s)://[user:pass@]host:port[/prefix].
    #[arg(long, env = "RQBIT_GUI_URL", default_value = "http://127.0.0.1:3030")]
    pub url: String,
}

/// Opens the window and runs the GPUI event loop on the current thread
/// (must be the main thread, and must not be inside a tokio runtime).
#[cfg(not(target_family = "wasm"))]
pub fn run(opts: GuiOpts) -> anyhow::Result<()> {
    use std::cell::RefCell;
    use std::rc::Rc;

    logging::init();
    let transport = Transport::new()?;
    let open_error: Rc<RefCell<Option<anyhow::Error>>> = Default::default();
    let open_error_2 = open_error.clone();

    gpui::Application::new().run(move |cx: &mut App| {
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        match open_main_window(cx, transport, opts.url) {
            Ok(()) => cx.activate(true),
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
