//! Moving a torrent's files on disk: completion "move" actions, auto-organize, the manual
//! relocate API and "Move files individually as they complete".
//!
//! Rules (all moves go through here):
//! * A multi-file torrent always lives in its own folder: `<dest>/<TorrentName>/<subpaths>`.
//!   Its files are never put loose into the destination itself.
//! * "Move the whole folder at once" (default): when the torrent's files live in their own
//!   folder (`<incomplete>/<TorrentName>`) that no other torrent uses, that folder is moved
//!   as a unit: one `rename()` on the same file system; across file systems a verified copy
//!   (fsync + re-read checksum, permissions and mtimes kept) into a temporary sibling,
//!   renamed into place, then the source files are removed.
//! * Otherwise files move one by one (same rename / verified-copy rules), keeping their
//!   relative paths.
//! * Nothing is ever overwritten: renames use `RENAME_NOREPLACE` (Linux) or an existence
//!   check, copies use `create_new`. If the destination folder exists it is merged when
//!   none of the incoming paths exist there yet; otherwise a free `<Name> (2)` (for a single
//!   file `<name> (2).<ext>`) is used.
//! * Source files are removed only after their copy was verified. Afterwards only empty
//!   directories inside the old torrent folder are removed (the folder itself only when it
//!   was the torrent's own folder).

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, bail};
use serde::Serialize;
use sha1w::{ISha1, Sha1};
use tracing::{debug, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveMethod {
    /// Nothing had to move.
    None,
    /// `rename()` on the same file system.
    Rename,
    /// Verified copy (other file system, or copy mode).
    Copy,
}

impl MoveMethod {
    fn merge(self, other: MoveMethod) -> MoveMethod {
        match (self, other) {
            (MoveMethod::Copy, _) | (_, MoveMethod::Copy) => MoveMethod::Copy,
            (MoveMethod::Rename, _) | (_, MoveMethod::Rename) => MoveMethod::Rename,
            _ => MoveMethod::None,
        }
    }
}

/// `rename(src, dst)` that never replaces an existing `dst`.
pub fn rename_no_clobber(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let s = CString::new(src.as_os_str().as_bytes())?;
        let d = CString::new(dst.as_os_str().as_bytes())?;
        // SAFETY: valid NUL-terminated paths; renameat2 has no other preconditions.
        let r = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                s.as_ptr(),
                libc::AT_FDCWD,
                d.as_ptr(),
                1u32, // RENAME_NOREPLACE
            )
        };
        if r == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            // File system / kernel without RENAME_NOREPLACE: check, then rename.
            Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP) => {}
            _ => return Err(e),
        }
    }
    if dst.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{dst:?} already exists"),
        ));
    }
    fs::rename(src, dst)
}

fn is_cross_device(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::CrossesDevices
}

const BUF: usize = 1024 * 1024;

fn hash_file(path: &Path) -> anyhow::Result<([u8; 20], u64)> {
    let mut f = fs::File::open(path).with_context(|| format!("error opening {path:?}"))?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; BUF];
    let mut n = 0u64;
    loop {
        let r = f.read(&mut buf).with_context(|| format!("error reading {path:?}"))?;
        if r == 0 {
            break;
        }
        h.update(&buf[..r]);
        n += r as u64;
    }
    Ok((h.finish(), n))
}

