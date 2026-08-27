//! Small display helpers: byte sizes, age strings, and just enough RFC 3339
//! to turn pgbot's timestamps into "3m ago" without pulling in a date crate.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn human_bytes(n: i64) -> String {
    const KIB: f64 = 1024.0;
    let b = n as f64;
    let (v, unit) = if b >= KIB * KIB * KIB * KIB {
        (b / (KIB * KIB * KIB * KIB), "TiB")
    } else if b >= KIB * KIB * KIB {
        (b / (KIB * KIB * KIB), "GiB")
    } else if b >= KIB * KIB {
        (b / (KIB * KIB), "MiB")
    } else if b >= KIB {
        (b / KIB, "KiB")
    } else {
        return format!("{n} B");
    };
    if v >= 10.0 {
        format!("{v:.0} {unit}")
    } else {
        format!("{v:.1} {unit}")
    }
}

pub fn human_count(n: i64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.1}B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1}M", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.1}k", f / 1e3)
    } else {
        format!("{n}")
    }
}

/// Compact "how long ago" for status lines.
pub fn ago(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

/// Renders a millisecond mean like pgbot does: sub-second in ms, then seconds.
pub fn human_ms(ms: f64) -> String {
    if ms < 1.0 {
        format!("{ms:.2} ms")
    } else if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.1} s", ms / 1000.0)
    }
}

/// Parses an RFC 3339 timestamp ("2026-08-26T09:57:00Z", fractional seconds
/// and ±hh:mm offsets included) into SystemTime. Returns None on anything it
/// does not understand — callers degrade to "—".
pub fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    if b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b't' && b[10] != b' ') {
        return None;
    }
    let (hour, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let mut i = 19;
    let mut nanos: u32 = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let frac = &s[start..i];
        if frac.is_empty() {
            return None;
        }
        let scale = 10u64.pow(9u32.saturating_sub(frac.len() as u32).min(9));
        nanos = (frac[..frac.len().min(9)].parse::<u64>().ok()? * scale) as u32;
    }
    let offset_sec: i64 = match b.get(i) {
        Some(b'Z') | Some(b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            let mag = oh * 3600 + om * 60;
            if sign == b'+' {
                mag
            } else {
                -mag
            }
        }
        _ => return None,
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days-from-civil.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + hour * 3600 + min * 60 + sec - offset_sec;
    if secs < 0 {
        return None;
    }
    Some(UNIX_EPOCH + Duration::new(secs as u64, nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_scale_with_one_decimal_under_ten() {
        assert_eq!(human_bytes(500), "500 B");
        assert_eq!(human_bytes(12_884_901_888), "12 GiB");
        assert_eq!(human_bytes(9_019_431_321), "8.4 GiB");
        assert_eq!(human_bytes(45_097_156_608), "42 GiB");
    }

    #[test]
    fn counts_compact() {
        assert_eq!(human_count(950), "950");
        assert_eq!(human_count(18_201), "18.2k");
        assert_eq!(human_count(18_200_000), "18.2M");
        assert_eq!(human_count(840_000_000), "840.0M");
    }

    #[test]
    fn ago_buckets() {
        assert_eq!(ago(Duration::from_secs(12)), "12s ago");
        assert_eq!(ago(Duration::from_secs(3 * 60 + 5)), "3m ago");
        assert_eq!(ago(Duration::from_secs(7200)), "2h ago");
        assert_eq!(ago(Duration::from_secs(200_000)), "2d ago");
    }

    #[test]
    fn rfc3339_z_and_offsets_and_fractions() {
        let a = parse_rfc3339("2026-08-26T10:00:00Z").unwrap();
        let b = parse_rfc3339("2026-08-26T12:00:00+02:00").unwrap();
        assert_eq!(a, b);
        let c = parse_rfc3339("2026-08-26T09:59:30.500Z").unwrap();
        assert_eq!(a.duration_since(c).unwrap(), Duration::from_millis(29_500));
        // epoch sanity: 2026-08-26T10:00:00Z
        let secs = a.duration_since(UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(secs, 1_787_738_400);
    }

    #[test]
    fn rfc3339_garbage_is_none() {
        for s in [
            "",
            "not a date",
            "2026-13-01T00:00:00Z",
            "2026-08-26",
            "2026-08-26T10:00:00",
        ] {
            assert!(parse_rfc3339(s).is_none(), "{s}");
        }
    }
}
