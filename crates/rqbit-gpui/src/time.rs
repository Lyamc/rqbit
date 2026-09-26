//! `std::time::Instant` panics on wasm32-unknown-unknown; use `web-time` there.

#[cfg(not(target_family = "wasm"))]
pub use std::time::Instant;
#[cfg(target_family = "wasm")]
pub use web_time::Instant;