fn fsync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Copy one file to `dst` (must not exist), fsync it, and verify it by re-reading it.
/// Permissions and modification time are kept.
pub fn copy_file_verified(src: &Path, dst: &Path) -> anyhow::Result<u64> {
    let mut input = fs::File::open(src).with_context(|| format!("error opening {src:?}"))?;
    let meta = input.metadata()?;
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .with_context(|| format!("error creating {dst:?}"))?;
    let result = (|| -> anyhow::Result<u64> {
        let mut h = Sha1::new();
        let mut buf = vec![0u8; BUF];
        let mut n = 0u64;
        loop {
            let r = input.read(&mut buf).with_context(|| format!("error reading {src:?}"))?;
            if r == 0 {
                break;
            }
            h.update(&buf[..r]);
            out.write_all(&buf[..r])
                .with_context(|| format!("error writing {dst:?}"))?;
            n += r as u64;
        }
        out.sync_all().with_context(|| format!("error syncing {dst:?}"))?;
        if n != meta.len() {
            bail!("{src:?} changed while copying ({} bytes expected, {n} copied)", meta.len());
        }
        let want = h.finish();
        let (got, got_len) = hash_file(dst)?;
        if got != want || got_len != n {
            bail!("verification of the copy {dst:?} failed (contents differ)");
        }
        let _ = fs::set_permissions(dst, meta.permissions());
        if let Ok(m) = meta.modified() {
            let _ = out.set_times(fs::FileTimes::new().set_modified(m));
        }
        Ok(n)
    })();
    if result.is_err() {
        drop(out);
        let _ = fs::remove_file(dst);
    }
    result
}

/// Copy directory `src` to `dst` (must not exist); every file verified. Returns the
/// source files that were copied.
fn copy_tree_verified(src: &Path, dst: &Path, copied: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    fs::create_dir(dst).with_context(|| format!("error creating {dst:?}"))?;
    if let Ok(m) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, m.permissions());
    }
    for entry in fs::read_dir(src).with_context(|| format!("error listing {src:?}"))? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_tree_verified(&from, &to, copied)?;
        } else if ft.is_file() {
            copy_file_verified(&from, &to)?;
            copied.push(from);
        } else if ft.is_symlink() {
            #[cfg(unix)]
            {
                let target = fs::read_link(&from)?;
                std::os::unix::fs::symlink(&target, &to)
                    .with_context(|| format!("error copying symlink {from:?}"))?;
                copied.push(from);
            }
        } else {
            warn!(path = ?from, "not moving special file");
        }
    }
    fsync_dir(dst);
    Ok(())
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_sibling(dst: &Path) -> PathBuf {
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    dst.with_file_name(format!(".{name}.rqbit-moving-{}-{n}", std::process::id()))
}

/// Remove `dir` and its parents while they are empty, never going above `stop`
/// (`stop` itself is removed only when `include_stop`).
pub fn remove_empty_dirs(dir: &Path, stop: &Path, include_stop: bool) {
    let mut cur = dir.to_path_buf();
    loop {
        if !cur.starts_with(stop) || (cur == stop && !include_stop) {
            return;
        }
        let empty = fs::read_dir(&cur).map(|mut d| d.next().is_none()).unwrap_or(false);
        if !empty || fs::remove_dir(&cur).is_err() {
            return;
        }
        debug!(dir = ?cur, "removed empty directory");
        if cur == stop {
            return;
        }
        match cur.parent() {
            Some(p) => cur = p.to_path_buf(),
            None => return,
        }
    }
}

/// Move (or copy) a file or directory to `dst`, which must not exist. Same file system:
/// one rename. Otherwise (or `copy`): verified copy into a temporary sibling of `dst`,
/// renamed into place; then (move only) the copied source files are removed and the
/// emptied source directories with them.
pub fn move_path(src: &Path, dst: &Path, copy: bool) -> anyhow::Result<MoveMethod> {
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p).with_context(|| format!("error creating {p:?}"))?;
    }
    if dst.symlink_metadata().is_ok() {
        bail!("{dst:?} already exists; not overwriting");
    }
    if !copy {
        match rename_no_clobber(src, dst) {
            Ok(()) => {
                if let Some(p) = dst.parent() {
                    fsync_dir(p);
                }
                return Ok(MoveMethod::Rename);
            }
            Err(e) if is_cross_device(&e) => {
                debug!(?src, ?dst, "different file systems: copying");
            }
            Err(e) => {
                return Err(anyhow::Error::from(e))
                    .with_context(|| format!("error moving {src:?} -> {dst:?}"));
            }
        }
    }
    let tmp = temp_sibling(dst);
    let is_dir = fs::symlink_metadata(src)
        .with_context(|| format!("{src:?} not found"))?
        .is_dir();
    let mut copied = Vec::new();
    let r = if is_dir {
        copy_tree_verified(src, &tmp, &mut copied)
    } else {
        copy_file_verified(src, &tmp).map(|_| copied.push(src.to_path_buf()))
    };
    if let Err(e) = r {
        // Only our temporary copy is removed; the source is untouched.
        let _ = if is_dir { fs::remove_dir_all(&tmp) } else { fs::remove_file(&tmp) };
        return Err(e.context(format!("error copying {src:?} -> {dst:?}")));
    }
    if let Err(e) = rename_no_clobber(&tmp, dst) {
        let _ = if is_dir { fs::remove_dir_all(&tmp) } else { fs::remove_file(&tmp) };
        return Err(anyhow::Error::from(e))
            .with_context(|| format!("error putting the copy in place at {dst:?}"));
    }
    if let Some(p) = dst.parent() {
        fsync_dir(p);
    }
    if !copy {
        for f in &copied {
            if let Err(e) = fs::remove_file(f) {
                warn!(file = ?f, "copied and verified, but could not remove the source: {e}");
            }
        }
        if is_dir {
            remove_empty_tree(src);
        }
    }
    Ok(MoveMethod::Copy)
}

