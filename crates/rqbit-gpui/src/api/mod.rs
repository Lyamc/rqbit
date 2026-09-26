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

use std::time::Duration;

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

/// One HTTP request, as handed to a transport.
pub struct Request {
    pub method: Method,
    pub url: Url,
    pub basic_auth: Option<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub content_type: Option<&'static str>,
    /// `Accept` header (binary endpoints like `/haves` need one).
    pub accept: Option<&'static str>,
    /// Overall timeout (native only; the browser's fetch has none, callers
    /// that need one run their own timer, as the web UI does).
    pub timeout: Option<Duration>,
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
        match self.url(path) {
            Ok(url) => self.send(method, url, None, None),
            Err(e) => futures::future::ready(Err(e)).boxed(),
        }
    }

    fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<(Vec<u8>, &'static str)>,
        timeout: Option<Duration>,
    ) -> ApiFuture<Vec<u8>> {
        let (body, content_type) = match body {
            Some((b, ct)) => (Some(b), Some(ct)),
            None => (None, None),
        };
        let fut = self.transport.send(Request {
            method,
            url,
            basic_auth: self.basic_auth.clone(),
            body,
            content_type,
            accept: None,
            timeout,
        });
        async move {
            let resp = fut.await?;
            check_status(resp.status, &resp.body)?;
            Ok(resp.body)
        }
        .boxed()
    }

    fn post_json<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> ApiFuture<T> {
        let url = match self.url(path) {
            Ok(u) => u,
            Err(e) => return futures::future::ready(Err(e)).boxed(),
        };
        let body = body.map(|b| (b.to_string().into_bytes(), "application/json"));
        let fut = self.send(Method::Post, url, body, None);
        let path = path.to_owned();
        async move { parse_json(&path, &fut.await?) }.boxed()
    }

    /// POST a JSON body, ignoring the response body.
    fn post_json_unit(&self, path: &str, body: &serde_json::Value) -> ApiFuture<()> {
        let url = match self.url(path) {
            Ok(u) => u,
            Err(e) => return futures::future::ready(Err(e)).boxed(),
        };
        let body = (body.to_string().into_bytes(), "application/json");
        self.send(Method::Post, url, Some(body), None)
            .map(|r| r.map(|_| ()))
            .boxed()
    }

    fn get_json<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        path: &str,
    ) -> ApiFuture<T> {
        let fut = self.request(Method::Get, path);
        let path = path.to_owned();
        async move { parse_json(&path, &fut.await?) }.boxed()
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

    pub fn restart(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/restart"))
    }

    /// Soft re-check of a torrent in error state (web UI "Fix errors").
    pub fn fix_errors(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/fix_errors"))
    }

    /// Full re-hash of all pieces (`POST /torrents/{id}/recheck`).
    pub fn recheck(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/recheck"))
    }

    /// Repair damaged files (punch out unreadable ranges, re-download).
    /// `files` = None repairs all damaged files.
    pub fn repair_files(&self, id: usize, files: Option<Vec<usize>>) -> ApiFuture<()> {
        let body = match files {
            Some(f) => serde_json::json!({ "files": f }),
            None => serde_json::json!({}),
        };
        self.post_json_unit(&format!("torrents/{id}/repair_files"), &body)
    }

    /// Queue order: action is "top" | "up" | "down" | "bottom".
    pub fn queue_move(&self, ids: &[usize], action: &str) -> ApiFuture<()> {
        self.post_json_unit(
            "torrents/queue/move",
            &serde_json::json!({ "ids": ids, "action": action }),
        )
    }

    /// `GET /torrents/{id}`: name, files (with `included`), output folder.
    pub fn get_details(&self, id: usize) -> ApiFuture<TorrentDetails> {
        self.get_json(&format!("torrents/{id}"))
    }

    /// `GET /torrents/{id}/haves`: piece bitfield, MSB first.
    pub fn get_haves(&self, id: usize) -> ApiFuture<Vec<u8>> {
        let url = match self.url(&format!("torrents/{id}/haves")) {
            Ok(u) => u,
            Err(e) => return futures::future::ready(Err(e)).boxed(),
        };
        let fut = self.transport.send(Request {
            method: Method::Get,
            url,
            basic_auth: self.basic_auth.clone(),
            body: None,
            content_type: None,
            accept: Some("application/octet-stream"),
            timeout: None,
        });
        async move {
            let resp = fut.await?;
            check_status(resp.status, &resp.body)?;
            Ok(resp.body)
        }
        .boxed()
    }

    pub fn get_peer_stats(&self, id: usize) -> ApiFuture<PeerStatsSnapshot> {
        self.get_json(&format!("torrents/{id}/peer_stats?state=live"))
    }

    pub fn update_only_files(&self, id: usize, files: &[usize]) -> ApiFuture<()> {
        self.post_json_unit(
            &format!("torrents/{id}/update_only_files"),
            &serde_json::json!({ "only_files": files }),
        )
    }

    pub fn rename_file(&self, id: usize, file_id: usize, new_path: &str) -> ApiFuture<()> {
        self.post_json_unit(
            &format!("torrents/{id}/rename_file"),
            &serde_json::json!({ "file_id": file_id, "new_path": new_path }),
        )
    }

    /// Move (or copy) the torrent's files to `destination`.
    pub fn relocate(&self, id: usize, destination: &str, copy: bool) -> ApiFuture<()> {
        self.post_json_unit(
            &format!("torrents/{id}/relocate"),
            &serde_json::json!({ "destination": destination, "copy": copy }),
        )
    }

    pub fn get_events(&self, q: &EventQuery) -> ApiFuture<EventPage> {
        let mut qs = url::form_urlencoded::Serializer::new(String::new());
        if let Some(v) = &q.kind {
            qs.append_pair("kind", v);
        }
        if let Some(v) = q.torrent_id {
            qs.append_pair("torrent_id", &v.to_string());
        }
        if let Some(v) = &q.info_hash {
            qs.append_pair("info_hash", v);
        }
        if let Some(v) = &q.severity {
            qs.append_pair("severity", v);
        }
        if let Some(v) = q.before_seq {
            qs.append_pair("before_seq", &v.to_string());
        }
        if let Some(v) = q.since_seq {
            qs.append_pair("since_seq", &v.to_string());
        }
        if let Some(v) = q.limit {
            qs.append_pair("limit", &v.to_string());
        }
        let qs = qs.finish();
        self.get_json(&if qs.is_empty() {
            "events".to_owned()
        } else {
            format!("events?{qs}")
        })
    }

    pub fn get_events_summary(&self, since_seq: Option<u64>) -> ApiFuture<EventSummary> {
        self.get_json(&match since_seq {
            Some(s) => format!("events/summary?since_seq={s}"),
            None => "events/summary".to_owned(),
        })
    }

    pub fn reset_event_counters(&self) -> ApiFuture<()> {
        self.post("events/counters/reset")
    }

    /// `GET /stats`: session totals and speeds (header stats).
    pub fn session_stats(&self) -> ApiFuture<SessionStats> {
        self.get_json("stats")
    }

    pub fn fs_roots(&self) -> ApiFuture<FsRootsResponse> {
        self.get_json("fs/roots")
    }

    pub fn fs_list(&self, path: &str) -> ApiFuture<FsListResponse> {
        let qs: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("path", path)
            .finish();
        self.get_json(&format!("fs/list?{qs}"))
    }

    /// Add a .torrent file that already exists on the server.
    pub fn add_from_server_path(
        &self,
        path: &str,
        overwrite: bool,
    ) -> ApiFuture<AddTorrentResponse> {
        let qs: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("overwrite", if overwrite { "true" } else { "false" })
            .append_pair("from_server_path", path)
            .finish();
        self.post_json(&format!("torrents?{qs}"), None)
    }

    /// `POST /admin/reload`: re-read preferences.json etc. from disk.
    pub fn admin_reload(&self) -> ApiFuture<()> {
        self.post("admin/reload")
    }

    /// `POST /admin/restart`: restart the rqbit process (if supported).
    pub fn admin_restart(&self) -> ApiFuture<()> {
        self.post("admin/restart")
    }

    /// Removes the torrent and deletes its downloaded files.
    pub fn delete(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/delete"))
    }

    /// Removes the torrent from the session but keeps the downloaded files
    /// (`/forget`, as opposed to `/delete`).
    pub fn forget(&self, id: usize) -> ApiFuture<()> {
        self.post(&format!("torrents/{id}/forget"))
    }

    /// `POST /torrents`: add a magnet / http(s) URL or .torrent file bytes,
    /// with the same query parameters as the web UI (overwrite, output_folder,
    /// magnet_timeout_secs, add_job_id, is_url).
    pub fn add_torrent(
        &self,
        source: AddSource,
        opts: &AddTorrentOpts,
    ) -> ApiFuture<AddTorrentResponse> {
        let mut url = match self.url("torrents") {
            Ok(u) => u,
            Err(e) => return futures::future::ready(Err(e)).boxed(),
        };
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("overwrite", if opts.overwrite { "true" } else { "false" });
            if let Some(f) = opts.output_folder.as_deref().filter(|f| !f.is_empty()) {
                q.append_pair("output_folder", f);
            }
            if let Some(t) = opts.magnet_timeout_secs {
                q.append_pair("magnet_timeout_secs", &t.to_string());
            }
            if let Some(id) = &opts.add_job_id {
                q.append_pair("add_job_id", id);
            }
            if matches!(source, AddSource::Url(_)) {
                q.append_pair("is_url", "true");
            }
        }
        let body = match source {
            AddSource::Url(u) => (u.into_bytes(), "text/plain"),
            AddSource::TorrentFile(b) => (b, "application/x-bittorrent"),
        };
        let fut = self.send(Method::Post, url, Some(body), opts.timeout);
        async move { parse_json("torrents", &fut.await?) }.boxed()
    }

    /// `GET /add_jobs/{id}`: what the server is doing with an in-flight add.
    pub fn get_add_job(&self, job_id: &str) -> ApiFuture<AddJobStatus> {
        self.get_json(&format!("add_jobs/{}", encode_path_segment(job_id)))
    }

    /// `POST /add_jobs/{id}/cancel`.
    pub fn cancel_add_job(&self, job_id: &str) -> ApiFuture<AddJobCancelOutcome> {
        self.post_json(
            &format!("add_jobs/{}/cancel", encode_path_segment(job_id)),
            None,
        )
    }

    /// `GET /torrents/limits`.
    pub fn get_limits(&self) -> ApiFuture<LimitsConfig> {
        self.get_json("torrents/limits")
    }

    /// `POST /torrents/limits`.
    pub fn set_limits(&self, limits: &LimitsConfig) -> ApiFuture<()> {
        let v = serde_json::to_value(limits).unwrap_or_default();
        self.post_json_unit("torrents/limits", &v)
    }

    /// `GET /torrents/preferences`, kept as raw JSON so saving round-trips
    /// fields this client doesn't know about.
    pub fn get_preferences(&self) -> ApiFuture<serde_json::Map<String, serde_json::Value>> {
        self.get_json("torrents/preferences")
    }

    /// `POST /torrents/preferences` (the full object, like the web UI).
    pub fn set_preferences(
        &self,
        prefs: &serde_json::Map<String, serde_json::Value>,
    ) -> ApiFuture<()> {
        let v = serde_json::Value::Object(prefs.clone());
        self.post_json_unit("torrents/preferences", &v)
    }

    /// `GET /admin`.
    pub fn get_admin_status(&self) -> ApiFuture<AdminStatus> {
        self.get_json("admin")
    }

    /// `POST /admin/config` with a patch (only the changed keys).
    pub fn update_admin_config(&self, patch: &serde_json::Value) -> ApiFuture<()> {
        self.post_json_unit("admin/config", patch)
    }
}

/// What to add.
pub enum AddSource {
    /// Magnet link or http(s) URL of a .torrent.
    Url(String),
    /// Contents of a .torrent file.
    TorrentFile(Vec<u8>),
}

#[derive(Clone, Debug, Default)]
pub struct AddTorrentOpts {
    pub overwrite: bool,
    pub output_folder: Option<String>,
    pub magnet_timeout_secs: Option<u64>,
    pub add_job_id: Option<String>,
    pub timeout: Option<Duration>,
}

fn encode_path_segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn parse_json<T: serde::de::DeserializeOwned>(path: &str, body: &[u8]) -> anyhow::Result<T> {
    serde_json::from_slice(body).with_context(|| {
        format!(
            "unexpected response from {path} (is this an rqbit server?): {}",
            String::from_utf8_lossy(&body[..body.len().min(200)])
        )
    })
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
