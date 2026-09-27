//! Download order (piece picker ordering).
//!
//! Levels: global defaults (preferences) → torrent-wide settings → per-file settings; the
//! most specific one set wins. The result is a single piece order the picker walks
//! (streaming priority pieces still come first).
//!
//! - `sequential_files`: finish files one after another (in `file_order`); off = interleave
//!   the files round-robin.
//! - `file_order`: name (A→Z, the historical default), torrent order, smallest or largest first.
//! - per file `sequential`: pieces in order within the file; off = scattered order (a stable
//!   shuffle, spreading requests over the file).
//! - per file `first_last_first`: fetch the first and last piece of the file before the rest
//!   (media headers / previews).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};

use crate::file_info::FileInfo;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileOrder {
    #[default]
    Name,
    Torrent,
    SmallestFirst,
    LargestFirst,
}

fn t() -> bool {
    true
}

/// Global defaults (preferences). Defaults reproduce the historical picker behaviour.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct DownloadOrderDefaults {
    #[serde(default = "t")]
    pub sequential_files: bool,
    #[serde(default)]
    pub file_order: FileOrder,
    #[serde(default = "t")]
    pub sequential: bool,
    #[serde(default = "t")]
    pub first_last_first: bool,
}

impl Default for DownloadOrderDefaults {
    fn default() -> Self {
        Self { sequential_files: true, file_order: FileOrder::Name, sequential: true, first_last_first: true }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileOrderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequential: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_last_first: Option<bool>,
}

impl FileOrderSettings {
    fn is_empty(&self) -> bool {
        self.sequential.is_none() && self.first_last_first.is_none()
    }
}

/// Per-torrent settings (None = inherit).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TorrentDownloadOrder {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequential_files: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_order: Option<FileOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequential: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_last_first: Option<bool>,
    /// Per-file overrides by file index.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<usize, FileOrderSettings>,
}

impl TorrentDownloadOrder {
    pub fn is_empty(&self) -> bool {
        self.sequential_files.is_none()
            && self.file_order.is_none()
            && self.sequential.is_none()
            && self.first_last_first.is_none()
            && self.files.is_empty()
    }
}

/// Distinguish "absent" (keep) from `null` (clear override) in patches.
fn double<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Ok(Some(Option::<T>::deserialize(d)?))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FilesPatch {
    pub ids: Vec<usize>,
    #[serde(default, deserialize_with = "double")]
    pub sequential: Option<Option<bool>>,
    #[serde(default, deserialize_with = "double")]
    pub first_last_first: Option<Option<bool>>,
}

/// `POST /torrents/{id}/download_order` body. Absent = unchanged, null = inherit.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DownloadOrderPatch {
    #[serde(default, deserialize_with = "double")]
    pub sequential_files: Option<Option<bool>>,
    #[serde(default, deserialize_with = "double")]
    pub file_order: Option<Option<FileOrder>>,
    #[serde(default, deserialize_with = "double")]
    pub sequential: Option<Option<bool>>,
    #[serde(default, deserialize_with = "double")]
    pub first_last_first: Option<Option<bool>>,
    #[serde(default)]
    pub files: Vec<FilesPatch>,
    /// Drop every per-torrent and per-file setting.
    #[serde(default)]
    pub reset: bool,
}

impl DownloadOrderPatch {
    pub fn apply(&self, o: &mut TorrentDownloadOrder, file_count: usize) -> anyhow::Result<()> {
        if self.reset {
            *o = TorrentDownloadOrder::default();
        }
        if let Some(v) = self.sequential_files {
            o.sequential_files = v;
        }
        if let Some(v) = self.file_order {
            o.file_order = v;
        }
        if let Some(v) = self.sequential {
            o.sequential = v;
        }
        if let Some(v) = self.first_last_first {
            o.first_last_first = v;
        }
        for fp in &self.files {
            for id in &fp.ids {
                if *id >= file_count {
                    anyhow::bail!("file id {id} out of range (torrent has {file_count} files)");
                }
                let e = o.files.entry(*id).or_default();
                if let Some(v) = fp.sequential {
                    e.sequential = v;
                }
                if let Some(v) = fp.first_last_first {
                    e.first_last_first = v;
                }
            }
        }
        o.files.retain(|_, v| !v.is_empty());
        Ok(())
    }
}

/// Resolved torrent-level settings.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct EffectiveTorrentOrder {
    pub sequential_files: bool,
    pub file_order: FileOrder,
    pub sequential: bool,
    pub first_last_first: bool,
}

