//! "Clean up orphaned downloads": find files and folders in the download
//! locations that no torrent in rqbit uses any more, review them, and move
//! them to a quarantine folder (default, reversible) or delete them.
//!
//! Safety rules:
//! - Scanning is read-only and never follows symlinks (they are skipped and
//!   counted), never leaves the chosen roots and ignores hidden/system entries
//!   (dot files, `Thumbs.db`, `desktop.ini`, `$RECYCLE.BIN`, `@eaDir`, ...).
//! - Anything modified in the last N minutes (default 60) is left alone.
//! - Anything a torrent uses is protected: every file path (with its rename
//!   and incomplete-extension variants and common foreign partial suffixes),
//!   and torrents without a file list (still resolving metadata, errored)
//!   protect their whole output folder.
//! - Apply re-validates every item against the current torrents first.
//! - Quarantine keeps the relative layout under `<root>/.rqbit-quarantine/<batch>/`
//!   with a manifest; restore puts things back if the original path is free.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub const QUARANTINE_DIR: &str = ".rqbit-quarantine";
/// Partial-file suffixes other clients (and rqbit's own option) use.
pub const FOREIGN_PARTIAL_SUFFIXES: &[&str] = &[".!qB", ".part", ".!ut", ".!bt", ".crdownload"];
pub const DEFAULT_MIN_AGE_MINUTES: u64 = 60;
const MAX_ITEMS: usize = 5000;
const MAX_ENTRIES: u64 = 2_000_000;
const KEEP_SCANS: usize = 4;

/// Names that are never reported (system / NAS metadata / our own).
pub fn is_hidden_or_system(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    let l = name.to_ascii_lowercase();
    matches!(
        l.as_str(),
        "thumbs.db"
            | "desktop.ini"
            | "$recycle.bin"
            | "system volume information"
            | "@eadir"
            | "#recycle"
            | "#snapshot"
            | "lost+found"
            | "icon\r"
    )
}

/// What the torrents currently use.
#[derive(Default, Debug, Clone)]
pub struct OwnedSet {
    /// Absolute paths of files torrents use (all candidate names).
    pub files: HashSet<PathBuf>,
    /// Every ancestor directory of an owned file or protected dir.
    pub ancestors: HashSet<PathBuf>,
    /// Folders protected as a whole (torrents without a file list).
    pub protected_dirs: Vec<PathBuf>,
    pub torrents: usize,
}

impl OwnedSet {
    pub fn add_file(&mut self, p: PathBuf) {
        self.add_ancestors(&p);
        self.files.insert(p);
    }
    pub fn add_protected_dir(&mut self, p: PathBuf) {
        self.add_ancestors(&p);
        self.protected_dirs.push(p);
    }
    fn add_ancestors(&mut self, p: &Path) {
        let mut cur = p.parent();
        while let Some(a) = cur {
            if !self.ancestors.insert(a.to_path_buf()) {
                break;
            }
            cur = a.parent();
        }
    }
    pub fn is_protected(&self, p: &Path) -> bool {
        self.protected_dirs.iter().any(|d| p.starts_with(d))
    }
    /// True if `p` or anything below it is used by a torrent.
    pub fn touches(&self, p: &Path) -> bool {
        self.files.contains(p) || self.ancestors.contains(p) || self.is_protected(p)
    }
}

