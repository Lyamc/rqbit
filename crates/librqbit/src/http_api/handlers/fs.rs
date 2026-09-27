//! Server filesystem browse + zip extract APIs for the Add UI.
//!
//! Roots are constrained to the session default output folder plus optional
//! `RQBIT_FS_BROWSE_ROOTS` (comma or colon-separated absolute paths). Path
//! traversal is blocked by canonicalizing and requiring a root prefix.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::anyhow;
use axum::{
    body::{Body, to_bytes},
    extract::{Query, State},
    response::IntoResponse,
};
use http::StatusCode;
use serde::{Deserialize, Serialize};

use super::ApiState;
use crate::{ApiError, api::Result};

const MAX_ZIP_BYTES: usize = 50 * 1024 * 1024;
const MAX_TORRENT_BYTES: usize = 10 * 1024 * 1024;
const MAX_UNCOMPRESSED_TOTAL: usize = 100 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 2000;
const MAX_LIST_ENTRIES: usize = 5000;
const MAX_RECURSE_DEPTH: u32 = 8;

#[derive(Debug, Serialize)]
pub struct FsRoot {
    pub label: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct FsRootsResponse {
    pub roots: Vec<FsRoot>,
}

#[derive(Debug, Serialize)]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_torrent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct FsListResponse {
    pub path: String,
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct FsListQuery {
    pub path: Option<String>,
    /// When true, return only .torrent files under path (recursive).
    #[serde(default)]
    pub recursive: bool,
    /// When true with recursive, only include .torrent files (skip dirs in result).
    #[serde(default)]
    pub torrents_only: bool,
}

#[derive(Debug, Serialize)]
pub struct ExtractItem {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magnet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExtractResponse {
    pub items: Vec<ExtractItem>,
}

fn env_browse_roots() -> Vec<PathBuf> {
    let raw = match std::env::var("RQBIT_FS_BROWSE_ROOTS") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Vec::new(),
    };
    raw.split(|c| c == ',' || c == ':')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect()
}

fn canonicalize_existing(path: &Path) -> std::io::Result<PathBuf> {
    // canonicalize requires the path to exist
    path.canonicalize()
}

fn collect_allowed_roots(state: &ApiState) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    let mut push = |label: String, path: PathBuf| {
        let Ok(canon) = canonicalize_existing(&path) else {
            return;
        };
        if out.iter().any(|(_, p)| p == &canon) {
            return;
        }
        out.push((label, canon));
    };

    let default = state.api.session().get_default_output_folder().to_path_buf();
    push("Downloads".into(), default);

    for p in env_browse_roots() {
        let label = p
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.display().to_string());
        push(label, p);
    }

    out
}

