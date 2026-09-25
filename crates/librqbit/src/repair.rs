//! Repair of torrent files whose on-disk data can no longer be read.
//!
//! Some filesystems can end up with ranges of a file that return EIO on every read
//! (seen on bcachefs as `data read error ... key_type_error`: the extent was replaced by
//! an "error" key, the data is gone). Overwriting such a range in place fails as well:
//! rqbit writes 16 KiB blocks at file offsets that are generally not page aligned (files
//! start at arbitrary offsets inside a torrent), so the kernel's buffered write path has
//! to read the rest of the page/folio first, that read hits the errored extent and
//! `pwritev()` itself returns EIO. Soft recovery then re-downloads the same piece forever.
//!
//! Repair strategy, per file:
//! 1. Scan the file (O_DIRECT when possible, so the page cache is not involved) in 4 MiB
//!    block-aligned windows, re-reading failing windows in 64 KiB sub-chunks, and record
//!    the ranges that return EIO.
//! 2. Punch holes (`FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE`) over those ranges, widened
//!    to filesystem block boundaries. That drops the broken extents: the range reads back
//!    as zeros and later partial-page writes no longer have to read broken data. Only data
//!    that is already unreadable (plus block-alignment slack) is touched.
//! 3. Re-scan the punched ranges. If punching is unsupported or a range still fails, fall
//!    back to copy-and-replace: salvage-copy everything readable into a temp file in the
//!    same directory (unreadable ranges stay zero), fsync it, atomically rename it over the
//!    original and fsync the directory. The original is never removed before the
//!    replacement is complete; on any failure the temp file is removed and the original
//!    is left as it was.
//! 4. The caller marks every piece overlapping a zeroed range as missing, so only those
//!    pieces are downloaded again.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context;
use librqbit_core::lengths::ValidPieceIndex;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    file_ops::FileOps,
    session::Session,
    torrent_state::{ManagedTorrentHandle, ManagedTorrentState, TorrentMetadata},
};

/// Size of the windows the scanner / copier reads at once.
pub const SCAN_WINDOW: u64 = 4 * 1024 * 1024;
/// Granularity used to isolate unreadable data inside a failing window.
pub const SUB_CHUNK: u64 = 64 * 1024;
/// A file becomes "damaged" after this many failures of the same piece, even without EIO.
pub const REPEAT_FAILURES_THRESHOLD: u32 = 3;
/// Don't auto-repair the same torrent more often than this.
pub const AUTO_REPAIR_MIN_INTERVAL: Duration = Duration::from_secs(30 * 60);

const DIRECT_ALIGN: usize = 4096;

// ---------------------------------------------------------------------------------------
// Error classification
// ---------------------------------------------------------------------------------------

pub fn io_is_eio(e: &io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::EIO)
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// Whether an error chain contains an EIO coming from the OS.
pub fn anyhow_is_eio(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        if let Some(io) = c.downcast_ref::<io::Error>() {
            return io_is_eio(io);
        }
        #[cfg(unix)]
        if let Some(errno) = c.downcast_ref::<nix::errno::Errno>() {
            return *errno == nix::errno::Errno::EIO;
        }
        false
    })
}

// ---------------------------------------------------------------------------------------
// Scanning / salvage primitives (unit tested with a fake reader)
// ---------------------------------------------------------------------------------------

/// Positional reader over the damaged file. Abstracted so tests can inject EIO.
pub trait RangeReader {
    /// Read up to `buf.len()` bytes at `offset`. May return fewer bytes only at EOF.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;
}

/// A heap buffer aligned for O_DIRECT.
struct AlignedBuf {
    raw: Vec<u8>,
    off: usize,
    len: usize,
}

impl AlignedBuf {
    fn new(len: usize) -> Self {
        let len = len.div_ceil(DIRECT_ALIGN) * DIRECT_ALIGN;
        let raw = vec![0u8; len + DIRECT_ALIGN];
        let off = raw.as_ptr().align_offset(DIRECT_ALIGN);
        Self { raw, off, len }
    }
    fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.raw[self.off..self.off + self.len]
    }
}

