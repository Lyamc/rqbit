//! Standalone GUI binary: `rqbit-gpui --url http://host:3030`.
//! Lighter than `rqbit --features gpui` because it doesn't compile the server.
//!
//! Windows: built as a GUI-subsystem program so opening a magnet link or a
//! .torrent file (default-handler launch) doesn't flash a console window.
//! When started from a terminal it attaches to that console, so `--help`,
//! `--register-handlers` etc. still print.

#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "rqbit-gpui",
    version,
    about = "Native desktop client for an rqbit server"
)]
struct Cli {
    #[command(flatten)]
    gui: rqbit_gpui::GuiOpts,
}

#[cfg(windows)]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: plain Win32 call; failure (no parent console) is fine.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    attach_parent_console();
    rqbit_gpui::run(Cli::parse().gui)
}
