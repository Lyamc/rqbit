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

/// Progress percentage, truncated (floored) at `decimals` places, never
/// rounded: 99.9% shows "99", 99.99% at one decimal shows "99.9", and "100"
/// only appears when `have >= total` (a zero-length total counts as complete).
pub fn format_progress(have: u64, total: u64, decimals: u32) -> String {
    let scale = 10u128.pow(decimals);
    let units: u128 = if have >= total {
        100 * scale
    } else {
        // Integer math: exact for any size, and capped below 100.
        (have as u128 * 100 * scale / total as u128).min(100 * scale - 1)
    };
    if decimals == 0 {
        return units.to_string();
    }
    format!(
        "{}.{:0width$}",
        units / scale,
        units % scale,
        width = decimals as usize
    )
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

/// Duration for inputs: "2h 30m", "15m", "0s" (two most significant units),
/// matching the web UI `formatDuration`.
pub fn format_duration(secs: u64) -> String {
    if secs == 0 {
        return "0s".into();
    }
    let mut rest = secs;
    let mut parts = Vec::new();
    for (u, n) in [("d", 86400), ("h", 3600), ("m", 60), ("s", 1)] {
        if rest >= n {
            parts.push(format!("{}{u}", rest / n));
            rest %= n;
        }
        if parts.len() == 2 {
            break;
        }
    }
    parts.join(" ")
}

/// Parses "90" (bare = `bare_unit_secs`), "15m", "2h 30m", "1.5h", "3 days".
pub fn parse_duration(input: &str, bare_unit_secs: u64) -> Option<u64> {
    let s = input.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    if let Ok(v) = s.parse::<f64>() {
        return (v >= 0.0).then(|| (v * bare_unit_secs as f64).round() as u64);
    }
    let mut total = 0f64;
    let mut rest = s.as_str();
    let mut any = false;
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let num_len = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_len == 0 {
            return None;
        }
        let v: f64 = rest[..num_len].parse().ok()?;
        rest = rest[num_len..].trim_start();
        let unit_len = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        let mult = match unit.chars().next()? {
            's' => 1.0,
            'm' => 60.0,
            'h' => 3600.0,
            'd' => 86400.0,
            _ => return None,
        };
        total += v * mult;
        rest = &rest[unit_len..];
        any = true;
    }
    any.then(|| total.round() as u64)
}

/// Parses "500 MB", "1.5GB", "2 TiB", "1024" / "10 Bytes". Binary units
/// (1 KB = 1024), like `format_bytes`.
pub fn parse_size(input: &str) -> Option<u64> {
    let s: String = input.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return None;
    }
    let num_len = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let v: f64 = s[..num_len].parse().ok()?;
    let exp = match &s[num_len..] {
        "" | "b" | "byte" | "bytes" => 0,
        "k" | "kb" | "kib" | "ki" => 1,
        "m" | "mb" | "mib" | "mi" => 2,
        "g" | "gb" | "gib" | "gi" => 3,
        "t" | "tb" | "tib" | "ti" => 4,
        _ => return None,
    };
    Some((v * 1024f64.powi(exp)).round() as u64)
}

#[cfg(test)]
mod tests {
    #[test]
    fn progress_is_floored_never_rounded() {
        assert_eq!(format_progress(999, 1000, 0), "99");
        assert_eq!(format_progress(9999, 10000, 1), "99.9");
        assert_eq!(format_progress(99_999, 100_000, 2), "99.99");
        assert_eq!(format_progress(1000, 1000, 0), "100");
        assert_eq!(format_progress(1000, 1000, 1), "100.0");
        assert_eq!(format_progress(0, 1000, 0), "0");
        assert_eq!(format_progress(1, 1000, 0), "0");
        assert_eq!(format_progress(29, 100, 0), "29");
        assert_eq!(format_progress(57, 100, 1), "57.0");
        assert_eq!(format_progress(1, 3, 2), "33.33");
        assert_eq!(format_progress(2, 3, 0), "66");
        assert_eq!(format_progress(5, 1000, 1), "0.5");
        let big = 3u64 << 40; // 3 TiB, one byte short
        assert_eq!(format_progress(big - 1, big, 0), "99");
        assert_eq!(format_progress(big - 1, big, 3), "99.999");
        assert_eq!(format_progress(0, 0, 0), "100");
        assert_eq!(format_progress(u64::MAX - 1, u64::MAX, 1), "99.9");
    }

    use super::*;

    #[test]
    fn durations_and_sizes() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(7800), "2h 10m");
        assert_eq!(format_duration(90061), "1d 1h");
        assert_eq!(parse_duration("15", 60), Some(900));
        assert_eq!(parse_duration("2h 30m", 60), Some(9000));
        assert_eq!(parse_duration("1.5h", 60), Some(5400));
        assert_eq!(parse_duration("3 days", 60), Some(259200));
        assert_eq!(parse_duration("2h10m", 60), Some(7800));
        assert_eq!(parse_duration("abc", 60), None);
        assert_eq!(parse_duration("", 60), None);
        assert_eq!(parse_size("500 MB"), Some(500 * 1024 * 1024));
        assert_eq!(parse_size("2GiB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("1024"), Some(1024));
        assert_eq!(parse_size("0 Bytes"), Some(0));
        assert_eq!(parse_size("1.5 x"), None);
        // format_bytes output round-trips.
        let v = 5 * 1024 * 1024 * 1024u64;
        assert_eq!(parse_size(&format_bytes(v)), Some(v));
    }

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
