//! Dates, clock times and durations as plate-reader exports write them, to ISO-8601.

use openreadout_core::time::{civil_from_days, full_year};

/// A calendar date and whether its day/month order had to be assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Date {
    pub(crate) year: u32,
    pub(crate) month: u32,
    pub(crate) day: u32,
    /// `M/D/Y` was assumed for a slash date whose first two fields are both ≤ 12.
    pub(crate) order_assumed: bool,
}

fn nums(s: &str, sep: char) -> Option<Vec<u32>> {
    let v: Option<Vec<u32>> = s.split(sep).map(|p| p.trim().parse().ok()).collect();
    v.filter(|v| v.len() == 3)
}

fn valid(d: Date) -> Option<Date> {
    (d.month >= 1 && d.month <= 12 && d.day >= 1 && d.day <= 31 && d.year >= 1900 && d.year < 3000)
        .then_some(d)
}

/// `4/11/2024` (US), `29/02/2016` (day first, detected), `20.02.2024` (day first),
/// `2024-04-11`, `2025/01/01`.
pub(crate) fn parse_date(s: &str) -> Option<Date> {
    let t = s.trim().trim_start_matches('\'');
    let t = t.split(['T', ' ']).next().unwrap_or(t);
    if let Some(v) = nums(t, '-').or_else(|| nums(t, '/').filter(|v| v[0] > 31))
        && v[0] > 31
    {
        return valid(Date {
            year: v[0],
            month: v[1],
            day: v[2],
            order_assumed: false,
        });
    }
    if let Some(v) = nums(t, '.') {
        return valid(Date {
            year: full_year(v[2]),
            month: v[1],
            day: v[0],
            order_assumed: false,
        });
    }
    let v = nums(t, '/')?;
    let year = full_year(v[2]);
    if v[0] > 12 {
        return valid(Date {
            year,
            month: v[1],
            day: v[0],
            order_assumed: false,
        });
    }
    valid(Date {
        year,
        month: v[0],
        day: v[1],
        order_assumed: v[1] <= 12 && v[0] != v[1],
    })
}

/// `5:27:15 PM`, `14:34:46`, `'3:42:12 AM`, `10:30:00.5` → seconds since midnight.
pub(crate) fn parse_clock(s: &str) -> Option<f64> {
    let t = s.trim().trim_start_matches('\'').trim();
    let (hms, pm, am) = if let Some(x) = t.strip_suffix("PM").or_else(|| t.strip_suffix("pm")) {
        (x.trim(), true, false)
    } else if let Some(x) = t.strip_suffix("AM").or_else(|| t.strip_suffix("am")) {
        (x.trim(), false, true)
    } else {
        (t, false, false)
    };
    let parts: Vec<&str> = hms.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut h: u32 = parts[0].trim().parse().ok()?;
    let m: u32 = parts[1].trim().parse().ok()?;
    let sec: f64 = parts.get(2).map_or(Some(0.0), |p| p.trim().parse().ok())?;
    if pm && h < 12 {
        h += 12;
    }
    if am && h == 12 {
        h = 0;
    }
    (h < 24 && m < 60 && (0.0..61.0).contains(&sec)).then(|| f64::from(h * 3600 + m * 60) + sec)
}

/// ISO-8601 local date-time (no zone: plate-reader exports record none), seconds kept only
/// when fractional.
pub(crate) fn iso(d: Date, clock_s: Option<f64>) -> String {
    let date = format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
    let Some(clock) = clock_s else {
        return date;
    };
    let whole = clock.floor();
    let hour = (whole / 3600.0) as u32;
    let minute = ((whole - f64::from(hour) * 3600.0) / 60.0) as u32;
    let second = clock - f64::from(hour * 3600 + minute * 60);
    if second.fract() == 0.0 {
        format!("{date}T{hour:02}:{minute:02}:{:02}", second as u32)
    } else {
        format!("{date}T{hour:02}:{minute:02}:{second:06.3}")
    }
}

