//! FluoView settings files (`.oif`, `.pty`, `OibInfo.txt`, `.roi`, `.lut`): INI-style text,
//! UTF-16LE with a byte-order mark (or UTF-8), `[Section]` headers and `Key=Value` lines.
//! A `[ColorLUTData]` section, when present, is binary and ends the text. Layout and vocabulary:
//! `docs/formats/oif.md` § Settings files.

use openreadout_core::bytes::{find, utf16le};
use serde_json::{Map, Value};

/// Largest settings file parsed (main files are ~60 KB; property files ~5 KB).
pub const MAX_SETTINGS_BYTES: u64 = 16 << 20;

/// One `[Section]` with its keys in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    /// Name between the brackets.
    pub name: String,
    /// `(key, raw value)` pairs; values keep their quotes (see [`unquote`]).
    pub entries: Vec<(String, String)>,
}

/// A parsed settings file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// Sections in file order.
    pub sections: Vec<Section>,
    /// Byte length of a binary `[ColorLUTData]` tail, when the file has one.
    pub lut_bytes: usize,
}

/// Decode settings text: UTF-16LE when the file starts with a byte-order mark `FF FE` (or when
/// the second byte is 0, as in `[\0`), UTF-8 otherwise. `None` when it is neither.
fn decode(bytes: &[u8]) -> Option<(String, usize)> {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.get(1) == Some(&0);
    if utf16 {
        let body = bytes.strip_prefix(&[0xFF, 0xFE]).unwrap_or(bytes);
        // the binary LUT tail starts after the UTF-16 line "[ColorLUTData]\r\n"
        let marker: Vec<u8> = "[ColorLUTData]\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let (text, lut) = match find(body, &marker) {
            Some(i) => (&body[..i], body.len() - i - marker.len()),
            None => (body, 0),
        };
        Some((utf16le(text), lut))
    } else if bytes.first() == Some(&b'[') {
        let marker = b"[ColorLUTData]\r\n";
        let (text, lut) = match find(bytes, marker) {
            Some(i) => (&bytes[..i], bytes.len() - i - marker.len()),
            None => (bytes, 0),
        };
        Some((String::from_utf8_lossy(text).into_owned(), lut))
    } else {
        None
    }
}

/// Strip one pair of matching single or double quotes.
pub fn unquote(v: &str) -> &str {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return &v[1..v.len() - 1];
        }
    }
    v
}

/// Parse a number as FluoView writes it. Besides plain decimals, some files (written on systems
/// whose locale uses a decimal comma) hold values such as `0,207.0` and `211,761.0`, which mean
/// 0.207 and 211.761: a decimal comma followed by a spurious `.0` (inferred from corpus file
/// `zenodo7080902` whose pixel size 0.207 µm × 1023 = 211.761 µm, see `docs/formats/oif.md`).
pub fn number(v: &str) -> Option<f64> {
    let v = unquote(v);
    if let Ok(x) = v.parse::<f64>() {
        return x.is_finite().then_some(x);
    }
    if let Some((int, frac)) = v.split_once(',') {
        let frac = frac.strip_suffix(".0").unwrap_or(frac);
        if !int.is_empty()
            && !frac.is_empty()
            && int
                .trim_start_matches('-')
                .bytes()
                .all(|b| b.is_ascii_digit())
            && frac.bytes().all(|b| b.is_ascii_digit())
        {
            return format!("{int}.{frac}").parse().ok();
        }
    }
    None
}

