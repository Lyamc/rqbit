//! Serves the browser (wasm32) build of the GPUI client under `/gpui/`.
//!
//! The files are not embedded: they are read from a directory at request time,
//! so building rqbit does not need the wasm toolchain. Build them with
//! `crates/rqbit-gpui/web/build.sh` and copy `dist/*` into the directory given
//! by `RQBIT_GPUI_WEB_DIR`, or by default `$XDG_DATA_HOME/rqbit/gpui-web`
//! (`~/.local/share/rqbit/gpui-web`).

use std::path::PathBuf;

use axum::{
    Router,
    extract::Path,
    response::{IntoResponse, Response},
    routing::get,
};
use http::{HeaderMap, HeaderValue, StatusCode, header};

pub fn gpui_web_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RQBIT_GPUI_WEB_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|d| !d.is_empty())
                .map(|h| PathBuf::from(h).join(".local/share"))
        })?;
    Some(data_home.join("rqbit").join("gpui-web"))
}

/// Only flat file names: the dist folder has no subdirectories, and this rules
/// out path traversal.
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|enc| enc.split(';').next().map(str::trim) == Some("gzip"))
}

async fn serve_file(name: &str, headers: &HeaderMap) -> Response {
    let Some(dir) = gpui_web_dir() else {
        return (
            StatusCode::NOT_FOUND,
            "GPUI web client directory not configured",
        )
            .into_response();
    };
    if !is_safe_name(name) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = dir.join(name);
    let content_type = mime_guess::from_path(&path)
        .first_raw()
        .unwrap_or("application/octet-stream");

    // Prefer a precompressed sibling (name.gz) when the client accepts gzip.
    let mut body = None;
    let mut gzipped = false;
    if accepts_gzip(headers) {
        let mut gz = path.clone().into_os_string();
        gz.push(".gz");
        if let Ok(b) = tokio::fs::read(&gz).await {
            body = Some(b);
            gzipped = true;
        }
    }
    let body = match body {
        Some(b) => b,
        None => match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let msg = if name == "index.html" {
                    format!(
                        "GPUI web client not installed. Build crates/rqbit-gpui/web and copy dist/* to {dir:?} (or set RQBIT_GPUI_WEB_DIR)."
                    )
                } else {
                    "not found".to_owned()
                };
                return (StatusCode::NOT_FOUND, msg).into_response();
            }
            Err(e) => {
                tracing::warn!(?path, "error reading GPUI web file: {e:#}");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        },
    };

    let mut resp = body.into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    // Files keep their names across rebuilds; always revalidate.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if gzipped {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    }
    resp
}

pub fn make_gpui_web_router() -> Router {
    Router::new()
        .route(
            "/",
            get(|headers: HeaderMap| async move { serve_file("index.html", &headers).await }),
        )
        .route(
            "/{name}",
            get(|Path(name): Path<String>, headers: HeaderMap| async move {
                serve_file(&name, &headers).await
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names() {
        assert!(is_safe_name("rqbit_gpui_web_bg.wasm"));
        assert!(is_safe_name("index.html"));
        assert!(!is_safe_name(".."));
        assert!(!is_safe_name(".hidden"));
        assert!(!is_safe_name("a/b"));
        assert!(!is_safe_name("..%2f"));
        assert!(!is_safe_name(""));
    }

    #[test]
    fn gzip_accept() {
        let mut h = HeaderMap::new();
        assert!(!accepts_gzip(&h));
        h.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip;q=0.9"),
        );
        assert!(accepts_gzip(&h));
        h.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzipx"));
        assert!(!accepts_gzip(&h));
    }
}
