//! Adopt data another BitTorrent client left behind ("transfer from other
//! client").
//!
//! Nothing here is client specific. For every torrent file we know the
//! expected relative path, size and piece hashes, so an in-progress file
//! written by another client under a suffixed name (`Movie.mkv.!qB`,
//! `Movie.mkv.part`, `Movie.mkv.incomplete`, `Movie.mkv~`, ...) is found by
//! looking for a *unique* sibling named `<expected name><any suffix>` and
//! then checked against the torrent's piece hashes before it is renamed.
//!
//! Safety rules:
//! - never delete or truncate anything; only rename, and never onto an
//!   existing path;
//! - a name that is itself a file of this torrent (e.g. split archives
//!   `a.zip.0.part`, `a.zip.1.part`) is never treated as a partial of another
//!   file, and neither is a name that belongs to a longer expected name;
//! - more than one candidate for a file = ambiguous, nothing renamed;
//! - a candidate larger than the expected size is ignored;
//! - sampled pieces must hash-match, or be all zeros (unfetched,
//!   preallocated space); otherwise the file is left alone.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use serde::Serialize;
use sha1w::{ISha1, Sha1};
use tracing::{info, warn};

/// One selected, non-padding torrent file.
#[derive(Debug, Clone)]
pub(crate) struct AdoptFile {
    pub file_id: usize,
    pub rel: PathBuf,
    pub len: u64,
    pub offset_in_torrent: u64,
}