/// A date and a time given separately or together (`1/15/2024 10:30:00 AM`,
/// `20.02.2024 18:19:42`, `2020-10-26T16:16:18.2284559-04:00`). Returns the ISO text and
/// whether the day/month order was assumed. ISO input with a zone is passed through.
pub(crate) fn combine(date: &str, time: Option<&str>) -> Option<(String, bool)> {
    let d = date.trim().trim_start_matches('\'');
    if d.len() >= 19 && d.as_bytes().get(10) == Some(&b'T') && d.as_bytes().get(4) == Some(&b'-') {
        // a spreadsheet date cell (midnight) with the time of day in its own cell
        if let (Some(day), Some(t)) = (d.strip_suffix("T00:00:00"), time)
            && let (Some(parsed), Some(clock)) = (parse_date(day), parse_clock(t))
        {
            return Some((iso(parsed, Some(clock)), false));
        }
        return Some((d.to_string(), false));
    }
    let parsed = parse_date(d)?;
    let clock = match time {
        Some(t) => parse_clock(t),
        None => d.split_once(' ').and_then(|(_, t)| parse_clock(t)),
    };
    Some((iso(parsed, clock), parsed.order_assumed))
}

/// Elapsed time `H:MM:SS`, `HH:MM:SS`, `M:SS` or plain seconds → seconds.
pub(crate) fn parse_duration(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.contains(':') {
        return crate::sheet::parse_number(trimmed);
    }
    let parts: Vec<&str> = trimmed.split(':').collect();
    let vals: Option<Vec<f64>> = parts.iter().map(|p| p.trim().parse::<f64>().ok()).collect();
    let v = vals?;
    match v.as_slice() {
        [minutes, seconds] => Some(minutes * 60.0 + seconds),
        [hours, minutes, seconds] => Some(hours * 3600.0 + minutes * 60.0 + seconds),
        [days, hours, minutes, seconds] => {
            Some(days * 86400.0 + hours * 3600.0 + minutes * 60.0 + seconds)
        }
        _ => None,
    }
    .filter(|x| x.is_finite() && *x >= 0.0)
}

/// Excel serial day number (1900 date system) → ISO date-time.
pub(crate) fn excel_serial(v: f64) -> Option<String> {
    if !(1.0..2_958_466.0).contains(&v) {
        return None;
    }
    let days = v.floor() as i64;
    let secs = ((v - v.floor()) * 86400.0).round();
    // 1899-12-30 is serial 0 (accounting for the 1900 leap-year bug for dates after Feb 1900).
    let (y, m, d) = civil_from_days(days - 25569);
    Some(iso(
        Date {
            year: u32::try_from(y).ok()?,
            month: m,
            day: d,
            order_assumed: false,
        },
        Some(secs),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        let d = parse_date("4/11/2024").unwrap();
        assert_eq!(
            (d.year, d.month, d.day, d.order_assumed),
            (2024, 4, 11, true)
        );
        let d = parse_date("29/02/2016").unwrap();
        assert_eq!((d.month, d.day, d.order_assumed), (2, 29, false));
        let d = parse_date("20.02.2024").unwrap();
        assert_eq!((d.month, d.day), (2, 20));
        let d = parse_date("2021-07-06").unwrap();
        assert_eq!((d.year, d.month, d.day), (2021, 7, 6));
        assert!(parse_date("13/13/2020").is_none());
        assert_eq!(parse_date("6/6/2024").map(|d| d.order_assumed), Some(false));
    }

    #[test]
    fn clocks_and_combine() {
        assert_eq!(
            parse_clock("5:27:15 PM"),
            Some(17.0 * 3600.0 + 27.0 * 60.0 + 15.0)
        );
        assert_eq!(parse_clock("12:00:01 AM"), Some(1.0));
        assert_eq!(
            parse_clock("'3:42:12 AM"),
            Some(3.0 * 3600.0 + 42.0 * 60.0 + 12.0)
        );
        assert_eq!(
            combine("4/11/2024", Some("5:27:15 PM")),
            Some(("2024-04-11T17:27:15".into(), true))
        );
        assert_eq!(
            combine("20.02.2024 18:19:42", None),
            Some(("2024-02-20T18:19:42".into(), false))
        );
        assert_eq!(
            combine("2020-10-26T16:16:18.2284559-04:00", None)
                .unwrap()
                .0,
            "2020-10-26T16:16:18.2284559-04:00"
        );
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("0:04:22"), Some(262.0));
        assert_eq!(parse_duration("1:00"), Some(60.0));
        assert_eq!(parse_duration("00:01:30"), Some(90.0));
        assert_eq!(parse_duration("95.3"), Some(95.3));
        assert_eq!(parse_duration("x"), None);
        assert_eq!(
            excel_serial(45000.5).as_deref(),
            Some("2023-03-15T12:00:00")
        );
    }
}
