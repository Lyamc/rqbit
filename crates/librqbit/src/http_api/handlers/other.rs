use anyhow::Context;
use axum::{extract::State, response::IntoResponse};
use bencode::AsDisplay;
use buffers::ByteBuf;
use http::{HeaderMap, HeaderValue, StatusCode};

use super::ApiState;
use crate::{
    AddTorrent, AddTorrentOptions, ListOnlyResponse, WithStatus, api::Result,
    http_api::timeout::Timeout,
};

pub async fn h_resolve_magnet(
    State(state): State<ApiState>,
    Timeout(timeout): Timeout<600_000, 3_600_000>,
    inp_headers: HeaderMap,
    url: String,
) -> Result<impl IntoResponse> {
    // A preview, not an add: it needs the metadata. If it isn't available in time
    // the answer is 202 `{"resolving": true, ...}` (retry later, or just add the
    // magnet, which queues it), never an error. Malformed magnets are 400.
    let url = url.trim().to_string();
    if crate::pending_magnets::is_magnet_like(&url) {
        librqbit_core::magnet::Magnet::parse(&url)
            .ok()
            .filter(|m| m.as_id20().is_some())
            .ok_or_else(|| {
                crate::ApiError::invalid_input(anyhow::anyhow!(
                    "provided path is not a valid magnet URL with a BTv1 (urn:btih) info hash"
                ))
            })?;
    }
    let metadata_pending = |msg: String| {
        let info_hash = librqbit_core::magnet::Magnet::parse(&url)
            .ok()
            .and_then(|m| m.as_id20())
            .map(|h| h.as_string());
        (
            StatusCode::ACCEPTED,
            axum::Json(serde_json::json!({
                "resolving": true,
                "state": "resolving_metadata",
                "info_hash": info_hash,
                "message": format!("metadata not available yet ({msg}); retry later"),
            })),
        )
            .into_response()
    };
    let cap = timeout
        .saturating_sub(std::time::Duration::from_secs(2))
        .max(std::time::Duration::from_secs(1));
    let added = match tokio::time::timeout(
        timeout,
        state.api.session().add_torrent(
            AddTorrent::from_url(&url),
            Some(AddTorrentOptions {
                list_only: true,
                magnet_resolve_timeout: Some(cap),
                ..Default::default()
            }),
        ),
    )
    .await
    {
        Err(_) => return Ok(metadata_pending(format!("{}s request timeout", timeout.as_secs()))),
        Ok(Err(e)) if crate::pending_magnets::is_metadata_wait(&e) => {
            return Ok(metadata_pending(format!("{e:#}")));
        }
        Ok(r) => r.with_status(StatusCode::BAD_REQUEST)?,
    };

    let (info, content) = match added {
        crate::AddTorrentResponse::AlreadyManaged(_, handle) => {
            handle.with_metadata(|r| (r.info.clone(), r.torrent_bytes.clone()))?
        }
        crate::AddTorrentResponse::ListOnly(ListOnlyResponse {
            info,
            torrent_bytes,
            ..
        }) => (info, torrent_bytes),
        crate::AddTorrentResponse::Added(_, _) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "bug: torrent was added to session, but shouldn't have been",
            )
                .into());
        }
    };

    let mut headers = HeaderMap::new();

    if inp_headers
        .get("Accept")
        .and_then(|v| std::str::from_utf8(v.as_bytes()).ok())
        == Some("application/json")
    {
        let data = bencode::dyn_from_bytes::<AsDisplay<ByteBuf>>(&content)
            .map_err(|e| {
                tracing::trace!("error decoding .torrent file content: {e:#}");
                e.into_kind()
            })
            .context("error decoding .torrent file content")?;
        let data = serde_json::to_string(&data).context("error serializing")?;
        headers.insert("Content-Type", HeaderValue::from_static("application/json"));
        return Ok((headers, data).into_response());
    }

    headers.insert(
        "Content-Type",
        HeaderValue::from_static("application/x-bittorrent"),
    );

    if let Some(name) = info.name()
        && let Ok(h) = HeaderValue::from_str(&format!("attachment; filename=\"{name}.torrent\""))
    {
        headers.insert("Content-Disposition", h);
    }
    Ok((headers, content).into_response())
}

#[derive(serde::Deserialize, Default)]
pub struct PublicIpQuery {
    #[serde(default)]
    refresh: bool,
}

/// `GET /public_ip`: this server's public IPv4/IPv6 (its own egress, e.g.
/// the VPN exit), with the last check time and per-family errors.
pub async fn h_public_ip(
    State(state): State<ApiState>,
    axum::extract::Query(q): axum::extract::Query<PublicIpQuery>,
) -> Result<impl IntoResponse> {
    Ok(axum::Json(state.public_ip.get(q.refresh).await))
}
