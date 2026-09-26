//! Human readable formatting, matching the web UI (`helper/formatBytes.ts`).

pub fn format_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "0 Bytes".into();
    }
    const SIZES: [&str; 6] = ["Bytes", "KB", "MB", "GB", "TB", "PB"];
    let i = ((bytes as f64).ln() / 1024f64.ln()).floor() as usize;
    let i = i.min(SIZES.len() - 1);
    let v = bytes as f64 / 1024f64.powi(i as i32);
    // toFixed(2) then parseFloat drops trailing zeros.
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s} {}", SIZES[i])
}

/// `mbps` is MiB/s as reported by rqbit.
pub fn format_speed(mbps: f64) -> String {
    if mbps <= 0.0 {
        return String::new();
    }
    format!("{}/s", format_bytes((mbps * 1024.0 * 1024.0) as u64))
}

/// Web UI `formatSecondsToTime`-like: "3d 4h", "2h 5m", "4m 10s", "12s".
pub fn format_uptime(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60, secs % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

pub fn format_ratio(r: f64) -> String {
    format!("{r:.2}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(0), "0 Bytes");
        assert_eq!(format_bytes(1023), "1023 Bytes");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024 * 1024), "5 GB");
    }

    #[test]
    fn uptime() {
        assert_eq!(format_uptime(12), "12s");
        assert_eq!(format_uptime(250), "4m 10s");
        assert_eq!(format_uptime(7500), "2h 5m");
        assert_eq!(format_uptime(3 * 86400 + 4 * 3600 + 5), "3d 4h");
    }

    #[test]
    fn speed() {
        assert_eq!(format_speed(0.0), "");
        assert_eq!(format_speed(1.5), "1.5 MB/s");
    }
}
