//! Native transport: `reqwest::blocking`. The returned future does the
//! blocking I/O when polled, so it must be polled on a background executor.

use std::time::Duration;

use anyhow::Context;
use futures::FutureExt;
use url::Url;

use super::{ApiFuture, Method, RawResponse};

#[derive(Clone)]
pub struct Transport {
    http: reqwest::blocking::Client,
}

impl Transport {
    /// Must be called outside of any tokio runtime (reqwest::blocking owns its own).
    pub fn new() -> anyhow::Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("rqbit-gpui/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("error building HTTP client")?;
        Ok(Self { http })
    }

    pub fn send(
        &self,
        method: Method,
        url: Url,
        basic_auth: Option<(String, String)>,
    ) -> ApiFuture<RawResponse> {
        let http = self.http.clone();
        async move {
            let req = match method {
                Method::Get => http.get(url),
                Method::Post => http.post(url),
            };
            let req = match &basic_auth {
                Some((u, p)) => req.basic_auth(u, Some(p)),
                None => req,
            };
            let resp = req.send().map_err(describe_reqwest_error)?;
            let status = resp.status().as_u16();
            let body = resp.bytes().map_err(describe_reqwest_error)?.to_vec();
            Ok(RawResponse { status, body })
        }
        .boxed()
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