/// Piece geometry and hashes of the torrent.
pub(crate) struct AdoptPieces<'a> {
    pub piece_len: u64,
    pub total_len: u64,
    /// Concatenated 20-byte SHA1 piece hashes.
    pub hashes: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AdoptEvidence {
    /// At least one sampled piece matched the torrent's hash.
    PieceHashMatch,
    /// No sampled piece matched, but all sampled regions were zeros
    /// (preallocated space the other client hadn't downloaded yet).
    UnfetchedZeros,
    /// No piece lies entirely inside the file/candidate, so only the
    /// unique-name and size rules applied.
    SizeOnly,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AdoptRename {
    pub file_id: usize,
    pub from: PathBuf,
    pub to: PathBuf,
    pub evidence: AdoptEvidence,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AdoptSkip {
    pub file: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default, Serialize)]
pub(crate) struct AdoptPlan {
    /// Files that exist under their final name and are reused as-is.
    pub keep_final: HashSet<usize>,
    /// Files already present under rqbit's own incomplete name.
    pub own_partial: HashSet<usize>,
    pub renames: Vec<AdoptRename>,
    /// More than one suffixed candidate: nothing renamed.
    pub ambiguous: Vec<AdoptSkip>,
    /// Candidate rejected (larger than expected, or piece sample mismatch).
    pub rejected: Vec<AdoptSkip>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct AdoptSummary {
    pub reused: usize,
    pub renamed: usize,
    pub ambiguous: Vec<String>,
    pub rejected: Vec<String>,
    pub skipped_target_exists: Vec<String>,
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut os = p.as_os_str().to_owned();
    os.push(suffix);
    PathBuf::from(os)
}

fn regular_file_len(p: &Path) -> anyhow::Result<Option<u64>> {
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_file() => Ok(Some(m.len())),
        Ok(_) => bail!("{p:?} exists but is not a regular file"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("error inspecting {p:?}")),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Sample {
    Verified,
    Zeros,
    NoFullPiece,
    Mismatch,
}

/// Pick up to `n` indices spread over `0..count` (first, last, then evenly).
fn spread(count: usize, n: usize) -> Vec<usize> {
    if count == 0 {
        return vec![];
    }
    if count <= n {
        return (0..count).collect();
    }
    let mut v: Vec<usize> = (0..n).map(|k| k * (count - 1) / (n - 1)).collect();
    v.dedup();
    v
}

/// Check a candidate's content against pieces lying fully inside both the
/// torrent file's byte range and the candidate's current length.
fn sample_candidate(
    path: &Path,
    file: &AdoptFile,
    cand_len: u64,
    pieces: &AdoptPieces<'_>,
) -> anyhow::Result<Sample> {
    let pl = pieces.piece_len;
    if pl == 0 {
        return Ok(Sample::NoFullPiece);
    }
    let fstart = file.offset_in_torrent;
    let limit = fstart + file.len.min(cand_len);
    let piece_end = |i: u64| ((i + 1) * pl).min(pieces.total_len);
    let first = fstart.div_ceil(pl);
    // Pieces first..last_excl are fully inside [fstart, limit).
    let mut last_excl = first;
    if first * pl < limit {
        // All pieces except the torrent's last have length pl.
        let n_full = (limit - first * pl) / pl;
        last_excl = first + n_full;
        // The torrent's final (short) piece may also fit.
        if last_excl * pl < pieces.total_len && piece_end(last_excl) <= limit {
            last_excl += 1;
        }
    }
    let count = last_excl.saturating_sub(first) as usize;
    if count == 0 {
        return Ok(Sample::NoFullPiece);
    }

    let mut f = std::fs::File::open(path).with_context(|| format!("error opening {path:?}"))?;
    let mut buf = vec![0u8; pl as usize];
    let mut sampled = HashSet::new();
    let mut all_zero = true;
    // First a few pieces; if inconclusive, a wider sample before giving up
    // (a partial file may have half-written, unverified pieces).
    for n in [3usize, 16] {
        for k in spread(count, n) {
            if !sampled.insert(k) {
                continue;
            }
            let i = first + k as u64;
            let start = i * pl;
            let len = (piece_end(i) - start) as usize;
            f.seek(SeekFrom::Start(start - fstart))?;
            f.read_exact(&mut buf[..len])
                .with_context(|| format!("error reading {path:?}"))?;
            let data = &buf[..len];
            let mut h = Sha1::new();
            h.update(data);
            let hash = h.finish();
            let expected = pieces.hashes.get(i as usize * 20..i as usize * 20 + 20);
            if expected == Some(&hash[..]) {
                return Ok(Sample::Verified);
            }
            if data.iter().any(|b| *b != 0) {
                all_zero = false;
            }
        }
        if all_zero {
            return Ok(Sample::Zeros);
        }
    }
    Ok(Sample::Mismatch)
}

/// Work out what to adopt, without touching anything.
///
/// `files`: selected non-padding files. `all_rel`: relative paths of *all*
/// files of the torrent (used to never mistake a real torrent file for a
/// partial). `incomplete_ext`: rqbit's own incomplete-file suffix, if enabled.
///
/// Errors (refusing the whole add) if a file exists under its final name but
/// is larger than the torrent expects: rqbit would truncate it.
pub(crate) fn plan_adoption(
    output_folder: &Path,
    files: &[AdoptFile],
    all_rel: &[PathBuf],
    pieces: &AdoptPieces<'_>,
    incomplete_ext: Option<&str>,
) -> anyhow::Result<AdoptPlan> {
    let expected: HashSet<&Path> = all_rel.iter().map(|p| p.as_path()).collect();
    // Expected basenames per relative directory.
    let mut names_in_dir: HashMap<PathBuf, Vec<OsString>> = HashMap::new();
    for p in all_rel {
        if let Some(n) = p.file_name() {
            let dir = p.parent().map(|d| d.to_owned()).unwrap_or_default();
            names_in_dir.entry(dir).or_default().push(n.to_owned());
        }
    }
    let mut dir_cache: HashMap<PathBuf, Vec<(OsString, u64)>> = HashMap::new();

    let mut plan = AdoptPlan::default();
    let mut too_big = Vec::new();

    for file in files {
        let final_path = output_folder.join(&file.rel);
        if let Some(sz) = regular_file_len(&final_path)? {
            if sz > file.len {
                too_big.push(format!("{final_path:?} is {sz} bytes, torrent expects {}", file.len));
            }
            plan.keep_final.insert(file.file_id);
            continue;
        }
        let target = match incomplete_ext {
            Some(ext) => with_suffix(&final_path, ext),
            None => final_path.clone(),
        };
        if incomplete_ext.is_some() {
            if let Some(sz) = regular_file_len(&target)? {
                if sz > file.len {
                    too_big.push(format!("{target:?} is {sz} bytes, torrent expects {}", file.len));
                }
                plan.own_partial.insert(file.file_id);
                continue;
            }
        }

        let (Some(base), Some(abs_dir)) = (file.rel.file_name(), final_path.parent()) else {
            continue;
        };
        let rel_dir = file.rel.parent().map(|d| d.to_owned()).unwrap_or_default();
        let entries = match dir_cache.get(abs_dir) {
            Some(e) => e,
            None => {
                let mut v = Vec::new();
                match std::fs::read_dir(abs_dir) {
                    Ok(rd) => {
                        for e in rd {
                            let e = e.with_context(|| format!("error listing {abs_dir:?}"))?;
                            // symlink_metadata semantics: skip dirs and symlinks.
                            let Ok(ft) = e.file_type() else { continue };
                            if !ft.is_file() {
                                continue;
                            }
                            let Ok(md) = e.metadata() else { continue };
                            v.push((e.file_name(), md.len()));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(e).with_context(|| format!("error listing {abs_dir:?}"));
                    }
                }
                dir_cache.entry(abs_dir.to_owned()).or_insert(v)
            }
        };

        let base_b = base.as_encoded_bytes();
        let longer_expected: Vec<&[u8]> = names_in_dir
            .get(&rel_dir)
            .into_iter()
            .flatten()
            .map(|n| n.as_encoded_bytes())
            .filter(|n| n.len() > base_b.len() && n.starts_with(base_b))
            .collect();

        let mut candidates = Vec::new();
        for (name, sz) in entries {
            let nb = name.as_encoded_bytes();
            if nb.len() <= base_b.len() || !nb.starts_with(base_b) {
                continue;
            }
            if expected.contains(rel_dir.join(name).as_path()) {
                continue;
            }
            // `a.zip.0.part.!qB` belongs to expected `a.zip.0.part`, not `a.zip`.
            if longer_expected.iter().any(|l| nb.starts_with(l)) {
                continue;
            }
            let path = abs_dir.join(name);
            if *sz > file.len {
                plan.rejected.push(AdoptSkip {
                    file: path,
                    reason: format!("larger than expected ({sz} > {} bytes)", file.len),
                });
                continue;
            }
            candidates.push((path, *sz));
        }

        match candidates.len() {
            0 => {}
            1 => {
                let (path, sz) = candidates.pop().unwrap();
                let evidence = match sample_candidate(&path, file, sz, pieces)? {
                    Sample::Verified => AdoptEvidence::PieceHashMatch,
                    Sample::Zeros => AdoptEvidence::UnfetchedZeros,
                    Sample::NoFullPiece => AdoptEvidence::SizeOnly,
                    Sample::Mismatch => {
                        plan.rejected.push(AdoptSkip {
                            file: path,
                            reason: "sampled pieces don't match the torrent (probably unrelated data)"
                                .into(),
                        });
                        continue;
                    }
                };
                plan.renames.push(AdoptRename {
                    file_id: file.file_id,
                    from: path,
                    to: target,
                    evidence,
                });
            }
            _ => plan.ambiguous.push(AdoptSkip {
                file: final_path,
                reason: format!(
                    "{} candidates: {}",
                    candidates.len(),
                    candidates
                        .iter()
                        .map(|(p, _)| p.file_name().unwrap_or_default().to_string_lossy())
                        .join_str(", ")
                ),
            }),
        }
    }

    if !too_big.is_empty() {
        let n = too_big.len();
        too_big.truncate(5);
        bail!(
            "refusing to adopt existing data in {output_folder:?}: {n} file(s) are larger than the torrent expects, so this is probably different data and would be truncated: {}",
            too_big.join("; ")
        );
    }
    Ok(plan)
}

trait JoinStr {
    fn join_str(self, sep: &str) -> String;
}
impl<I: Iterator<Item = S>, S: AsRef<str>> JoinStr for I {
    fn join_str(self, sep: &str) -> String {
        let mut out = String::new();
        for (i, s) in self.enumerate() {
            if i > 0 {
                out.push_str(sep);
            }
            out.push_str(s.as_ref());
        }
        out
    }
}

/// UTC RFC 3339 timestamp without extra dependencies.
fn rfc3339_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

#[derive(Serialize)]
struct LogLine<'a> {
    ts: String,
    info_hash: &'a str,
    name: Option<&'a str>,
    output_folder: &'a Path,
    from: &'a Path,
    to: &'a Path,
    evidence: AdoptEvidence,
}

/// Carry out a plan: rename (never onto an existing path), append each rename
/// to `log_path` as a JSON line, and log a summary. Returns the file ids that
/// keep their final name (so rqbit's incomplete suffix isn't applied).
pub(crate) fn apply_adoption(
    plan: AdoptPlan,
    output_folder: &Path,
    incomplete_ext: Option<&str>,
    log_path: Option<&Path>,
    info_hash: &str,
    name: Option<&str>,
) -> anyhow::Result<(HashSet<usize>, AdoptSummary)> {
    let mut keep_final = plan.keep_final;
    let mut summary = AdoptSummary {
        reused: keep_final.len() + plan.own_partial.len(),
        ..Default::default()
    };
    for a in &plan.ambiguous {
        warn!(file = ?a.file, reason = %a.reason, "adopt: ambiguous partial file, not renaming");
        summary.ambiguous.push(format!("{}: {}", a.file.display(), a.reason));
    }
    for r in &plan.rejected {
        warn!(file = ?r.file, reason = %r.reason, "adopt: not adopting candidate");
        summary.rejected.push(format!("{}: {}", r.file.display(), r.reason));
    }
    for r in plan.renames {
        if r.from == r.to {
            continue;
        }
        if std::fs::symlink_metadata(&r.to).is_ok() {
            warn!(from = ?r.from, to = ?r.to, "adopt: not renaming partial file, target already exists");
            summary.skipped_target_exists.push(r.from.display().to_string());
            continue;
        }
        std::fs::rename(&r.from, &r.to)
            .with_context(|| format!("error renaming {:?} to {:?}", r.from, r.to))?;
        info!(from = ?r.from, to = ?r.to, evidence = ?r.evidence, info_hash, "adopt: renamed partial file");
        if let Some(lp) = log_path {
            let line = LogLine {
                ts: rfc3339_now(),
                info_hash,
                name,
                output_folder,
                from: &r.from,
                to: &r.to,
                evidence: r.evidence,
            };
            let res = serde_json::to_string(&line)
                .map_err(anyhow::Error::from)
                .and_then(|mut s| {
                    s.push('\n');
                    let mut f = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(lp)?;
                    f.write_all(s.as_bytes())?;
                    Ok(())
                });
            if let Err(e) = res {
                warn!(log = ?lp, "adopt: error appending to rename log: {e:#}");
            }
        }
        summary.renamed += 1;
        if incomplete_ext.is_none() {
            keep_final.insert(r.file_id);
        }
    }
    info!(
        ?output_folder,
        reused = summary.reused,
        renamed = summary.renamed,
        ambiguous = summary.ambiguous.len(),
        rejected = summary.rejected.len(),
        "adopted data from other client"
    );
    Ok((keep_final, summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PL: u64 = 32;

    struct T {
        dir: tempfile::TempDir,
        files: Vec<AdoptFile>,
        all_rel: Vec<PathBuf>,
        data: Vec<Vec<u8>>,
        hashes: Vec<u8>,
        total: u64,
    }

    /// Build a torrent layout with deterministic content per file.
    fn torrent(spec: &[(&str, u64)]) -> T {
        let mut files = Vec::new();
        let mut data = Vec::new();
        let mut all = Vec::new();
        let mut off = 0;
        for (i, (name, len)) in spec.iter().enumerate() {
            let d: Vec<u8> = (0..*len).map(|b| ((b * 7 + i as u64 * 13) % 251 + 1) as u8).collect();
            all.extend_from_slice(&d);
            data.push(d);
            files.push(AdoptFile {
                file_id: i,
                rel: PathBuf::from(name),
                len: *len,
                offset_in_torrent: off,
            });
            off += len;
        }
        let mut hashes = Vec::new();
        for chunk in all.chunks(PL as usize) {
            let mut h = Sha1::new();
            h.update(chunk);
            hashes.extend_from_slice(&h.finish());
        }
        T {
            dir: tempfile::tempdir().unwrap(),
            all_rel: files.iter().map(|f| f.rel.clone()).collect(),
            files,
            data,
            hashes,
            total: off,
        }
    }

    impl T {
        fn write(&self, name: &str, content: &[u8]) {
            let p = self.dir.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }
        fn plan(&self, ext: Option<&str>) -> anyhow::Result<AdoptPlan> {
            plan_adoption(
                self.dir.path(),
                &self.files,
                &self.all_rel,
                &AdoptPieces {
                    piece_len: PL,
                    total_len: self.total,
                    hashes: &self.hashes,
                },
                ext,
            )
        }
        fn exists(&self, name: &str) -> bool {
            self.dir.path().join(name).exists()
        }
    }

    /// Partial: first `have` bytes real, the rest zeros (preallocated).
    fn partial(d: &[u8], have: usize) -> Vec<u8> {
        let mut v = vec![0u8; d.len()];
        v[..have].copy_from_slice(&d[..have]);
        v
    }

    #[test]
    fn qbit_partial_adopted() {
        let t = torrent(&[("Movie.mkv", 320)]);
        t.write("Movie.mkv.!qB", &partial(&t.data[0], 64));
        let plan = t.plan(None).unwrap();
        assert_eq!(plan.renames.len(), 1);
        assert_eq!(plan.renames[0].evidence, AdoptEvidence::PieceHashMatch);
        assert_eq!(plan.renames[0].to, t.dir.path().join("Movie.mkv"));

        let log = t.dir.path().join("adopt-log.jsonl");
        let (keep, summary) =
            apply_adoption(plan, t.dir.path(), None, Some(&log), "abcd", Some("Movie")).unwrap();
        assert_eq!(summary.renamed, 1);
        assert!(keep.contains(&0));
        assert!(t.exists("Movie.mkv") && !t.exists("Movie.mkv.!qB"));
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.contains("Movie.mkv.!qB") && logged.contains("\"info_hash\":\"abcd\""));
        assert_eq!(logged.lines().count(), 1);
    }

    #[test]
    fn part_partial_adopted_to_rqbit_incomplete_name() {
        let t = torrent(&[("Show/ep1.mkv", 200), ("Show/ep2.mkv", 150)]);
        // A truncated partial (the other client doesn't preallocate).
        t.write("Show/ep2.mkv.part", &t.data[1][..100]);
        t.write("Show/ep1.mkv", &t.data[0]);
        let plan = t.plan(Some(".rqbit-incomplete")).unwrap();
        assert!(plan.keep_final.contains(&0));
        assert_eq!(plan.renames.len(), 1);
        assert_eq!(
            plan.renames[0].to,
            t.dir.path().join("Show/ep2.mkv.rqbit-incomplete")
        );
        let (keep, _) =
            apply_adoption(plan, t.dir.path(), Some(".rqbit-incomplete"), None, "x", None).unwrap();
        assert!(!keep.contains(&1));
        assert!(t.exists("Show/ep2.mkv.rqbit-incomplete"));
    }

    #[test]
    fn preallocated_zero_partial_adopted() {
        let t = torrent(&[("a.bin", 256)]);
        t.write("a.bin.crdownload", &vec![0u8; 256]);
        let plan = t.plan(None).unwrap();
        assert_eq!(plan.renames.len(), 1);
        assert_eq!(plan.renames[0].evidence, AdoptEvidence::UnfetchedZeros);
    }

    #[test]
    fn split_archive_parts_are_not_partials() {
        // The torrent itself contains a.zip and its split parts; a.zip is
        // missing on disk but must not be "adopted" from a.zip.0.part.
        let t = torrent(&[("a.zip", 100), ("a.zip.0.part", 64), ("a.zip.1.part", 64)]);
        t.write("a.zip.0.part", &t.data[1]);
        t.write("a.zip.1.part", &t.data[2]);
        let plan = t.plan(None).unwrap();
        assert!(plan.renames.is_empty(), "{:?}", plan.renames);
        assert!(plan.ambiguous.is_empty());
        assert!(plan.keep_final.contains(&1) && plan.keep_final.contains(&2));

        // Parts only, one of them partial: its partial belongs to it, not to a.zip.
        let t = torrent(&[("a.zip", 100), ("a.zip.0.part", 64), ("a.zip.1.part", 64)]);
        t.write("a.zip.0.part", &t.data[1]);
        t.write("a.zip.1.part.!qB", &t.data[2]);
        let plan = t.plan(None).unwrap();
        assert_eq!(plan.renames.len(), 1);
        assert_eq!(plan.renames[0].file_id, 2);
        assert!(plan.ambiguous.is_empty());
    }

    #[test]
    fn two_candidates_are_ambiguous() {
        let t = torrent(&[("Movie.mkv", 320)]);
        t.write("Movie.mkv.!qB", &partial(&t.data[0], 64));
        t.write("Movie.mkv.part", &partial(&t.data[0], 64));
        let plan = t.plan(None).unwrap();
        assert!(plan.renames.is_empty());
        assert_eq!(plan.ambiguous.len(), 1);
        let (_, summary) = apply_adoption(plan, t.dir.path(), None, None, "x", None).unwrap();
        assert_eq!(summary.renamed, 0);
        assert!(t.exists("Movie.mkv.!qB") && t.exists("Movie.mkv.part"));
    }

    #[test]
    fn larger_candidate_rejected() {
        let t = torrent(&[("Movie.mkv", 320)]);
        let mut big = t.data[0].clone();
        big.extend_from_slice(&[1, 2, 3]);
        t.write("Movie.mkv.!qB", &big);
        let plan = t.plan(None).unwrap();
        assert!(plan.renames.is_empty());
        assert_eq!(plan.rejected.len(), 1);
        assert!(plan.rejected[0].reason.contains("larger"));
    }

    #[test]
    fn unrelated_nonzero_data_rejected_by_piece_sample() {
        let t = torrent(&[("Movie.mkv", 320)]);
        let junk: Vec<u8> = (0..320u32).map(|i| (i * 31 % 253) as u8 | 1).collect();
        t.write("Movie.mkv.!qB", &junk);
        let plan = t.plan(None).unwrap();
        assert!(plan.renames.is_empty());
        assert_eq!(plan.rejected.len(), 1);
        assert!(plan.rejected[0].reason.contains("don't match"));
    }

    #[test]
    fn small_file_falls_back_to_size_rule() {
        // Second file lies inside pieces shared with neighbours: no full piece.
        let t = torrent(&[("a.bin", 40), ("b.nfo", 10), ("c.bin", 40)]);
        t.write("b.nfo.!qB", b"garbage!!!");
        let plan = t.plan(None).unwrap();
        assert_eq!(plan.renames.len(), 1);
        assert_eq!(plan.renames[0].evidence, AdoptEvidence::SizeOnly);
    }

    #[test]
    fn larger_final_file_refuses_add() {
        let t = torrent(&[("Movie.mkv", 64)]);
        t.write("Movie.mkv", &vec![1u8; 100]);
        assert!(t.plan(None).is_err());
    }

    #[test]
    fn target_exists_is_skipped() {
        let t = torrent(&[("Movie.mkv", 320)]);
        t.write("Movie.mkv.!qB", &partial(&t.data[0], 64));
        let plan = t.plan(Some(".part")).unwrap();
        assert_eq!(plan.renames.len(), 1);
        // Something appears at the target between planning and applying.
        t.write("Movie.mkv.part", b"x");
        let (_, s) = apply_adoption(plan, t.dir.path(), Some(".part"), None, "x", None).unwrap();
        assert_eq!(s.renamed, 0);
        assert_eq!(s.skipped_target_exists.len(), 1);
        assert!(t.exists("Movie.mkv.!qB"));
    }

    #[test]
    fn own_incomplete_name_is_used_as_is() {
        let t = torrent(&[("Movie.mkv", 320)]);
        t.write("Movie.mkv.part", &partial(&t.data[0], 64));
        let plan = t.plan(Some(".part")).unwrap();
        assert!(plan.renames.is_empty() && plan.ambiguous.is_empty());
        assert!(plan.own_partial.contains(&0));
    }

    #[test]
    fn rfc3339_shape() {
        let s = rfc3339_now();
        assert_eq!(s.len(), 20);
        assert!(s.ends_with('Z') && s.starts_with("20"));
    }
}
