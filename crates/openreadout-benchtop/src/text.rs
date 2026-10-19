//! Text helpers shared by the export readers: decoding, splitting delimited lines, numbers with a
//! decimal comma, dates and concentration units.

use openreadout_core::bytes::{Endian, latin1, utf16};

/// Decode an export: UTF-16 with a byte-order mark, UTF-8 (with or without one), else Latin-1
/// (Windows exports write `µ` as the single byte 0xB5).
pub(crate) fn decode(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, Endian::Little);
    }
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, Endian::Big);
    }
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(b),
    }
}

/// Split one line on `delim`, honouring double quotes (`""` inside quotes is a quote).
pub(crate) fn split(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' && cur.trim().is_empty() {
            cur.clear();
            quoted = true;
        } else if c == delim {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// A number as an export writes it: `1.5`, `1,5` (decimal comma), `1.449E+0`, `2.45E6`,
/// `274 000` (a thousands space). `None` for anything else (`NaN`, `ERROR`, empty).
pub(crate) fn number(s: &str) -> Option<f64> {
    let t = s.trim().trim_matches('"').trim();
    if t.is_empty() {
        return None;
    }
    let mut c: String = t
        .chars()
        .filter(|c| !matches!(c, '\u{a0}' | '\u{202f}' | ' '))
        .collect();
    // a decimal comma: one comma and no point
    if c.matches(',').count() == 1 && !c.contains('.') {
        c = c.replace(',', ".");
    }
    if !c
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+' | 'e' | 'E'))
    {
        return None;
    }
    c.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A date and time as the exports write it, as ISO 8601 (local time, with the offset when the
/// export gives one). `month_first`: how to read an ambiguous `a/b/yyyy` (both ≤ 12).
///
/// Seen: `06/16/2022 04:38:28 PM(-07:00)`, `5/26/2020` + `3:36 PM`, `7/19/2023 3:27:29 PM`,
/// `13/12/2022 04:41:09 PM`, `11/09/2023 14:19`, `2023-12-12`, `5/14/2024 5:04 PM`.
#[allow(clippy::many_single_char_names)] // y, m, d, h: the parts of a date
pub(crate) fn datetime(s: &str, month_first: bool) -> Option<(String, bool)> {
    let s = s.trim();
    let (main, offset) = match s.find('(') {
        Some(i) => (
            s[..i].trim(),
            Some(s[i + 1..].trim_end_matches(')').trim().to_string()),
        ),
        None => (s, None),
    };
    let mut parts = main.split_whitespace();
    let date = parts.next()?;
    let time = parts.next();
    let ampm = parts.next();
    let (y, m, d, ambiguous) = if date.contains('-') && date.len() == 10 {
        let mut it = date.split('-');
        let y: i32 = it.next()?.parse().ok()?;
        let m: u32 = it.next()?.parse().ok()?;
        let d: u32 = it.next()?.parse().ok()?;
        (y, m, d, false)
    } else {
        let sep = if date.contains('/') { '/' } else { '.' };
        let f: Vec<&str> = date.split(sep).collect();
        let [a, b, c] = f[..] else { return None };
        if a.len() == 4 {
            // year first (`2019/08/06`): always year, month, day
            let (y, m, d): (i32, u32, u32) = (a.parse().ok()?, b.parse().ok()?, c.parse().ok()?);
            return finish(y, m, d, false, time, ampm, offset);
        }
        let (a, b, y): (u32, u32, i32) = (a.parse().ok()?, b.parse().ok()?, c.parse().ok()?);
        let y = if y < 100 { 2000 + y } else { y };
        if sep == '.' || a > 12 {
            (y, b, a, false)
        } else if b > 12 {
            (y, a, b, false)
        } else if month_first {
            (y, a, b, a != b)
        } else {
            (y, b, a, a != b)
        }
    };
    finish(y, m, d, ambiguous, time, ampm, offset)
}

/// The ISO text of a date and an optional time of day (`ampm`: `AM`/`PM`; `offset`: `+hh:mm`).
fn finish(
    y: i32,
    m: u32,
    d: u32,
    ambiguous: bool,
    time: Option<&str>,
    ampm: Option<&str>,
    offset: Option<String>,
) -> Option<(String, bool)> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1900..=2200).contains(&y) {
        return None;
    }
    let mut iso = format!("{y:04}-{m:02}-{d:02}");
    if let Some(t) = time {
        let f: Vec<&str> = t.split(':').collect();
        let h: u32 = f.first()?.parse().ok()?;
        let mi: u32 = f.get(1).and_then(|x| x.parse().ok()).unwrap_or(0);
        let se: u32 = f
            .get(2)
            .and_then(|x| x.split('.').next())
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        let h = match ampm.map(str::to_ascii_uppercase).as_deref() {
            Some("PM") if h < 12 => h + 12,
            Some("AM") if h == 12 => 0,
            _ => h,
        };
        if h > 23 || mi > 59 || se > 60 {
            return None;
        }
        iso.push_str(&format!("T{h:02}:{mi:02}:{se:02}"));
        if let Some(o) =
            offset.filter(|o| o.len() == 6 && (o.starts_with('-') || o.starts_with('+')))
        {
            iso.push_str(&o);
        }
    }
    Some((iso, ambiguous))
}

/// A concentration unit as written (`ng/ul`, `ng/µL`, `mg/ml`, `ug/mL`) in our spelling, with
/// its factor to ng/µL (= µg/mL = mg/L). `None` for anything else.
pub(crate) fn concentration_unit(s: &str) -> Option<(String, f64)> {
    let t = s.trim().replace('μ', "µ");
    let (mass, vol) = t.split_once('/')?;
    let mass_f = match mass.trim() {
        "pg" => 1e-3,
        "ng" => 1.0,
        "ug" | "µg" => 1e3,
        "mg" => 1e6,
        "g" => 1e9,
        _ => return None,
    };
    let vol_lc = vol.trim().to_ascii_lowercase();
    let vol_f = match vol_lc.as_str() {
        "ul" | "µl" => 1.0,
        "ml" => 1e3,
        "l" => 1e6,
        _ => return None,
    };
    let mass_s = mass.trim().replace("ug", "µg");
    let vol_s = match vol_lc.as_str() {
        "ul" | "µl" => "µL",
        "ml" => "mL",
        _ => "L",
    };
    Some((format!("{mass_s}/{vol_s}"), mass_f / vol_f))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number("1.5"), Some(1.5));
        assert_eq!(number("-0,4122"), Some(-0.4122));
        assert_eq!(number("1.449E+0"), Some(1.449));
        assert_eq!(number("274\u{a0}000"), Some(274_000.0));
        assert_eq!(number("NaN"), None);
        assert_eq!(number("ERROR"), None);
        assert_eq!(number(""), None);
        assert_eq!(number("10.0-20.0"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(
            datetime("06/16/2022 04:38:28 PM(-07:00)", true).unwrap().0,
            "2022-06-16T16:38:28-07:00"
        );
        assert_eq!(
            datetime("13/12/2022 04:41:09 PM", true).unwrap().0,
            "2022-12-13T16:41:09"
        );
        assert_eq!(datetime("2023-12-12", true).unwrap().0, "2023-12-12");
        assert_eq!(
            datetime("2019/08/06 12:31:14", false).unwrap(),
            ("2019-08-06T12:31:14".to_string(), false)
        );
        let (iso, amb) = datetime("04/11/2023 1:06:59 AM", false).unwrap();
        assert_eq!(iso, "2023-11-04T01:06:59");
        assert!(amb);
        assert!(datetime("hello", true).is_none());
        assert!(datetime("13/13/2023", true).is_none());
    }

    #[test]
    fn units_and_splitting() {
        assert_eq!(concentration_unit("ng/ul").unwrap(), ("ng/µL".into(), 1.0));
        assert_eq!(concentration_unit("ng/mL").unwrap().1, 1e-3);
        assert_eq!(concentration_unit("mg/ml").unwrap().0, "mg/mL");
        assert!(concentration_unit("cells/ml").is_none());
        assert_eq!(split("a,\"b,c\",d", ','), vec!["a", "b,c", "d"]);
        assert_eq!(split("x\t\ty", '\t'), vec!["x", "", "y"]);
        assert_eq!(decode(&[0xFF, 0xFE, b'a', 0, b'b', 0]), "ab");
        assert_eq!(decode(b"\xb5L"), "µL");
    }
}