/// Canonical form when the path exists (resolves symlinked mount paths),
/// otherwise the path unchanged.
pub fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CleanupRoot {
    pub path: String,
    /// download | move | organize | custom
    pub kind: String,
    /// Ticked by default in the UI (the download folder; move/organize
    /// targets are libraries that often hold files on purpose).
    pub default_on: bool,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanItem {
    pub id: u32,
    pub path: String,
    pub root: String,
    /// "file" or "dir"
    pub kind: String,
    pub size: u64,
    /// Newest modification time inside (unix seconds).
    pub mtime: Option<u64>,
    /// Files inside (1 for a file).
    pub files: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanSkipped {
    pub recent: u64,
    pub symlinks: u64,
    pub hidden: u64,
    pub errors: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub scan_id: String,
    /// unix seconds
    pub time: u64,
    pub roots: Vec<String>,
    pub min_age_minutes: u64,
    pub items: Vec<ScanItem>,
    pub skipped: ScanSkipped,
    pub total_bytes: u64,
    pub torrents_checked: usize,
    pub protected_folders: usize,
    pub scheduled: bool,
    pub duration_ms: u64,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mtime_secs(md: &std::fs::Metadata) -> Option<u64> {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

/// Removes roots that are inside another root (they'd be reported twice).
pub fn dedupe_roots(mut roots: Vec<PathBuf>) -> Vec<PathBuf> {
    roots.sort();
    roots.dedup();
    let all = roots.clone();
    roots
        .into_iter()
        .filter(|r| !all.iter().any(|o| o != r && r.starts_with(o)))
        .collect()
}

struct DirSummary {
    size: u64,
    files: u64,
    newest: Option<u64>,
    symlinks: u64,
}

fn summarize_dir(dir: &Path, budget: &mut u64) -> std::io::Result<DirSummary> {
    let mut s = DirSummary {
        size: 0,
        files: 0,
        newest: std::fs::symlink_metadata(dir).ok().as_ref().and_then(mtime_secs),
        symlinks: 0,
    };
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let e = e?;
            let md = match std::fs::symlink_metadata(e.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if md.file_type().is_symlink() {
                s.symlinks += 1;
                continue;
            }
            let mt = mtime_secs(&md);
            s.newest = s.newest.max(mt);
            if md.is_dir() {
                stack.push(e.path());
            } else if md.is_file() {
                s.files += 1;
                s.size += md.len();
            }
        }
    }
    Ok(s)
}

fn partial_suffix(name: &str, ext: Option<&str>) -> Option<String> {
    ext.into_iter()
        .chain(FOREIGN_PARTIAL_SUFFIXES.iter().copied())
        .find(|s| !s.is_empty() && name.ends_with(s))
        .map(|s| s.to_owned())
}

/// Read-only scan. `roots` must be canonical and deduped.
pub fn scan(
    roots: &[PathBuf],
    owned: &OwnedSet,
    min_age: Duration,
    incomplete_ext: Option<&str>,
    now: u64,
) -> (Vec<ScanItem>, ScanSkipped) {
    let mut items = Vec::new();
    let mut sk = ScanSkipped::default();
    let cutoff = now.saturating_sub(min_age.as_secs());
    let mut budget = MAX_ENTRIES;
    for root in roots {
        let root_s = root.to_string_lossy().into_owned();
        if owned.is_protected(root) {
            continue;
        }
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(e) => {
                    if sk.errors.len() < 20 {
                        sk.errors.push(format!("{}: {e}", dir.display()));
                    }
                    continue;
                }
            };
            let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                if budget == 0 || items.len() >= MAX_ITEMS {
                    sk.truncated = true;
                    break;
                }
                budget -= 1;
                let name = e.file_name().to_string_lossy().into_owned();
                if is_hidden_or_system(&name) {
                    sk.hidden += 1;
                    continue;
                }
                let path = e.path();
                let md = match std::fs::symlink_metadata(&path) {
                    Ok(m) => m,
                    Err(err) => {
                        if sk.errors.len() < 20 {
                            sk.errors.push(format!("{}: {err}", path.display()));
                        }
                        continue;
                    }
                };
                if md.file_type().is_symlink() {
                    sk.symlinks += 1;
                    continue;
                }
                if owned.is_protected(&path) || owned.files.contains(&path) {
                    continue;
                }
                if md.is_dir() {
                    if owned.ancestors.contains(&path) {
                        stack.push(path);
                        continue;
                    }
                    let s = match summarize_dir(&path, &mut budget) {
                        Ok(s) => s,
                        Err(err) => {
                            if sk.errors.len() < 20 {
                                sk.errors.push(format!("{}: {err}", path.display()));
                            }
                            continue;
                        }
                    };
                    if s.newest.is_some_and(|t| t > cutoff) {
                        sk.recent += 1;
                        continue;
                    }
                    if s.symlinks > 0 {
                        // Never act on a folder that contains symlinks.
                        sk.symlinks += s.symlinks;
                        continue;
                    }
                    let reason = if s.files == 0 {
                        "Empty folder, no torrent uses it".to_owned()
                    } else {
                        format!(
                            "No torrent in rqbit uses anything in this folder ({} file{})",
                            s.files,
                            if s.files == 1 { "" } else { "s" }
                        )
                    };
                    items.push(ScanItem {
                        id: u32::try_from(items.len()).unwrap_or(u32::MAX),
                        path: path.to_string_lossy().into_owned(),
                        root: root_s.clone(),
                        kind: "dir".into(),
                        size: s.size,
                        mtime: s.newest,
                        files: s.files,
                        reason,
                    });
                } else if md.is_file() {
                    let mt = mtime_secs(&md);
                    if mt.is_some_and(|t| t > cutoff) {
                        sk.recent += 1;
                        continue;
                    }
                    let reason = match partial_suffix(&name, incomplete_ext) {
                        Some(sfx) => format!("Leftover partial file ({sfx}), no torrent uses it"),
                        None => "Not part of any torrent in rqbit".to_owned(),
                    };
                    items.push(ScanItem {
                        id: u32::try_from(items.len()).unwrap_or(u32::MAX),
                        path: path.to_string_lossy().into_owned(),
                        root: root_s.clone(),
                        kind: "file".into(),
                        size: md.len(),
                        mtime: mt,
                        files: 1,
                        reason,
                    });
                }
            }
        }
    }
    (items, sk)
}

/// Why an item may no longer be touched, or None if it is still an orphan.
pub fn revalidate(
    item: &ScanItem,
    roots: &[PathBuf],
    owned: &OwnedSet,
    min_age: Duration,
    now: u64,
) -> Option<String> {
    let p = PathBuf::from(&item.path);
    let Some(root) = roots.iter().find(|r| p.starts_with(r) && p != **r) else {
        return Some("outside the scanned folders".into());
    };
    if p.components().any(|c| c.as_os_str() == QUARANTINE_DIR) {
        return Some("inside the quarantine folder".into());
    }
    // Every component between root and item must be a real directory.
    let mut cur = root.clone();
    if let Ok(rel) = p.strip_prefix(root) {
        for c in rel.components() {
            cur.push(c);
            match std::fs::symlink_metadata(&cur) {
                Ok(m) if m.file_type().is_symlink() => return Some("is (or is inside) a symlink".into()),
                Ok(_) => {}
                Err(_) => return Some("no longer exists".into()),
            }
        }
    }
    if owned.touches(&p) {
        return Some("a torrent uses it now".into());
    }
    let md = std::fs::symlink_metadata(&p).ok()?;
    let cutoff = now.saturating_sub(min_age.as_secs());
    if md.is_dir() {
        let mut budget = MAX_ENTRIES;
        match summarize_dir(&p, &mut budget) {
            Ok(s) if s.symlinks > 0 => return Some("contains symlinks".into()),
            Ok(s) if s.newest.is_some_and(|t| t > cutoff) => {
                return Some("changed recently".into());
            }
            Ok(_) => {}
            Err(e) => return Some(format!("can't read: {e}")),
        }
        // A torrent may have started using something inside since the scan.
        if owned.files.iter().any(|f| f.starts_with(&p)) {
            return Some("a torrent uses files inside it now".into());
        }
    } else if mtime_secs(&md).is_some_and(|t| t > cutoff) {
        return Some("changed recently".into());
    }
    None
}

