//! `Slidedat.ini`: `[SECTION]` headers and `KEY = VALUE` lines, UTF-8 (optionally with a
//! byte-order mark) or UTF-16LE with a byte-order mark, CRLF or LF line ends. Sections and keys
//! keep their file order; a repeated key in one section keeps its first value.

use openreadout_core::bytes::utf16le;
use serde_json::{Map, Value};

/// One `[SECTION]` of the settings file, keys in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IniSection {
    /// Section name without brackets.
    pub name: String,
    /// `(key, value)` pairs, trimmed.
    pub entries: Vec<(String, String)>,
}

impl IniSection {
    /// Value of `key` (exact, case-sensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// `key` parsed as a number (decimal point or comma).
    pub fn f64(&self, key: &str) -> Option<f64> {
        parse_f64(self.get(key)?)
    }

    /// `key` parsed as an integer.
    pub fn i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }
}

/// A parsed settings file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ini {
    /// Sections in file order (a repeated section header continues the first one).
    pub sections: Vec<IniSection>,
}

/// A number written with a decimal point or a decimal comma; finite values only.
pub fn parse_f64(s: &str) -> Option<f64> {
    let t = s.trim();
    let v: f64 = t.parse().or_else(|_| t.replace(',', ".").parse()).ok()?;
    v.is_finite().then_some(v)
}

/// Decode the file's text: UTF-16LE with a byte-order mark, else UTF-8 (lossy, BOM dropped).
pub fn decode_text(data: &[u8]) -> String {
    if let Some(rest) = data.strip_prefix(&[0xFF, 0xFE]) {
        return utf16le(rest);
    }
    let d = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    String::from_utf8_lossy(d).into_owned()
}

impl Ini {
    /// Parse settings text. Lines outside any section, comments (`;`, `#`) and lines without
    /// `=` are skipped.
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        let mut current: Option<usize> = None;
        for raw in text.lines() {
            let line = raw.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim();
                current = Some(
                    if let Some(i) = ini.sections.iter().position(|s| s.name == name) {
                        i
                    } else {
                        ini.sections.push(IniSection {
                            name: name.to_string(),
                            entries: Vec::new(),
                        });
                        ini.sections.len() - 1
                    },
                );
                continue;
            }
            let (Some(i), Some((k, v))) = (current, line.split_once('=')) else {
                continue;
            };
            let s = &mut ini.sections[i];
            let k = k.trim();
            if !k.is_empty() && s.get(k).is_none() {
                s.entries.push((k.to_string(), v.trim().to_string()));
            }
        }
        ini
    }

    /// The section named `name`.
    pub fn section(&self, name: &str) -> Option<&IniSection> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// `[section] key`.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)?.get(key)
    }

    /// Every section as a JSON object of strings, in file order.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        for s in &self.sections {
            let mut o = Map::new();
            for (k, v) in &s.entries {
                o.insert(k.clone(), Value::String(v.clone()));
            }
            m.insert(s.name.clone(), Value::Object(o));
        }
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_keys_and_encodings() {
        let text = "\u{feff}[GENERAL]\r\nSLIDE_ID = AB12\r\nIMAGENUMBER_X = 352\r\n; note\r\n[LAYER_0_LEVEL_0_SECTION]\r\nOVERLAP_X = 7,5\r\nOVERLAP_X = 9\r\n";
        let ini = Ini::parse(text);
        assert_eq!(ini.get("GENERAL", "SLIDE_ID"), Some("AB12"));
        assert_eq!(
            ini.section("GENERAL").unwrap().i64("IMAGENUMBER_X"),
            Some(352)
        );
        assert_eq!(
            ini.section("LAYER_0_LEVEL_0_SECTION")
                .unwrap()
                .f64("OVERLAP_X"),
            Some(7.5)
        );
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("[A]\nK = v\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(Ini::parse(&decode_text(&utf16)).get("A", "K"), Some("v"));
        assert_eq!(decode_text(b"\xEF\xBB\xBF[X]"), "[X]");
        assert!(parse_f64("inf").is_none());
    }
}
