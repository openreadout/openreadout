//! The 16 KiB text header shared by every Neuralynx file.

use openreadout_core::bytes::{latin1, until_nul};

/// Bytes of the text header at the start of every file.
pub const HEADER_LEN: u64 = 16_384;
/// First line written by Cheetah and Pegasus (optional in practice).
pub const HEADER_MAGIC: &str = "######## Neuralynx Data File Header";

/// One `-Key value` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderEntry {
    pub key: String,
    pub value: String,
}

/// The parsed text header.
#[derive(Debug, Clone, Default)]
pub struct TextHeader {
    /// `-Key value` pairs in file order (keys without the dash; a trailing `:` removed).
    pub entries: Vec<HeaderEntry>,
    /// `##` lines without the leading hashes.
    pub comments: Vec<String>,
    /// True when the first line is the Neuralynx header line.
    pub has_magic: bool,
    /// Bytes of text before the NUL padding.
    pub text_len: usize,
}

/// Decode header bytes: UTF-8 when valid (the µ is sometimes written as UTF-8), else Latin-1.
pub fn decode_text(b: &[u8]) -> String {
    let b = until_nul(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(b),
    }
}

fn unquote(v: &str) -> String {
    let t = v.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// Parse the header text.
pub fn parse_header(bytes: &[u8]) -> TextHeader {
    let text = decode_text(bytes);
    let mut h = TextHeader {
        text_len: text.len(),
        ..TextHeader::default()
    };
    for (i, raw) in text.split(['\n', '\r']).enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if i == 0 && line.starts_with(HEADER_MAGIC) {
            h.has_magic = true;
            continue;
        }
        if let Some(c) = line.strip_prefix("##") {
            h.comments.push(c.trim().to_string());
            continue;
        }
        if let Some(kv) = line.strip_prefix('-') {
            let (key, value) = match kv.find(char::is_whitespace) {
                Some(p) => (&kv[..p], &kv[p..]),
                None => (kv, ""),
            };
            let key = key.trim_end_matches(':').to_string();
            if key.is_empty() {
                continue;
            }
            h.entries.push(HeaderEntry {
                key,
                value: value.trim().to_string(),
            });
        }
    }
    h
}

impl TextHeader {
    /// Value of a key (case-insensitive; the µ in `DspFilterDelay_µs` may be decoded wrongly, so
    /// keys are compared on their ASCII letters).
    pub fn get(&self, key: &str) -> Option<&str> {
        let want = ascii_key(key);
        self.entries
            .iter()
            .find(|e| ascii_key(&e.key) == want)
            .map(|e| e.value.as_str())
    }

    /// Value without surrounding quotes.
    pub fn text(&self, key: &str) -> Option<String> {
        self.get(key).map(unquote).filter(|s| !s.is_empty())
    }

    /// First number of a (possibly multi-valued) numeric key.
    pub fn number(&self, key: &str) -> Option<f64> {
        self.numbers(key).into_iter().next()
    }

