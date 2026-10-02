//! Bruker parameter files (`acqus`, `acqu2s`, `procs`, `proc2s`, …): JCAMP-DX-style text with
//! `##$NAME= value` records. Syntax as documented by nmrglue (`bruker.read_jcamp`); see
//! `docs/formats/bruker-nmr.md`.

use serde_json::{Value, json};

use crate::text::decode_text;

/// One parameter value, as written.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    /// A whole number without a decimal point or exponent.
    Int(i64),
    /// Any other number (`1e-06`, `600.13`).
    Float(f64),
    /// A `<…>` string (brackets removed) or a bare word that is not a number.
    Text(String),
    /// A `(0..n)` array.
    List(Vec<ParamValue>),
}

impl ParamValue {
    /// The value as a number, if it is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ParamValue::Int(i) => Some(*i as f64),
            ParamValue::Float(f) => Some(*f),
            ParamValue::Text(_) | ParamValue::List(_) => None,
        }
    }

    /// The value as an integer, if it is a whole number.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ParamValue::Int(i) => Some(*i),
            ParamValue::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
            _ => None,
        }
    }

    /// The value as text, if it is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ParamValue::Text(s) => Some(s),
            _ => None,
        }
    }

    /// JSON with names and values untouched (numbers stay numbers, non-finite floats become strings).
    pub fn to_json(&self) -> Value {
        match self {
            ParamValue::Int(i) => json!(i),
            ParamValue::Float(f) if f.is_finite() => json!(f),
            ParamValue::Float(f) => json!(f.to_string()),
            ParamValue::Text(s) => json!(s),
            ParamValue::List(v) => Value::Array(v.iter().map(ParamValue::to_json).collect()),
        }
    }
}

/// A parsed parameter file.
#[derive(Debug, Clone, Default)]
pub struct ParamFile {
    /// File name (`acqus`, `procs`, …).
    pub name: String,
    /// Text of `##TITLE=` (e.g. `Parameter file, TopSpin 4.3.0`).
    pub title: Option<String>,
    /// Records whose label does not start with `$` (`##JCAMPDX=`, `##ORIGIN=`, …), in file order.
    pub header: Vec<(String, String)>,
    /// `$$` comment lines, in file order.
    pub comments: Vec<String>,
    /// `##$NAME=` records, in file order.
    pub params: Vec<(String, ParamValue)>,
    /// Syntax problems (stray lines, short arrays, unterminated strings).
    pub issues: Vec<String>,
    /// True when `##END=` was found.
    pub ended: bool,
    /// True when the file is not valid UTF-8 and was read as Latin-1.
    pub latin1: bool,
    /// Size of the file in bytes.
    pub size: u64,
}

