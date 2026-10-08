//! "Disk full" detection and free-space queries.
//!
//! A write that fails with ENOSPC / EDQUOT (Rust: `StorageFull` / `QuotaExceeded`) is not
//! a fault of the data or of the disk: retrying it can only succeed once space is freed.
//! Live torrents therefore stop downloading when they hit it (seeding goes on), check the
//! free space periodically and resume by themselves; such failures don't count toward the
//! I/O recovery backoff / give-up (see `torrent_state::live`).

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Free space for unprivileged writers (`statvfs` `f_bavail`) on the filesystem holding
/// a path, and that filesystem's mount point.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DiskSpace {
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub mount: String,
}

fn io_is_disk_full(e: &io::Error) -> bool {
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        if code == libc::ENOSPC || code == libc::EDQUOT {
            return true;
        }
    }
    matches!(
        e.kind(),
        io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded
    )
}

/// Whether any error in the chain is "no space left" / "quota exceeded".
pub fn is_disk_full(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        if let Some(io) = c.downcast_ref::<io::Error>() {
            return io_is_disk_full(io);
        }
        #[cfg(unix)]
        if let Some(errno) = c.downcast_ref::<nix::errno::Errno>() {
            return matches!(
                *errno,
                nix::errno::Errno::ENOSPC | nix::errno::Errno::EDQUOT
            );
        }
        false
    })
}

/// Same, for an error that was already turned into text.
pub fn is_disk_full_text(s: &str) -> bool {
    [
        "ENOSPC",
        "EDQUOT",
        "No space left on device",
        "Disk quota exceeded",
        "Disk full",
    ]
    .iter()
    .any(|m| s.contains(m))
}

/// "12.3 GB" (decimal gigabytes, one decimal).
pub fn format_gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// "(0.4 GB free on /mnt/data)", or "" when unknown.
pub fn free_note(space: Option<&DiskSpace>) -> String {
    match space {
        Some(s) => format!(" ({} free on {})", format_gb(s.free_bytes), s.mount),
        None => String::new(),
    }
}

/// Status / event text while a torrent waits for space.
pub fn waiting_message(space: Option<&DiskSpace>) -> String {
    format!(
        "Disk full: downloading paused until space is freed{}",
        free_note(space)
    )
}

/// Error context when a torrent without soft recovery stops on a full disk.
pub fn stopped_message(space: Option<&DiskSpace>) -> String {
    format!("Disk full: torrent stopped{}", free_note(space))
}

/// Free space needed before a waiting torrent resumes: what's left to download (plus a
/// piece of slack), at most 1 GiB, and at least two pieces.
pub fn resume_threshold(remaining_bytes: u64, piece_len: u64) -> u64 {
    const CAP: u64 = 1 << 30;
    remaining_bytes
        .saturating_add(piece_len)
        .min(CAP)
        .max(piece_len.saturating_mul(2))
}

/// Mount point from `/proc/self/mountinfo` text: the longest mount point that is a path
/// prefix of `path` (component-wise).
pub fn mount_point_from_mountinfo(mountinfo: &str, path: &Path) -> Option<PathBuf> {
    mountinfo
        .lines()
        .filter_map(|l| l.split(' ').nth(4))
        .map(|m| PathBuf::from(unescape_mount(m)))
        .filter(|m| path.starts_with(m))
        .max_by_key(|m| m.components().count())
}

/// mountinfo escapes space, tab, newline and backslash as `\ooo`.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Free space on the filesystem holding `path` (its nearest existing ancestor). Blocking
/// (one `statvfs` and a read of `/proc/self/mountinfo`); call off the async workers.
#[cfg(unix)]
pub fn query(path: &Path) -> Option<DiskSpace> {
    let mut p = path.to_path_buf();
    while !p.exists() {
        if !p.pop() {
            return None;
        }
    }
    let p = p.canonicalize().unwrap_or(p);
    let st = nix::sys::statvfs::statvfs(&p).ok()?;
    let frsize = st.fragment_size() as u64;
    let mount = std::fs::read_to_string("/proc/self/mountinfo")
        .ok()
        .and_then(|mi| mount_point_from_mountinfo(&mi, &p))
        .unwrap_or_else(|| p.clone());
    Some(DiskSpace {
        free_bytes: (st.blocks_available() as u64).saturating_mul(frsize),
        total_bytes: (st.blocks() as u64).saturating_mul(frsize),
        mount: mount.to_string_lossy().into_owned(),
    })
}

#[cfg(not(unix))]
pub fn query(_path: &Path) -> Option<DiskSpace> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn detects_disk_full_in_chain() {
        let e = anyhow::Error::from(nix::errno::Errno::ENOSPC).context("error calling pwritev");
        assert!(is_disk_full(&e));
        let e = anyhow::Error::from(io::Error::from_raw_os_error(libc::EDQUOT))
            .context("error writing to file 0");
        assert!(is_disk_full(&e));
        let e: anyhow::Result<()> = Err(io::Error::from(io::ErrorKind::StorageFull)).context("x");
        assert!(is_disk_full(&e.unwrap_err()));
        let e = anyhow::Error::from(nix::errno::Errno::EIO).context("error calling pwritev");
        assert!(!is_disk_full(&e));
        assert!(is_disk_full_text(
            "error writing to file 0 (\"a\"): error calling pwritev: ENOSPC: No space left on device"
        ));
        assert!(!is_disk_full_text("EIO: Input/output error"));
    }

    #[test]
    fn messages() {
        let s = DiskSpace {
            free_bytes: 432_100_000,
            total_bytes: 4_000_000_000_000,
            mount: "/mnt/data".into(),
        };
        assert_eq!(
            waiting_message(Some(&s)),
            "Disk full: downloading paused until space is freed (0.4 GB free on /mnt/data)"
        );
        assert_eq!(stopped_message(None), "Disk full: torrent stopped");
    }

    #[test]
    fn threshold() {
        let mib = 1 << 20;
        assert_eq!(resume_threshold(10 * mib, 4 * mib), 14 * mib);
        assert_eq!(resume_threshold(100 << 30, 4 * mib), 1 << 30);
        assert_eq!(resume_threshold(0, 16 * mib), 32 * mib);
    }

    #[test]
    fn mountinfo_longest_prefix() {
        let mi = "22 1 0:21 / / rw,relatime - ext4 /dev/root rw\n\
                  30 22 8:17 / /media/usb rw - ext4 /dev/sdb1 rw\n\
                  31 22 8:18 / /media/usb\\040disk rw - ext4 /dev/sdc1 rw\n\
                  32 22 8:19 / /media/us rw - ext4 /dev/sdd1 rw\n";
        let m = |p: &str| mount_point_from_mountinfo(mi, Path::new(p)).unwrap();
        assert_eq!(m("/media/usb/a/b"), PathBuf::from("/media/usb"));
        assert_eq!(m("/media/usb disk/x"), PathBuf::from("/media/usb disk"));
        assert_eq!(m("/media/usbx"), PathBuf::from("/"));
        assert_eq!(m("/home"), PathBuf::from("/"));
    }

    #[cfg(unix)]
    #[test]
    fn query_existing_ancestor() {
        let d = tempfile::tempdir().unwrap();
        let s = query(&d.path().join("not/yet/created")).unwrap();
        assert!(s.total_bytes > 0);
        assert!(!s.mount.is_empty());
    }
}
