//! HTTP client for the rqbit API.
//!
//! The request/response logic is shared; only the transport differs:
//! - native: `reqwest::blocking`, polled on GPUI's background executor threads
//!   (GPUI has its own executor, so no tokio runtime is started);
//! - browser (wasm32): GPUI's HTTP client, which on the web platform is the
//!   browser's `fetch`.
//!
//! All methods return `'static` futures; spawn them with
//! `cx.background_executor().spawn(..)`.

pub mod types;

#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod web;

#[cfg(not(target_family = "wasm"))]
pub use native::Transport;
#[cfg(target_family = "wasm")]
pub use web::Transport;

use anyhow::{Context, bail};
use futures::FutureExt;
use futures::future::BoxFuture;
use url::Url;

pub use types::*;

pub type ApiFuture<T> = BoxFuture<'static, anyhow::Result<T>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

/// What a transport hands back: status code and body.
pub struct RawResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// A cheap-to-clone handle to one rqbit server.
#[derive(Clone)]
pub struct ApiClient {
    transport: Transport,
    base: Url,
    basic_auth: Option<(String, String)>,
}

/// Normalises what the user typed into the connection field into a base URL.
/// Accepts `host:port`, `http://host:port`, `https://host/prefix/` and
/// `http://user:pass@host:port` (basic auth, see RQBIT_HTTP_BASIC_AUTH_USERPASS).
pub fn parse_base_url(input: &str) -> anyhow::Result<Url> {
    let input = input.trim();
    if input.is_empty() {
        bail!("server URL is empty");
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("http://{input}")
    };
    let mut url = Url::parse(&with_scheme).with_context(|| format!("invalid URL {input:?}"))?;
    match url.scheme() {
        "http" | "https" => {}
        s => bail!("unsupported URL scheme {s:?} (use http:// or https://)"),
    }
    if url.host_str().is_none() {
        bail!("URL {input:?} has no host");
    }
    // Make relative joins keep any path prefix (reverse proxies).
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path());
        url.set_path(&p);
    }
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

impl ApiClient {
    pub fn new(transport: Transport, mut base: Url) -> Self {
        let basic_auth = if !base.username().is_empty() {
            let user = base.username().to_owned();
            let pass = base.password().unwrap_or_default().to_owned();
            let _ = base.set_username("");
            let _ = base.set_password(None);
            Some((user, pass))
        } else {
            None
        };
        Self {
            transport,
            base,
            basic_auth,
        }
    }

    /// Base URL without credentials, for display.
    pub fn base_url(&self) -> &Url {
        &self.base
    }

    fn url(&self, path: &str) -> anyhow::Result<Url> {
        self.base
            .join(path.trim_start_matches('/'))
            .with_context(|| format!("bad API path {path}"))
    }

    fn request(&self, method: Method, path: &str) -> ApiFuture<Vec<u8>> {
        let url = match self.url(path) {
            Ok(u) => u,
            Err(e) => return futures::future::ready(Err(e)).boxed(),
        };
        let fut = self.transport.send(method, url, self.basic_auth.clone());
        async move {
            let resp = fut.await?;
            check_status(resp.status, &resp.body)?;
            Ok(resp.body)
        }
        .boxed()
    }

    fn get_json<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
    ) -> ApiFuture<T> {
        let fut = self.request(Method::Get, path);
        let path = path.to_owned();
        async move {
            let body = fut.await?;
            serde_json::from_slice(&body).with_context(|| {
                format!(
                    "unexpected response from {path} (is this an rqbit server?): {}",
                    String::from_utf8_lossy(&body[..body.len().min(200)])
                )
            })
        }
        .boxed()
    }

    fn post(&self, path: &str) -> ApiFuture<()> {
        self.request(Method::Post, path)
            .map(|r| r.map(|_| ()))
            .boxed()
    }

    /// `GET /torrents?with_stats=true` — the same bulk call the web UI polls.
    pub fn list_torrents(&self) -> ApiFuture<ListTorrentsResponse> {
        self.get_json("torrents?with_stats=true")
    }

    pub fn start(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/start"))
    }

    pub fn pause(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/pause"))
    }

    /// Removes the torrent from the session but keeps the downloaded files
    /// (`/forget`, as opposed to `/delete`).
    pub fn forget(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/forget"))
    }
}

fn check_status(status: u16, body: &[u8]) -> anyhow::Result<()> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let text = String::from_utf8_lossy(body);
    let msg = serde_json::from_slice::<ApiErrorBody>(body)
        .ok()
        .and_then(|e| e.human_readable)
        .unwrap_or_else(|| text.trim().to_owned());
    if status == 401 {
        bail!(
            "HTTP 401 Unauthorized: use http://user:pass@host:port if the server has basic auth enabled"
        );
    }
    if msg.is_empty() {
        bail!("HTTP {status}");
    }
    bail!("HTTP {status}: {msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_normalisation() {
        assert_eq!(
            parse_base_url("localhost:3030").unwrap().as_str(),
            "http://localhost:3030/"
        );
        assert_eq!(
            parse_base_url(" https://example.com/rqbit ")
                .unwrap()
                .as_str(),
            "https://example.com/rqbit/"
        );
        assert!(parse_base_url("").is_err());
        assert!(parse_base_url("ftp://x").is_err());
    }

    #[test]
    fn credentials_are_split_out() {
        let c = ApiClient::new(
            Transport::new().unwrap(),
            parse_base_url("http://u:p@h:1").unwrap(),
        );
        assert_eq!(c.base_url().as_str(), "http://h:1/");
        assert_eq!(c.basic_auth, Some(("u".into(), "p".into())));
        assert_eq!(
            c.url("torrents/5/start").unwrap().as_str(),
            "http://h:1/torrents/5/start"
        );
    }

    #[test]
    fn status_errors() {
        assert!(check_status(200, b"").is_ok());
        let e = check_status(400, br#"{"human_readable":"nope"}"#).unwrap_err();
        assert_eq!(e.to_string(), "HTTP 400: nope");
        assert!(
            check_status(401, b"")
                .unwrap_err()
                .to_string()
                .contains("user:pass")
        );
    }
}