impl ParamFile {
    /// Look up a parameter by its exact (case-sensitive) name.
    pub fn get(&self, name: &str) -> Option<&ParamValue> {
        self.params.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// A numeric parameter.
    pub fn float(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(ParamValue::as_f64)
    }

    /// An integer parameter.
    pub fn int(&self, name: &str) -> Option<i64> {
        self.get(name).and_then(ParamValue::as_i64)
    }

    /// A string parameter, trimmed; `None` when absent or empty.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name)
            .and_then(ParamValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// Software and version from `##TITLE= Parameter file, TopSpin 4.3.0` (also
    /// `XWIN-NMR  Version 3.1`, `TOPSPIN\t\tVersion 1.3`).
    pub fn software(&self) -> Option<(String, Option<String>)> {
        let t = self.title.as_deref()?;
        let rest = t.split_once(',').map_or(t, |(_, r)| r).trim();
        if rest.is_empty() {
            return None;
        }
        let words: Vec<&str> = rest
            .split_whitespace()
            .filter(|w| !w.eq_ignore_ascii_case("version"))
            .collect();
        let name = (*words.first()?).to_string();
        let version = (words.len() > 1).then(|| words[1..].join(" "));
        Some((name, version))
    }

    /// All records as JSON: `{title, header, comments, parameters}`.
    pub fn to_json(&self) -> Value {
        let mut params = serde_json::Map::new();
        for (k, v) in &self.params {
            params.insert(k.clone(), v.to_json());
        }
        let mut header = serde_json::Map::new();
        for (k, v) in &self.header {
            header.insert(k.clone(), json!(v));
        }
        json!({
            "title": self.title,
            "header": header,
            "comments": self.comments,
            "parameters": params,
        })
    }
}

fn parse_scalar(text: &str) -> ParamValue {
    let t = text.trim();
    if let Ok(i) = t.parse::<i64>() {
        return ParamValue::Int(i);
    }
    let numeric_chars = t
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e' | b'E'));
    if numeric_chars
        && t.bytes().any(|b| b.is_ascii_digit())
        && let Ok(f) = t.parse::<f64>()
    {
        return ParamValue::Float(f);
    }
    if matches!(t, "inf" | "-inf" | "nan" | "NaN")
        && let Ok(f) = t.parse::<f64>()
    {
        return ParamValue::Float(f);
    }
    ParamValue::Text(t.to_string())
}

/// Split array text into items: `<…>` strings (which may contain blanks) and blank-separated words.
fn tokenize_items(text: &str, out: &mut Vec<ParamValue>, open: &mut Option<String>) {
    let mut chars = text.char_indices().peekable();
    if let Some(acc) = open.as_mut() {
        // continuing a string that started on a previous line
        if let Some(end) = text.find('>') {
            acc.push('\n');
            acc.push_str(&text[..end]);
            out.push(ParamValue::Text(std::mem::take(acc)));
            *open = None;
            tokenize_items(&text[end + 1..], out, open);
        } else {
            acc.push('\n');
            acc.push_str(text);
        }
        return;
    }
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        if c == '<' {
            if let Some(len) = text[i + 1..].find('>') {
                out.push(ParamValue::Text(text[i + 1..i + 1 + len].to_string()));
                let end = i + 1 + len + 1;
                while chars.peek().is_some_and(|&(j, _)| j < end) {
                    chars.next();
                }
            } else {
                *open = Some(text[i + 1..].to_string());
                return;
            }
            continue;
        }
        let start = i;
        let mut end = text.len();
        while let Some(&(j, d)) = chars.peek() {
            if d.is_whitespace() || d == '<' {
                end = j;
                break;
            }
            chars.next();
        }
        out.push(parse_scalar(&text[start..end]));
    }
}

/// Parse `(a..b)` at the start of `text`; returns the item count and the rest of the text.
fn array_header(text: &str) -> Option<(usize, &str)> {
    let t = text.trim_start();
    let inner = t.strip_prefix('(')?;
    let close = inner.find(')')?;
    let (a, b) = inner[..close].split_once("..")?;
    let a: i64 = a.trim().parse().ok()?;
    let b: i64 = b.trim().parse().ok()?;
    let n = usize::try_from(b.checked_sub(a)?.checked_add(1)?).ok()?;
    Some((n, &inner[close + 1..]))
}

/// Parse a parameter file. Never fails: syntax problems are collected in `issues`.
pub fn parse_param_file(name: &str, bytes: &[u8]) -> ParamFile {
    let (text, latin1) = decode_text(bytes);
    let mut pf = ParamFile {
        name: name.to_string(),
        latin1,
        size: bytes.len() as u64,
        ..ParamFile::default()
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim_end_matches('\r');
        i += 1;
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(c) = trimmed.strip_prefix("$$") {
            pf.comments.push(c.trim().to_string());
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("##") else {
            pf.issues.push(format!(
                "line {i}: text outside a record: {}",
                clip(trimmed)
            ));
            continue;
        };
        let Some((label, value)) = rest.split_once('=') else {
            pf.issues.push(format!("line {i}: record without '='"));
            continue;
        };
        if label.trim() == "END" {
            pf.ended = true;
            break;
        }
        let Some(pname) = label.strip_prefix('$') else {
            let (v, _) = crate::text::split_comment(value);
            let v = v.trim().to_string();
            if label.trim() == "TITLE" && pf.title.is_none() {
                pf.title = Some(v.clone());
            }
            pf.header.push((label.trim().to_string(), v));
            continue;
        };
        let pname = pname.trim().to_string();
        let value = value.trim_start();
        if let Some((n, tail)) = array_header(value) {
            let mut items = Vec::new();
            let mut open = None;
            tokenize_items(tail, &mut items, &mut open);
            while (items.len() < n || open.is_some())
                && i < lines.len()
                && !lines[i].trim_start().starts_with("##")
            {
                let l = lines[i].trim_end_matches('\r');
                i += 1;
                if l.trim_start().starts_with("$$") && open.is_none() {
                    pf.comments.push(l.trim_start()[2..].trim().to_string());
                    continue;
                }
                tokenize_items(l, &mut items, &mut open);
            }
            if open.is_some() {
                pf.issues
                    .push(format!("{pname}: unterminated <string> in array"));
            }
            if items.len() != n {
                pf.issues.push(format!(
                    "{pname}: array declares {n} values, found {}",
                    items.len()
                ));
            }
            pf.params.push((pname, ParamValue::List(items)));
        } else if let Some(s) = value.strip_prefix('<') {
            let mut acc = String::new();
            let mut cur = s.to_string();
            loop {
                if let Some(end) = cur.rfind('>') {
                    acc.push_str(&cur[..end]);
                    break;
                }
                acc.push_str(&cur);
                if i >= lines.len() || lines[i].trim_start().starts_with("##") {
                    pf.issues.push(format!("{pname}: unterminated <string>"));
                    break;
                }
                acc.push('\n');
                cur = lines[i].trim_end_matches('\r').to_string();
                i += 1;
            }
            pf.params.push((pname, ParamValue::Text(acc)));
        } else {
            let (v, _) = crate::text::split_comment(value);
            pf.params.push((pname, parse_scalar(v)));
        }
    }
    pf
}

fn clip(s: &str) -> String {
    if s.chars().count() > 40 {
        format!("{}…", s.chars().take(40).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "##TITLE= Parameter file, TopSpin 3.7.0\n##JCAMPDX= 5.0\n##DATATYPE= Parameter Values\n$$ 2025-02-20 19:00:25.587 -0600  user@host\n##$AMP= (0..3)\n100 100\n100 100\n##$AQ_mod= 3\n##$AUNM= <au_zg>\n##$GPNAM= (0..2)\n<sine.100> <> <two words>\n##$D= (0..1)\n3e-06 1\n##$NUC1= <1H>\n##$SW_h= 11061.9469026549\n##$TD= 1300\n##$NC= -2\n##$ML= <line one\nline two>\n##$scaledByNS= no\n##END=\n";

    #[test]
    fn parses_records() {
        let p = parse_param_file("acqus", SAMPLE.as_bytes());
        assert!(p.ended, "{:?}", p.issues);
        assert!(p.issues.is_empty(), "{:?}", p.issues);
        assert_eq!(p.int("TD"), Some(1300));
        assert_eq!(p.int("NC"), Some(-2));
        assert_eq!(p.float("SW_h"), Some(11_061.946_902_654_9));
        assert_eq!(p.text("NUC1"), Some("1H"));
        assert_eq!(p.text("AUNM"), Some("au_zg"));
        assert_eq!(p.text("scaledByNS"), Some("no"));
        assert_eq!(
            p.get("ML"),
            Some(&ParamValue::Text("line one\nline two".into()))
        );
        assert_eq!(
            p.get("GPNAM"),
            Some(&ParamValue::List(vec![
                ParamValue::Text("sine.100".into()),
                ParamValue::Text(String::new()),
                ParamValue::Text("two words".into())
            ]))
        );
        assert_eq!(
            p.get("D"),
            Some(&ParamValue::List(vec![
                ParamValue::Float(3e-6),
                ParamValue::Int(1)
            ]))
        );
        match p.get("AMP") {
            Some(ParamValue::List(v)) => assert_eq!(v.len(), 4),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            p.software(),
            Some(("TopSpin".to_string(), Some("3.7.0".to_string())))
        );
        assert_eq!(p.comments.len(), 1);
    }

    #[test]
    fn titles_and_problems() {
        let p = parse_param_file(
            "acqus",
            b"##TITLE= Parameter file, XWIN-NMR\t\tVersion 3.1\n##$X= (0..2)\n1 2\n",
        );
        assert_eq!(
            p.software(),
            Some(("XWIN-NMR".to_string(), Some("3.1".to_string())))
        );
        assert!(!p.ended);
        assert!(p.issues.iter().any(|m| m.contains("declares 3")));
        let q = parse_param_file(
            "procs",
            b"##TITLE= Parameter file, TOPSPIN\t\tVersion 1.3\n##$S= <abc\n",
        );
        assert_eq!(
            q.software(),
            Some(("TOPSPIN".to_string(), Some("1.3".to_string())))
        );
        assert!(q.issues.iter().any(|m| m.contains("unterminated")));
    }
}