fn is_under_root(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn resolve_under_roots(state: &ApiState, requested: &str) -> Result<(PathBuf, PathBuf)> {
    let req = PathBuf::from(requested);
    if !req.is_absolute() {
        return Err(ApiError::from((
            StatusCode::BAD_REQUEST,
            "path must be absolute",
        )));
    }
    let canon = canonicalize_existing(&req).map_err(|e| {
        ApiError::from((
            StatusCode::NOT_FOUND,
            anyhow!("path not found: {e}"),
        ))
    })?;
    let roots = collect_allowed_roots(state);
    if roots.is_empty() {
        return Err(ApiError::from((
            StatusCode::FORBIDDEN,
            "no browse roots configured",
        )));
    }
    for (_, root) in &roots {
        if is_under_root(&canon, root) {
            return Ok((canon, root.clone()));
        }
    }
    Err(ApiError::from((
        StatusCode::FORBIDDEN,
        "path is outside allowed browse roots",
    )))
}

pub async fn h_fs_roots(State(state): State<ApiState>) -> Result<impl IntoResponse> {
    let roots = collect_allowed_roots(&state)
        .into_iter()
        .map(|(label, path)| FsRoot {
            label,
            path: path.to_string_lossy().into_owned(),
        })
        .collect();
    Ok(axum::Json(FsRootsResponse { roots }))
}

fn list_dir(path: &Path) -> Result<Vec<FsEntry>> {
    let mut entries = Vec::new();
    let rd = std::fs::read_dir(path).map_err(|e| {
        ApiError::from((StatusCode::BAD_REQUEST, anyhow!("cannot read directory: {e}")))
    })?;
    for ent in rd {
        let ent = match ent {
            Ok(e) => e,
            Err(_) => continue,
        };
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let meta = match ent.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let full = ent.path();
        let is_dir = meta.is_dir();
        let is_torrent = !is_dir && name.to_lowercase().ends_with(".torrent");
        entries.push(FsEntry {
            name,
            path: full.to_string_lossy().into_owned(),
            is_dir,
            is_torrent,
            size: if is_dir { None } else { Some(meta.len()) },
        });
        if entries.len() >= MAX_LIST_ENTRIES {
            break;
        }
    }
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}

fn collect_torrents_recursive(
    dir: &Path,
    depth: u32,
    out: &mut Vec<FsEntry>,
    truncated: &mut bool,
) {
    if depth > MAX_RECURSE_DEPTH || out.len() >= MAX_LIST_ENTRIES {
        *truncated = true;
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for ent in rd.flatten() {
        if out.len() >= MAX_LIST_ENTRIES {
            *truncated = true;
            return;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = ent.metadata() else {
            continue;
        };
        let full = ent.path();
        if meta.is_dir() {
            collect_torrents_recursive(&full, depth + 1, out, truncated);
        } else if name.to_lowercase().ends_with(".torrent") {
            out.push(FsEntry {
                name,
                path: full.to_string_lossy().into_owned(),
                is_dir: false,
                is_torrent: true,
                size: Some(meta.len()),
            });
        }
    }
}

pub async fn h_fs_list(
    State(state): State<ApiState>,
    Query(q): Query<FsListQuery>,
) -> Result<impl IntoResponse> {
    let roots = collect_allowed_roots(&state);
    if roots.is_empty() {
        return Err(ApiError::from((
            StatusCode::FORBIDDEN,
            "no browse roots configured",
        )));
    }

    let path_str = match q.path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_string(),
        None => roots[0].1.to_string_lossy().into_owned(),
    };

    let (canon, _root) = resolve_under_roots(&state, &path_str)?;
    if !canon.is_dir() {
        return Err(ApiError::from((
            StatusCode::BAD_REQUEST,
            "path is not a directory",
        )));
    }

    let parent = canon.parent().and_then(|p| {
        let roots = collect_allowed_roots(&state);
        if roots.iter().any(|(_, r)| r == &canon) {
            None
        } else if roots.iter().any(|(_, r)| is_under_root(p, r) || p == r.as_path()) {
            // parent must still be under a root (or equal)
            let Ok(pc) = canonicalize_existing(p) else {
                return None;
            };
            if roots.iter().any(|(_, r)| is_under_root(&pc, r)) {
                Some(pc.to_string_lossy().into_owned())
            } else {
                None
            }
        } else {
            None
        }
    });

    let (entries, truncated) = if q.recursive {
        let mut entries = Vec::new();
        let mut truncated = false;
        collect_torrents_recursive(&canon, 0, &mut entries, &mut truncated);
        if !q.torrents_only {
            // Still only torrents when recursive — folders of torrents use torrents_only.
        }
        (entries, truncated)
    } else {
        let entries = list_dir(&canon)?;
        let truncated = entries.len() >= MAX_LIST_ENTRIES;
        (entries, truncated)
    };

    Ok(axum::Json(FsListResponse {
        path: canon.to_string_lossy().into_owned(),
        parent,
        entries,
        truncated: if truncated { Some(true) } else { None },
    }))
}

fn looks_like_torrent(bytes: &[u8]) -> bool {
    // bencoded torrent usually starts with "d" (dict)
    bytes.first() == Some(&b'd')
}

fn extract_magnets_from_text(text: &str) -> Vec<String> {
    let re = regex::Regex::new(r#"(?i)magnet:\?[^\s<>"'\]]+"#).unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in re.find_iter(text) {
        let cleaned = m.as_str().trim_end_matches(|c: char| {
            matches!(c, '.' | ',' | ';' | ':' | ')' | ']' | '}' | '>')
        });
        if seen.insert(cleaned.to_string()) {
            out.push(cleaned.to_string());
        }
    }
    out
}

fn process_named_bytes(name: &str, data: &[u8], items: &mut Vec<ExtractItem>) {
    let lower = name.to_lowercase();
    if lower.ends_with(".torrent") {
        if data.len() > MAX_TORRENT_BYTES {
            items.push(ExtractItem {
                name: name.to_string(),
                kind: "error".into(),
                data_base64: None,
                magnet: None,
                error: Some("torrent file too large".into()),
            });
            return;
        }
        if !looks_like_torrent(data) {
            items.push(ExtractItem {
                name: name.to_string(),
                kind: "error".into(),
                data_base64: None,
                magnet: None,
                error: Some("not a valid .torrent (expected bencode)".into()),
            });
            return;
        }
        use base64::Engine;
        items.push(ExtractItem {
            name: name.to_string(),
            kind: "torrent".into(),
            data_base64: Some(base64::engine::general_purpose::STANDARD.encode(data)),
            magnet: None,
            error: None,
        });
        return;
    }

    // Text files that may contain magnets
    if lower.ends_with(".txt")
        || lower.ends_with(".magnet")
        || lower.ends_with(".md")
        || lower.ends_with(".csv")
        || lower.ends_with(".url")
    {
        if data.len() > 2 * 1024 * 1024 {
            items.push(ExtractItem {
                name: name.to_string(),
                kind: "skipped".into(),
                data_base64: None,
                magnet: None,
                error: Some("text file too large to scan for magnets".into()),
            });
            return;
        }
        if let Ok(text) = std::str::from_utf8(data) {
            let magnets = extract_magnets_from_text(text);
            if magnets.is_empty() {
                items.push(ExtractItem {
                    name: name.to_string(),
                    kind: "skipped".into(),
                    data_base64: None,
                    magnet: None,
                    error: Some("no magnets found".into()),
                });
            } else {
                for (i, m) in magnets.into_iter().enumerate() {
                    items.push(ExtractItem {
                        name: if i == 0 {
                            name.to_string()
                        } else {
                            format!("{name}#{i}")
                        },
                        kind: "magnet".into(),
                        data_base64: None,
                        magnet: Some(m),
                        error: None,
                    });
                }
            }
            return;
        }
    }

    items.push(ExtractItem {
        name: name.to_string(),
        kind: "skipped".into(),
        data_base64: None,
        magnet: None,
        error: Some("not a .torrent or magnet text file".into()),
    });
}

pub async fn h_fs_extract(
    State(state): State<ApiState>,
    body: Body,
) -> Result<impl IntoResponse> {
    let max_size = state
        .opts
        .max_upload_body_size
        .unwrap_or(MAX_ZIP_BYTES)
        .min(MAX_ZIP_BYTES);
    let data = to_bytes(body, max_size)
        .await
        .map_err(|_| ApiError::from((StatusCode::PAYLOAD_TOO_LARGE, "body too large")))?
        .to_vec();

    if data.is_empty() {
        return Err(ApiError::from((StatusCode::BAD_REQUEST, "empty body")));
    }

    // Single .torrent uploaded as raw body
    if looks_like_torrent(&data) && data.len() < MAX_TORRENT_BYTES {
        use base64::Engine;
        return Ok(axum::Json(ExtractResponse {
            items: vec![ExtractItem {
                name: "upload.torrent".into(),
                kind: "torrent".into(),
                data_base64: Some(base64::engine::general_purpose::STANDARD.encode(&data)),
                magnet: None,
                error: None,
            }],
        }));
    }

    // Treat as zip
    let reader = std::io::Cursor::new(&data);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| {
        ApiError::from((
            StatusCode::BAD_REQUEST,
            anyhow!("not a zip archive (or corrupt): {e}"),
        ))
    })?;

    let mut items = Vec::new();
    let mut uncompressed_total: usize = 0;
    let len = archive.len().min(MAX_ZIP_ENTRIES);

    for i in 0..len {
        let mut file = match archive.by_index(i) {
            Ok(f) => f,
            Err(e) => {
                items.push(ExtractItem {
                    name: format!("entry#{i}"),
                    kind: "error".into(),
                    data_base64: None,
                    magnet: None,
                    error: Some(format!("zip entry error: {e}")),
                });
                continue;
            }
        };
        if file.is_dir() {
            continue;
        }
        let name = file
            .enclosed_name()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.name().to_string());

        // Skip junk paths
        if name.contains("..") || name.starts_with('/') || name.starts_with('\\') {
            items.push(ExtractItem {
                name: name.clone(),
                kind: "error".into(),
                data_base64: None,
                magnet: None,
                error: Some("unsafe zip path".into()),
            });
            continue;
        }

        let lower = name.to_lowercase();
        let interesting = lower.ends_with(".torrent")
            || lower.ends_with(".txt")
            || lower.ends_with(".magnet")
            || lower.ends_with(".md")
            || lower.ends_with(".csv")
            || lower.ends_with(".url");
        if !interesting {
            // silently skip non-torrent junk (still report briefly)
            items.push(ExtractItem {
                name: name.clone(),
                kind: "skipped".into(),
                data_base64: None,
                magnet: None,
                error: Some("not a .torrent or magnet text file".into()),
            });
            continue;
        }

        let size_hint = file.size() as usize;
        if size_hint > MAX_TORRENT_BYTES && lower.ends_with(".torrent") {
            items.push(ExtractItem {
                name: name.clone(),
                kind: "error".into(),
                data_base64: None,
                magnet: None,
                error: Some("torrent entry too large".into()),
            });
            continue;
        }
        if uncompressed_total.saturating_add(size_hint) > MAX_UNCOMPRESSED_TOTAL {
            items.push(ExtractItem {
                name: name.clone(),
                kind: "error".into(),
                data_base64: None,
                magnet: None,
                error: Some("zip uncompressed size limit exceeded".into()),
            });
            break;
        }

        let mut buf = Vec::new();
        if let Err(e) = file.read_to_end(&mut buf) {
            items.push(ExtractItem {
                name: name.clone(),
                kind: "error".into(),
                data_base64: None,
                magnet: None,
                error: Some(format!("read error: {e}")),
            });
            continue;
        }
        uncompressed_total = uncompressed_total.saturating_add(buf.len());
        if uncompressed_total > MAX_UNCOMPRESSED_TOTAL {
            items.push(ExtractItem {
                name: name.clone(),
                kind: "error".into(),
                data_base64: None,
                magnet: None,
                error: Some("zip uncompressed size limit exceeded".into()),
            });
            break;
        }

        let base = Path::new(&name)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or(name.clone());
        process_named_bytes(&base, &buf, &mut items);
    }

    if archive.len() > MAX_ZIP_ENTRIES {
        items.push(ExtractItem {
            name: "(archive)".into(),
            kind: "error".into(),
            data_base64: None,
            magnet: None,
            error: Some(format!(
                "archive has {} entries; only first {MAX_ZIP_ENTRIES} processed",
                archive.len()
            )),
        });
    }

    Ok(axum::Json(ExtractResponse { items }))
}

