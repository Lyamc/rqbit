//! `POST /torrents` with `Content-Type: application/json`:
//!
//! ```json
//! {"url": "magnet:?xt=..." | "https://.../x.torrent"}  or  {"torrent_base64": "<.torrent bytes>"}
//! ```
//!
//! plus any add option under its query-parameter name (`torznab_category`, `category`,
//! `category_source`, `category_id`, `paused`, `output_folder`, `only_files`, ...), with
//! JSON types: numbers, booleans, strings, and arrays for the list options
//! (`only_files: [0, 2]`, `initial_peers: ["192.0.2.1:6881"]`). Exactly one of `url` /
//! `torrent_base64`. Options also given in the query string are overridden by the JSON
//! (`null` unsets one). The values go through the same parsing and validation as the
//! query string, so both forms accept and reject the same things. Without that
//! Content-Type the body is handled as before (magnet / URL / .torrent bytes), so a
//! .torrent that happens to start with `{` is never taken for JSON.

use anyhow::{Context, bail};
use axum::body::Body;
use base64::Engine;
use http::{HeaderMap, StatusCode, header::CONTENT_TYPE};
use serde_json::Value;

use crate::ApiError;
use crate::http_api_types::TorrentAddQueryParams;

/// What to add.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JsonAddSource {
    Url(String),
    TorrentBytes(Vec<u8>),
}

/// `application/json` (any parameters such as `charset`), case-insensitive.
pub(crate) fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
}

/// Reads the (already decompressed) body: 413 above `max` bytes, 400 if it can't be
/// read (e.g. corrupt compressed data).
pub(crate) async fn read_body_capped(body: Body, max: usize) -> Result<Vec<u8>, ApiError> {
    match axum::body::to_bytes(body, max).await {
        Ok(b) => Ok(b.to_vec()),
        Err(e) => {
            // `to_bytes` reports the cap as http-body-util's LengthLimitError.
            let mut too_large = false;
            let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(&e);
            while let Some(err) = cur {
                too_large |= err.to_string().contains("length limit exceeded");
                cur = err.source();
            }
            if too_large {
                Err(ApiError::from((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "body too large",
                )))
            } else {
                Err(ApiError::invalid_input(anyhow::anyhow!(
                    "could not read the request body: {e}"
                )))
            }
        }
    }
}

/// Field names of a `#[derive(Deserialize)]` struct.
fn struct_fields<'de, T: serde::Deserialize<'de>>() -> &'static [&'static str] {
    use serde::de::{Error as _, Visitor};
    struct Probe<'a>(&'a mut Option<&'static [&'static str]>);
    impl<'de> serde::Deserializer<'de> for Probe<'_> {
        type Error = serde::de::value::Error;
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(Self::Error::custom("probe"))
        }
        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, Self::Error> {
            *self.0 = Some(fields);
            Err(Self::Error::custom("probe"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map
            enum identifier ignored_any
        }
    }
    let mut fields = None;
    let _ = T::deserialize(Probe(&mut fields));
    fields.unwrap_or(&[])
}

/// Keys that only make sense for the legacy form.
const NOT_IN_JSON: &[&str] = &["is_url", "from_server_path"];

fn to_param(key: &str, v: &Value) -> anyhow::Result<Option<String>> {
    Ok(Some(match v {
        Value::Null => return Ok(None),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => {
            let mut parts = Vec::with_capacity(a.len());
            for x in a {
                match x {
                    Value::String(s) => parts.push(s.clone()),
                    Value::Number(n) => parts.push(n.to_string()),
                    _ => bail!("{key}: list items must be strings or numbers"),
                }
            }
            parts.join(",")
        }
        Value::Object(_) => bail!("{key}: objects are not allowed"),
    }))
}

fn decode_base64(s: &str) -> anyhow::Result<Vec<u8>> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let s: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|e| e.decode(&s).ok())
        .context("torrent_base64 is not valid base64")
}

