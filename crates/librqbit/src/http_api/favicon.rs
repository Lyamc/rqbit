//! Favicons (rqbit's logo) for the web UI (`/web/`), the GPUI web page (`/gpui/`) and the
//! server root. Embedded in the binary; generated from `webui/assets/logo.svg`.

use axum::{Router, response::IntoResponse, routing::get};
use http::{HeaderMap, HeaderValue, StatusCode, header};

/// A week; the ETag lets browsers revalidate cheaply after that.
const CACHE_CONTROL: &str = "public, max-age=604800";

struct Icon {
    name: &'static str,
    content_type: &'static str,
    body: &'static [u8],
}

const ICONS: &[Icon] = &[
    Icon {
        name: "favicon.svg",
        content_type: "image/svg+xml",
        body: include_bytes!("favicon/favicon.svg"),
    },
    Icon {
        name: "favicon.ico",
        content_type: "image/x-icon",
        body: include_bytes!("favicon/favicon.ico"),
    },
    Icon {
        name: "favicon-32x32.png",
        content_type: "image/png",
        body: include_bytes!("favicon/favicon-32x32.png"),
    },
    Icon {
        name: "apple-touch-icon.png",
        content_type: "image/png",
        body: include_bytes!("favicon/apple-touch-icon.png"),
    },
];

/// FNV-1a; only needs to change when the bytes change.
const fn fnv1a(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x100000001b3);
        i += 1;
    }
    h
}

fn respond(icon: &'static Icon, headers: &HeaderMap) -> axum::response::Response {
    let etag = format!("\"{:016x}\"", fnv1a(icon.body));
    let etag = HeaderValue::from_str(&etag).expect("valid etag");
    let not_modified = headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|t| {
            let t = t.trim();
            t == "*" || t.trim_start_matches("W/") == etag.to_str().unwrap_or_default()
        });
    let common = [
        (
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_CONTROL),
        ),
        (header::ETAG, etag),
    ];
    if not_modified {
        return (StatusCode::NOT_MODIFIED, common).into_response();
    }
    (
        common,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(icon.content_type),
        )],
        icon.body,
    )
        .into_response()
}

/// Add `/favicon.svg`, `/favicon.ico`, `/favicon-32x32.png` and `/apple-touch-icon.png`.
pub fn add_routes<S: Clone + Send + Sync + 'static>(mut router: Router<S>) -> Router<S> {
    for icon in ICONS {
        router = router.route(
            &format!("/{}", icon.name),
            get(move |headers: HeaderMap| async move { respond(icon, &headers) }),
        );
    }
    router
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use tower::ServiceExt;

    async fn get(path: &str, inm: Option<&str>) -> axum::response::Response {
        let mut req = http::Request::get(path);
        if let Some(v) = inm {
            req = req.header(header::IF_NONE_MATCH, v);
        }
        add_routes(Router::new())
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn served_with_type_cache_and_etag() {
        for (path, ct, magic) in [
            ("/favicon.svg", "image/svg+xml", &b"<svg"[..]),
            ("/favicon.ico", "image/x-icon", &[0, 0, 1, 0][..]),
            ("/favicon-32x32.png", "image/png", &b"\x89PNG"[..]),
            ("/apple-touch-icon.png", "image/png", &b"\x89PNG"[..]),
        ] {
            let r = get(path, None).await;
            assert_eq!(r.status(), StatusCode::OK, "{path}");
            assert_eq!(r.headers()[header::CONTENT_TYPE], ct);
            assert_eq!(r.headers()[header::CACHE_CONTROL], CACHE_CONTROL);
            let etag = r.headers()[header::ETAG].to_str().unwrap().to_owned();
            let body = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
            assert!(body.starts_with(magic), "{path}");

            let r = get(path, Some(&etag)).await;
            assert_eq!(r.status(), StatusCode::NOT_MODIFIED, "{path}");
            let r = get(path, Some("\"other\"")).await;
            assert_eq!(r.status(), StatusCode::OK, "{path}");
        }
    }

    #[test]
    fn png_sizes() {
        // PNG IHDR width/height at bytes 16..24.
        let dims = |b: &[u8]| {
            (
                u32::from_be_bytes(b[16..20].try_into().unwrap()),
                u32::from_be_bytes(b[20..24].try_into().unwrap()),
            )
        };
        assert_eq!(dims(ICONS[2].body), (32, 32));
        assert_eq!(dims(ICONS[3].body), (180, 180));
        // ICO: 3 images (16, 32, 48).
        assert_eq!(u16::from_le_bytes([ICONS[1].body[4], ICONS[1].body[5]]), 3);
    }
}
