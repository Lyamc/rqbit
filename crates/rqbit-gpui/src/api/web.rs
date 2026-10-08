//! Browser transport: GPUI's HTTP client, which on GPUI's web platform is the
//! browser `fetch` API. Same-origin requests carry the browser's credentials,
//! so a page served by rqbit itself needs no CORS and reuses any basic-auth
//! login the browser already has.

use std::sync::Arc;

use super::{ApiFuture, Method, RawResponse, Request};
use anyhow::Context;
use futures::{AsyncReadExt, FutureExt};
use gpui::http_client::{AsyncBody, HttpClient, http};

#[derive(Clone)]
pub struct Transport {
    http: Arc<dyn HttpClient>,
}

impl Transport {
    pub fn new(http: Arc<dyn HttpClient>) -> Self {
        Self { http }
    }

    pub fn send(&self, r: Request) -> ApiFuture<RawResponse> {
        let http = self.http.clone();
        async move {
            let mut builder = http::Request::builder()
                .method(match r.method {
                    Method::Get => http::Method::GET,
                    Method::Post => http::Method::POST,
                })
                .uri(r.url.as_str());
            if let Some((u, p)) = r.basic_auth {
                builder = builder.header(
                    "Authorization",
                    format!("Basic {}", super::base64_encode(format!("{u}:{p}").as_bytes())),
                );
            }
            if let Some(ct) = r.content_type {
                builder = builder.header("Content-Type", ct);
            }
            if let Some(a) = r.accept {
                builder = builder.header("Accept", a);
            }
            let body = match r.body {
                Some(b) => AsyncBody::from(b),
                None => AsyncBody::empty(),
            };
            let req = builder.body(body).context("error building request")?;
            let resp = http
                .send(req)
                .await
                .map_err(|e| anyhow::anyhow!("request failed: {e:#}"))?;
            let status = resp.status().as_u16();
            let mut body = Vec::new();
            resp.into_body()
                .read_to_end(&mut body)
                .await
                .context("error reading response body")?;
            Ok(RawResponse {
                status,
                body,
                conn: None,
            })
        }
        .boxed()
    }
}