impl Settings {
    /// Parse a settings file; `None` when the bytes are not settings text.
    pub fn parse(bytes: &[u8]) -> Option<Settings> {
        let (text, lut_bytes) = decode(bytes)?;
        let mut sections: Vec<Section> = Vec::new();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                sections.push(Section {
                    name: line[1..line.len() - 1].to_string(),
                    entries: Vec::new(),
                });
            } else if let Some((k, v)) = line.split_once('=')
                && let Some(s) = sections.last_mut()
            {
                s.entries.push((k.trim_end().to_string(), v.to_string()));
            }
        }
        if sections.is_empty() {
            return None;
        }
        Some(Settings {
            sections,
            lut_bytes,
        })
    }

    /// The first section with this name.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// Raw value of `key` in `section` (quotes kept).
    pub fn raw(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)?.raw(key)
    }

    /// Unquoted text value; empty strings are `None`.
    pub fn text(&self, section: &str, key: &str) -> Option<String> {
        self.section(section)?.text(key)
    }

    /// Numeric value (see [`number`]).
    pub fn number(&self, section: &str, key: &str) -> Option<f64> {
        self.section(section)?.number(key)
    }

    /// The whole file as `{section: {key: value}}`, values typed as integers, floats or text the
    /// way they are written (quotes removed). Repeated section names get a `#2`, `#3` suffix.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        for s in &self.sections {
            let mut m = Map::new();
            for (k, v) in &s.entries {
                m.insert(k.clone(), typed(v));
            }
            let mut name = s.name.clone();
            let mut n = 1;
            while out.contains_key(&name) {
                n += 1;
                name = format!("{}#{n}", s.name);
            }
            out.insert(name, Value::Object(m));
        }
        if self.lut_bytes > 0 {
            out.insert("ColorLUTData".into(), Value::from(self.lut_bytes));
        }
        Value::Object(out)
    }
}

impl Section {
    /// Raw value of `key` (quotes kept).
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Unquoted text value; empty strings are `None`.
    pub fn text(&self, key: &str) -> Option<String> {
        self.raw(key)
            .map(unquote)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// Numeric value (see [`number`]).
    pub fn number(&self, key: &str) -> Option<f64> {
        self.raw(key).and_then(number)
    }
}

/// A settings value as JSON: quoted → text, otherwise integer, float or text.
fn typed(v: &str) -> Value {
    let t = v.trim();
    if t.len() >= 2 && (t.starts_with('"') || t.starts_with('\'')) {
        return Value::from(unquote(t));
    }
    if let Ok(i) = t.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = t.parse::<f64>()
        && f.is_finite()
    {
        return Value::from(f);
    }
    Value::from(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut b = vec![0xFF, 0xFE];
        b.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        b
    }

    #[test]
    fn parses_utf16_and_utf8() {
        let text = "[File Info]\r\nDataName=\"a.oib\"\r\n[Axis 0 Parameters Common]\r\nMaxSize=640\r\nEndPosition=635.166\r\n";
        for bytes in [utf16(text), text.as_bytes().to_vec()] {
            let s = Settings::parse(&bytes).unwrap();
            assert_eq!(s.text("File Info", "DataName").as_deref(), Some("a.oib"));
            assert_eq!(s.number("Axis 0 Parameters Common", "MaxSize"), Some(640.0));
            assert_eq!(s.to_json()["Axis 0 Parameters Common"]["MaxSize"], 640);
        }
        assert!(Settings::parse(b"\x00\x01garbage").is_none());
    }

    #[test]
    fn lut_tail_is_cut() {
        let mut b = utf16("[LUT]\r\nX=1\r\n[ColorLUTData]\r\n");
        b.extend([1, 2, 3, 4, 0, 0, 0, 0]);
        let s = Settings::parse(&b).unwrap();
        assert_eq!(s.lut_bytes, 8);
        assert_eq!(s.sections.len(), 1);
    }

    #[test]
    fn locale_comma_numbers() {
        assert_eq!(number("0,207.0"), Some(0.207));
        assert_eq!(number("211,761.0"), Some(211.761));
        assert_eq!(number("\"1.5\""), Some(1.5));
        assert_eq!(number("-96840.0"), Some(-96840.0));
        assert_eq!(number("abc"), None);
        assert_eq!(number("1,2,3"), None);
    }
}
