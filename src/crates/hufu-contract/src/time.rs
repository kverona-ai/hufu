//! Time helpers: timestamps, durations and time-sortable ids. No external clock crate is needed.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The current time as an RFC 3339 UTC timestamp with millisecond precision.
pub fn now_rfc3339() -> String {
    rfc3339(now_ms())
}

/// Formats milliseconds since the epoch as `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn rfc3339(ms: u64) -> String {
    let secs = ms / 1000;
    let millis = ms % 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// Parses the timestamps this module writes. Returns milliseconds since the epoch.
pub fn parse_rfc3339(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut millis = 0;
    if b.get(19) == Some(&b'.') {
        let frac: String = s[20..].chars().take_while(|c| c.is_ascii_digit()).collect();
        let padded = format!("{frac:0<3}");
        millis = padded.get(0..3)?.parse::<i64>().ok()?;
    }
    let days = days_from_civil(y, m, d);
    let total = days * 86_400 + hh * 3600 + mm * 60 + ss;
    if total < 0 {
        return None;
    }
    Some(total as u64 * 1000 + millis as u64)
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parses a duration such as `30m`, `2h`, `90s`, `1h30m`, `500ms`, `2d`. A bare number is seconds.
pub fn parse_duration(input: &str) -> Result<Duration, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("empty duration".into());
    }
    if let Ok(n) = s.parse::<u64>() {
        return Ok(Duration::from_secs(n));
    }
    let mut total_ms: u64 = 0;
    let mut digits = String::new();
    let mut chars = s.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() {
            digits.push(c);
            continue;
        }
        if digits.is_empty() {
            return Err(format!(
                "invalid duration `{input}`: expected a number before `{c}`; examples: 90s, 30m, 2h, 1h30m"
            ));
        }
        let n: u64 = digits.parse().map_err(|_| format!("invalid duration `{input}`"))?;
        digits.clear();
        let unit_ms = match c {
            'd' => 86_400_000,
            'h' => 3_600_000,
            'm' => {
                if chars.peek() == Some(&'s') {
                    chars.next();
                    1
                } else {
                    60_000
                }
            }
            's' => 1000,
            _ => {
                return Err(format!("invalid duration `{input}`: unknown unit `{c}`; use ms, s, m, h or d"));
            }
        };
        total_ms = total_ms.saturating_add(n.saturating_mul(unit_ms));
        any = true;
    }
    if !digits.is_empty() || !any {
        return Err(format!("invalid duration `{input}`: missing unit; examples: 90s, 30m, 2h"));
    }
    Ok(Duration::from_millis(total_ms))
}

/// Formats seconds compactly: `42s`, `5m12s`, `2h03m`.
pub fn fmt_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// A short, time-sortable id such as `j_7k2m9x4qpd`: six characters of time, four of randomness.
pub fn new_id(prefix: &str) -> String {
    // Seconds since 2025-01-01, which keeps six base-32 digits meaningful for decades.
    let secs = (now_ms() / 1000).saturating_sub(1_735_689_600);
    let mut out = String::with_capacity(prefix.len() + 11);
    out.push_str(prefix);
    out.push('_');
    for i in (0..6).rev() {
        out.push(ALPHABET[((secs >> (5 * i)) & 31) as usize] as char);
    }
    let mut rnd = [0u8; 4];
    if getrandom::fill(&mut rnd).is_err() {
        let n = now_ms();
        rnd = [(n & 255) as u8, (n >> 3) as u8, (n >> 7) as u8, (n >> 11) as u8];
    }
    for b in rnd {
        out.push(ALPHABET[(b & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_round_trip() {
        for ms in [0u64, 1_000, 951_782_400_000, 1_794_000_000_123, 4_102_444_800_999] {
            assert_eq!(parse_rfc3339(&rfc3339(ms)), Some(ms));
        }
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(parse_rfc3339("nonsense"), None);
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("30m").unwrap().as_secs(), 1800);
        assert_eq!(parse_duration("1h30m").unwrap().as_secs(), 5400);
        assert_eq!(parse_duration("500ms").unwrap().as_millis(), 500);
        assert_eq!(parse_duration("45").unwrap().as_secs(), 45);
        assert_eq!(parse_duration("2d").unwrap().as_secs(), 172_800);
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("10x").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn ids_are_prefixed_and_sortable_in_time() {
        let a = new_id("j");
        assert!(a.starts_with("j_"));
        assert_eq!(a.len(), 12);
        assert_eq!(fmt_duration(312), "5m12s");
        assert_eq!(fmt_duration(7380), "2h03m");
    }
}