/// Read a validated .torrent path into bytes (for from_server_path adds).
pub fn read_torrent_under_roots(state: &ApiState, path: &str) -> Result<Vec<u8>> {
    let (canon, _) = resolve_under_roots(state, path)?;
    if !canon.is_file() {
        return Err(ApiError::from((
            StatusCode::BAD_REQUEST,
            "path is not a file",
        )));
    }
    let name = canon
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !name.ends_with(".torrent") {
        return Err(ApiError::from((
            StatusCode::BAD_REQUEST,
            "only .torrent files can be added from the server filesystem",
        )));
    }
    let meta = std::fs::metadata(&canon).map_err(|e| {
        ApiError::from((StatusCode::BAD_REQUEST, anyhow!("stat failed: {e}")))
    })?;
    if meta.len() as usize > MAX_TORRENT_BYTES {
        return Err(ApiError::from((
            StatusCode::PAYLOAD_TOO_LARGE,
            "torrent file too large",
        )));
    }
    let mut f = File::open(&canon).map_err(|e| {
        ApiError::from((StatusCode::BAD_REQUEST, anyhow!("open failed: {e}")))
    })?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).map_err(|e| {
        ApiError::from((StatusCode::BAD_REQUEST, anyhow!("read failed: {e}")))
    })?;
    if !looks_like_torrent(&buf) {
        return Err(ApiError::from((
            StatusCode::BAD_REQUEST,
            "file does not look like a .torrent",
        )));
    }
    Ok(buf)
}
