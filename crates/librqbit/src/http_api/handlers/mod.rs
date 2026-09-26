mod admin;
mod fs;
mod configure;
mod dht;
mod events;
mod logging;
mod other;
mod playlist;
mod streaming;
mod torrents;

use std::sync::Arc;

#[cfg(feature = "webui")]
use axum::response::Redirect;

use axum::{
    Router,
    response::IntoResponse,
    routing::{get, post},
};
use http::request::Parts;

use super::HttpApi;
type ApiState = Arc<HttpApi>;

#[allow(unused_variables)]
async fn h_api_root(parts: Parts) -> impl IntoResponse {
    // If browser, and webui enabled, redirect to web
    #[cfg(feature = "webui")]
    {
        if parts
            .headers
            .get("Accept")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h.contains("text/html"))
        {
            return Redirect::temporary("./web/").into_response();
        }
    }

    let json = serde_json::json!({
        "apis": {
            "GET /": "list all available APIs",
            "GET /dht/stats": "DHT stats",
            "GET /dht/table": "DHT routing table",
            "GET /torrents": "List torrents",
            "GET /torrents/playlist": "Generate M3U8 playlist for all files in all torrents",
            "GET /stats": "Global session stats",
            "GET /metrics": "Prometheus metrics",
            "GET /stream_logs": "Continuously stream logs",
            "GET /web/": "Web UI",
            "GET /gpui/": "GPUI client compiled to WebAssembly (if installed)",
            "GET /torrents/playlist": "Playlist for supported players",
            "GET /torrents/{id_or_infohash}": "Torrent details",
            "GET /torrents/{id_or_infohash}/metadata": "Download the corresponding torrent file",
            "GET /torrents/{id_or_infohash}/haves": "The bitfield of have pieces",
            "GET /torrents/{id_or_infohash}/playlist": "Generate M3U8 playlist for this torrent",
            "GET /torrents/{id_or_infohash}/stats/v1": "Torrent stats",
            "GET /torrents/{id_or_infohash}/peer_stats": "Per peer stats",
            "GET /torrents/{id_or_infohash}/peer_stats/prometheus": "Per peer stats in prometheus format",
            "GET /torrents/{id_or_infohash}/stream/{file_idx}": "Stream a file. Accepts Range header to seek.",
            "GET /torrents/{id_or_infohash}/playlist": "Playlist for supported players",
            "POST /torrents": "Add a torrent here. magnet: or http:// or a local file.",
            "GET /add_jobs/{job_id}": "Status of an add started with ?add_job_id= (stage: resolving_metadata, waiting_for_server, ...)",
            "POST /add_jobs/{job_id}/cancel": "Cancel an add started with ?add_job_id= (no-op + already_added if it was committed)",
            "GET /fs/roots": "List allowed filesystem browse roots",
            "GET /fs/list": "List a directory under browse roots (?path=&recursive=&torrents_only=)",
            "POST /fs/extract": "Extract .torrent / magnets from a zip (or raw .torrent body)",
            "POST /torrents/create": "Create a torrent and start seeding. Body should be a local folder",
            "POST /torrents/resolve_magnet": "Resolve a magnet to torrent file bytes",
            "POST /torrents/{id_or_infohash}/pause": "Pause torrent",
            "POST /torrents/{id_or_infohash}/start": "Resume torrent",
            "POST /torrents/{id_or_infohash}/forget": "Forget about the torrent, keep the files",
            "POST /torrents/{id_or_infohash}/delete": "Forget about the torrent, remove the files",
            "POST /torrents/{id_or_infohash}/add_peers": "Add peers (newline-delimited)",
            "POST /torrents/{id_or_infohash}/update_only_files": "Change the selection of files to download. You need to POST json of the following form {\"only_files\": [0, 1, 2]}",
            "POST /rust_log": "Set RUST_LOG to this post launch (for debugging)",
        },
        "server": "rqbit",
        "version": env!("CARGO_PKG_VERSION"),
    });

    ([("Content-Type", "application/json")], axum::Json(json)).into_response()
}

