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
    fn speed() {
        assert_eq!(format_speed(0.0), "");
        assert_eq!(format_speed(1.5), "1.5 MB/s");
    }
}
