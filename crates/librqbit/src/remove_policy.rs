//! Remove policy: what "Remove" does with a torrent's files, separately for complete and
//! incomplete torrents, and the pure planning helpers behind `POST /torrents/{id}/remove`.
//!
//! "Finish what's done" (FWD) for an incomplete torrent: every file that is not 100%
//! verified is deselected and its partial data deleted, the torrent is then treated as
//! complete and the configured completion actions run on the finished files; only when
//! they all succeed is the torrent forgotten (files kept at their post-action location).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What to do with the files of a *complete* torrent when it is removed.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompleteRemoveAction {
    #[default]
    Keep,
    Delete,
}

/// What to do with the files of an *incomplete* torrent when it is removed.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteRemoveAction {
    #[default]
    Keep,
    /// Delete all of the torrent's files (complete ones too).
    Delete,
    /// Finish what's done: keep verified files, delete partial ones, run completion actions.
    Finish,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemovePolicy {
    #[serde(default)]
    pub complete: CompleteRemoveAction,
    #[serde(default)]
    pub incomplete: IncompleteRemoveAction,
}

impl RemovePolicy {
    pub const KEEP: RemovePolicy = RemovePolicy {
        complete: CompleteRemoveAction::Keep,
        incomplete: IncompleteRemoveAction::Keep,
    };
    pub const DELETE: RemovePolicy = RemovePolicy {
        complete: CompleteRemoveAction::Delete,
        incomplete: IncompleteRemoveAction::Delete,
    };

    /// True if applying this policy can delete any file (FWD deletes partial files).
    pub fn may_delete_files(&self) -> bool {
        self.complete == CompleteRemoveAction::Delete
            || self.incomplete != IncompleteRemoveAction::Keep
    }

    pub fn decide(&self, complete: bool) -> RemoveDecision {
        if complete {
            match self.complete {
                CompleteRemoveAction::Keep => RemoveDecision::Forget,
                CompleteRemoveAction::Delete => RemoveDecision::DeleteAll,
            }
        } else {
            match self.incomplete {
                IncompleteRemoveAction::Keep => RemoveDecision::Forget,
                IncompleteRemoveAction::Delete => RemoveDecision::DeleteAll,
                IncompleteRemoveAction::Finish => RemoveDecision::FinishWhatsDone,
            }
        }
    }
}

/// Legacy single "default remove action" (kept for migration of older preferences.json).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoveAction {
    #[default]
    KeepFiles,
    DeleteFiles,
}

