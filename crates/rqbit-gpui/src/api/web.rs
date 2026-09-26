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
                    format!("Basic {}", base64(format!("{u}:{p}").as_bytes())),
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
            Ok(RawResponse { status, body })
        }
        .boxed()
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