/// Remove empty directories under (and including) `dir`, bottom-up. Non-empty ones stay.
fn remove_empty_tree(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                remove_empty_tree(&e.path());
            }
        }
    }
    let _ = fs::remove_dir(dir);
}

/// Paths under `src` that already exist under `dst` (files, or dirs that aren't dirs there).
pub fn find_conflicts(src: &Path, dst: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(src: &Path, dst: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = fs::read_dir(src.join(rel)) else {
            return;
        };
        for e in rd.flatten() {
            let r = rel.join(e.file_name());
            let target = dst.join(&r);
            let Ok(tm) = target.symlink_metadata() else {
                continue;
            };
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) && tm.is_dir() {
                walk(src, dst, &r, out);
            } else {
                out.push(r);
            }
        }
    }
    walk(src, dst, Path::new(""), &mut out);
    out
}

/// Merge directory `src` into existing `dst` (no conflicts, see [`find_conflicts`]).
fn merge_dir(src: &Path, dst: &Path, copy: bool) -> anyhow::Result<MoveMethod> {
    let mut method = MoveMethod::None;
    for e in fs::read_dir(src).with_context(|| format!("error listing {src:?}"))? {
        let e = e?;
        let from = e.path();
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() && to.is_dir() {
            method = method.merge(merge_dir(&from, &to, copy)?);
        } else {
            method = method.merge(move_path(&from, &to, copy)?);
        }
    }
    if !copy {
        let _ = fs::remove_dir(src);
    }
    Ok(method)
}

/// `path` with " (n)" added: `Name (2)`, or `file (2).ext` for files.
pub fn suffixed(path: &Path, n: u32, is_file: bool) -> PathBuf {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new = match (is_file, name.rfind('.')) {
        (true, Some(dot)) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    };
    path.with_file_name(new)
}

/// First `path`, `path (2)`, `path (3)`, … that doesn't exist.
pub fn free_path(path: &Path, is_file: bool) -> PathBuf {
    if path.symlink_metadata().is_err() {
        return path.to_path_buf();
    }
    for n in 2..10_000 {
        let p = suffixed(path, n, is_file);
        if p.symlink_metadata().is_err() {
            return p;
        }
    }
    suffixed(path, std::process::id(), is_file)
}

/// One torrent file for a relocation.
#[derive(Debug, Clone)]
pub struct FileLoc {
    pub file_id: usize,
    /// Where it is now (absolute).
    pub current: PathBuf,
    /// Its path inside the torrent folder (relative): where it goes under the new folder.
    pub rel: PathBuf,
}

#[derive(Debug)]
pub struct Plan<'a> {
    pub old_output: &'a Path,
    pub new_output: &'a Path,
    pub files: &'a [FileLoc],
    /// The old folder is this torrent's own folder and nothing else uses it: move it as one.
    pub whole_folder: bool,
    /// The old folder is this torrent's own folder (may be removed once empty).
    pub own_folder: bool,
    /// A single-file torrent (a name clash renames the file instead of the folder).
    pub single_file: bool,
    pub copy: bool,
}

