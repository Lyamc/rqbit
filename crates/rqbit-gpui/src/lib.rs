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
pub mod handlers;
pub mod launch;
#[cfg(not(target_family = "wasm"))]
mod logging;
pub mod sources;
mod store;
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
    /// Default: the server last connected to, else http://127.0.0.1:3030.
    #[arg(long, env = "RQBIT_GUI_URL")]
    pub url: Option<String>,

    /// Register rqbit (for the current user) as the handler for magnet links
    /// and .torrent files, then exit. On Windows 10/11 this opens Settings >
    /// Default apps, where you confirm rqbit.
    #[arg(long, conflicts_with = "unregister_handlers")]
    pub register_handlers: bool,

    /// Undo --register-handlers, then exit.
    #[arg(long)]
    pub unregister_handlers: bool,

    /// Magnet links, http(s) .torrent URLs or .torrent files to add. If rqbit
    /// is already running they are handed to that window.
    #[arg(value_name = "MAGNET_OR_TORRENT")]
    pub open: Vec<String>,
}

/// Default server URL when none is given: the last one connected to.
pub const DEFAULT_URL: &str = "http://127.0.0.1:3030";

/// Opens the window and runs the GPUI event loop on the current thread
/// (must be the main thread, and must not be inside a tokio runtime).
#[cfg(not(target_family = "wasm"))]
pub fn run(opts: GuiOpts) -> anyhow::Result<()> {
    use std::cell::RefCell;
    use std::rc::Rc;

    logging::init();

    if opts.register_handlers || opts.unregister_handlers {
        let l = handlers::register::Launcher::current()?;
        if opts.unregister_handlers {
            handlers::register::unregister(&l);
            println!("rqbit is no longer registered for magnet links and .torrent files.");
            return Ok(());
        }
        let notes = handlers::register::register(&l).map_err(anyhow::Error::msg)?;
        for n in notes {
            println!("{n}");
        }
        println!("{}", handlers::register::status(&l).describe());
        if cfg!(windows) {
            println!(
                "Windows doesn't let apps make themselves the default: in the Settings page that just opened, choose rqbit for MAGNET and .torrent."
            );
            handlers::register::open_default_apps_settings();
        }
        return Ok(());
    }

    let items: Vec<launch::LaunchItem> = opts
        .open
        .iter()
        .filter_map(|a| {
            let item = launch::classify(a);
            if item.is_none() {
                log::warn!("ignoring argument {a:?}: not a magnet link, URL or .torrent file");
            }
            item
        })
        .collect();
    if !items.is_empty() && handlers::forward(&items) {
        log::info!("handed {} item(s) to the running rqbit window", items.len());
        return Ok(());
    }
    handlers::listen();
    launch::send(items);

    let url = opts
        .url
        .clone()
        .or_else(|| store::get("last_url"))
        .unwrap_or_else(|| DEFAULT_URL.to_owned());
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

        match open_main_window(cx, transport, url) {
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
