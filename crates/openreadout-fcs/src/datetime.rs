//! `$DATE`, `$BTIM`, `$ETIM`, `$LAST_MODIFIED` to ISO-8601 (no time zone: FCS records none).

use openreadout_core::time::month_from_abbrev;

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// A calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FcsDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl FcsDate {
    pub fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// The following day.
    pub fn next_day(self) -> FcsDate {
        if self.day < days_in_month(self.year, self.month) {
            FcsDate {
                day: self.day + 1,
                ..self
            }
        } else if self.month < 12 {
            FcsDate {
                month: self.month + 1,
                day: 1,
                ..self
            }
        } else {
            FcsDate {
                year: self.year + 1,
                month: 1,
                day: 1,
            }
        }
    }
}

/// Parse `dd-mmm-yyyy` (the spec form), `dd-mmm-yy` (two-digit years: < 70 → 20yy, else 19yy)
/// and `yyyy-mmm-dd` (seen in Miltenyi MACSQuantify files). Month names are case-insensitive.
pub fn parse_date(v: &str) -> Option<FcsDate> {
    let parts: Vec<&str> = v.trim().split('-').map(str::trim).collect();
    if parts.len() != 3 {
        return None;
    }
    let (d, m, y) = if parts[0].len() == 4 {
        (parts[2], parts[1], parts[0])
    } else {
        (parts[0], parts[1], parts[2])
    };
    let day: u32 = d.parse().ok()?;
    let month = month_from_abbrev(m.trim())?;
    let mut year: i32 = y.parse().ok()?;
    if y.len() == 2 {
        year += if year < 70 { 2000 } else { 1900 };
    } else if y.len() != 4 {
        return None;
    }
    (1..=days_in_month(year, month))
        .contains(&day)
        .then_some(FcsDate { year, month, day })
}

/// Parse `hh:mm:ss`, `hh:mm:ss.cc` (FCS 3.1, hundredths) or `hh:mm:ss:tt` (FCS 2.0/3.0, sixtieths).
/// Returns the time as `hh:mm:ss[.ff]`. A fourth colon field that is not a valid sixtieth
/// (three digits, or 60 and above) is dropped rather than guessed at.
pub fn parse_time(value: &str) -> Option<String> {
    let text = value.trim();
    let (main, fraction) = match text.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (text, None),
    };
    let fields: Vec<&str> = main.split(':').collect();
    if fields.len() < 3 || fields.len() > 4 {
        return None;
    }
    let hours: u32 = fields[0].parse().ok()?;
    let minutes: u32 = fields[1].parse().ok()?;
    let seconds: u32 = fields[2].parse().ok()?;
    if hours > 23 || minutes > 59 || seconds > 60 {
        return None;
    }
    let mut out = format!("{hours:02}:{minutes:02}:{seconds:02}");
    if let Some(digits) = fraction {
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        out.push('.');
        out.push_str(digits);
    } else if fields.len() == 4 {
        let sixtieths = fields[3];
        if sixtieths.len() <= 2
            && let Ok(n) = sixtieths.parse::<u32>()
            && n < 60
        {
            out.push_str(&format!(".{:02}", n * 100 / 60));
        }
    }
    Some(out)
}

/// `$DATE` + `$BTIM`/`$ETIM` → (start, end) ISO timestamps; the end rolls to the next day when it
/// is earlier than the start.
pub fn acquisition_span(
    date: Option<&str>,
    btim: Option<&str>,
    etim: Option<&str>,
) -> (Option<String>, Option<String>) {
    let Some(d) = date.and_then(parse_date) else {
        return (None, None);
    };
    let b = btim.and_then(parse_time);
    let e = etim.and_then(parse_time);
    let start = b.as_ref().map(|t| format!("{}T{t}", d.iso()));
    let end = e.as_ref().map(|t| {
        let day = match &b {
            Some(bt) if t.as_str() < bt.as_str() => d.next_day(),
            _ => d,
        };
        format!("{}T{t}", day.iso())
    });
    (start, end)
}

/// `$LAST_MODIFIED`: `dd-mmm-yyyy hh:mm:ss[.cc]`.
pub fn parse_timestamp(v: &str) -> Option<String> {
    let (d, t) = v.trim().split_once(' ')?;
    Some(format!("{}T{}", parse_date(d)?.iso(), parse_time(t)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            parse_date("01-OCT-1994").map(FcsDate::iso).as_deref(),
            Some("1994-10-01")
        );
        assert_eq!(
            parse_date("22-Jul-2020").map(FcsDate::iso).as_deref(),
            Some("2020-07-22")
        );
        assert_eq!(
            parse_date("12-JAN-2022 ").map(FcsDate::iso).as_deref(),
            Some("2022-01-12")
        );
        assert_eq!(
            parse_date("22-Sep-13").map(FcsDate::iso).as_deref(),
            Some("2013-09-22")
        );
        assert_eq!(
            parse_date("18-May-99").map(FcsDate::iso).as_deref(),
            Some("1999-05-18")
        );
        assert_eq!(
            parse_date("2014-Sep-26").map(FcsDate::iso).as_deref(),
            Some("2014-09-26")
        );
        assert_eq!(parse_date("31-FEB-2020"), None);
        assert_eq!(parse_date("yesterday"), None);
    }

    #[test]
    fn times() {
        assert_eq!(parse_time("14:22:10.47").as_deref(), Some("14:22:10.47"));
        assert_eq!(parse_time("09:44:26").as_deref(), Some("09:44:26"));
        assert_eq!(parse_time("17:29:39:51").as_deref(), Some("17:29:39.85"));
        assert_eq!(parse_time("09:42:05:509").as_deref(), Some("09:42:05"));
        assert_eq!(parse_time("25:00:00"), None);
    }

    #[test]
    fn spans_roll_over_midnight() {
        let (s, e) = acquisition_span(Some("31-DEC-2019"), Some("23:59:00"), Some("00:01:00"));
        assert_eq!(s.as_deref(), Some("2019-12-31T23:59:00"));
        assert_eq!(e.as_deref(), Some("2020-01-01T00:01:00"));
        assert_eq!(
            parse_timestamp("25-SEP-2008 15:22:10.47").as_deref(),
            Some("2008-09-25T15:22:10.47")
        );
        assert_eq!(
            parse_timestamp("2014-Sep-26 13:41:05").as_deref(),
            Some("2014-09-26T13:41:05")
        );
    }
}