impl From<RemoveAction> for RemovePolicy {
    fn from(a: RemoveAction) -> Self {
        match a {
            RemoveAction::KeepFiles => RemovePolicy::KEEP,
            RemoveAction::DeleteFiles => RemovePolicy::DELETE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveDecision {
    Forget,
    DeleteAll,
    FinishWhatsDone,
}

/// Request body of `POST /torrents/{id}/remove`. `policy` absent or `"default"` = use the
/// saved preference.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RemoveRequest {
    #[serde(default, deserialize_with = "de_policy")]
    pub policy: Option<RemovePolicy>,
}

fn de_policy<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<RemovePolicy>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum P {
        Str(String),
        Policy(RemovePolicy),
    }
    match Option::<P>::deserialize(d)? {
        None => Ok(None),
        Some(P::Policy(p)) => Ok(Some(p)),
        Some(P::Str(s)) if s == "default" || s.is_empty() => Ok(None),
        Some(P::Str(s)) => Err(serde::de::Error::custom(format!(
            "policy must be \"default\" or {{complete, incomplete}}, got {s:?}"
        ))),
    }
}

/// Per-file view used to plan a removal.
#[derive(Debug, Clone)]
pub struct FileView {
    pub id: usize,
    pub length: u64,
    pub have: u64,
    pub padding: bool,
}

/// Split files into complete (100% verified, non-empty) and partial ones (everything else
/// that is not padding). Zero-length files follow the complete set: kept when anything is
/// complete, deleted otherwise.
pub fn split_files(files: &[FileView]) -> (Vec<usize>, Vec<usize>) {
    let mut complete = Vec::new();
    let mut partial = Vec::new();
    let mut empty = Vec::new();
    for f in files.iter().filter(|f| !f.padding) {
        if f.length == 0 {
            empty.push(f.id);
        } else if f.have >= f.length {
            complete.push(f.id);
        } else {
            partial.push(f.id);
        }
    }
    if complete.is_empty() {
        partial.extend(empty);
    } else {
        complete.extend(empty);
    }
    complete.sort_unstable();
    partial.sort_unstable();
    (complete, partial)
}

/// On-disk candidates (relative to the output folder) that belong to a file of the torrent:
/// the path the torrent writes to (its rename, e.g. with an incomplete extension), and the
/// original name + incomplete extension. The original name itself is only included when the
/// file has no rename (then it *is* the path the torrent writes to).
pub fn file_candidates(original: &Path, rename: Option<&Path>, ext: Option<&str>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let current = rename.unwrap_or(original).to_path_buf();
    out.push(current.clone());
    if let Some(ext) = ext.map(str::trim).filter(|e| !e.is_empty()) {
        let mut os = original.as_os_str().to_owned();
        os.push(ext);
        let with_ext = PathBuf::from(os);
        if with_ext != current {
            out.push(with_ext);
        }
    }
    out.retain(|p| is_safe_relative(p));
    out
}

/// Relative, no `..`, not empty: never lets a crafted name escape the output folder.
pub fn is_safe_relative(p: &Path) -> bool {
    use std::path::Component;
    !p.as_os_str().is_empty()
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Result of a removal, returned by the API and logged to the event log.
#[derive(Debug, Clone, Serialize, Default)]
pub struct RemoveOutcome {
    pub id: usize,
    pub name: String,
    pub was_complete: bool,
    pub policy: RemovePolicy,
    /// forgot | deleted_files | finishing | finished_and_removed | deleted_nothing_complete
    pub result: String,
    pub deleted_files: Vec<String>,
    pub deleted_bytes: u64,
    pub kept_files: usize,
    pub actions_run: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_folder: Option<String>,
}

/// Preview of one torrent for the remove dialog (`GET /torrents/remove_preview`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemovePreviewItem {
    pub id: usize,
    pub name: String,
    pub complete: bool,
    pub files_complete: usize,
    pub files_partial: usize,
    pub bytes_complete: u64,
    pub bytes_partial: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemovePreview {
    pub policy: RemovePolicy,
    pub confirm_remove: bool,
    pub completion_actions: Vec<String>,
    pub items: Vec<RemovePreviewItem>,
    pub complete: usize,
    pub incomplete: usize,
    /// Incomplete torrents without a single complete file (FWD deletes everything).
    pub incomplete_nothing_done: usize,
}

pub fn preview_item(id: usize, name: String, complete: bool, files: &[FileView]) -> RemovePreviewItem {
    let (c, p) = split_files(files);
    let len = |ids: &[usize]| -> u64 {
        ids.iter()
            .filter_map(|i| files.iter().find(|f| f.id == *i))
            .map(|f| f.length)
            .sum()
    };
    let have = |ids: &[usize]| -> u64 {
        ids.iter()
            .filter_map(|i| files.iter().find(|f| f.id == *i))
            .map(|f| f.have.min(f.length))
            .sum()
    };
    RemovePreviewItem {
        id,
        name,
        complete,
        files_complete: c.len(),
        files_partial: p.len(),
        bytes_complete: len(&c),
        bytes_partial: have(&p),
    }
}

pub fn summarize_preview(
    policy: RemovePolicy,
    confirm_remove: bool,
    completion_actions: Vec<String>,
    items: Vec<RemovePreviewItem>,
) -> RemovePreview {
    let complete = items.iter().filter(|i| i.complete).count();
    let incomplete = items.len() - complete;
    let incomplete_nothing_done = items
        .iter()
        .filter(|i| !i.complete && i.files_complete == 0)
        .count();
    RemovePreview {
        policy,
        confirm_remove,
        completion_actions,
        items,
        complete,
        incomplete,
        incomplete_nothing_done,
    }
}

/// Whether the UI may remove without showing the dialog: confirmation off *and* nothing in
/// the selection would have files deleted.
pub fn may_skip_dialog(confirm_remove: bool, policy: RemovePolicy, any_complete: bool, any_incomplete: bool) -> bool {
    if confirm_remove {
        return false;
    }
    let deletes_complete = any_complete && policy.complete == CompleteRemoveAction::Delete;
    let deletes_incomplete = any_incomplete && policy.incomplete != IncompleteRemoveAction::Keep;
    !(deletes_complete || deletes_incomplete)
}

/// Who/what triggered a removal: recorded in the `torrent_removed` event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoveTrigger {
    /// A person asked for it (HTTP API: web UI, GPUI, scripts).
    Manual,
    /// An automation rule, a policy, a resolver dedup, ...
    Automation,
    /// A direct library call (`Session::delete`) without more context.
    Library,
}

/// Context of a removal, carried from the entry point into the Events log.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RemoveOrigin {
    pub trigger: RemoveTrigger,
    /// e.g. "POST /torrents/12/forget" or "rule: stalled".
    pub source: String,
    /// Original client IP (X-Forwarded-For / X-Real-IP when behind a proxy, else the peer).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<String>,
    /// TCP peer when it differs from `client_ip` (i.e. the proxy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

impl RemoveOrigin {
    pub fn manual(source: impl Into<String>) -> Self {
        Self {
            trigger: RemoveTrigger::Manual,
            source: source.into(),
            client_ip: None,
            peer: None,
            user_agent: None,
        }
    }

