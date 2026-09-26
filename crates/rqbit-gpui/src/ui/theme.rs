//! Colours. A small dark palette roughly matching the web UI's dark theme.

use gpui::{Hsla, Rgba, rgb, rgba};

pub fn bg() -> Rgba {
    rgb(0x18181b)
}
pub fn surface() -> Rgba {
    rgb(0x1f1f23)
}
pub fn surface_hover() -> Rgba {
    rgb(0x27272a)
}
pub fn border() -> Rgba {
    rgb(0x3f3f46)
}
pub fn text() -> Rgba {
    rgb(0xe4e4e7)
}
pub fn text_muted() -> Rgba {
    rgb(0xa1a1aa)
}
pub fn primary() -> Rgba {
    rgb(0x3b82f6)
}
pub fn success() -> Rgba {
    rgb(0x22c55e)
}
pub fn warning() -> Rgba {
    rgb(0xf59e0b)
}
pub fn error() -> Rgba {
    rgb(0xef4444)
}
pub fn error_bg() -> Rgba {
    rgba(0xef44442e)
}
pub fn overlay() -> Hsla {
    gpui::hsla(0.0, 0.0, 0.0, 0.55)
}

/// Colour for a server `status_detail.kind`.
pub fn status_color(kind: &str) -> Rgba {
    match kind {
        "downloading" | "resolving_metadata" | "queued_for_downloading" => primary(),
        "seeding" | "complete" | "queued_for_seeding" => success(),
        "error" | "needs_attention" => error(),
        "checking"
        | "queued_for_checking"
        | "initializing"
        | "repairing"
        | "queued_for_repair"
        | "waiting_to_retry"
        | "moving"
        | "renaming"
        | "stalled" => warning(),
        _ => text_muted(),
    }
}