// ---------------------------------------------------------------- quarantine

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineItem {
    pub original: String,
    pub stored: String,
    pub kind: String,
    pub size: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineBatch {
    pub id: String,
    pub root: String,
    pub dir: String,
    /// unix seconds
    pub created: u64,
    pub items: Vec<QuarantineItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuarantineIndex {
    pub batches: Vec<QuarantineBatch>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ItemResult {
    pub id: Option<u32>,
    pub path: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moved_to: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ApplyOutcome {
    pub action: String,
    pub ok: usize,
    pub failed: usize,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub batches: Vec<String>,
    pub results: Vec<ItemResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApplyRequest {
    pub scan_id: String,
    pub item_ids: Vec<u32>,
    /// "quarantine" (default) or "delete"
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RestoreRequest {
    pub batch: String,
    /// Indexes into the batch's items; empty/None = all.
    #[serde(default)]
    pub items: Option<Vec<usize>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PurgeRequest {
    pub batch: String,
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ScanQuery {
    /// Comma separated absolute paths; empty = default roots.
    #[serde(default)]
    pub roots: Option<String>,
    #[serde(default)]
    pub min_age_minutes: Option<u64>,
}

/// In-memory recent scans + the quarantine index file.
pub struct CleanupState {
    pub index_path: PathBuf,
    scans: Mutex<Vec<ScanResult>>,
    pub last_scheduled: Mutex<Option<u64>>,
    index_lock: Mutex<()>,
}

impl CleanupState {
    pub fn new(index_path: PathBuf) -> Self {
        Self {
            index_path,
            scans: Mutex::new(Vec::new()),
            last_scheduled: Mutex::new(None),
            index_lock: Mutex::new(()),
        }
    }
    pub fn remember(&self, s: ScanResult) {
        let mut v = self.scans.lock();
        v.push(s);
        let n = v.len();
        if n > KEEP_SCANS {
            v.drain(0..n - KEEP_SCANS);
        }
    }
    pub fn get(&self, id: &str) -> Option<ScanResult> {
        self.scans.lock().iter().find(|s| s.scan_id == id).cloned()
    }
    pub fn latest(&self) -> Option<ScanResult> {
        self.scans.lock().last().cloned()
    }
    pub fn load_index(&self) -> QuarantineIndex {
        std::fs::read(&self.index_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save_index(&self, idx: &QuarantineIndex) -> anyhow::Result<()> {
        let tmp = self.index_path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(idx)?)?;
        std::fs::rename(&tmp, &self.index_path)?;
        Ok(())
    }

    /// Moves items into `<root>/.rqbit-quarantine/<batch>/<rel>`, one batch per root.
    pub fn quarantine(&self, items: &[(ScanItem, PathBuf)], results: &mut Vec<ItemResult>) -> Vec<String> {
        let _g = self.index_lock.lock();
        let mut idx = self.load_index();
        let stamp = chrono_like_stamp();
        let mut batch_ids = Vec::new();
        let mut roots: Vec<&PathBuf> = items.iter().map(|(_, r)| r).collect();
        roots.sort();
        roots.dedup();
        for (n, root) in roots.into_iter().enumerate() {
            let id = if n == 0 { stamp.clone() } else { format!("{stamp}-{n}") };
            let dir = root.join(QUARANTINE_DIR).join(&id);
            let mut batch = QuarantineBatch {
                id: id.clone(),
                root: root.to_string_lossy().into_owned(),
                dir: dir.to_string_lossy().into_owned(),
                created: now_secs(),
                items: Vec::new(),
            };
            for (it, _) in items.iter().filter(|(_, r)| r == root) {
                let src = PathBuf::from(&it.path);
                let rel = match src.strip_prefix(root) {
                    Ok(r) => r.to_path_buf(),
                    Err(_) => continue,
                };
                let dst = dir.join(&rel);
                let res = dst
                    .parent()
                    .map(std::fs::create_dir_all)
                    .transpose()
                    .and_then(|_| {
                        if std::fs::symlink_metadata(&dst).is_ok() {
                            Err(std::io::Error::other("destination exists"))
                        } else {
                            std::fs::rename(&src, &dst)
                        }
                    });
                match res {
                    Ok(()) => {
                        batch.items.push(QuarantineItem {
                            original: it.path.clone(),
                            stored: dst.to_string_lossy().into_owned(),
                            kind: it.kind.clone(),
                            size: it.size,
                            files: it.files,
                        });
                        results.push(ItemResult {
                            id: Some(it.id),
                            path: it.path.clone(),
                            ok: true,
                            moved_to: Some(dst.to_string_lossy().into_owned()),
                            size: it.size,
                            ..Default::default()
                        });
                    }
                    Err(e) => results.push(ItemResult {
                        id: Some(it.id),
                        path: it.path.clone(),
                        ok: false,
                        error: Some(if e.raw_os_error() == Some(18) {
                            "on a different filesystem than the quarantine folder (not moved)".into()
                        } else {
                            e.to_string()
                        }),
                        size: it.size,
                        ..Default::default()
                    }),
                }
            }
            if !batch.items.is_empty() {
                let _ = std::fs::write(
                    dir.join("manifest.json"),
                    serde_json::to_vec_pretty(&batch).unwrap_or_default(),
                );
                batch_ids.push(id);
                idx.batches.push(batch);
            }
        }
        if let Err(e) = self.save_index(&idx) {
            tracing::warn!("cleanup: saving quarantine index failed: {e:#}");
        }
        batch_ids
    }

    pub fn restore(&self, req: &RestoreRequest) -> anyhow::Result<Vec<ItemResult>> {
        let _g = self.index_lock.lock();
        let mut idx = self.load_index();
        let b = idx
            .batches
            .iter_mut()
            .find(|b| b.id == req.batch)
            .ok_or_else(|| anyhow::anyhow!("no such quarantine batch"))?;
        let want: Option<HashSet<usize>> = req.items.as_ref().filter(|v| !v.is_empty()).map(|v| v.iter().copied().collect());
        let mut results = Vec::new();
        let mut keep = Vec::new();
        for (i, it) in b.items.drain(..).enumerate() {
            if want.as_ref().is_some_and(|w| !w.contains(&i)) {
                keep.push(it);
                continue;
            }
            let orig = PathBuf::from(&it.original);
            let res = if std::fs::symlink_metadata(&orig).is_ok() {
                Err(anyhow::anyhow!("the original path is in use, not overwritten"))
            } else {
                orig.parent()
                    .map(std::fs::create_dir_all)
                    .transpose()
                    .and_then(|_| std::fs::rename(&it.stored, &orig))
                    .map_err(anyhow::Error::from)
            };
            match res {
                Ok(()) => results.push(ItemResult {
                    path: it.original.clone(),
                    ok: true,
                    size: it.size,
                    ..Default::default()
                }),
                Err(e) => {
                    results.push(ItemResult {
                        path: it.original.clone(),
                        ok: false,
                        error: Some(format!("{e:#}")),
                        size: it.size,
                        ..Default::default()
                    });
                    keep.push(it);
                }
            }
        }
        b.items = keep;
        if b.items.is_empty() {
            let dir = PathBuf::from(&b.dir);
            let _ = std::fs::remove_file(dir.join("manifest.json"));
            remove_empty_dirs(&dir);
            let _ = std::fs::remove_dir(&dir);
            let id = b.id.clone();
            idx.batches.retain(|x| x.id != id);
        } else {
            let _ = std::fs::write(
                PathBuf::from(&b.dir).join("manifest.json"),
                serde_json::to_vec_pretty(&*b).unwrap_or_default(),
            );
        }
        self.save_index(&idx)?;
        Ok(results)
    }

    pub fn purge(&self, req: &PurgeRequest) -> anyhow::Result<(usize, u64)> {
        if !req.confirm {
            anyhow::bail!("purging deletes files permanently: send confirm=true");
        }
        let _g = self.index_lock.lock();
        let mut idx = self.load_index();
        let pos = idx
            .batches
            .iter()
            .position(|b| b.id == req.batch)
            .ok_or_else(|| anyhow::anyhow!("no such quarantine batch"))?;
        let b = idx.batches.remove(pos);
        let dir = PathBuf::from(&b.dir);
        // Only ever delete inside a .rqbit-quarantine folder.
        if dir.parent().and_then(|p| p.file_name()).is_none_or(|n| n != QUARANTINE_DIR) {
            anyhow::bail!("refusing to delete {}: not a quarantine batch folder", dir.display());
        }
        let n = b.items.len();
        let bytes = b.items.iter().map(|i| i.size).sum();
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        self.save_index(&idx)?;
        Ok((n, bytes))
    }
}

fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                remove_empty_dirs(&e.path());
                let _ = std::fs::remove_dir(e.path());
            }
        }
    }
}

/// Local-time-free sortable stamp (UTC): 20260926-231500.
fn chrono_like_stamp() -> String {
    let s = now_secs();
    let days = s / 86400;
    let rem = s % 86400;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    // Both are small (day 1..=31, month 1..=12).
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Deletes one item permanently (file, or folder recursively).
pub fn delete_item(p: &Path) -> std::io::Result<()> {
    let md = std::fs::symlink_metadata(p)?;
    if md.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
}

// ------------------------------------------------------------- session glue

use std::sync::Arc;

use crate::session::Session;

/// kinds for the event log
pub mod kind {
    pub const CLEANUP_SCAN: &str = "cleanup_scan";
    pub const CLEANUP_QUARANTINED: &str = "cleanup_quarantined";
    pub const CLEANUP_DELETED: &str = "cleanup_deleted";
    pub const CLEANUP_RESTORED: &str = "cleanup_restored";
    pub const CLEANUP_PURGED: &str = "cleanup_purged";
}

#[derive(Debug, Clone, Serialize)]
pub struct RootsResponse {
    pub roots: Vec<CleanupRoot>,
    pub min_age_minutes: u64,
    pub scan_hours: Option<u64>,
    /// Folders custom roots may live under.
    pub allowed_parents: Vec<String>,
    pub latest_scan: Option<ScanSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanSummary {
    pub scan_id: String,
    pub time: u64,
    pub items: usize,
    pub total_bytes: u64,
    pub scheduled: bool,
}

fn fmt_bytes(b: u64) -> String {
    crate::torrent_rules::fmt_bytes(b)
}

impl Session {
    fn cleanup_min_age(&self) -> u64 {
        self.preferences
            .get()
            .cleanup_min_age_minutes
            .unwrap_or(DEFAULT_MIN_AGE_MINUTES)
    }

    /// Download folder, completion Move folders, organize root (+ type folders),
    /// saved extra roots.
    pub fn cleanup_roots(&self) -> Vec<CleanupRoot> {
        let prefs = self.preferences.get();
        let mut out: Vec<CleanupRoot> = Vec::new();
        let mut push = |p: PathBuf, kind: &str, on: bool| {
            let s = p.to_string_lossy().into_owned();
            if s.is_empty() || !p.is_absolute() || out.iter().any(|r| r.path == s) {
                return;
            }
            out.push(CleanupRoot {
                exists: p.is_dir(),
                path: s,
                kind: kind.to_owned(),
                default_on: on,
            });
        };
        let dl = self.get_default_output_folder().to_path_buf();
        push(dl.clone(), "download", true);
        for a in prefs.effective_actions() {
            match a {
                crate::session_preferences::CompletionAction::Move { path, .. } => {
                    push(PathBuf::from(path), "move", false)
                }
                crate::session_preferences::CompletionAction::Organize => {
                    let root = prefs
                        .auto_organize_root
                        .clone()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| dl.clone());
                    push(root, "organize", false);
                }
                _ => {}
            }
        }
        for r in &prefs.cleanup_extra_roots {
            push(PathBuf::from(r), "custom", false);
        }
        out
    }

    fn cleanup_allowed_parents(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .cleanup_roots()
            .into_iter()
            .filter(|r| r.kind != "custom")
            .map(|r| canon(Path::new(&r.path)))
            .collect();
        if let Ok(raw) = std::env::var("RQBIT_FS_BROWSE_ROOTS") {
            v.extend(
                raw.split([',', ':'])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute())
                    .map(|p| canon(&p)),
            );
        }
        v
    }

    pub fn cleanup_roots_response(&self) -> RootsResponse {
        let prefs = self.preferences.get();
        RootsResponse {
            roots: self.cleanup_roots(),
            min_age_minutes: self.cleanup_min_age(),
            scan_hours: prefs.cleanup_scan_hours.filter(|h| *h > 0),
            allowed_parents: self
                .cleanup_allowed_parents()
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            latest_scan: self.cleanup.latest().map(|s| ScanSummary {
                scan_id: s.scan_id,
                time: s.time,
                items: s.items.len(),
                total_bytes: s.total_bytes,
                scheduled: s.scheduled,
            }),
        }
    }

    /// Everything the torrents (and still-resolving magnets) use right now.
    pub fn cleanup_owned(&self) -> OwnedSet {
        let prefs = self.preferences.get();
        let ext = prefs.incomplete_extension_str().map(|s| s.to_owned());
        let mut owned = OwnedSet::default();
        let handles: Vec<_> = self.with_torrents(|it| it.map(|(_, h)| h.clone()).collect());
        for h in handles {
            owned.torrents += 1;
            let folder = canon(&h.output_folder());
            let infos = h
                .with_metadata(|m| {
                    m.file_infos
                        .iter()
                        .map(|fi| (fi.relative_filename.clone(), fi.attrs.padding))
                        .collect::<Vec<_>>()
                })
                .ok();
            match infos {
                Some(infos) if !infos.is_empty() => {
                    for (id, (rel, padding)) in infos.iter().enumerate() {
                        if *padding {
                            continue;
                        }
                        let rename = h.shared().file_rename(id);
                        for c in crate::remove_policy::file_candidates(rel, rename.as_deref(), ext.as_deref()) {
                            let abs = folder.join(&c);
                            for sfx in FOREIGN_PARTIAL_SUFFIXES {
                                let mut os = abs.clone().into_os_string();
                                os.push(sfx);
                                owned.add_file(PathBuf::from(os));
                            }
                            owned.add_file(abs);
                        }
                    }
                }
                // No file list (yet): protect the whole folder.
                _ => owned.add_protected_dir(folder),
            }
        }
        for p in self.pending.list() {
            owned.torrents += 1;
            let base = match (&p.output_folder, &p.sub_folder) {
                (Some(o), _) => PathBuf::from(o),
                (None, Some(s)) => self.get_default_output_folder().join(s),
                (None, None) => match &p.name {
                    Some(n) if crate::remove_policy::is_safe_relative(Path::new(n)) => {
                        self.get_default_output_folder().join(n)
                    }
                    _ => continue,
                },
            };
            owned.add_protected_dir(canon(&base));
            if let Some(n) = p.name.as_deref().filter(|n| crate::remove_policy::is_safe_relative(Path::new(n))) {
                owned.add_protected_dir(canon(&base.join(n)));
            }
        }
        owned
    }

    fn cleanup_resolve_roots(&self, requested: Option<&str>) -> anyhow::Result<Vec<PathBuf>> {
        let known = self.cleanup_roots();
        let allowed = self.cleanup_allowed_parents();
        let list: Vec<PathBuf> = match requested.map(str::trim).filter(|s| !s.is_empty()) {
            None => known
                .iter()
                .filter(|r| r.default_on)
                .map(|r| PathBuf::from(&r.path))
                .collect(),
            Some(raw) => raw
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .collect(),
        };
        let mut out = Vec::new();
        for p in list {
            if !p.is_absolute() {
                anyhow::bail!("{}: not an absolute path", p.display());
            }
            if !p.is_dir() {
                anyhow::bail!("{}: not a folder", p.display());
            }
            let c = canon(&p);
            if c.parent().is_none() {
                anyhow::bail!("refusing to scan the filesystem root");
            }
            let is_known = known.iter().any(|k| canon(Path::new(&k.path)) == c);
            if !is_known && !allowed.iter().any(|a| c.starts_with(a)) {
                anyhow::bail!(
                    "{}: not under the download folder, a completion folder or RQBIT_FS_BROWSE_ROOTS",
                    p.display()
                );
            }
            out.push(c);
        }
        if out.is_empty() {
            anyhow::bail!("no folders to scan");
        }
        Ok(dedupe_roots(out))
    }

    /// Read-only scan (dry run). Nothing is moved or deleted.
    pub async fn cleanup_scan(
        self: &Arc<Self>,
        roots: Option<String>,
        min_age_minutes: Option<u64>,
        scheduled: bool,
    ) -> anyhow::Result<ScanResult> {
        let roots = self.cleanup_resolve_roots(roots.as_deref())?;
        let min_age = min_age_minutes.unwrap_or_else(|| self.cleanup_min_age());
        let owned = self.cleanup_owned();
        let ext = self
            .preferences
            .get()
            .incomplete_extension_str()
            .map(|s| s.to_owned());
        let started = std::time::Instant::now();
        let r2 = roots.clone();
        let owned2 = owned.clone();
        let (items, skipped) = tokio::task::spawn_blocking(move || {
            scan(&r2, &owned2, Duration::from_secs(min_age * 60), ext.as_deref(), now_secs())
        })
        .await?;
        let total_bytes = items.iter().map(|i| i.size).sum();
        let res = ScanResult {
            scan_id: format!("{:x}", now_secs() ^ (started.elapsed().subsec_nanos() as u64).rotate_left(20)),
            time: now_secs(),
            roots: roots.iter().map(|r| r.to_string_lossy().into_owned()).collect(),
            min_age_minutes: min_age,
            items,
            skipped,
            total_bytes,
            torrents_checked: owned.torrents,
            protected_folders: owned.protected_dirs.len(),
            scheduled,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        if scheduled || !res.items.is_empty() {
            let msg = format!(
                "{}Cleanup scan: {} orphaned item(s), {} in {} folder(s){}",
                if scheduled { "Scheduled " } else { "" },
                res.items.len(),
                fmt_bytes(res.total_bytes),
                res.roots.len(),
                if res.items.is_empty() { "" } else { ". Review them in Cleanup; nothing was changed." }
            );
            self.events.emit(
                crate::event_log::NewEvent::new(kind::CLEANUP_SCAN, crate::event_log::Severity::Info, msg)
                    .details(serde_json::json!({
                        "scan_id": res.scan_id, "items": res.items.len(),
                        "bytes": res.total_bytes, "roots": res.roots, "scheduled": scheduled,
                    })),
            );
        }
        self.cleanup.remember(res.clone());
        Ok(res)
    }

    pub fn cleanup_get_scan(&self, id: Option<&str>) -> Option<ScanResult> {
        match id {
            Some(id) => self.cleanup.get(id),
            None => self.cleanup.latest(),
        }
    }

    /// Quarantine (default) or delete reviewed items. Every item is re-checked first.
    pub async fn cleanup_apply(self: &Arc<Self>, req: ApplyRequest) -> anyhow::Result<ApplyOutcome> {
        let action = req.action.as_deref().unwrap_or("quarantine");
        if action != "quarantine" && action != "delete" {
            anyhow::bail!("action must be quarantine or delete");
        }
        if action == "delete" && !req.confirm {
            anyhow::bail!("delete is permanent: send confirm=true");
        }
        let scan = self
            .cleanup
            .get(&req.scan_id)
            .ok_or_else(|| anyhow::anyhow!("scan not found (it may be too old): scan again"))?;
        let roots: Vec<PathBuf> = scan.roots.iter().map(PathBuf::from).collect();
        let owned = self.cleanup_owned();
        let min_age = Duration::from_secs(scan.min_age_minutes * 60);
        let want: HashSet<u32> = req.item_ids.iter().copied().collect();
        let picked: Vec<ScanItem> = scan.items.into_iter().filter(|i| want.contains(&i.id)).collect();
        let cleanup = self.cleanup.clone();
        let action_s = action.to_owned();
        let out = tokio::task::spawn_blocking(move || {
            let now = now_secs();
            let mut out = ApplyOutcome {
                action: action_s.clone(),
                ..Default::default()
            };
            let mut ok_items = Vec::new();
            for it in picked {
                if let Some(why) = revalidate(&it, &roots, &owned, min_age, now) {
                    out.results.push(ItemResult {
                        id: Some(it.id),
                        path: it.path.clone(),
                        ok: false,
                        error: Some(format!("skipped: {why}")),
                        size: it.size,
                        ..Default::default()
                    });
                    continue;
                }
                let root = roots
                    .iter()
                    .find(|r| Path::new(&it.path).starts_with(r))
                    .cloned()
                    .unwrap_or_default();
                ok_items.push((it, root));
            }
            if action_s == "delete" {
                for (it, _) in ok_items {
                    match delete_item(Path::new(&it.path)) {
                        Ok(()) => out.results.push(ItemResult {
                            id: Some(it.id),
                            path: it.path,
                            ok: true,
                            size: it.size,
                            ..Default::default()
                        }),
                        Err(e) => out.results.push(ItemResult {
                            id: Some(it.id),
                            path: it.path,
                            ok: false,
                            error: Some(e.to_string()),
                            size: it.size,
                            ..Default::default()
                        }),
                    }
                }
            } else {
                out.batches = cleanup.quarantine(&ok_items, &mut out.results);
            }
            out.ok = out.results.iter().filter(|r| r.ok).count();
            out.failed = out.results.len() - out.ok;
            out.bytes = out.results.iter().filter(|r| r.ok).map(|r| r.size).sum();
            out
        })
        .await?;
        let (k, verb) = if action == "delete" {
            (kind::CLEANUP_DELETED, "Deleted")
        } else {
            (kind::CLEANUP_QUARANTINED, "Moved to quarantine")
        };
        if out.ok > 0 || out.failed > 0 {
            self.events.emit(
                crate::event_log::NewEvent::new(
                    k,
                    if out.failed > 0 {
                        crate::event_log::Severity::Warning
                    } else {
                        crate::event_log::Severity::Info
                    },
                    format!(
                        "Cleanup: {verb} {} orphaned item(s), {}{}",
                        out.ok,
                        fmt_bytes(out.bytes),
                        if out.failed > 0 {
                            format!("; {} skipped or failed", out.failed)
                        } else {
                            String::new()
                        }
                    ),
                )
                .details(serde_json::to_value(&out).unwrap_or_default()),
            );
        }
        Ok(out)
    }

    pub fn cleanup_quarantine(&self) -> QuarantineIndex {
        self.cleanup.load_index()
    }

    pub async fn cleanup_restore(self: &Arc<Self>, req: RestoreRequest) -> anyhow::Result<Vec<ItemResult>> {
        let c = self.cleanup.clone();
        let batch = req.batch.clone();
        let res = tokio::task::spawn_blocking(move || c.restore(&req)).await??;
        let ok = res.iter().filter(|r| r.ok).count();
        self.events.emit(crate::event_log::NewEvent::new(
            kind::CLEANUP_RESTORED,
            crate::event_log::Severity::Info,
            format!(
                "Cleanup: restored {ok} item(s) from quarantine {batch}{}",
                if ok < res.len() {
                    format!("; {} not restored (original path in use)", res.len() - ok)
                } else {
                    String::new()
                }
            ),
        ));
        Ok(res)
    }

    pub async fn cleanup_purge(self: &Arc<Self>, req: PurgeRequest) -> anyhow::Result<(usize, u64)> {
        let c = self.cleanup.clone();
        let batch = req.batch.clone();
        let (n, bytes) = tokio::task::spawn_blocking(move || c.purge(&req)).await??;
        self.events.emit(crate::event_log::NewEvent::new(
            kind::CLEANUP_PURGED,
            crate::event_log::Severity::Info,
            format!("Cleanup: permanently deleted quarantine {batch} ({n} item(s), {})", fmt_bytes(bytes)),
        ));
        Ok((n, bytes))
    }

    /// Scheduled scans (report only), checked every 10 minutes.
    pub(crate) fn start_cleanup_scheduler(self: &Arc<Self>) {
        let s = Arc::downgrade(self);
        self.spawn(
            tracing::debug_span!(parent: self.rs(), "cleanup_scheduler"),
            "cleanup_scheduler",
            async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(600)).await;
                    let Some(s) = s.upgrade() else {
                        return Ok(());
                    };
                    let Some(h) = s.preferences.get().cleanup_scan_hours.filter(|h| *h > 0) else {
                        continue;
                    };
                    let now = now_secs();
                    let due = {
                        let mut last = s.cleanup.last_scheduled.lock();
                        match *last {
                            None => {
                                // First check after start: wait one period.
                                *last = Some(now);
                                false
                            }
                            Some(t) if now.saturating_sub(t) >= h * 3600 => {
                                *last = Some(now);
                                true
                            }
                            _ => false,
                        }
                    };
                    if due && let Err(e) = s.cleanup_scan(None, None, true).await {
                        tracing::warn!("scheduled cleanup scan failed: {e:#}");
                    }
                }
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch_old(p: &Path, bytes: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![1u8; bytes]).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 3600);
        let f = fs::File::options().write(true).open(p).unwrap();
        f.set_modified(old).unwrap();
    }

    fn age_dir(p: &Path) {
        let old = SystemTime::now() - Duration::from_secs(3 * 3600);
        if let Ok(f) = fs::File::open(p) {
            let _ = f.set_modified(old);
        }
    }

    #[test]
    fn scan_reports_only_unowned_old_entries() {
        let t = tempfile::tempdir().unwrap();
        let root = canon(t.path());
        // Owned torrent folder with one used file and one stray file.
        touch_old(&root.join("Show/ep1.mkv"), 10);
        touch_old(&root.join("Show/stray.nfo"), 3);
        touch_old(&root.join("Show/ep2.mkv.part"), 4);
        // Unowned folder, unowned file, recent file, hidden file.
        touch_old(&root.join("Old Movie/movie.mkv"), 20);
        touch_old(&root.join("Old Movie/sub/a.srt"), 2);
        touch_old(&root.join("loose.iso"), 7);
        fs::write(root.join("fresh.bin"), b"x").unwrap();
        touch_old(&root.join(".hidden"), 1);
        touch_old(&root.join("Thumbs.db"), 1);
        // A torrent without metadata protects its whole folder.
        touch_old(&root.join("Pending/x.bin"), 5);
        // Symlink is skipped, never followed.
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("Old Movie"), root.join("link")).unwrap();
        for d in ["Show", "Old Movie/sub", "Old Movie", "Pending"] {
            age_dir(&root.join(d));
        }

        let mut owned = OwnedSet::default();
        owned.add_file(root.join("Show/ep1.mkv"));
        owned.add_protected_dir(root.join("Pending"));

        let (items, sk) = scan(std::slice::from_ref(&root), &owned, Duration::from_secs(3600), None, now_secs());
        let got: Vec<(String, String)> = items
            .iter()
            .map(|i| (i.path.strip_prefix(&*root.to_string_lossy()).unwrap().to_owned(), i.kind.clone()))
            .collect();
        assert!(got.contains(&("/Old Movie".into(), "dir".into())), "{got:?}");
        assert!(got.contains(&("/loose.iso".into(), "file".into())));
        assert!(got.contains(&("/Show/stray.nfo".into(), "file".into())));
        assert!(got.contains(&("/Show/ep2.mkv.part".into(), "file".into())));
        assert!(!got.iter().any(|(p, _)| p.contains("ep1") || p.contains("fresh") || p.contains("Pending") || p.contains("hidden") || p.contains("Thumbs") || p == "/link"));
        let dir = items.iter().find(|i| i.path.ends_with("Old Movie")).unwrap();
        assert_eq!(dir.size, 22);
        assert_eq!(dir.files, 2);
        let part = items.iter().find(|i| i.path.ends_with(".part")).unwrap();
        assert!(part.reason.contains("partial"), "{}", part.reason);
        assert_eq!(sk.recent, 1);
        assert_eq!(sk.hidden, 2);
        #[cfg(unix)]
        assert_eq!(sk.symlinks, 1);
    }

    #[test]
    fn revalidate_catches_new_owners_and_changes() {
        let t = tempfile::tempdir().unwrap();
        let root = canon(t.path());
        touch_old(&root.join("A/f.bin"), 1);
        age_dir(&root.join("A"));
        touch_old(&root.join("b.bin"), 1);
        let owned = OwnedSet::default();
        let (items, _) = scan(std::slice::from_ref(&root), &owned, Duration::from_secs(3600), None, now_secs());
        assert_eq!(items.len(), 2);
        let min = Duration::from_secs(3600);
        for i in &items {
            assert_eq!(revalidate(i, std::slice::from_ref(&root), &owned, min, now_secs()), None);
        }
        // A torrent now uses a file inside A.
        let mut owned2 = OwnedSet::default();
        owned2.add_file(root.join("A/f.bin"));
        let a = items.iter().find(|i| i.kind == "dir").unwrap();
        assert!(revalidate(a, std::slice::from_ref(&root), &owned2, min, now_secs()).is_some());
        // b.bin changed just now.
        fs::write(root.join("b.bin"), b"yy").unwrap();
        let b = items.iter().find(|i| i.kind == "file").unwrap();
        assert_eq!(revalidate(b, std::slice::from_ref(&root), &owned, min, now_secs()).as_deref(), Some("changed recently"));
        // Outside roots.
        let mut out = b.clone();
        out.path = "/etc/passwd".into();
        assert!(revalidate(&out, std::slice::from_ref(&root), &owned, min, now_secs()).is_some());
    }

    #[test]
    fn quarantine_restore_purge_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let root = canon(t.path());
        touch_old(&root.join("A/f.bin"), 3);
        touch_old(&root.join("b.bin"), 4);
        let st = CleanupState::new(root.join("idx.json"));
        let owned = OwnedSet::default();
        let (items, _) = scan(std::slice::from_ref(&root), &owned, Duration::from_secs(0), None, now_secs() + 10);
        let items: Vec<_> = items.into_iter().filter(|i| !i.path.ends_with("idx.json")).collect();
        let mut res = Vec::new();
        let pairs: Vec<_> = items.iter().map(|i| (i.clone(), root.clone())).collect();
        let batches = st.quarantine(&pairs, &mut res);
        assert_eq!(batches.len(), 1);
        assert!(res.iter().all(|r| r.ok), "{res:?}");
        assert!(!root.join("b.bin").exists());
        assert!(root.join(QUARANTINE_DIR).join(&batches[0]).join("b.bin").exists());
        assert!(root.join(QUARANTINE_DIR).join(&batches[0]).join("manifest.json").exists());
        // Original path taken again: restore refuses that one.
        fs::write(root.join("b.bin"), b"new").unwrap();
        let r = st.restore(&RestoreRequest { batch: batches[0].clone(), items: None }).unwrap();
        assert_eq!(r.iter().filter(|x| x.ok).count(), 1);
        assert!(root.join("A/f.bin").exists());
        assert_eq!(fs::read(root.join("b.bin")).unwrap(), b"new");
        assert_eq!(st.load_index().batches[0].items.len(), 1);
        // Purge needs confirm, then removes the batch folder.
        assert!(st.purge(&PurgeRequest { batch: batches[0].clone(), confirm: false }).is_err());
        let (n, bytes) = st.purge(&PurgeRequest { batch: batches[0].clone(), confirm: true }).unwrap();
        assert_eq!((n, bytes), (1, 4));
        assert!(!root.join(QUARANTINE_DIR).join(&batches[0]).exists());
        assert!(st.load_index().batches.is_empty());
    }

    #[test]
    fn roots_dedupe_and_names() {
        let r = dedupe_roots(vec!["/a/b".into(), "/a".into(), "/c".into(), "/a".into()]);
        assert_eq!(r, vec![PathBuf::from("/a"), PathBuf::from("/c")]);
        assert!(is_hidden_or_system("@eaDir"));
        assert!(is_hidden_or_system("$RECYCLE.BIN"));
        assert!(!is_hidden_or_system("Movie (2020)"));
        assert_eq!(civil_from_days(20722), (2026, 9, 26));
    }
}
