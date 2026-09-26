//! Standalone GUI binary: `rqbit-gpui --url http://host:3030`.
//! Lighter than `rqbit --features gpui` because it doesn't compile the server.

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

fn main() -> anyhow::Result<()> {
    rqbit_gpui::run(Cli::parse().gui)
}