fn read_full<R: RangeReader + ?Sized>(
    r: &mut R,
    offset: u64,
    buf: &mut [u8],
    want: u64,
) -> io::Result<()> {
    let mut got = 0usize;
    while (got as u64) < want {
        match r.read_at(offset + got as u64, &mut buf[got..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "file ended early (did it shrink while repairing?)",
                ));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn round_up(n: u64) -> usize {
    (n as usize).div_ceil(DIRECT_ALIGN) * DIRECT_ALIGN
}

fn push_range(v: &mut Vec<Range<u64>>, r: Range<u64>) {
    if r.start >= r.end {
        return;
    }
    if let Some(last) = v.last_mut() {
        if last.end >= r.start {
            last.end = last.end.max(r.end);
            return;
        }
    }
    v.push(r);
}

/// Walk `range` of a file in `window`-sized reads. When a window fails with EIO, re-read it
/// in `sub`-sized pieces. `on_data` is called for every readable piece of data (offset, bytes);
/// the returned ranges are the ones that could not be read. Non-EIO errors abort.
fn walk<R: RangeReader + ?Sized>(
    r: &mut R,
    range: Range<u64>,
    window: u64,
    sub: u64,
    on_data: &mut dyn FnMut(u64, &[u8]) -> io::Result<()>,
    progress: &mut dyn FnMut(u64),
) -> io::Result<Vec<Range<u64>>> {
    assert!(window >= sub && sub > 0);
    let mut bad = Vec::new();
    let mut buf = AlignedBuf::new(window as usize);
    let mut off = range.start;
    while off < range.end {
        // Keep windows aligned to `window` in file coordinates (needed for O_DIRECT and
        // so sub-chunks line up with filesystem blocks).
        let window_end = ((off / window) + 1) * window;
        let want = window_end.min(range.end) - off;
        let b = buf.as_mut_slice();
        // Never ask for more than we need (rounded up for O_DIRECT): reading past the
        // requested range could hit unrelated unreadable data.
        let blen = round_up(want);
        match read_full(r, off, &mut b[..blen], want) {
            Ok(()) => on_data(off, &b[..want as usize])?,
            Err(e) if io_is_eio(&e) => {
                let end = off + want;
                let mut s = off;
                while s < end {
                    let sub_end = ((s / sub) + 1) * sub;
                    let swant = sub_end.min(end) - s;
                    let b = buf.as_mut_slice();
                    let blen = round_up(swant);
                    match read_full(r, s, &mut b[..blen], swant) {
                        Ok(()) => on_data(s, &b[..swant as usize])?,
                        Err(e) if io_is_eio(&e) => push_range(&mut bad, s..s + swant),
                        Err(e) => return Err(e),
                    }
                    s += swant;
                }
            }
            Err(e) => return Err(e),
        }
        off += want;
        progress(off - range.start);
    }
    Ok(bad)
}

/// Find the ranges of `range` that return EIO.
pub fn scan_unreadable<R: RangeReader + ?Sized>(
    r: &mut R,
    range: Range<u64>,
    window: u64,
    sub: u64,
    progress: &mut dyn FnMut(u64),
) -> io::Result<Vec<Range<u64>>> {
    walk(r, range, window, sub, &mut |_, _| Ok(()), progress)
}

/// Widen ranges outward to `block` boundaries and merge overlaps.
pub fn align_ranges(ranges: &[Range<u64>], block: u64) -> Vec<Range<u64>> {
    let mut sorted = ranges.to_vec();
    sorted.sort_by_key(|r| r.start);
    let mut out: Vec<Range<u64>> = Vec::new();
    for r in &sorted {
        let s = r.start / block * block;
        let e = r.end.div_ceil(block) * block;
        push_range(&mut out, s..e);
    }
    out
}

/// Clip ranges to `[0, len)`.
pub fn clip_ranges(ranges: &[Range<u64>], len: u64) -> Vec<Range<u64>> {
    ranges
        .iter()
        .filter_map(|r| {
            let e = r.end.min(len);
            (r.start < e).then_some(r.start..e)
        })
        .collect()
}

/// Pieces (torrent-wide indices) overlapping any of `ranges` (file offsets) of a file that
/// starts at `file_offset` in the torrent and is `file_len` bytes long.
pub fn pieces_overlapping(
    file_offset: u64,
    file_len: u64,
    piece_len: u64,
    ranges: &[Range<u64>],
) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for r in clip_ranges(ranges, file_len) {
        let first = (file_offset + r.start) / piece_len;
        let last = (file_offset + r.end - 1) / piece_len;
        for p in first..=last {
            out.insert(p as u32);
        }
    }
    out
}

pub fn temp_path_for(target: &Path) -> anyhow::Result<PathBuf> {
    let name = target
        .file_name()
        .context("target has no file name")?
        .to_string_lossy();
    let parent = target.parent().context("target has no parent directory")?;
    Ok(parent.join(format!(".{name}.rqbit-repair")))
}

#[derive(Clone, Debug)]
pub struct SalvageOptions {
    pub window: u64,
    pub sub_chunk: u64,
    /// Require this much free space on top of the file size before copying.
    pub free_space_margin: u64,
    /// Tests only: fail with ENOSPC once this many bytes were written.
    #[cfg(test)]
    pub fail_write_after: Option<u64>,
}

impl Default for SalvageOptions {
    fn default() -> Self {
        Self {
            window: SCAN_WINDOW,
            sub_chunk: SUB_CHUNK,
            free_space_margin: 512 * 1024 * 1024,
            #[cfg(test)]
            fail_write_after: None,
        }
    }
}

/// Copy everything readable from `src` (`len` bytes) into a temp file next to `target`,
/// leaving unreadable ranges as zeros (holes), fsync it, then atomically rename it over
/// `target` and fsync the directory. Returns the zeroed ranges.
///
/// On any error before the rename, the temp file is removed and `target` is untouched.
#[cfg(unix)]
pub fn salvage_copy_replace<R: RangeReader + ?Sized>(
    src: &mut R,
    len: u64,
    target: &Path,
    opts: &SalvageOptions,
    progress: &mut dyn FnMut(u64),
) -> anyhow::Result<Vec<Range<u64>>> {
    use std::os::unix::fs::FileExt;

    let tmp = temp_path_for(target)?;
    let dir = target.parent().context("no parent")?;

    let free = free_bytes(dir).with_context(|| format!("statvfs {dir:?}"))?;
    if free < len.saturating_add(opts.free_space_margin) {
        anyhow::bail!(
            "not enough free space for copy-and-replace of {target:?}: need {} + {} margin, have {}",
            len,
            opts.free_space_margin,
            free
        );
    }

    // A leftover temp from an interrupted earlier repair of this very file.
    if tmp.exists() {
        std::fs::remove_file(&tmp).with_context(|| format!("error removing stale {tmp:?}"))?;
    }

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .with_context(|| format!("error creating {tmp:?}"))?;

    let result = (|| -> anyhow::Result<Vec<Range<u64>>> {
        // Unreadable ranges are simply never written: they stay holes and read as zeros.
        file.set_len(len)?;
        let mut written = 0u64;
        #[cfg(test)]
        let fail_after = opts.fail_write_after;
        let bad = walk(
            src,
            0..len,
            opts.window,
            opts.sub_chunk,
            &mut |off, data| {
                #[cfg(test)]
                if let Some(limit) = fail_after {
                    if written + data.len() as u64 > limit {
                        return Err(io::Error::from_raw_os_error(libc::ENOSPC));
                    }
                }
                if data.iter().all(|b| *b == 0) {
                    // Keep holes sparse.
                    return Ok(());
                }
                file.write_all_at(data, off)?;
                written += data.len() as u64;
                Ok(())
            },
            progress,
        )?;
        file.sync_all().context("fsync temp file")?;
        Ok(bad)
    })();

    let bad = match result {
        Ok(bad) => bad,
        Err(e) => {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(e.context(format!("salvage copy of {target:?} failed; original left untouched")));
        }
    };
    drop(file);

    if let Ok(meta) = std::fs::metadata(target) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::from(e).context(format!("error renaming {tmp:?} -> {target:?}")));
    }
    std::fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .with_context(|| format!("fsync directory {dir:?}"))?;
    Ok(bad)
}