pub fn make_api_router(state: ApiState) -> Router {
    let mut api_router = Router::new()
        .route("/", get(h_api_root))
        .route("/stream_logs", get(logging::h_stream_logs))
        .route("/rust_log", post(logging::h_set_rust_log))
        .route("/dht/stats", get(dht::h_dht_stats))
        .route("/dht/table", get(dht::h_dht_table))
        .route("/stats", get(torrents::h_session_stats))
        .route("/torrents", get(torrents::h_torrents_list))
        .route("/torrents/{id}", get(torrents::h_torrent_details))
        .route("/torrents/{id}/haves", get(torrents::h_torrent_haves))
        .route("/torrents/{id}/metadata", get(torrents::h_metadata))
        .route("/torrents/{id}/stats", get(torrents::h_torrent_stats_v0))
        .route("/torrents/{id}/stats/v1", get(torrents::h_torrent_stats_v1))
        .route("/torrents/{id}/peer_stats", get(torrents::h_peer_stats))
        .route(
            "/torrents/{id}/peer_stats/prometheus",
            get(torrents::h_peer_stats_prometheus),
        )
        .route("/torrents/{id}/playlist", get(playlist::h_torrent_playlist))
        .route("/torrents/playlist", get(playlist::h_global_playlist))
        .route("/torrents/resolve_magnet", post(other::h_resolve_magnet))
        .route(
            "/torrents/{id}/stream/{file_id}",
            get(streaming::h_torrent_stream_file),
        )
        .route(
            "/torrents/{id}/stream/{file_id}/{*filename}",
            get(streaming::h_torrent_stream_file),
        )
        .route("/torrents/limits", get(configure::h_get_session_ratelimits))
        .route(
            "/torrents/preferences",
            get(configure::h_get_session_preferences),
        )
        .route("/add_jobs/{job_id}", get(torrents::h_add_job_status))
        .route("/events", get(events::h_events))
        .route("/events/summary", get(events::h_events_summary))
        .route("/admin", get(admin::h_admin_status))
        .route("/fs/roots", get(fs::h_fs_roots))
        .route("/fs/list", get(fs::h_fs_list));

    if !state.opts.read_only {
        api_router = api_router
            .route("/torrents", post(torrents::h_torrents_post))
            .route("/add_jobs/{job_id}/cancel", post(torrents::h_add_job_cancel))
            .route("/fs/extract", post(fs::h_fs_extract))
            .route(
                "/torrents/limits",
                post(configure::h_update_session_ratelimits),
            )
            .route(
                "/torrents/preferences",
                post(configure::h_update_session_preferences),
            )
            .route(
                "/admin/reload",
                post(admin::h_admin_reload_preferences),
            )
            .route("/admin/config", post(admin::h_admin_update_config))
            .route(
                "/events/counters/reset",
                post(events::h_events_counters_reset),
            )
            .route("/admin/restart", post(admin::h_admin_restart))
            .route(
                "/torrents/{id}/pause",
                post(torrents::h_torrent_action_pause),
            )
            .route(
                "/torrents/{id}/start",
                post(torrents::h_torrent_action_start),
            )
            .route(
                "/torrents/{id}/restart",
                post(torrents::h_torrent_action_restart),
            )
            .route(
                "/torrents/{id}/fix_errors",
                post(torrents::h_torrent_action_fix_errors),
            )
            .route(
                "/torrents/{id}/recheck",
                post(torrents::h_torrent_action_recheck),
            )
            .route("/torrents/queue", get(torrents::h_queue_order))
            .route("/torrents/queue/move", post(torrents::h_queue_move))
            .route(
                "/torrents/{id}/repair_files",
                post(torrents::h_torrent_action_repair_files),
            )
            .route(
                "/torrents/{id}/forget",
                post(torrents::h_torrent_action_forget),
            )
            .route(
                "/torrents/{id}/delete",
                post(torrents::h_torrent_action_delete),
            )
            .route(
                "/torrents/{id}/update_only_files",
                post(torrents::h_torrent_action_update_only_files),
            )
            .route("/torrents/{id}/add_peers", post(torrents::h_add_peers))
            .route("/torrents/{id}/rename_file", post(torrents::h_rename_file))
            .route("/torrents/{id}/relocate", post(torrents::h_relocate))
            .route("/torrents/create", post(torrents::h_create_torrent));
    }

    api_router.with_state(state)
}