#[derive(Debug)]
pub struct Outcome {
    /// The torrent's folder now (may differ from the requested one: "Name (2)").
    pub output_folder: PathBuf,
    /// New absolute path of every file in the plan.
    pub paths: Vec<(usize, PathBuf)>,
    pub method: MoveMethod,
    pub whole_folder: bool,
    pub moved_files: usize,
    /// Set when the move stopped halfway: `paths` still describes where everything is.
    pub error: Option<anyhow::Error>,
}

/// Carry out a relocation. Never overwrites. On a failure halfway, the returned
/// `Outcome` says where every file is (moved ones at the new place, the rest where they
/// were, `output_folder` unchanged) and carries the error.
pub fn execute(plan: &Plan<'_>) -> Outcome {
    let old = plan.old_output;
    let unchanged = |error: Option<anyhow::Error>| Outcome {
        output_folder: old.to_path_buf(),
        paths: plan.files.iter().map(|f| (f.file_id, f.current.clone())).collect(),
        method: MoveMethod::None,
        whole_folder: false,
        moved_files: 0,
        error,
    };
    if plan.new_output == old {
        return unchanged(None);
    }
    if plan.whole_folder && plan.new_output.starts_with(old) {
        return unchanged(Some(anyhow::anyhow!(
            "cannot move {old:?} into itself ({:?})",
            plan.new_output
        )));
    }

    if plan.whole_folder && old.is_dir() {
        let target = plan.new_output;
        let (dest, merge) = if target.symlink_metadata().is_err() {
            (target.to_path_buf(), false)
        } else if target.is_dir() && find_conflicts(old, target).is_empty() {
            (target.to_path_buf(), true)
        } else {
            (free_path(target, false), false)
        };
        let r = if merge {
            merge_dir(old, &dest, plan.copy)
        } else {
            move_path(old, &dest, plan.copy)
        };
        let mut method = match r {
            Ok(m) => m,
            Err(e) => {
                // A merge can stop halfway: report where each file is now.
                let paths = plan
                    .files
                    .iter()
                    .map(|f| {
                        let moved = f
                            .current
                            .strip_prefix(old)
                            .ok()
                            .map(|r| dest.join(r))
                            .filter(|p| merge && p.exists() && !f.current.exists());
                        (f.file_id, moved.unwrap_or_else(|| f.current.clone()))
                    })
                    .collect();
                return Outcome {
                    output_folder: old.to_path_buf(),
                    paths,
                    method: MoveMethod::None,
                    whole_folder: true,
                    moved_files: 0,
                    error: Some(e),
                };
            }
        };
        // Files that were already elsewhere (moved individually earlier) join them.
        let mut paths = Vec::with_capacity(plan.files.len());
        let mut error = None;
        let mut moved = 0;
        for f in plan.files {
            if let Ok(r) = f.current.strip_prefix(old) {
                paths.push((f.file_id, dest.join(r)));
                moved += 1;
                continue;
            }
            let to = dest.join(&f.rel);
            if f.current == to || error.is_some() || !f.current.exists() {
                paths.push((f.file_id, if f.current.exists() { f.current.clone() } else { to }));
                continue;
            }
            match move_path(&f.current, &to, plan.copy) {
                Ok(m) => {
                    method = method.merge(m);
                    moved += 1;
                    paths.push((f.file_id, to));
                }
                Err(e) => {
                    paths.push((f.file_id, f.current.clone()));
                    error = Some(e);
                }
            }
        }
        return Outcome {
            output_folder: dest,
            paths,
            method,
            whole_folder: true,
            moved_files: moved,
            error,
        };
    }

    // File by file.
    let target = plan.new_output.to_path_buf();
    // Files already under the target (moved there individually) don't clash.
    let clash = plan.files.iter().any(|f| {
        let to = target.join(&f.rel);
        !f.current.starts_with(&target) && to != f.current && to.symlink_metadata().is_ok()
    });
    let mut dest = target.clone();
    let mut single_rename: Option<PathBuf> = None;
    if clash {
        if plan.single_file {
            if let Some(f) = plan.files.first() {
                single_rename = Some(free_path(&target.join(&f.rel), true));
            }
        } else {
            dest = free_path(&target, false);
        }
    }
    let mut paths = Vec::with_capacity(plan.files.len());
    let mut method = MoveMethod::None;
    let mut moved = 0;
    let mut error: Option<anyhow::Error> = None;
    let mut sources = Vec::new();
    for f in plan.files {
        let to = match &single_rename {
            Some(p) => p.clone(),
            None => dest.join(&f.rel),
        };
        if f.current == to || error.is_some() {
            paths.push((f.file_id, f.current.clone()));
            continue;
        }
        if f.current.symlink_metadata().is_err() {
            // Not on disk (e.g. a deselected file never created): only the path changes.
            paths.push((f.file_id, to));
            continue;
        }
        match move_path(&f.current, &to, plan.copy) {
            Ok(m) => {
                method = method.merge(m);
                moved += 1;
                sources.push(f.current.clone());
                paths.push((f.file_id, to));
            }
            Err(e) => {
                paths.push((f.file_id, f.current.clone()));
                error = Some(e);
            }
        }
    }
    if !plan.copy {
        for s in &sources {
            if let Some(p) = s.parent() {
                remove_empty_dirs(p, old, plan.own_folder);
            }
        }
    }
    let output_folder = if error.is_some() && moved == 0 {
        old.to_path_buf()
    } else {
        dest
    };
    Outcome {
        output_folder,
        paths,
        method,
        whole_folder: false,
        moved_files: moved,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }
    fn files(old: &Path, rels: &[&str]) -> Vec<FileLoc> {
        rels.iter()
            .enumerate()
            .map(|(i, r)| FileLoc {
                file_id: i,
                current: old.join(r),
                rel: PathBuf::from(r),
            })
            .collect()
    }

    #[test]
    fn whole_folder_rename_keeps_structure() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("inc/Show");
        write(&old.join("a.mkv"), "a");
        write(&old.join("Subs/en.srt"), "s");
        write(&old.join("extra.nfo"), "user file");
        let new = t.path().join("done/Show");
        let fl = files(&old, &["a.mkv", "Subs/en.srt"]);
        let o = execute(&Plan {
            old_output: &old,
            new_output: &new,
            files: &fl,
            whole_folder: true,
            own_folder: true,
            single_file: false,
            copy: false,
        });
        assert!(o.error.is_none(), "{:?}", o.error);
        assert_eq!(o.method, MoveMethod::Rename);
        assert_eq!(o.output_folder, new);
        assert_eq!(fs::read_to_string(new.join("Subs/en.srt")).unwrap(), "s");
        assert_eq!(fs::read_to_string(new.join("extra.nfo")).unwrap(), "user file");
        assert!(!old.exists());
        assert!(t.path().join("inc").is_dir(), "parent untouched");
        assert!(!t.path().join("done/a.mkv").exists(), "never loose in the completed dir");
    }

    #[test]
    fn existing_destination_merges_or_gets_suffix() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("inc/Show");
        write(&old.join("a.mkv"), "new a");
        let new = t.path().join("done/Show");
        write(&new.join("other.txt"), "keep");
        let fl = files(&old, &["a.mkv"]);
        // No clash: merged.
        let o = execute(&Plan {
            old_output: &old,
            new_output: &new,
            files: &fl,
            whole_folder: true,
            own_folder: true,
            single_file: false,
            copy: false,
        });
        assert!(o.error.is_none());
        assert_eq!(o.output_folder, new);
        assert_eq!(fs::read_to_string(new.join("other.txt")).unwrap(), "keep");
        assert_eq!(fs::read_to_string(new.join("a.mkv")).unwrap(), "new a");
        // Clash: never overwritten, goes to "Show (2)".
        let old2 = t.path().join("inc2/Show");
        write(&old2.join("a.mkv"), "second a");
        let fl2 = files(&old2, &["a.mkv"]);
        let o = execute(&Plan {
            old_output: &old2,
            new_output: &new,
            files: &fl2,
            whole_folder: true,
            own_folder: true,
            single_file: false,
            copy: false,
        });
        assert!(o.error.is_none());
        assert_eq!(o.output_folder, t.path().join("done/Show (2)"));
        assert_eq!(fs::read_to_string(new.join("a.mkv")).unwrap(), "new a");
        assert_eq!(
            fs::read_to_string(t.path().join("done/Show (2)/a.mkv")).unwrap(),
            "second a"
        );
    }

    #[test]
    fn per_file_into_folder_and_cleanup() {
        // Loose multi-file torrent in a shared folder: files move into their own folder,
        // other people's files and the shared folder stay.
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Complete");
        write(&old.join("ep1.mkv"), "1");
        write(&old.join("Extras/x.mkv"), "x");
        write(&old.join("someone-else.mkv"), "z");
        let new = t.path().join("Complete/Show");
        let fl = files(&old, &["ep1.mkv", "Extras/x.mkv"]);
        let o = execute(&Plan {
            old_output: &old,
            new_output: &new,
            files: &fl,
            whole_folder: false,
            own_folder: false,
            single_file: false,
            copy: false,
        });
        assert!(o.error.is_none(), "{:?}", o.error);
        assert_eq!(o.moved_files, 2);
        assert_eq!(fs::read_to_string(new.join("Extras/x.mkv")).unwrap(), "x");
        assert!(!old.join("Extras").exists(), "emptied subfolder removed");
        assert!(old.join("someone-else.mkv").exists());
    }

    #[test]
    fn single_file_clash_gets_suffixed_name() {
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("inc");
        write(&old.join("movie.mkv"), "new");
        let new = t.path().join("done");
        write(&new.join("movie.mkv"), "old");
        let fl = files(&old, &["movie.mkv"]);
        let o = execute(&Plan {
            old_output: &old,
            new_output: &new,
            files: &fl,
            whole_folder: false,
            own_folder: false,
            single_file: true,
            copy: false,
        });
        assert!(o.error.is_none());
        assert_eq!(o.paths[0].1, new.join("movie (2).mkv"));
        assert_eq!(fs::read_to_string(new.join("movie.mkv")).unwrap(), "old");
        assert!(old.is_dir(), "shared download folder never removed");
    }

    #[test]
    fn copy_is_verified_and_no_clobber() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a/x.bin");
        write(&a, "hello");
        let b = t.path().join("b/x.bin");
        assert_eq!(move_path(&a, &b, true).unwrap(), MoveMethod::Copy);
        assert!(a.exists() && fs::read_to_string(&b).unwrap() == "hello");
        assert!(move_path(&a, &b, false).is_err(), "never overwrites");
        assert!(rename_no_clobber(&a, &b).is_err());
        assert_eq!(fs::read_to_string(&b).unwrap(), "hello");
        assert_eq!(suffixed(Path::new("/x/Show"), 2, false), Path::new("/x/Show (2)"));
        assert_eq!(suffixed(Path::new("/x/a.mkv"), 3, true), Path::new("/x/a (3).mkv"));
    }

    #[test]
    fn cross_fs_copy_when_dev_shm_available() {
        // tmpfs vs the tempdir's file system: exercises the copy+verify+remove path.
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let old = t.path().join("Show");
        write(&old.join("a.bin"), "aaa");
        write(&old.join("Sub/b.bin"), "bbb");
        let s = tempfile::tempdir_in(shm).unwrap();
        let new = s.path().join("Show");
        let fl = files(&old, &["a.bin", "Sub/b.bin"]);
        let o = execute(&Plan {
            old_output: &old,
            new_output: &new,
            files: &fl,
            whole_folder: true,
            own_folder: true,
            single_file: false,
            copy: false,
        });
        assert!(o.error.is_none(), "{:?}", o.error);
        assert_eq!(fs::read_to_string(new.join("Sub/b.bin")).unwrap(), "bbb");
        assert!(!old.exists(), "source removed after verified copy");
        // Same device (tmpfs/overlay inside one tempdir) would have been Rename.
        assert!(matches!(o.method, MoveMethod::Copy | MoveMethod::Rename));
    }
}