    /// Every whitespace-separated number of a key (`-ADBitVolts v1 v2 v3 v4`).
    pub fn numbers(&self, key: &str) -> Vec<f64> {
        self.get(key)
            .map(|v| {
                v.split_whitespace()
                    .filter_map(|t| t.parse::<f64>().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A `True`/`False` key.
    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.get(key)?.trim().to_ascii_lowercase().as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    /// Recording application and version: `-ApplicationName Cheetah "5.7.4 "` or `-CheetahRev 5.5.1`.
    pub fn application(&self) -> Option<(String, String)> {
        if let Some(v) = self.get("ApplicationName") {
            let v = v.trim();
            let (name, ver) = match v.find(char::is_whitespace) {
                Some(p) => (&v[..p], unquote(&v[p..])),
                None => (v, String::new()),
            };
            return Some((name.to_string(), ver.trim().to_string()));
        }
        self.get("CheetahRev")
            .map(|v| ("Cheetah".to_string(), v.trim().to_string()))
    }

    /// Time the file was opened, as ISO-8601 (local time; the header records no zone).
    pub fn opened_at(&self) -> Option<String> {
        if let Some(v) = self.get("TimeCreated") {
            return iso_from_slash_time(v);
        }
        self.comments
            .iter()
            .find(|c| c.starts_with("Time Opened") || c.starts_with("Date Opened"))
            .and_then(|c| iso_from_comment(c))
    }

    /// Time the file was closed, as ISO-8601.
    pub fn closed_at(&self) -> Option<String> {
        if let Some(v) = self.get("TimeClosed") {
            return iso_from_slash_time(v);
        }
        self.comments
            .iter()
            .find(|c| c.starts_with("Time Closed") || c.starts_with("Date Closed"))
            .and_then(|c| iso_from_comment(c))
    }

    /// Original file name as recorded (`-OriginalFileName` or `## File Name`).
    pub fn original_file_name(&self) -> Option<String> {
        if let Some(v) = self.text("OriginalFileName") {
            return Some(v);
        }
        self.comments.iter().find_map(|c| {
            c.strip_prefix("File Name")
                .map(|r| r.trim_start_matches(':').trim().to_string())
                .filter(|s| !s.is_empty() && s != "null")
        })
    }
}

fn ascii_key(k: &str) -> String {
    k.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// `2017/02/16 17:56:04` → `2017-02-16T17:56:04`.
fn iso_from_slash_time(v: &str) -> Option<String> {
    let mut it = v.split_whitespace();
    let d: Vec<u32> = it
        .next()?
        .split('/')
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    let t = it.next().unwrap_or("0:0:0");
    if d.len() != 3 {
        return None;
    }
    iso(d[0], d[1], d[2], t)
}

/// `Time Opened (m/d/y): 11/29/2013  (h:m:s.ms) 17:5:16.793`,
/// `Time Opened: (m/d/y): 12/11/15  At Time: 11:37:39.000`,
/// `Date Opened: (mm/dd/yyy): 12/14/2015 At Time: 15:58:32`.
fn iso_from_comment(c: &str) -> Option<String> {
    let date = c.split_whitespace().find(|t| {
        t.split('/').count() == 3 && t.chars().next().is_some_and(|x| x.is_ascii_digit())
    })?;
    let parts: Vec<u32> = date
        .split('/')
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    let time = c
        .split_whitespace()
        .rev()
        .find(|t| t.contains(':') && t.chars().next().is_some_and(|x| x.is_ascii_digit()))
        .unwrap_or("0:0:0");
    let year = if parts[2] < 100 {
        if parts[2] >= 70 {
            1900 + parts[2]
        } else {
            2000 + parts[2]
        }
    } else {
        parts[2]
    };
    iso(year, parts[0], parts[1], time)
}

fn iso(year: u32, month: u32, day: u32, time: &str) -> Option<String> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || !(1970..=2200).contains(&year) {
        return None;
    }
    let mut hms = time.split(':');
    let hour: u32 = hms.next()?.parse().ok()?;
    let minute: u32 = hms.next().unwrap_or("0").parse().ok()?;
    let sec = hms.next().unwrap_or("0");
    let (second, frac) = sec.split_once('.').unwrap_or((sec, ""));
    let second: u32 = second.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let frac: String = frac.chars().filter(char::is_ascii_digit).take(3).collect();
    let date = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
    Some(if frac.is_empty() {
        date
    } else {
        format!("{date}.{frac:0<3}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_styles() {
        let t = b"######## Neuralynx Data File Header\r\n## File Name C:\\x\\CSC1.ncs\r\n## Time Opened (m/d/y): 11/29/2013  (h:m:s.ms) 17:5:16.793\r\n-FileType: CSC \r\n\t-ADBitVolts\t3.05185e-008 \r\n-ADChannel 40 41 42 43\r\n-ApplicationName Cheetah \"5.7.4 \"\r\n-InputInverted True\r\n-DspFilterDelay_\xb5s 484\r\n\0\0\0";
        let h = parse_header(t);
        assert!(h.has_magic);
        assert_eq!(h.get("FileType"), Some("CSC"));
        assert_eq!(h.number("ADBitVolts"), Some(3.05185e-8));
        assert_eq!(h.numbers("ADChannel"), vec![40.0, 41.0, 42.0, 43.0]);
        assert_eq!(h.application(), Some(("Cheetah".into(), "5.7.4".into())));
        assert_eq!(h.flag("InputInverted"), Some(true));
        assert_eq!(h.get("DspFilterDelay_µs"), Some("484"));
        assert_eq!(h.opened_at().as_deref(), Some("2013-11-29T17:05:16.793"));
        assert_eq!(h.original_file_name().as_deref(), Some("C:\\x\\CSC1.ncs"));
    }

    #[test]
    fn dates() {
        assert_eq!(
            iso_from_comment("Time Opened: (m/d/y): 12/11/15  At Time: 11:37:39.000").as_deref(),
            Some("2015-12-11T11:37:39.000")
        );
        assert_eq!(
            iso_from_slash_time("2017/02/16 17:56:04").as_deref(),
            Some("2017-02-16T17:56:04")
        );
        assert_eq!(
            iso_from_comment("Time Opened (m/d/y): 10/4/2003  At Time: 10:3:0.578").as_deref(),
            Some("2003-10-04T10:03:00.578")
        );
    }
}