#[cfg(unix)]
fn free_bytes(dir: &Path) -> io::Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok(s.f_bavail as u64 * s.f_frsize as u64)
}

#[cfg(target_os = "linux")]
pub fn punch_hole(file: &std::fs::File, r: &Range<u64>) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let rc = unsafe {
        libc::fallocate(
            file.as_raw_fd(),
            libc::FALLOC_FL_PUNCH_HOLE | libc::FALLOC_FL_KEEP_SIZE,
            r.start as libc::off_t,
            (r.end - r.start) as libc::off_t,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
pub fn punch_hole(_file: &std::fs::File, _r: &Range<u64>) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "hole punching is only implemented on Linux",
    ))
}

/// Reads a real file, with O_DIRECT when the filesystem allows it.
#[cfg(unix)]
pub struct FileRangeReader {
    path: PathBuf,
    file: std::fs::File,
    direct: bool,
}

#[cfg(unix)]
impl FileRangeReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(file) = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECT)
                .open(path)
            {
                return Ok(Self {
                    path: path.to_owned(),
                    file,
                    direct: true,
                });
            }
        }
        Ok(Self {
            path: path.to_owned(),
            file: std::fs::File::open(path)?,
            direct: false,
        })
    }
}

#[cfg(unix)]
impl RangeReader for FileRangeReader {
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        match self.file.read_at(buf, offset) {
            Err(e) if self.direct && e.raw_os_error() == Some(libc::EINVAL) => {
                // O_DIRECT not usable here after all (alignment/fs); fall back to buffered.
                self.file = std::fs::File::open(&self.path)?;
                self.direct = false;
                self.file.read_at(buf, offset)
            }
            r => r,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Per-file repair
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RepairMethod {
    /// Nothing unreadable was found.
    #[default]
    None,
    PunchHole,
    CopyReplace,
    Failed,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct FileRepairOutcome {
    pub file_id: usize,
    pub path: String,
    pub bytes_total: u64,
    pub bytes_unreadable: u64,
    pub bytes_zeroed: u64,
    pub ranges_zeroed: Vec<[u64; 2]>,
    pub pieces: Vec<u32>,
    pub method: RepairMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct FileToRepair {
    pub file_id: usize,
    pub full_path: PathBuf,
    pub relative_path: PathBuf,
    pub offset_in_torrent: u64,
    pub len: u64,
}

/// Callback that runs a closure while the torrent's own handle to the file is closed, and
/// reopens it afterwards.
pub type WithFileClosed<'a> =
    dyn FnMut(&mut dyn FnMut() -> anyhow::Result<()>) -> anyhow::Result<()> + 'a;

#[cfg(unix)]
fn fs_block_size(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let b = std::fs::metadata(path).map(|m| m.blksize()).unwrap_or(4096);
    if b.is_power_of_two() && (4096..=SUB_CHUNK).contains(&b) {
        b
    } else {
        4096
    }
}

#[cfg(unix)]
pub fn repair_file(
    f: &FileToRepair,
    piece_len: u64,
    with_file_closed: &mut WithFileClosed<'_>,
    progress: &mut dyn FnMut(u64),
) -> FileRepairOutcome {
    let mut out = FileRepairOutcome {
        file_id: f.file_id,
        path: f.relative_path.to_string_lossy().into_owned(),
        bytes_total: f.len,
        ..Default::default()
    };
    let disk_len = match std::fs::metadata(&f.full_path) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            out.note = Some("file does not exist yet".into());
            return out;
        }
        Err(e) => {
            out.method = RepairMethod::Failed;
            out.error = Some(format!("stat failed: {e}"));
            return out;
        }
    };
    let scan_len = disk_len.min(f.len);

    // 1. Scan.
    let bad = match FileRangeReader::open(&f.full_path).and_then(|mut r| {
        scan_unreadable(&mut r, 0..scan_len, SCAN_WINDOW, SUB_CHUNK, progress)
    }) {
        Ok(b) => b,
        Err(e) => {
            out.method = RepairMethod::Failed;
            out.error = Some(format!("scan failed: {e}"));
            return out;
        }
    };
    out.bytes_unreadable = bad.iter().map(|r| r.end - r.start).sum();
    if bad.is_empty() {
        return out;
    }

    // 2. Punch holes over the unreadable ranges (block aligned).
    let block = fs_block_size(&f.full_path);
    let aligned = align_ranges(&bad, block);
    let punch_result: anyhow::Result<()> = (|| {
        let wf = std::fs::OpenOptions::new()
            .write(true)
            .open(&f.full_path)
            .context("open for punching")?;
        for r in &aligned {
            punch_hole(&wf, r).with_context(|| format!("fallocate(PUNCH_HOLE) {r:?}"))?;
        }
        wf.sync_all().context("fsync after punching")?;
        // 3. Verify.
        let mut rdr = FileRangeReader::open(&f.full_path)?;
        let mut still_bad = Vec::new();
        for r in clip_ranges(&aligned, scan_len) {
            still_bad.extend(scan_unreadable(
                &mut rdr,
                r,
                SCAN_WINDOW,
                SUB_CHUNK,
                &mut |_| {},
            )?);
        }
        if !still_bad.is_empty() {
            let n: u64 = still_bad.iter().map(|r| r.end - r.start).sum();
            anyhow::bail!("{n} bytes still unreadable after punching holes");
        }
        Ok(())
    })();

    let zeroed = match punch_result {
        Ok(()) => {
            out.method = RepairMethod::PunchHole;
            clip_ranges(&aligned, f.len)
        }
        Err(punch_err) => {
            warn!(path=?f.full_path, error=format!("{punch_err:#}"), "punch-hole repair did not work; falling back to copy-and-replace");
            let mut copied: Option<Vec<Range<u64>>> = None;
            let r = with_file_closed(&mut || {
                let mut rdr = FileRangeReader::open(&f.full_path)?;
                copied = Some(salvage_copy_replace(
                    &mut rdr,
                    scan_len,
                    &f.full_path,
                    &SalvageOptions::default(),
                    &mut |_| {},
                )?);
                Ok(())
            });
            match (r, copied) {
                (Ok(()), Some(bad)) => {
                    out.method = RepairMethod::CopyReplace;
                    out.note = Some(format!("punch-hole failed: {punch_err:#}"));
                    bad
                }
                (r, _) => {
                    out.method = RepairMethod::Failed;
                    out.error = Some(format!(
                        "punch-hole failed: {punch_err:#}; copy-and-replace failed: {:#}",
                        r.err().unwrap_or_else(|| anyhow::anyhow!("unknown"))
                    ));
                    return out;
                }
            }
        }
    };

    out.bytes_zeroed = zeroed.iter().map(|r| r.end - r.start).sum();
    out.ranges_zeroed = zeroed.iter().map(|r| [r.start, r.end]).collect();
    out.pieces = pieces_overlapping(f.offset_in_torrent, f.len, piece_len, &zeroed)
        .into_iter()
        .collect();
    out
}

#[cfg(not(unix))]
pub fn repair_file(
    f: &FileToRepair,
    _piece_len: u64,
    _with_file_closed: &mut WithFileClosed<'_>,
    _progress: &mut dyn FnMut(u64),
) -> FileRepairOutcome {
    FileRepairOutcome {
        file_id: f.file_id,
        path: f.relative_path.to_string_lossy().into_owned(),
        bytes_total: f.len,
        method: RepairMethod::Failed,
        error: Some("repair is not supported on this platform".into()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------------------
// Damage tracking (per torrent, in memory)
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DamagedFileStats {
    pub file_id: usize,
    pub path: String,
    pub errors: u64,
    /// At least one failure was an OS-level EIO.
    pub eio: bool,
    pub last_error: String,
    pub first_seen: String,
    pub last_seen: String,
    pub pieces_failed: usize,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairState {
    Running,
    Done,
    Failed,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RepairSummary {
    pub files_scanned: usize,
    pub files_repaired: usize,
    pub files_failed: usize,
    pub bytes_unreadable: u64,
    pub bytes_zeroed: u64,
    /// Pieces now marked missing (they'll be downloaded again).
    pub pieces_to_redownload: usize,
    /// Of those, pieces that had previously been verified.
    pub pieces_invalidated: usize,
    /// Only files where something was found or went wrong.
    pub files: Vec<FileRepairOutcome>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RepairStatus {
    pub state: RepairState,
    pub auto: bool,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub scanned_bytes: u64,
    pub total_bytes: u64,
    pub files_total: usize,
    pub files_done: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RepairSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DamageStats {
    pub damaged_files: Vec<DamagedFileStats>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair: Option<RepairStatus>,
}

struct DamagedFileState {
    errors: u64,
    eio: bool,
    last_error: String,
    first_seen: String,
    last_seen: String,
    pieces: BTreeSet<u32>,
}

#[derive(Default)]
struct DamageInner {
    files: BTreeMap<usize, DamagedFileState>,
    piece_failures: HashMap<u32, u32>,
    repair: Option<RepairStatus>,
    last_auto_repair: Option<Instant>,
}

#[derive(Default)]
pub struct DamageTracker {
    inner: Mutex<DamageInner>,
}

impl DamageTracker {
    /// Record an I/O failure while handling `piece` (on `file_id` when known). Returns true
    /// when the file just became "damaged" (EIO, or the same piece failed repeatedly).
    pub fn record_failure(
        &self,
        file_id: Option<usize>,
        piece: u32,
        eio: bool,
        error: &str,
    ) -> bool {
        let mut g = self.inner.lock();
        let n = {
            let c = g.piece_failures.entry(piece).or_default();
            *c = c.saturating_add(1);
            *c
        };
        let Some(file_id) = file_id else {
            return false;
        };
        if !(eio || n >= REPEAT_FAILURES_THRESHOLD) {
            return false;
        }
        let now = crate::adopt::rfc3339_now();
        let newly = !g.files.contains_key(&file_id);
        let e = g.files.entry(file_id).or_insert_with(|| DamagedFileState {
            errors: 0,
            eio: false,
            last_error: String::new(),
            first_seen: now.clone(),
            last_seen: now.clone(),
            pieces: BTreeSet::new(),
        });
        e.errors += 1;
        e.eio |= eio;
        e.last_error = error.chars().take(400).collect();
        e.last_seen = now;
        if e.pieces.len() < 10_000 {
            e.pieces.insert(piece);
        }
        newly
    }

    pub fn damaged_file_ids(&self) -> Vec<usize> {
        self.inner.lock().files.keys().copied().collect()
    }

    pub fn has_damage(&self) -> bool {
        !self.inner.lock().files.is_empty()
    }

    fn clear_after_repair(&self, repaired: &[usize]) {
        let mut g = self.inner.lock();
        for id in repaired {
            g.files.remove(id);
        }
        g.piece_failures.clear();
    }

    pub fn is_repair_running(&self) -> bool {
        matches!(
            self.inner.lock().repair.as_ref().map(|r| r.state),
            Some(RepairState::Running)
        )
    }

    fn try_begin(&self, total_bytes: u64, files_total: usize, auto: bool) -> anyhow::Result<()> {
        let mut g = self.inner.lock();
        if matches!(g.repair.as_ref().map(|r| r.state), Some(RepairState::Running)) {
            anyhow::bail!("a repair is already running for this torrent");
        }
        g.repair = Some(RepairStatus {
            state: RepairState::Running,
            auto,
            started_at: crate::adopt::rfc3339_now(),
            finished_at: None,
            scanned_bytes: 0,
            total_bytes,
            files_total,
            files_done: 0,
            current_file: None,
            summary: None,
            error: None,
        });
        Ok(())
    }

    fn set_progress(&self, scanned: u64, files_done: usize, current_file: Option<String>) {
        let mut g = self.inner.lock();
        if let Some(r) = g.repair.as_mut() {
            r.scanned_bytes = scanned;
            r.files_done = files_done;
            if current_file.is_some() {
                r.current_file = current_file;
            }
        }
    }

    fn finish(&self, result: Result<RepairSummary, String>) {
        let mut g = self.inner.lock();
        if let Some(r) = g.repair.as_mut() {
            r.finished_at = Some(crate::adopt::rfc3339_now());
            r.current_file = None;
            match result {
                Ok(s) => {
                    r.state = if s.files_failed > 0 {
                        RepairState::Failed
                    } else {
                        RepairState::Done
                    };
                    r.files_done = r.files_total;
                    r.scanned_bytes = r.total_bytes;
                    if s.files_failed > 0 {
                        r.error = Some(format!("{} file(s) could not be repaired", s.files_failed));
                    }
                    r.summary = Some(s);
                }
                Err(e) => {
                    r.state = RepairState::Failed;
                    r.error = Some(e);
                }
            }
        }
    }

    /// Rate limit for automatic repairs. Returns true (and records the attempt) if an
    /// auto repair may start now.
    fn claim_auto_repair(&self) -> bool {
        let mut g = self.inner.lock();
        if matches!(g.repair.as_ref().map(|r| r.state), Some(RepairState::Running)) {
            return false;
        }
        if let Some(t) = g.last_auto_repair {
            if t.elapsed() < AUTO_REPAIR_MIN_INTERVAL {
                return false;
            }
        }
        g.last_auto_repair = Some(Instant::now());
        true
    }

    pub fn snapshot(&self, file_name: impl Fn(usize) -> String) -> Option<DamageStats> {
        let g = self.inner.lock();
        if g.files.is_empty() && g.repair.is_none() {
            return None;
        }
        Some(DamageStats {
            damaged_files: g
                .files
                .iter()
                .map(|(id, s)| DamagedFileStats {
                    file_id: *id,
                    path: file_name(*id),
                    errors: s.errors,
                    eio: s.eio,
                    last_error: s.last_error.clone(),
                    first_seen: s.first_seen.clone(),
                    last_seen: s.last_seen.clone(),
                    pieces_failed: s.pieces.len(),
                })
                .collect(),
            repair: g.repair.clone(),
        })
    }
}

// ---------------------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepairScope {
    /// Only files currently marked damaged.
    Damaged,
    /// Scan every file of the torrent (repairs whatever is unreadable).
    #[default]
    All,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RepairRequest {
    /// Explicit file indices; overrides `scope`.
    #[serde(default)]
    pub files: Option<Vec<usize>>,
    #[serde(default)]
    pub scope: RepairScope,
    #[serde(skip)]
    pub auto: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RepairStartResponse {
    pub started: bool,
    pub files: usize,
    pub total_bytes: u64,
}

#[derive(Serialize)]
struct RepairLogRecord<'a> {
    time: String,
    torrent_id: usize,
    info_hash: String,
    file_id: usize,
    path: String,
    bytes_total: u64,
    bytes_unreadable: u64,
    bytes_zeroed: u64,
    ranges_zeroed: &'a [[u64; 2]],
    pieces_affected: &'a [u32],
    method: RepairMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

fn append_log(path: &Path, line: &impl Serialize) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {path:?}"))?;
    let mut s = serde_json::to_string(line)?;
    s.push('\n');
    f.write_all(s.as_bytes())?;
    f.sync_data()?;
    Ok(())
}

impl Session {
    /// Start repairing a torrent's damaged files in the background. Progress and the result
    /// are exposed in the torrent's stats (`damage.repair`).
    pub fn start_repair_files(
        self: &Arc<Self>,
        handle: &ManagedTorrentHandle,
        req: RepairRequest,
    ) -> anyhow::Result<RepairStartResponse> {
        let metadata = handle
            .metadata
            .load_full()
            .context("torrent metadata is not resolved yet")?;
        handle.with_state(|s| match s {
            ManagedTorrentState::Initializing(_) => {
                anyhow::bail!("torrent is checking its files; retry when the check finishes")
            }
            ManagedTorrentState::None => anyhow::bail!("bug: torrent is in empty state"),
            _ => Ok(()),
        })?;

        let damaged = handle.shared.damage.damaged_file_ids();
        let mut files: Vec<usize> = match (&req.files, req.scope) {
            (Some(f), _) => f.clone(),
            (None, RepairScope::Damaged) => damaged.clone(),
            (None, RepairScope::All) => (0..metadata.file_infos.len()).collect(),
        };
        files.retain(|id| {
            metadata
                .file_infos
                .get(*id)
                .map(|fi| !fi.attrs.padding)
                .unwrap_or(false)
        });
        files.sort_unstable();
        files.dedup();
        // Known-damaged files first.
        files.sort_by_key(|id| !damaged.contains(id));
        if files.is_empty() {
            anyhow::bail!("no files to repair");
        }
        let total_bytes: u64 = files.iter().map(|id| metadata.file_infos[*id].len).sum();
        handle
            .shared
            .damage
            .try_begin(total_bytes, files.len(), req.auto)?;

        info!(
            id = handle.id(),
            info_hash = ?handle.info_hash(),
            files = files.len(),
            total_bytes,
            auto = req.auto,
            "starting repair of damaged files"
        );

        let session = self.clone();
        let h = handle.clone();
        let nfiles = files.len();
        librqbit_core::spawn_utils::spawn(
            tracing::debug_span!(parent: handle.shared.span.clone(), "repair_files"),
            "repair_files",
            async move {
                let r = session.run_repair(&h, metadata, files).await;
                match r {
                    Ok(s) => {
                        info!(
                            id = h.id(),
                            files_repaired = s.files_repaired,
                            files_failed = s.files_failed,
                            bytes_zeroed = s.bytes_zeroed,
                            pieces_to_redownload = s.pieces_to_redownload,
                            "repair finished"
                        );
                        h.shared.damage.finish(Ok(s));
                    }
                    Err(e) => {
                        warn!(id = h.id(), "repair failed: {e:#}");
                        h.shared.damage.finish(Err(format!("{e:#}")));
                    }
                }
                Ok::<_, anyhow::Error>(())
            },
        );
        Ok(RepairStartResponse {
            started: true,
            files: nfiles,
            total_bytes,
        })
    }

    /// Kick off an automatic repair (damaged files only) if allowed by prefs / rate limit.
    pub(crate) fn maybe_auto_repair(self: &Arc<Self>, handle: &ManagedTorrentHandle) {
        if !(self.preferences.soft_recover_on_io_error()
            && self.preferences.auto_repair_damaged_files())
        {
            return;
        }
        if !handle.shared.damage.claim_auto_repair() {
            return;
        }
        match self.start_repair_files(
            handle,
            RepairRequest {
                files: None,
                scope: RepairScope::Damaged,
                auto: true,
            },
        ) {
            Ok(_) => {}
            Err(e) => warn!(id = handle.id(), "could not start automatic repair: {e:#}"),
        }
    }

    async fn run_repair(
        self: &Arc<Self>,
        handle: &ManagedTorrentHandle,
        metadata: Arc<TorrentMetadata>,
        files: Vec<usize>,
    ) -> anyhow::Result<RepairSummary> {
        let was_live = handle.live().is_some();
        let was_error = handle.with_state(|s| matches!(s, ManagedTorrentState::Error(_)));
        if was_live {
            self.pause(handle)
                .await
                .context("error pausing torrent before repair")?;
        }
        let result = self.run_repair_stopped(handle, metadata, files).await;
        if was_live || was_error {
            if let Err(e) = self.unpause(handle).await {
                warn!(id = handle.id(), "error resuming torrent after repair: {e:#}");
                if result.is_ok() {
                    return Err(e.context("repair finished but resuming the torrent failed"));
                }
            }
        }
        result
    }

    async fn run_repair_stopped(
        self: &Arc<Self>,
        handle: &ManagedTorrentHandle,
        metadata: Arc<TorrentMetadata>,
        files: Vec<usize>,
    ) -> anyhow::Result<RepairSummary> {
        let log_path = self
            .preferences_path()
            .parent()
            .map(|d| d.join("repair-log.jsonl"));
        let h = handle.clone();
        let md = metadata.clone();
        let outcomes = tokio::task::spawn_blocking(move || {
            let shared = h.shared.clone();
            let piece_len = md.lengths().default_piece_length() as u64;
            let mut outcomes = Vec::new();
            let mut scanned_before = 0u64;
            for (i, file_id) in files.iter().copied().enumerate() {
                let fi = &md.file_infos[file_id];
                let rel = shared
                    .file_rename(file_id)
                    .unwrap_or_else(|| fi.relative_filename.clone());
                let full = shared.output_folder().join(&rel);
                shared.damage.set_progress(
                    scanned_before,
                    i,
                    Some(rel.to_string_lossy().into_owned()),
                );
                let target = FileToRepair {
                    file_id,
                    full_path: full.clone(),
                    relative_path: rel,
                    offset_in_torrent: fi.offset_in_torrent,
                    len: fi.len,
                };
                let hh = h.clone();
                let mut with_closed =
                    |f: &mut dyn FnMut() -> anyhow::Result<()>| hh.with_file_closed(file_id, f);
                let dmg = &shared.damage;
                let o = repair_file(&target, piece_len, &mut with_closed, &mut |n| {
                    dmg.set_progress(scanned_before + n, i, None)
                });
                scanned_before += fi.len;

                if o.method != RepairMethod::None || o.error.is_some() {
                    match o.method {
                        RepairMethod::Failed => warn!(
                            id = shared.id, file_id, path = ?full,
                            error = o.error.as_deref().unwrap_or(""),
                            "repair of damaged file failed"
                        ),
                        m => info!(
                            id = shared.id, file_id, path = ?full, method = ?m,
                            bytes_unreadable = o.bytes_unreadable,
                            bytes_zeroed = o.bytes_zeroed,
                            pieces = o.pieces.len(),
                            "repaired damaged file"
                        ),
                    }
                    if let Some(lp) = log_path.as_deref() {
                        let rec = RepairLogRecord {
                            time: crate::adopt::rfc3339_now(),
                            torrent_id: shared.id,
                            info_hash: shared.info_hash.as_string(),
                            file_id,
                            path: full.to_string_lossy().into_owned(),
                            bytes_total: o.bytes_total,
                            bytes_unreadable: o.bytes_unreadable,
                            bytes_zeroed: o.bytes_zeroed,
                            ranges_zeroed: &o.ranges_zeroed,
                            pieces_affected: &o.pieces,
                            method: o.method,
                            note: o.note.as_deref(),
                            error: o.error.as_deref(),
                        };
                        if let Err(e) = append_log(lp, &rec) {
                            warn!(path=?lp, "error writing repair log: {e:#}");
                        }
                    }
                }
                outcomes.push(o);
            }
            outcomes
        })
        .await
        .context("repair task panicked")?;

        // Pieces overlapping zeroed ranges must be downloaded again. Also re-verify the
        // pieces we believe we have in repaired files, in case anything else was off.
        let repaired_files: Vec<usize> = outcomes
            .iter()
            .filter(|o| matches!(o.method, RepairMethod::PunchHole | RepairMethod::CopyReplace))
            .map(|o| o.file_id)
            .collect();
        let mut pieces: BTreeSet<u32> = outcomes.iter().flat_map(|o| o.pieces.clone()).collect();
        let h = handle.clone();
        let md = metadata.clone();
        let (to_redownload, invalidated) = tokio::task::spawn_blocking(move || {
            h.mark_pieces_missing_after_repair(&md, &repaired_files, &mut pieces)
        })
        .await
        .context("repair task panicked")??;

        let ok_files: Vec<usize> = outcomes
            .iter()
            .filter(|o| o.method != RepairMethod::Failed)
            .map(|o| o.file_id)
            .collect();
        handle.shared.damage.clear_after_repair(&ok_files);

        let summary = RepairSummary {
            files_scanned: outcomes.len(),
            files_repaired: outcomes
                .iter()
                .filter(|o| matches!(o.method, RepairMethod::PunchHole | RepairMethod::CopyReplace))
                .count(),
            files_failed: outcomes
                .iter()
                .filter(|o| o.method == RepairMethod::Failed)
                .count(),
            bytes_unreadable: outcomes.iter().map(|o| o.bytes_unreadable).sum(),
            bytes_zeroed: outcomes.iter().map(|o| o.bytes_zeroed).sum(),
            pieces_to_redownload: to_redownload,
            pieces_invalidated: invalidated,
            files: outcomes
                .into_iter()
                .filter(|o| o.method != RepairMethod::None || o.error.is_some())
                .collect(),
        };
        Ok(summary)
    }
}

impl crate::torrent_state::ManagedTorrent {
    /// Run `f` while the torrent's handle to `file_id` is closed; reopen afterwards.
    pub(crate) fn with_file_closed(
        &self,
        file_id: usize,
        f: &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let metadata = self.metadata.load();
        let metadata = metadata.as_ref().context("torrent is not resolved")?;
        let g = self.locked.read();
        match &g.state {
            ManagedTorrentState::Paused(p) => {
                p.files.replace_file(&self.shared, metadata, file_id, f)
            }
            // No open handles in error state.
            ManagedTorrentState::Error(_) => f(),
            _ => anyhow::bail!("torrent must be paused to replace a file"),
        }
    }

    /// After a repair: re-verify pieces we have in `repaired_files` (adding failures to
    /// `pieces`), then mark all `pieces` missing and persist the bitfield.
    /// Returns (pieces now missing, pieces that were previously verified).
    pub(crate) fn mark_pieces_missing_after_repair(
        &self,
        metadata: &TorrentMetadata,
        repaired_files: &[usize],
        pieces: &mut BTreeSet<u32>,
    ) -> anyhow::Result<(usize, usize)> {
        let lengths = metadata.lengths();
        {
            let g = self.locked.read();
            match &g.state {
                ManagedTorrentState::Paused(p) => {
                    let ops = FileOps::new(&metadata.info, &*p.files, &metadata.file_infos);
                    for fid in repaired_files {
                        let fi = &metadata.file_infos[*fid];
                        for pid in fi.piece_range.clone() {
                            if pieces.contains(&pid) {
                                continue;
                            }
                            let Some(vp) = lengths.validate_piece_index(pid) else {
                                continue;
                            };
                            if !p.chunk_tracker.is_piece_have(vp) {
                                continue;
                            }
                            if !ops.check_piece(vp).unwrap_or(false) {
                                pieces.insert(pid);
                            }
                        }
                    }
                }
                // Resuming from error state does a full recheck anyway.
                ManagedTorrentState::Error(_) => return Ok((pieces.len(), 0)),
                _ => anyhow::bail!("torrent must be paused to update pieces after repair"),
            }
        }
        let mut g = self.locked.write();
        match &mut g.state {
            ManagedTorrentState::Paused(p) => {
                let valid: Vec<ValidPieceIndex> = pieces
                    .iter()
                    .filter_map(|p| lengths.validate_piece_index(*p))
                    .collect();
                let invalidated = p
                    .chunk_tracker
                    .mark_pieces_missing(&valid, &metadata.file_infos);
                p.chunk_tracker
                    .get_have_pieces_mut()
                    .flush(false)
                    .context("error flushing bitfield after repair")?;
                Ok((valid.len(), invalidated))
            }
            _ => anyhow::bail!("torrent state changed during repair"),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// In-memory source that returns EIO for reads touching any "bad" range.
    struct FakeReader {
        data: Vec<u8>,
        bad: Vec<Range<u64>>,
        reads: usize,
    }

    impl RangeReader for FakeReader {
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            let len = self.data.len() as u64;
            if offset >= len {
                return Ok(0);
            }
            let end = (offset + buf.len() as u64).min(len);
            if self.bad.iter().any(|b| b.start < end && offset < b.end) {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            let n = (end - offset) as usize;
            buf[..n].copy_from_slice(&self.data[offset as usize..end as usize]);
            Ok(n)
        }
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8 + 1).collect()
    }

    const K: u64 = 1024;

    #[test]
    fn scan_finds_eio_ranges_at_sub_chunk_granularity() {
        let len = 10 * 64 * K + 123; // not a multiple of anything
        let mut r = FakeReader {
            data: pattern(len as usize),
            // One range inside a single sub-chunk, one spanning two sub-chunks,
            // one touching EOF.
            bad: vec![70 * K..71 * K, 190 * K..260 * K, len - 10..len],
            reads: 0,
        };
        let bad = scan_unreadable(&mut r, 0..len, 256 * K, 64 * K, &mut |_| {}).unwrap();
        assert_eq!(bad, vec![64 * K..320 * K, 640 * K..len]);
    }

    #[test]
    fn scan_clean_file_reports_nothing() {
        let len = 3 * SCAN_WINDOW + 17;
        let mut r = FakeReader {
            data: pattern(len as usize),
            bad: vec![],
            reads: 0,
        };
        let mut last = 0;
        let bad = scan_unreadable(&mut r, 0..len, SCAN_WINDOW, SUB_CHUNK, &mut |p| last = p)
            .unwrap();
        assert!(bad.is_empty());
        assert_eq!(last, len);
        assert_eq!(r.reads, 4);
    }

    #[test]
    fn non_eio_errors_abort_the_scan() {
        struct Broken;
        impl RangeReader for Broken {
            fn read_at(&mut self, _: u64, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from_raw_os_error(libc::EACCES))
            }
        }
        assert!(scan_unreadable(&mut Broken, 0..100, 64, 64, &mut |_| {}).is_err());
    }

    #[test]
    fn alignment_and_piece_mapping() {
        let aligned = align_ranges(&[5000..5001, 8190..9000, 100..200], 4096);
        assert_eq!(aligned, vec![0..12288]);
        let aligned = align_ranges(&[100..200, 20000..20001], 4096);
        assert_eq!(aligned, vec![0..4096, 16384..20480]);

        // File starting at torrent offset 230 (unaligned, like the real case), 16 KiB pieces.
        let pl = 16 * K;
        let pieces = pieces_overlapping(230, 100 * K, pl, &[0..1, 16 * K - 230..16 * K - 229]);
        // byte 0 of the file is piece 0; file offset 16K-230 is torrent offset 16K -> piece 1.
        assert_eq!(pieces.into_iter().collect::<Vec<_>>(), vec![0, 1]);
        // Ranges beyond EOF are clipped.
        let pieces = pieces_overlapping(0, 20 * K, pl, &[16 * K..64 * K]);
        assert_eq!(pieces.into_iter().collect::<Vec<_>>(), vec![1]);
        assert!(pieces_overlapping(0, 20 * K, pl, &[20 * K..24 * K]).is_empty());
    }

    #[test]
    fn salvage_copy_zero_fills_and_replaces_atomically() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("movie.mkv");
        let len = 5 * 64 * K + 1000;
        let data = pattern(len as usize);
        std::fs::write(&target, b"old contents with unreadable extents").unwrap();
        let mut r = FakeReader {
            data: data.clone(),
            bad: vec![65 * K..66 * K, len - 1..len],
            reads: 0,
        };
        let opts = SalvageOptions {
            window: 128 * K,
            sub_chunk: 64 * K,
            free_space_margin: 0,
            ..Default::default()
        };
        let bad = salvage_copy_replace(&mut r, len, &target, &opts, &mut |_| {}).unwrap();
        assert_eq!(bad, vec![64 * K..128 * K, 320 * K..len]);

        let got = std::fs::read(&target).unwrap();
        assert_eq!(got.len() as u64, len);
        for (i, (g, d)) in got.iter().zip(data.iter()).enumerate() {
            let i = i as u64;
            let in_bad = bad.iter().any(|b| b.contains(&i));
            if in_bad {
                assert_eq!(*g, 0, "byte {i} should be zero-filled");
            } else {
                assert_eq!(g, d, "byte {i} should be copied");
            }
        }
        assert!(!temp_path_for(&target).unwrap().exists());
        let names: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "only the replaced file must remain");

        // Zeroed ranges map to the right pieces (file at torrent offset 0, 64K pieces).
        let pieces = pieces_overlapping(0, len, 64 * K, &bad);
        assert_eq!(pieces.into_iter().collect::<Vec<_>>(), vec![1, 5]);
    }

    #[test]
    fn salvage_copy_failure_removes_temp_and_keeps_original() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("movie.mkv");
        let original = b"original bytes must survive".to_vec();
        std::fs::write(&target, &original).unwrap();
        let len = 4 * 64 * K;
        let mut r = FakeReader {
            data: pattern(len as usize),
            bad: vec![0..10],
            reads: 0,
        };
        let opts = SalvageOptions {
            window: 64 * K,
            sub_chunk: 64 * K,
            free_space_margin: 0,
            fail_write_after: Some(100 * K),
        };
        let err = salvage_copy_replace(&mut r, len, &target, &opts, &mut |_| {}).unwrap_err();
        assert!(format!("{err:#}").contains("original left untouched"), "{err:#}");
        assert_eq!(std::fs::read(&target).unwrap(), original);
        assert!(!temp_path_for(&target).unwrap().exists());
    }

    #[test]
    fn salvage_copy_refuses_without_free_space() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("f");
        std::fs::write(&target, b"x").unwrap();
        let mut r = FakeReader {
            data: pattern(10),
            bad: vec![],
            reads: 0,
        };
        let opts = SalvageOptions {
            free_space_margin: u64::MAX / 2,
            ..Default::default()
        };
        assert!(salvage_copy_replace(&mut r, 10, &target, &opts, &mut |_| {}).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"x");
        assert!(!temp_path_for(&target).unwrap().exists());
    }

    #[test]
    fn stale_temp_is_replaced() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("f.bin");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(temp_path_for(&target).unwrap(), b"stale").unwrap();
        let mut r = FakeReader {
            data: pattern(1000),
            bad: vec![],
            reads: 0,
        };
        let opts = SalvageOptions {
            free_space_margin: 0,
            ..Default::default()
        };
        salvage_copy_replace(&mut r, 1000, &target, &opts, &mut |_| {}).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), pattern(1000));
        assert!(!temp_path_for(&target).unwrap().exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn punch_hole_zeroes_range_and_keeps_size() {
        use std::os::unix::fs::FileExt;
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("f");
        let data = pattern(256 * K as usize);
        std::fs::write(&p, &data).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        match punch_hole(&f, &(64 * K..128 * K)) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(libc::EOPNOTSUPP) => return, // e.g. tmpfs quirks
            Err(e) => panic!("{e}"),
        }
        let got = std::fs::read(&p).unwrap();
        assert_eq!(got.len(), data.len());
        assert!(got[64 * K as usize..128 * K as usize].iter().all(|b| *b == 0));
        assert_eq!(&got[..64 * K as usize], &data[..64 * K as usize]);
        assert_eq!(&got[128 * K as usize..], &data[128 * K as usize..]);
        // And a normal unaligned write into the punched range works.
        f.write_all_at(b"hello", 64 * K + 7).unwrap();
    }

    #[test]
    fn damage_tracker_marks_on_eio_or_repeats() {
        let t = DamageTracker::default();
        assert!(!t.record_failure(Some(2), 10, false, "x"));
        assert!(!t.record_failure(Some(2), 10, false, "x"));
        assert!(t.record_failure(Some(2), 10, false, "x"), "3rd failure of same piece");
        assert!(!t.record_failure(Some(2), 11, true, "eio"), "already damaged");
        assert!(t.record_failure(Some(5), 99, true, "eio"));
        assert_eq!(t.damaged_file_ids(), vec![2, 5]);
        let s = t.snapshot(|i| format!("f{i}")).unwrap();
        assert_eq!(s.damaged_files[0].errors, 2);
        assert!(s.damaged_files[0].eio);
        t.clear_after_repair(&[2]);
        assert_eq!(t.damaged_file_ids(), vec![5]);
        assert!(t.claim_auto_repair());
        assert!(!t.claim_auto_repair(), "rate limited");
    }

    #[test]
    fn eio_detection_through_anyhow_chains() {
        let e: anyhow::Error = io::Error::from_raw_os_error(libc::EIO).into();
        assert!(anyhow_is_eio(&e.context("error writing to file 2")));
        let e = anyhow::Error::from(nix::errno::Errno::EIO).context("error calling pwritev");
        assert!(anyhow_is_eio(&e));
        let e: anyhow::Error = io::Error::from_raw_os_error(libc::ENOSPC).into();
        assert!(!anyhow_is_eio(&e));
    }
}