/// Parses a JSON add body; options from `raw_query` apply unless the JSON sets them.
pub(crate) fn parse_json_add(
    raw_query: Option<&str>,
    body: &[u8],
) -> anyhow::Result<(TorrentAddQueryParams, JsonAddSource)> {
    let v: Value = serde_json::from_slice(body).context("invalid JSON body")?;
    let Value::Object(obj) = v else {
        bail!("the JSON body must be an object");
    };
    let known = struct_fields::<TorrentAddQueryParams>();
    let mut source = None;
    let mut overrides: Vec<(String, Option<String>)> = Vec::new();
    for (k, v) in &obj {
        match k.as_str() {
            "url" | "torrent_base64" => {
                if v.is_null() {
                    continue;
                }
                if source.is_some() {
                    bail!("give exactly one of \"url\" and \"torrent_base64\"");
                }
                let s = v
                    .as_str()
                    .with_context(|| format!("{k} must be a string"))?;
                source = Some(if k == "url" {
                    if !crate::SUPPORTED_SCHEMES.iter().any(|p| s.starts_with(p)) {
                        bail!("url must be a magnet: link or an http(s):// URL");
                    }
                    JsonAddSource::Url(s.to_owned())
                } else {
                    let bytes = decode_base64(s)?;
                    if bytes.is_empty() {
                        bail!("torrent_base64 is empty");
                    }
                    JsonAddSource::TorrentBytes(bytes)
                });
            }
            k if NOT_IN_JSON.contains(&k) => bail!("{k:?} can't be used in a JSON body"),
            k if known.contains(&k) => overrides.push((k.to_owned(), to_param(k, v)?)),
            k => bail!("unknown field {k:?}"),
        }
    }
    let source = source.context("give exactly one of \"url\" and \"torrent_base64\"")?;

    let mut pairs: Vec<(String, String)> =
        serde_urlencoded::from_str(raw_query.unwrap_or_default())
            .context("invalid query string")?;
    pairs.retain(|(k, _)| {
        !overrides.iter().any(|(o, _)| o == k) && !NOT_IN_JSON.contains(&k.as_str())
    });
    pairs.extend(overrides.into_iter().filter_map(|(k, v)| Some((k, v?))));
    let encoded = serde_urlencoded::to_string(&pairs).context("encoding options")?;
    let params: TorrentAddQueryParams = serde_urlencoded::from_str(&encoded)
        .map_err(|e| anyhow::anyhow!("invalid add option: {e}"))?;
    Ok((params, source))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(q: Option<&str>, v: Value) -> anyhow::Result<(TorrentAddQueryParams, JsonAddSource)> {
        parse_json_add(q, v.to_string().as_bytes())
    }

    #[test]
    fn content_type_detection() {
        let mut h = HeaderMap::new();
        assert!(!is_json(&h));
        for (ct, want) in [
            ("application/json", true),
            ("Application/JSON; charset=utf-8", true),
            ("application/x-bittorrent", false),
            ("application/octet-stream", false),
            ("text/plain", false),
        ] {
            h.insert(CONTENT_TYPE, ct.parse().unwrap());
            assert_eq!(is_json(&h), want, "{ct}");
        }
    }

    #[test]
    fn every_option_by_its_query_name() {
        let (p, src) = parse(
            None,
            json!({
                "url": "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
                "torznab_category": 5070,
                "category": "Anime - English-translated",
                "category_source": "nyaa",
                "category_id": "1_2",
                "paused": true,
                "overwrite": false,
                "output_folder": "/downloads/x",
                "sub_folder": "sub",
                "only_files": [0, 2, 5],
                "only_files_regex": "\\.mkv$",
                "peer_connect_timeout": 10,
                "peer_read_write_timeout": 20,
                "initial_peers": ["192.0.2.1:6881", "192.0.2.2:6881"],
                "list_only": false,
                "adopt_foreign_incomplete": "auto",
                "add_job_id": "job-1",
                "magnet_timeout_secs": 30,
                "defer_metadata": true,
                "wait_for_metadata": false,
                "add_dialog_id": "dlg-1",
            }),
        )
        .unwrap();
        assert!(matches!(src, JsonAddSource::Url(u) if u.starts_with("magnet:")));
        assert_eq!(p.torznab_category, Some(5070));
        assert_eq!(p.category.as_deref(), Some("Anime - English-translated"));
        assert_eq!(p.category_source.as_deref(), Some("nyaa"));
        assert_eq!(p.category_id.as_deref(), Some("1_2"));
        assert_eq!(p.paused, Some(true));
        assert_eq!(p.overwrite, Some(false));
        assert_eq!(p.output_folder.as_deref(), Some("/downloads/x"));
        assert_eq!(p.sub_folder.as_deref(), Some("sub"));
        assert_eq!(serde_json::to_value(&p.only_files).unwrap(), json!("0,2,5"));
        assert_eq!(p.only_files_regex.as_deref(), Some("\\.mkv$"));
        assert_eq!(
            (p.peer_connect_timeout, p.peer_read_write_timeout),
            (Some(10), Some(20))
        );
        assert_eq!(p.initial_peers.as_ref().map(|i| i.0.len()), Some(2));
        assert_eq!(p.list_only, Some(false));
        assert_eq!(p.adopt_foreign_incomplete.as_deref(), Some("auto"));
        assert_eq!(p.add_job_id.as_deref(), Some("job-1"));
        assert_eq!(p.magnet_timeout_secs, Some(30));
        assert_eq!(
            (p.defer_metadata, p.wait_for_metadata),
            (Some(true), Some(false))
        );
        assert_eq!(p.add_dialog_id.as_deref(), Some("dlg-1"));
    }

    #[test]
    fn json_wins_over_query_and_null_unsets() {
        let (p, _) = parse(
            Some("paused=false&category=FromQuery&torznab_category=2000&output_folder=/q"),
            json!({"url": "https://example.invalid/x.torrent", "paused": true, "category": "FromJson", "torznab_category": null}),
        )
        .unwrap();
        assert_eq!(p.paused, Some(true));
        assert_eq!(p.category.as_deref(), Some("FromJson"));
        assert_eq!(p.torznab_category, None, "null unsets the query value");
        assert_eq!(
            p.output_folder.as_deref(),
            Some("/q"),
            "query-only options still apply"
        );
    }

    #[test]
    fn torrent_base64() {
        let bytes = b"d4:infod4:name1:xee".to_vec();
        for enc in [
            base64::engine::general_purpose::STANDARD.encode(&bytes),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes),
        ] {
            let (_, src) = parse(None, json!({"torrent_base64": enc})).unwrap();
            assert_eq!(src, JsonAddSource::TorrentBytes(bytes.clone()));
        }
    }

    #[test]
    fn validation_errors() {
        let bad = [
            json!({}),
            json!({"url": "magnet:?xt=urn:btih:00", "torrent_base64": "AAAA"}),
            json!({"url": "ftp://x/y.torrent"}),
            json!({"url": 5}),
            json!({"torrent_base64": "%%%"}),
            json!({"torrent_base64": ""}),
            json!({"url": "magnet:?x", "torznab_category": "anime"}),
            json!({"url": "magnet:?x", "paused": "maybe"}),
            json!({"url": "magnet:?x", "only_files": []}),
            json!({"url": "magnet:?x", "only_files": [{"a": 1}]}),
            json!({"url": "magnet:?x", "output_folder": {"a": 1}}),
            json!({"url": "magnet:?x", "no_such_option": 1}),
            json!({"url": "magnet:?x", "is_url": true}),
            json!({"url": "magnet:?x", "from_server_path": "/x"}),
            json!(["magnet:?x"]),
        ];
        for b in bad {
            assert!(parse(None, b.clone()).is_err(), "{b} should be rejected");
        }
        assert!(parse_json_add(None, b"{not json").is_err());
    }

    /// The request decompression layer as the API uses it, on a tiny router: one
    /// handler reading with `read_body_capped` (POST /torrents), one with the `Bytes`
    /// extractor (the other POST endpoints, axum's 2 MiB default limit).
    mod http {
        use super::super::read_body_capped;
        use axum::{Router, body::Body, routing::post};
        use http::{Request, StatusCode, header};
        use std::io::Write;
        use tower::ServiceExt;

        const CAP: usize = 64 * 1024;

        fn app() -> Router {
            Router::new()
                .route(
                    "/capped",
                    post(|body: Body| async move {
                        read_body_capped(body, CAP)
                            .await
                            .map(|b| String::from_utf8(b).unwrap())
                    }),
                )
                .route(
                    "/bytes",
                    post(|b: axum::body::Bytes| async move { b.len().to_string() }),
                )
                .route_layer(crate::http_api::request_decompression_layer())
        }

        fn compress(enc: &str, data: &[u8]) -> Vec<u8> {
            match enc {
                "gzip" => {
                    let mut e = flate2::write::GzEncoder::new(Vec::new(), Default::default());
                    e.write_all(data).unwrap();
                    e.finish().unwrap()
                }
                "deflate" => {
                    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
                    e.write_all(data).unwrap();
                    e.finish().unwrap()
                }
                "br" => {
                    let mut out = Vec::new();
                    let mut w = brotli::CompressorWriter::new(&mut out, 4096, 4, 22);
                    w.write_all(data).unwrap();
                    drop(w);
                    out
                }
                "zstd" => zstd::encode_all(data, 3).unwrap(),
                "identity" | "" => data.to_vec(),
                _ => unreachable!(),
            }
        }

        async fn send(
            path: &str,
            enc: Option<&str>,
            body: Vec<u8>,
        ) -> (StatusCode, http::HeaderMap, String) {
            let mut req = Request::post(path);
            if let Some(enc) = enc {
                req = req.header(header::CONTENT_ENCODING, enc);
            }
            let res = app()
                .oneshot(req.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            let (parts, body) = res.into_parts();
            let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            (
                parts.status,
                parts.headers,
                String::from_utf8_lossy(&body).into_owned(),
            )
        }

        #[tokio::test]
        async fn every_encoding_round_trips() {
            let json = r#"{"url":"magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567","category":"Anime","torznab_category":5070}"#;
            for enc in ["gzip", "deflate", "br", "zstd", "identity"] {
                let (st, _, body) =
                    send("/capped", Some(enc), compress(enc, json.as_bytes())).await;
                assert_eq!((st, body.as_str()), (StatusCode::OK, json), "{enc}");
            }
            let (st, _, body) = send("/capped", None, json.as_bytes().to_vec()).await;
            assert_eq!((st, body.as_str()), (StatusCode::OK, json), "uncompressed");
        }

        #[tokio::test]
        async fn unsupported_encoding_is_415_with_accept_encoding() {
            for enc in ["compress", "x-custom", "lzma"] {
                let (st, h, _) = send("/capped", Some(enc), b"abc".to_vec()).await;
                assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{enc}");
                let accept = h.get(header::ACCEPT_ENCODING).unwrap().to_str().unwrap();
                let mut got: Vec<&str> = accept.split(',').map(str::trim).collect();
                got.sort();
                assert_eq!(got, ["br", "deflate", "gzip", "zstd"]);
            }
        }

        #[tokio::test]
        async fn decompressed_size_is_capped_413() {
            // ~10 MiB of zeros, a few KB compressed.
            let bomb = vec![0u8; 10 << 20];
            for enc in ["gzip", "deflate", "br", "zstd"] {
                let z = compress(enc, &bomb);
                assert!(z.len() < CAP, "{enc}: {}", z.len());
                let (st, _, _) = send("/capped", Some(enc), z.clone()).await;
                assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE, "{enc} /capped");
                let (st, _, _) = send("/bytes", Some(enc), z).await;
                assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE, "{enc} /bytes");
            }
            // Under the cap is fine.
            let ok = vec![b'a'; CAP - 1];
            let (st, _, body) = send("/capped", Some("zstd"), compress("zstd", &ok)).await;
            assert_eq!((st, body.len()), (StatusCode::OK, CAP - 1));
        }

        #[tokio::test]
        async fn corrupt_compressed_body_is_400() {
            let (st, _, body) =
                send("/capped", Some("gzip"), b"definitely not gzip".to_vec()).await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
        }
    }
}
