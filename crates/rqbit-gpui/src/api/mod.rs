//! Blocking HTTP client for the rqbit API.
//!
//! GPUI has its own async executor (not tokio), so instead of running a tokio
//! runtime next to it we use `reqwest::blocking` and call it from GPUI's
//! background executor threads. Every method here blocks; never call them on
//! the UI thread.

pub mod types;

use std::time::Duration;

use anyhow::{Context, bail};
use url::Url;

pub use types::*;

/// A cheap-to-clone handle to one rqbit server.
#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::blocking::Client,
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
    /// Creates the shared HTTP client. Must be called outside of any tokio
    /// runtime (reqwest::blocking owns its own).
    pub fn new_http() -> anyhow::Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("rqbit-gpui/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("error building HTTP client")
    }

    pub fn new(http: reqwest::blocking::Client, mut base: Url) -> Self {
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
            http,
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

    fn send(&self, req: reqwest::blocking::RequestBuilder) -> anyhow::Result<Vec<u8>> {
        let req = match &self.basic_auth {
            Some((u, p)) => req.basic_auth(u, Some(p)),
            None => req,
        };
        let resp = req.send().map_err(describe_reqwest_error)?;
        let status = resp.status();
        let body = resp.bytes().map_err(describe_reqwest_error)?.to_vec();
        if !status.is_success() {
            let text = String::from_utf8_lossy(&body);
            let msg = serde_json::from_slice::<ApiErrorBody>(&body)
                .ok()
                .and_then(|e| e.human_readable)
                .unwrap_or_else(|| text.trim().to_owned());
            if status == reqwest::StatusCode::UNAUTHORIZED {
                bail!(
                    "HTTP 401 Unauthorized: use http://user:pass@host:port if the server has basic auth enabled"
                );
            }
            if msg.is_empty() {
                bail!("HTTP {status}");
            }
            bail!("HTTP {status}: {msg}");
        }
        Ok(body)
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let body = self.send(self.http.get(self.url(path)?))?;
        serde_json::from_slice(&body).with_context(|| {
            format!(
                "unexpected response from {path} (is this an rqbit server?): {}",
                String::from_utf8_lossy(&body[..body.len().min(200)])
            )
        })
    }

    fn post(&self, path: &str) -> anyhow::Result<()> {
        self.send(self.http.post(self.url(path)?))?;
        Ok(())
    }

    /// `GET /torrents?with_stats=true` — the same bulk call the web UI polls.
    pub fn list_torrents(&self) -> anyhow::Result<ListTorrentsResponse> {
        self.get_json("torrents?with_stats=true")
    }

    pub fn start(&self, id: usize) -> anyhow::Result<()> {
        self.post(&format!("torrents/{id}/start"))
    }

    pub fn pause(&self, id: usize) -> anyhow::Result<()> {
        self.post(&format!("torrents/{id}/pause"))
    }

    /// Removes the torrent from the session but keeps the downloaded files
    /// (`/forget`, as opposed to `/delete`).
    pub fn forget(&self, id: usize) -> anyhow::Result<()> {
        self.post(&format!("torrents/{id}/forget"))
    }
}

fn describe_reqwest_error(e: reqwest::Error) -> anyhow::Error {
    use std::error::Error;
    let mut msg = if e.is_connect() {
        "connection failed".to_owned()
    } else if e.is_timeout() {
        "request timed out".to_owned()
    } else {
        e.to_string()
    };
    // Surface the root cause (e.g. "Connection refused (os error 111)").
    let mut src = e.source();
    let mut last = None;
    while let Some(s) = src {
        last = Some(s.to_string());
        src = s.source();
    }
    if let Some(root) = last
        && !msg.contains(&root)
    {
        msg = format!("{msg}: {root}");
    }
    anyhow::anyhow!(msg)
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
        let http = ApiClient::new_http().unwrap();
        let c = ApiClient::new(http, parse_base_url("http://u:p@h:1").unwrap());
        assert_eq!(c.base_url().as_str(), "http://h:1/");
        assert_eq!(c.basic_auth, Some(("u".into(), "p".into())));
        assert_eq!(
            c.url("torrents/5/start").unwrap().as_str(),
            "http://h:1/torrents/5/start"
        );
    }
}
