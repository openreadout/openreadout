//! The text parameter files of Bruker EPR data: the BES3T descriptor (`.DSC`) and the ESP/WinEPR
//! parameter file (`.par`). Both are `KEY value` lines (`docs/formats/bruker-epr.md`).

use std::collections::BTreeMap;

use openreadout_core::bytes::latin1;
use serde_json::{Map, Value, json};

/// Bytes of a descriptor or parameter file read at most (real ones are a few kB).
pub(crate) const MAX_TEXT: u64 = 16 << 20;

/// A parsed BES3T descriptor: the `#DESC` and `#SPL` layers as key → value, the `#DSL` layer as
/// device → key → value (in file order), and the layer versions.
#[derive(Debug, Clone, Default)]
pub(crate) struct Descriptor {
    pub(crate) desc: BTreeMap<String, String>,
    pub(crate) spl: BTreeMap<String, String>,
    pub(crate) dsl: Vec<(String, Vec<(String, String)>)>,
    pub(crate) versions: BTreeMap<String, String>,
    /// Lines of the manipulation history layer (not interpreted).
    pub(crate) history_lines: usize,
}

/// Text of a file: UTF-8, else Latin-1.
pub(crate) fn text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(bytes),
    }
}

/// A value with surrounding blanks and one pair of `'` quotes removed.
fn unquote(v: &str) -> String {
    let v = v.trim();
    let b = v.as_bytes();
    if b.len() >= 2 && b[0] == b'\'' && b[b.len() - 1] == b'\'' && !v[1..v.len() - 1].contains('\'')
    {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

/// A comma-separated list value (`'Intensity','',''` or `D,D`), each item unquoted.
pub(crate) fn list(v: &str) -> Vec<String> {
    v.split(',').map(unquote).collect()
}

/// Split a line into its key (first blank-delimited token) and the rest.
fn key_value(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let end = line.find(char::is_whitespace).unwrap_or(line.len());
    let (k, v) = line.split_at(end);
    (!k.is_empty()).then_some((k, v))
}

/// Physical lines joined where a line ends in `\` (the value continues; `\n` in it is a newline).
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc: Option<String> = None;
    for raw in text.split(['\n', '\r']) {
        let line = raw;
        if let Some(stripped) = line.strip_suffix('\\') {
            acc.get_or_insert_with(String::new).push_str(stripped);
            continue;
        }
        match acc.take() {
            Some(mut a) => {
                a.push_str(line);
                out.push(a.replace("\\n", "\n"));
            }
            None => out.push(line.to_string()),
        }
    }
    if let Some(a) = acc {
        out.push(a.replace("\\n", "\n"));
    }
    out
}

/// True when `head` is the start of a BES3T descriptor (`#DESC` after optional blanks or comment
/// lines).
pub(crate) fn is_descriptor(head: &[u8]) -> bool {
    let t = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    for line in t.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('*') {
            continue;
        }
        return l.starts_with("#DESC");
    }
    false
}

/// Parse a BES3T descriptor.
pub(crate) fn parse_descriptor(text: &str) -> Descriptor {
    let mut d = Descriptor::default();
    let mut layer = String::new();
    let mut device: Option<usize> = None;
    let mut in_history = false;
    for line in logical_lines(text) {
        let t = line.trim();
        if t.is_empty() || t.starts_with('*') {
            continue;
        }
        if in_history {
            d.history_lines += 1;
            continue;
        }
        if let Some(rest) = t.strip_prefix('#') {
            let name = rest.split_whitespace().next().unwrap_or("").to_string();
            let version = rest.split_whitespace().nth(1).unwrap_or("").to_string();
            if name == "MHL" {
                in_history = true;
            }
            d.versions.insert(name.clone(), version);
            layer = name;
            device = None;
            continue;
        }
        if let Some(rest) = t.strip_prefix(".DVC") {
            let name = rest.split(',').next().unwrap_or("").trim().to_string();
            d.dsl.push((name, Vec::new()));
            device = Some(d.dsl.len() - 1);
            continue;
        }
        let Some((k, v)) = key_value(t) else { continue };
        if !k.starts_with(|c: char| c.is_ascii_alphabetic()) {
            continue;
        }
        let v = unquote(v);
        match layer.as_str() {
            "DESC" => {
                d.desc.insert(k.to_string(), v);
            }
            "SPL" => {
                d.spl.insert(k.to_string(), v);
            }
            "DSL" => {
                if let Some(i) = device
                    && let Some((_, kv)) = d.dsl.get_mut(i)
                {
                    kv.push((k.to_string(), v));
                }
            }
            _ => {}
        }
    }
    d
}