pub fn effective(g: &DownloadOrderDefaults, t: &TorrentDownloadOrder) -> EffectiveTorrentOrder {
    EffectiveTorrentOrder {
        sequential_files: t.sequential_files.unwrap_or(g.sequential_files),
        file_order: t.file_order.unwrap_or(g.file_order),
        sequential: t.sequential.unwrap_or(g.sequential),
        first_last_first: t.first_last_first.unwrap_or(g.first_last_first),
    }
}

/// Resolved per-file settings: (sequential, first_last_first).
pub fn effective_file(e: &EffectiveTorrentOrder, t: &TorrentDownloadOrder, file: usize) -> (bool, bool) {
    let f = t.files.get(&file).copied().unwrap_or_default();
    (f.sequential.unwrap_or(e.sequential), f.first_last_first.unwrap_or(e.first_last_first))
}

fn scatter_key(piece: usize) -> u64 {
    // splitmix64: stable, cheap, well spread.
    let mut z = (piece as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Piece order within one file.
pub fn file_piece_order(range: std::ops::Range<usize>, sequential: bool, first_last_first: bool) -> Vec<usize> {
    let mut out = Vec::with_capacity(range.len());
    if range.is_empty() {
        return out;
    }
    let (first, last) = (range.start, range.end - 1);
    if first_last_first {
        out.push(first);
        if last != first {
            out.push(last);
        }
    }
    let mut rest: Vec<usize> = range.filter(|p| !first_last_first || (*p != first && *p != last)).collect();
    if !sequential {
        rest.sort_by_key(|p| scatter_key(*p));
    }
    out.extend(rest);
    out
}

/// The full piece order the picker walks. Pieces shared by two files appear once (at their
/// first occurrence). Padding files are skipped.
pub fn compute_piece_order(files: &[FileInfo], g: &DownloadOrderDefaults, t: &TorrentDownloadOrder) -> Vec<usize> {
    let e = effective(g, t);
    let mut ids: Vec<usize> = (0..files.len()).filter(|i| !files[*i].attrs.padding).collect();
    match e.file_order {
        FileOrder::Name => ids.sort_by(|a, b| files[*a].relative_filename.cmp(&files[*b].relative_filename)),
        FileOrder::Torrent => {}
        FileOrder::SmallestFirst => ids.sort_by_key(|i| (files[*i].len, *i)),
        FileOrder::LargestFirst => ids.sort_by_key(|i| (std::cmp::Reverse(files[*i].len), *i)),
    }
    let per_file: Vec<Vec<usize>> = ids
        .iter()
        .map(|i| {
            let (seq, fl) = effective_file(&e, t, *i);
            file_piece_order(files[*i].piece_range_usize(), seq, fl)
        })
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    if e.sequential_files {
        for p in per_file.into_iter().flatten() {
            if seen.insert(p) {
                out.push(p);
            }
        }
    } else {
        let mut iters: Vec<_> = per_file.into_iter().map(|v| v.into_iter()).collect();
        loop {
            let mut any = false;
            for it in iters.iter_mut() {
                if let Some(p) = it.next() {
                    any = true;
                    if seen.insert(p) {
                        out.push(p);
                    }
                }
            }
            if !any {
                break;
            }
        }
    }
    out
}

/// One file row of `GET /torrents/{id}/download_order`.
#[derive(Debug, Clone, Serialize)]
pub struct FileOrderView {
    pub id: usize,
    pub name: String,
    pub sequential: bool,
    pub first_last_first: bool,
    #[serde(rename = "override")]
    pub override_: FileOrderSettings,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadOrderView {
    pub global: DownloadOrderDefaults,
    #[serde(rename = "torrent")]
    pub torrent: TorrentDownloadOrder,
    pub effective: EffectiveTorrentOrder,
    pub files: Vec<FileOrderView>,
    /// Human summary, e.g. "Sequential files, name order; first & last pieces first".
    pub summary: String,
}

pub fn summary(e: &EffectiveTorrentOrder, overridden_files: usize) -> String {
    let order = match e.file_order {
        FileOrder::Name => "name order",
        FileOrder::Torrent => "torrent order",
        FileOrder::SmallestFirst => "smallest first",
        FileOrder::LargestFirst => "largest first",
    };
    let mut s = format!(
        "{} ({order}); pieces {}{}",
        if e.sequential_files { "Files one after another" } else { "Files interleaved" },
        if e.sequential { "in order" } else { "scattered" },
        if e.first_last_first { ", first & last pieces first" } else { "" }
    );
    if overridden_files > 0 {
        s += &format!("; {overridden_files} file(s) with their own setting");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, pieces: std::ops::Range<u32>, len: u64) -> FileInfo {
        FileInfo { relative_filename: name.into(), offset_in_torrent: 0, piece_range: pieces, attrs: Default::default(), len }
    }

    fn files() -> Vec<FileInfo> {
        // b: pieces 0..4 (400 bytes), a: 4..6 (200), c: 6..9 (300)
        vec![f("b", 0..4, 400), f("a", 4..6, 200), f("c", 6..9, 300)]
    }

    #[test]
    fn default_matches_historical_order() {
        let o = compute_piece_order(&files(), &Default::default(), &Default::default());
        // name order a, b, c; each first, last, middle
        assert_eq!(o, vec![4, 5, 0, 3, 1, 2, 6, 8, 7]);
    }

    #[test]
    fn file_orders_and_sequential_pieces() {
        let g = DownloadOrderDefaults { first_last_first: false, ..Default::default() };
        let t = TorrentDownloadOrder { file_order: Some(FileOrder::SmallestFirst), ..Default::default() };
        assert_eq!(compute_piece_order(&files(), &g, &t), vec![4, 5, 6, 7, 8, 0, 1, 2, 3]);
        let t = TorrentDownloadOrder { file_order: Some(FileOrder::LargestFirst), ..Default::default() };
        assert_eq!(compute_piece_order(&files(), &g, &t), vec![0, 1, 2, 3, 6, 7, 8, 4, 5]);
        let t = TorrentDownloadOrder { file_order: Some(FileOrder::Torrent), ..Default::default() };
        assert_eq!(compute_piece_order(&files(), &g, &t), (0..9).collect::<Vec<_>>());
    }

    #[test]
    fn interleaved_files() {
        let g = DownloadOrderDefaults { first_last_first: false, sequential_files: false, file_order: FileOrder::Torrent, ..Default::default() };
        assert_eq!(compute_piece_order(&files(), &g, &Default::default()), vec![0, 4, 6, 1, 5, 7, 2, 8, 3]);
    }

    #[test]
    fn per_file_overrides_torrent_overrides_global() {
        let g = DownloadOrderDefaults { first_last_first: false, file_order: FileOrder::Torrent, ..Default::default() };
        let mut t = TorrentDownloadOrder { first_last_first: Some(true), ..Default::default() };
        t.files.insert(0, FileOrderSettings { first_last_first: Some(false), sequential: None });
        let o = compute_piece_order(&files(), &g, &t);
        assert_eq!(&o[..4], &[0, 1, 2, 3]); // file 0: per-file says no first/last
        assert_eq!(&o[4..6], &[4, 5]);
        assert_eq!(&o[6..], &[6, 8, 7]); // file 2: torrent-wide first/last
        // Scattered: same set, not in order, stable.
        let mut t2 = TorrentDownloadOrder::default();
        t2.files.insert(0, FileOrderSettings { sequential: Some(false), first_last_first: Some(false) });
        let big = vec![f("x", 0..64, 64)];
        let o = compute_piece_order(&big, &g, &t2);
        let mut s = o.clone();
        s.sort();
        assert_eq!(s, (0..64).collect::<Vec<_>>());
        assert_ne!(o, s);
        assert_eq!(o, compute_piece_order(&big, &g, &t2));
    }

    #[test]
    fn shared_pieces_once_and_padding_skipped() {
        let mut fs = vec![f("a", 0..3, 10), f("b", 2..5, 10), f("pad", 5..6, 1)];
        fs[2].attrs.padding = true;
        let g = DownloadOrderDefaults { first_last_first: false, file_order: FileOrder::Torrent, ..Default::default() };
        assert_eq!(compute_piece_order(&fs, &g, &Default::default()), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn patch_semantics() {
        let mut o = TorrentDownloadOrder::default();
        let p: DownloadOrderPatch = serde_json::from_str(r#"{"sequential_files": false, "files":[{"ids":[1,2],"sequential":false}]}"#).unwrap();
        p.apply(&mut o, 3).unwrap();
        assert_eq!(o.sequential_files, Some(false));
        assert_eq!(o.files.len(), 2);
        let p: DownloadOrderPatch = serde_json::from_str(r#"{"sequential_files": null, "files":[{"ids":[1],"sequential":null}]}"#).unwrap();
        p.apply(&mut o, 3).unwrap();
        assert_eq!(o.sequential_files, None);
        assert_eq!(o.files.keys().copied().collect::<Vec<_>>(), vec![2]);
        let p: DownloadOrderPatch = serde_json::from_str(r#"{"files":[{"ids":[7],"sequential":true}]}"#).unwrap();
        assert!(p.apply(&mut o, 3).is_err());
        let p: DownloadOrderPatch = serde_json::from_str(r#"{"reset": true}"#).unwrap();
        p.apply(&mut o, 3).unwrap();
        assert!(o.is_empty());
    }
}