    pub fn automation(source: impl Into<String>) -> Self {
        Self {
            trigger: RemoveTrigger::Automation,
            ..Self::manual(source)
        }
    }

    pub fn library() -> Self {
        Self {
            trigger: RemoveTrigger::Library,
            ..Self::manual("library call")
        }
    }

    /// Short human label for event messages: "POST /torrents/3/forget from 192.168.0.30".
    pub fn label(&self) -> String {
        match &self.client_ip {
            Some(ip) => format!("{} from {ip}", self.source),
            None => self.source.clone(),
        }
    }

    /// Build from the HTTP request context. `forwarded_for` is the raw
    /// `X-Forwarded-For` header (first entry wins), `real_ip` is `X-Real-IP`.
    pub fn from_http(
        source: impl Into<String>,
        peer: Option<std::net::SocketAddr>,
        forwarded_for: Option<&str>,
        real_ip: Option<&str>,
        user_agent: Option<&str>,
    ) -> Self {
        fn clean(s: &str) -> Option<String> {
            let s = s.trim().trim_matches('"');
            if s.is_empty() || s.len() > 64 || s.eq_ignore_ascii_case("unknown") {
                return None;
            }
            // Accept "ip", "ip:port" and "[v6]:port".
            if let Ok(a) = s.parse::<std::net::SocketAddr>() {
                return Some(a.ip().to_canonical().to_string());
            }
            s.parse::<std::net::IpAddr>()
                .ok()
                .map(|ip| ip.to_canonical().to_string())
        }
        let peer_ip = peer.map(|p| p.ip().to_canonical().to_string());
        let fwd = forwarded_for
            .and_then(|h| h.split(',').next())
            .and_then(clean)
            .or_else(|| real_ip.and_then(clean));
        let client_ip = fwd.clone().or_else(|| peer_ip.clone());
        let peer = match (&fwd, &peer_ip) {
            (Some(f), Some(p)) if f != p => Some(p.clone()),
            _ => None,
        };
        let user_agent = user_agent
            .map(|u| u.trim())
            .filter(|u| !u.is_empty())
            .map(|u| u.chars().take(200).collect());
        Self {
            trigger: RemoveTrigger::Manual,
            source: source.into(),
            client_ip,
            peer,
            user_agent,
        }
    }