impl Descriptor {
    /// A `#DESC` or `#SPL` value.
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.desc
            .get(key)
            .or_else(|| self.spl.get(key))
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// A `#DSL` value of a device (`None` = any device).
    pub(crate) fn device(&self, device: Option<&str>, key: &str) -> Option<&str> {
        self.dsl
            .iter()
            .filter(|(d, _)| device.is_none_or(|n| n == d))
            .flat_map(|(_, kv)| kv.iter())
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// The vendor tree.
    pub(crate) fn to_json(&self) -> Value {
        let layer = |m: &BTreeMap<String, String>| {
            Value::Object(m.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
        };
        let mut dsl = Map::new();
        for (dev, kv) in &self.dsl {
            let o: Map<String, Value> = kv.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            dsl.insert(dev.clone(), Value::Object(o));
        }
        json!({
            "layer_versions": self.versions,
            "descriptor": layer(&self.desc),
            "standard_parameters": layer(&self.spl),
            "device_parameters": dsl,
            "history_lines": self.history_lines,
        })
    }
}

/// An ESP/WinEPR parameter file: key → value in file order (a later duplicate replaces the
/// earlier value, as EasySpin does).
#[derive(Debug, Clone, Default)]
pub(crate) struct ParFile {
    pub(crate) entries: Vec<(String, String)>,
}

impl ParFile {
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub(crate) fn number(&self, key: &str) -> Option<f64> {
        self.get(key)
            .and_then(|v| v.split_whitespace().next())
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite())
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut o = Map::new();
        for (k, v) in &self.entries {
            o.insert(k.clone(), json!(v));
        }
        Value::Object(o)
    }
}

/// Keys an ESP/WinEPR parameter file is recognised by (at least two must be present).
const PAR_KEYS: &[&str] = &[
    "DOS", "JSS", "HCF", "HSW", "RES", "ANZ", "MF", "MP", "RMA", "RRG", "JDA", "JTM", "JEX", "GST",
    "GSI", "SSX", "SSY", "XXLB", "XXWI", "JON", "JRE", "RCT", "RTC",
];

/// Parse an ESP/WinEPR parameter file; `None` when it does not look like one.
pub(crate) fn parse_par(text: &str) -> Option<ParFile> {
    let mut p = ParFile::default();
    let mut known = 0usize;
    let mut odd = 0usize;
    for t in text.split(['\n', '\r']) {
        if t.trim().is_empty() {
            continue;
        }
        let Some((k, v)) = key_value(t) else { continue };
        if !k.starts_with(|c: char| c.is_ascii_alphabetic()) {
            odd += 1;
            continue;
        }
        if PAR_KEYS.contains(&k) {
            known += 1;
        }
        p.entries.push((k.to_string(), unquote(v)));
    }
    (known >= 2 && odd <= p.entries.len()).then_some(p)
}

/// A number at the start of a value (`9.401346 GHz` → 9.401346).
pub(crate) fn leading_number(v: &str) -> Option<f64> {
    v.split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|x| x.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_layers_devices_and_continuations() {
        let t = "#DESC\t1.2 * DESCRIPTOR INFORMATION ***\n*\nDSRC\tEXP\nBSEQ\tBIG\nTITL\t'my sample'\n\
                 CMNT\tline one\\\nline two\n#SPL\t1.2\nMWFQ\t9.4e9\n#DSL\t1.0\n.DVC     acqStart, 1.0\n\
                 .DVC     mwBridge, 1.0\nPowerAtten\t20.00 dB\n#MHL\t1.0\nanything\n";
        let d = parse_descriptor(t);
        assert!(is_descriptor(t.as_bytes()));
        assert_eq!(d.get("BSEQ"), Some("BIG"));
        assert_eq!(d.get("TITL"), Some("my sample"));
        assert_eq!(d.get("CMNT"), Some("line oneline two"));
        assert_eq!(d.get("MWFQ"), Some("9.4e9"));
        assert_eq!(d.device(Some("mwBridge"), "PowerAtten"), Some("20.00 dB"));
        assert_eq!(d.device(None, "PowerAtten"), Some("20.00 dB"));
        assert_eq!(d.history_lines, 1);
        assert_eq!(d.versions["DESC"], "1.2");
    }

    #[test]
    fn par_needs_known_keys() {
        assert!(parse_par("hello world\n").is_none());
        let p = parse_par("DOS  Format\nHCF 3480\nHSW 200\nRES 1024\n").unwrap();
        assert_eq!(p.number("HCF"), Some(3480.0));
        assert_eq!(p.get("DOS"), Some("Format"));
    }
}
