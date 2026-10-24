//! Sidebar row meta (v2's shell): the short age on the right of a chat row
//! (`now`, `5m`, `3h`, `2d`, `3w`, `4mo`, `1y`). GPUI-free and tested.

/// Seconds since the Unix epoch for an RFC 3339 UTC or offset timestamp
/// as the store writes it (`2026-10-25T03:20:11.123Z`), or `None`.
pub fn unix_secs(rfc3339: &str) -> Option<i64> {
    let s = rfc3339.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, se) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    // Offset after optional fraction: Z, or ±HH:MM.
    let rest = &s[19..];
    let rest = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            let o = oh * 3600 + om * 60;
            if sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y2 = if mo <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + h * 3600 + mi * 60 + se - offset)
}

/// The age label for something made at `created` (RFC 3339) when it is
/// `now_secs` (Unix seconds). `None` when the timestamp does not parse.
pub fn age_label(created: &str, now_secs: i64) -> Option<String> {
    let secs = (now_secs - unix_secs(created)?).max(0);
    let min = secs / 60;
    let hours = min / 60;
    let days = hours / 24;
    Some(match () {
        _ if min < 1 => "now".to_owned(),
        _ if hours < 1 => format!("{min}m"),
        _ if days < 1 => format!("{hours}h"),
        _ if days < 7 => format!("{days}d"),
        _ if days < 30 => format!("{}w", days / 7),
        _ if days < 365 => format!("{}mo", days / 30),
        _ => format!("{}y", days / 365),
    })
}

/// Unix seconds now.
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_parse_as_the_store_writes_them() {
        assert_eq!(unix_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(unix_secs("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(
            unix_secs("2026-10-25T03:20:11.123456Z"),
            Some(1_792_898_411)
        );
        assert_eq!(
            unix_secs("2026-10-25T08:20:11+05:00"),
            unix_secs("2026-10-25T03:20:11Z")
        );
        assert_eq!(unix_secs("now"), None);
        assert_eq!(unix_secs("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn ages_read_short() {
        let t = unix_secs("2026-10-25T00:00:00Z").unwrap();
        let at = "2026-10-25T00:00:00Z";
        assert_eq!(age_label(at, t + 20).as_deref(), Some("now"));
        assert_eq!(age_label(at, t + 5 * 60).as_deref(), Some("5m"));
        assert_eq!(age_label(at, t + 3 * 3600).as_deref(), Some("3h"));
        assert_eq!(age_label(at, t + 2 * 86_400).as_deref(), Some("2d"));
        assert_eq!(age_label(at, t + 15 * 86_400).as_deref(), Some("2w"));
        assert_eq!(age_label(at, t + 100 * 86_400).as_deref(), Some("3mo"));
        assert_eq!(age_label(at, t + 800 * 86_400).as_deref(), Some("2y"));
        assert_eq!(age_label(at, t - 50).as_deref(), Some("now"), "clock skew");
        assert_eq!(age_label("bad", t), None);
    }
}