    /// Merge origin fields (trigger, source, client_ip, ...) into an event `details` object.
    pub fn merge_into(&self, mut details: serde_json::Value) -> serde_json::Value {
        if !details.is_object() {
            details = serde_json::json!({});
        }
        if let (Some(obj), Ok(serde_json::Value::Object(o))) =
            (details.as_object_mut(), serde_json::to_value(self))
        {
            for (k, v) in o {
                obj.insert(k, v);
            }
        }
        details
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn origin_from_http_prefers_forwarded_for() {
        let peer: std::net::SocketAddr = "172.17.0.2:5555".parse().unwrap();
        let o = RemoveOrigin::from_http(
            "POST /torrents/3/forget",
            Some(peer),
            Some("192.168.0.30, 172.17.0.2"),
            None,
            Some("Mozilla/5.0"),
        );
        assert_eq!(o.trigger, RemoveTrigger::Manual);
        assert_eq!(o.client_ip.as_deref(), Some("192.168.0.30"));
        assert_eq!(o.peer.as_deref(), Some("172.17.0.2"));
        assert_eq!(o.label(), "POST /torrents/3/forget from 192.168.0.30");
        let d = o.merge_into(serde_json::json!({"id": 3}));
        assert_eq!(d["id"], 3);
        assert_eq!(d["trigger"], "manual");
        assert_eq!(d["client_ip"], "192.168.0.30");
        assert_eq!(d["user_agent"], "Mozilla/5.0");
    }

    #[test]
    fn origin_from_http_falls_back_to_real_ip_then_peer() {
        let peer: std::net::SocketAddr = "[::ffff:10.0.0.5]:80".parse().unwrap();
        let o = RemoveOrigin::from_http("x", Some(peer), Some("garbage"), Some("10.1.1.1"), None);
        assert_eq!(o.client_ip.as_deref(), Some("10.1.1.1"));
        let o = RemoveOrigin::from_http("x", Some(peer), None, None, Some("  "));
        assert_eq!(o.client_ip.as_deref(), Some("10.0.0.5"));
        assert_eq!(o.peer, None);
        assert_eq!(o.user_agent, None);
        let o = RemoveOrigin::from_http("x", None, Some("[2001:db8::1]:443"), None, None);
        assert_eq!(o.client_ip.as_deref(), Some("2001:db8::1"));
        assert_eq!(RemoveOrigin::automation("rule: stalled").label(), "rule: stalled");
        let d = RemoveOrigin::automation("rule: stalled").merge_into(serde_json::Value::Null);
        assert_eq!(d["trigger"], "automation");
        assert!(d.get("client_ip").is_none());
    }

    use super::*;

    fn fv(id: usize, length: u64, have: u64) -> FileView {
        FileView { id, length, have, padding: false }
    }

    #[test]
    fn split_complete_and_partial() {
        let files = vec![fv(0, 100, 100), fv(1, 100, 40), fv(2, 0, 0), fv(3, 50, 0),
            FileView { id: 4, length: 10, have: 0, padding: true }];
        let (c, p) = split_files(&files);
        assert_eq!(c, vec![0, 2]);
        assert_eq!(p, vec![1, 3]);
        // Nothing complete: empty files go with the partial (deleted) set.
        let files = vec![fv(0, 100, 99), fv(1, 0, 0)];
        let (c, p) = split_files(&files);
        assert!(c.is_empty());
        assert_eq!(p, vec![0, 1]);
    }

    #[test]
    fn decisions() {
        let p = RemovePolicy { complete: CompleteRemoveAction::Keep, incomplete: IncompleteRemoveAction::Finish };
        assert_eq!(p.decide(true), RemoveDecision::Forget);
        assert_eq!(p.decide(false), RemoveDecision::FinishWhatsDone);
        assert!(p.may_delete_files());
        assert!(!RemovePolicy::KEEP.may_delete_files());
        assert_eq!(RemovePolicy::DELETE.decide(false), RemoveDecision::DeleteAll);
        assert_eq!(RemovePolicy::from(RemoveAction::DeleteFiles), RemovePolicy::DELETE);
    }

    #[test]
    fn candidates_stay_inside_torrent() {
        let c = file_candidates(Path::new("d/a.mkv"), Some(Path::new("d/a.mkv.!qB")), Some(".!qB"));
        assert_eq!(c, vec![PathBuf::from("d/a.mkv.!qB")]);
        let c = file_candidates(Path::new("d/a.mkv"), None, Some(".part"));
        assert_eq!(c, vec![PathBuf::from("d/a.mkv"), PathBuf::from("d/a.mkv.part")]);
        // Renamed by the user: the original name is not ours any more.
        let c = file_candidates(Path::new("a.mkv"), Some(Path::new("b.mkv")), None);
        assert_eq!(c, vec![PathBuf::from("b.mkv")]);
        let c = file_candidates(Path::new("../x"), None, None);
        assert!(c.is_empty());
    }

    #[test]
    fn request_parsing() {
        let r: RemoveRequest = serde_json::from_str(r#"{}"#).unwrap();
        assert!(r.policy.is_none());
        let r: RemoveRequest = serde_json::from_str(r#"{"policy":"default"}"#).unwrap();
        assert!(r.policy.is_none());
        let r: RemoveRequest =
            serde_json::from_str(r#"{"policy":{"complete":"keep","incomplete":"finish"}}"#).unwrap();
        assert_eq!(r.policy.unwrap().incomplete, IncompleteRemoveAction::Finish);
        assert!(serde_json::from_str::<RemoveRequest>(r#"{"policy":"nope"}"#).is_err());
    }

    #[test]
    fn dialog_skipping() {
        let keep = RemovePolicy::KEEP;
        assert!(may_skip_dialog(false, keep, true, true));
        assert!(!may_skip_dialog(true, keep, true, false));
        let fwd = RemovePolicy { complete: CompleteRemoveAction::Keep, incomplete: IncompleteRemoveAction::Finish };
        assert!(may_skip_dialog(false, fwd, true, false));
        assert!(!may_skip_dialog(false, fwd, true, true));
        assert!(!may_skip_dialog(false, RemovePolicy::DELETE, true, false));
    }

    #[test]
    fn preview_counts() {
        let a = preview_item(1, "a".into(), true, &[fv(0, 10, 10)]);
        let b = preview_item(2, "b".into(), false, &[fv(0, 10, 10), fv(1, 10, 3)]);
        let c = preview_item(3, "c".into(), false, &[fv(0, 10, 2)]);
        assert_eq!((b.files_complete, b.files_partial, b.bytes_complete, b.bytes_partial), (1, 1, 10, 3));
        let p = summarize_preview(RemovePolicy::KEEP, true, vec![], vec![a, b, c]);
        assert_eq!((p.complete, p.incomplete, p.incomplete_nothing_done), (1, 2, 1));
    }
}
