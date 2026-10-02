//! Minimal timestamp helpers (no chrono dependency in the core).

/// Convert a Windows FILETIME (100 ns ticks since 1601-01-01 UTC) to ISO-8601 UTC.
pub fn filetime_to_iso8601(ft: u64) -> String {
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) as i64 - EPOCH_DIFF_SECS;
    let millis = (ft % 10_000_000) / 10_000;
    unix_to_iso8601(secs, millis as u32)
}

/// Convert Unix seconds plus milliseconds to ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
pub fn unix_to_iso8601(secs: i64, millis: u32) -> String {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

/// Parse ISO-8601 `YYYY-MM-DDTHH:MM:SS[.fraction][Z|±HH:MM]` into Unix seconds (fraction kept).
/// A missing zone is taken as UTC. Returns `None` for anything else.
pub fn iso8601_to_unix(text: &str) -> Option<f64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 19
        || !text.is_ascii()
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b' ')
    {
        return None;
    }
    let field = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (year, month, day) = (field(0..4)?, field(5..7)?, field(8..10)?);
    let (hour, minute, second) = (field(11..13)?, field(14..16)?, field(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut rest = &text[19..];
    let mut frac = 0.0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits: String = after_dot.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        frac = format!("0.{digits}").parse().ok()?;
        rest = &after_dot[digits.len()..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        zone if zone.len() == 6
            && (zone.starts_with('+') || zone.starts_with('-'))
            && &zone[3..4] == ":" =>
        {
            let hours: i64 = zone[1..3].parse().ok()?;
            let minutes: i64 = zone[4..6].parse().ok()?;
            let secs = hours * 3600 + minutes * 60;
            if zone.starts_with('-') { -secs } else { secs }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month as u32, day as u32);
    Some((days * 86_400 + hour * 3600 + minute * 60 + second - offset) as f64 + frac)
}

/// Days since 1970-01-01 of a (year, month, day) in the proleptic Gregorian calendar (Howard
/// Hinnant's civil-to-days algorithm; inverse of [`civil_from_days`]).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// (year, month, day) of a count of days since 1970-01-01 in the proleptic Gregorian calendar
/// (Howard Hinnant's days-to-civil algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Month number (1-12) of an English three-letter month abbreviation (`Jan`, `FEB`, ...),
/// ignoring case.
pub fn month_from_abbrev(s: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let i = MONTHS.iter().position(|m| m.eq_ignore_ascii_case(s))?;
    u32::try_from(i + 1).ok()
}

/// A two-digit year as a full year: 0-69 are 20xx, 70-99 are 19xx; larger years are unchanged.
pub fn full_year(y: u32) -> u32 {
    match y {
        0..=69 => 2000 + y,
        70..=99 => 1900 + y,
        _ => y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch() {
        assert_eq!(unix_to_iso8601(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            unix_to_iso8601(1_700_000_000, 5),
            "2023-11-14T22:13:20.005Z"
        );
    }

    #[test]
    fn iso8601_parse() {
        assert_eq!(iso8601_to_unix("1970-01-01T00:00:00Z"), Some(0.0));
        let t = iso8601_to_unix("2023-11-14T22:13:20.005Z").unwrap();
        assert!((t - 1_700_000_000.005).abs() < 1e-6);
        let a = iso8601_to_unix("2022-08-22T06:38:37.6951779Z").unwrap();
        let b = iso8601_to_unix("2022-08-22T06:38:38.0333740Z").unwrap();
        assert!((b - a - 0.338_196_1).abs() < 1e-6);
        assert_eq!(
            iso8601_to_unix("2022-08-22T08:38:37+02:00"),
            iso8601_to_unix("2022-08-22T06:38:37Z")
        );
        assert_eq!(iso8601_to_unix("2000-02-29T00:00:00"), Some(951_782_400.0));
        for bad in [
            "",
            "2022-13-01T00:00:00Z",
            "2022-08-22",
            "2022-08-22T06:38:37.Z",
            "2022-08-22T06:38:37+0200",
            "2é22-08-22T06:38:37Z",
        ] {
            assert_eq!(iso8601_to_unix(bad), None, "{bad}");
        }
    }

    #[test]
    fn civil_dates() {
        for days in [-1, 0, 365, 14_371, 20_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        assert_eq!(month_from_abbrev("Nov"), Some(11));
        assert_eq!(month_from_abbrev("november"), None);
        assert_eq!(
            (full_year(10), full_year(99), full_year(2004)),
            (2010, 1999, 2004)
        );
    }

    #[test]
    fn filetime() {
        // 2017-01-30 in FILETIME ticks (from a LIF TimeStamp: HighInteger=30571214, LowInteger=209346723)
        let ft = (30_571_214u64 << 32) | 0x0c7a_60a3;
        assert!(filetime_to_iso8601(ft).starts_with("2017-01-30"));
    }
}
