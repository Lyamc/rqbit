//! Tiny persistent key/value store for UI state (e.g. the last seen event
//! seq for the Events badge). Browser: `localStorage`. Native: a JSON file
//! in the user's config directory (`rqbit-gpui/state.json`). Failures are
//! ignored: this is best-effort UI state.

#[cfg(target_family = "wasm")]
pub fn get(key: &str) -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()??
        .get_item(key)
        .ok()?
}

#[cfg(target_family = "wasm")]
pub fn set(key: &str, value: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(key, value);
    }
}

#[cfg(not(target_family = "wasm"))]
fn path() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    }?;
    Some(base.join("rqbit-gpui").join("state.json"))
}

#[cfg(not(target_family = "wasm"))]
fn load() -> serde_json::Map<String, serde_json::Value> {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

#[cfg(not(target_family = "wasm"))]
pub fn get(key: &str) -> Option<String> {
    load().get(key)?.as_str().map(str::to_owned)
}

#[cfg(not(target_family = "wasm"))]
pub fn set(key: &str, value: &str) {
    let Some(p) = path() else { return };
    let mut m = load();
    m.insert(key.to_owned(), value.into());
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(b) = serde_json::to_vec_pretty(&m) {
        let _ = std::fs::write(p, b);
    }
}
