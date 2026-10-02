//! The `procpar` parameter text of a Varian/Agilent VnmrJ data directory. See
//! `docs/formats/varian-nmr.md` § Parameters.
//!
//! Each parameter is three lines:
//!
//! ```text
//! sfrq 1 1 1000000000 0 0 2 1 11 1 64      name, then ten attribute numbers
//! 1 125.6811107                             value count, values (reals on this line)
//! 0                                         enumeration count, enumerated values
//! ```
//!
//! String parameters (basic type 2) put their first value on the count line and every further
//! value on a line of its own, each in double quotes. Problems are collected in `issues`; they
//! are never fatal.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Most values read for one parameter (arrays in real files hold at most a few thousand).
const MAX_VALUES: usize = 1 << 20;
/// Most continuation lines a quoted string may span.
const MAX_STRING_LINES: usize = 4096;

/// Values of one parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcparValues {
    /// Basic type 1: real numbers.
    Real(Vec<f64>),
    /// Basic type 2 (or anything else): strings, as written without their quotes.
    Text(Vec<String>),
}

/// One parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcparParam {
    /// Name as written (case-sensitive).
    pub name: String,
    /// First attribute: the parameter's subtype code (1 real, 2 string, 4 flag, 7 integer, …).
    pub subtype: i64,
    /// Second attribute: 1 real, 2 string.
    pub basic_type: i64,
    /// Values.
    pub values: ProcparValues,
    /// Allowed values, when the parameter is enumerated.
    pub enumeration: Vec<String>,
    /// Ninth attribute: 1 when the parameter is active (in use), 0 when switched off.
    pub active: bool,
}

impl ProcparParam {
    /// First value as a number (reals only).
    pub fn real(&self) -> Option<f64> {
        match &self.values {
            ProcparValues::Real(v) => v.first().copied().filter(|x| x.is_finite()),
            ProcparValues::Text(_) => None,
        }
    }
    /// First value as text, trimmed; `None` when empty or not a string.
    pub fn first_text(&self) -> Option<&str> {
        match &self.values {
            ProcparValues::Text(v) => v.first().map(|s| s.trim()).filter(|s| !s.is_empty()),
            ProcparValues::Real(_) => None,
        }
    }
    /// Number of values.
    pub fn len(&self) -> usize {
        match &self.values {
            ProcparValues::Real(v) => v.len(),
            ProcparValues::Text(v) => v.len(),
        }
    }
    /// True when the parameter has no values.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A parsed `procpar` file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Procpar {
    /// Parameters in file order.
    pub params: Vec<ProcparParam>,
    /// Syntax problems (line number and what was wrong).
    pub issues: Vec<String>,
    /// True when the text was not UTF-8 and was read as Latin-1.
    pub latin1: bool,
    index: BTreeMap<String, usize>,
}

impl Procpar {
    /// A parameter by name (the last one when a name repeats).
    pub fn get(&self, name: &str) -> Option<&ProcparParam> {
        self.index.get(name).map(|&i| &self.params[i])
    }
    /// First value of a real parameter.
    pub fn real(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(ProcparParam::real)
    }
    /// First value of a string parameter, trimmed, when not empty.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.get(name).and_then(ProcparParam::first_text)
    }
    /// Every parameter as `{name: {values, subtype, active, enumeration?}}` for `info --view full`.
    pub fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        for p in &self.params {
            let values = match &p.values {
                ProcparValues::Real(v) => json!(
                    v.iter()
                        .map(|x| if x.is_finite() {
                            json!(x)
                        } else {
                            json!(x.to_string())
                        })
                        .collect::<Vec<_>>()
                ),
                ProcparValues::Text(v) => json!(v),
            };
            let mut e = serde_json::Map::new();
            e.insert("values".into(), values);
            e.insert("subtype".into(), json!(p.subtype));
            e.insert("active".into(), json!(p.active));
            if !p.enumeration.is_empty() {
                e.insert("enumeration".into(), json!(p.enumeration));
            }
            m.insert(p.name.clone(), Value::Object(e));
        }
        Value::Object(m)
    }
}

struct Lines<'a> {
    lines: Vec<&'a str>,
    pos: usize,
}

impl<'a> Lines<'a> {
    fn next(&mut self) -> Option<(usize, &'a str)> {
        let l = *self.lines.get(self.pos)?;
        self.pos += 1;
        Some((self.pos, l))
    }
}

/// Read `count` double-quoted strings starting with `first` (the rest of the count line) and
/// continuing on following lines; `\"` and `\\` are unescaped. A string left open at the end of
/// a line continues on the next one (joined with a newline).
fn read_strings(
    first: &str,
    count: usize,
    lines: &mut Lines<'_>,
    issues: &mut Vec<String>,
    at: usize,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open: Option<String> = None;
    let mut line = first;
    let mut extra = 0usize;
    loop {
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if let Some(s) = open.as_mut() {
                match c {
                    '\\' => match chars.next() {
                        Some(n @ ('"' | '\\')) => s.push(n),
                        Some(n) => {
                            s.push('\\');
                            s.push(n);
                        }
                        None => s.push('\\'),
                    },
                    '"' => {
                        out.extend(open.take());
                        if out.len() >= count {
                            break;
                        }
                    }
                    other => s.push(other),
                }
            } else if c == '"' {
                open = Some(String::new());
            }
        }
        if out.len() >= count {
            break;
        }
        if extra >= MAX_STRING_LINES.max(count) {
            issues.push(format!(
                "line {at}: string values run on past {extra} lines"
            ));
            break;
        }
        let Some((_, l)) = lines.next() else {
            issues.push(format!("line {at}: file ends inside string values"));
            break;
        };
        if let Some(s) = open.as_mut() {
            s.push('\n');
        }
        line = l;
        extra += 1;
    }
    out
}

/// Parse `procpar` bytes.
pub fn parse_procpar(bytes: &[u8]) -> Procpar {
    let (text, latin1) = crate::text::decode_text(bytes);
    let mut pp = Procpar {
        latin1,
        ..Procpar::default()
    };
    let mut lines = Lines {
        lines: text.split('\n').map(|l| l.trim_end_matches('\r')).collect(),
        pos: 0,
    };
    while let Some((ln, head)) = lines.next() {
        let toks: Vec<&str> = head.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        if toks.len() < 3 {
            pp.issues.push(format!(
                "line {ln}: parameter header with {} fields",
                toks.len()
            ));
            continue;
        }
        let name = toks[0].to_string();
        let num = |i: usize| toks.get(i).and_then(|t| t.parse::<f64>().ok());
        let subtype = num(1).map_or(0, |v| v as i64);
        let basic_type = num(2).map_or(0, |v| v as i64);
        let active = toks.get(9).is_none_or(|t| *t != "0");
        let Some((vl, vline)) = lines.next() else {
            pp.issues
                .push(format!("line {ln}: {name} has no value line"));
            break;
        };
        let vline = vline.trim_start();
        let (count_tok, rest) = vline.split_once(char::is_whitespace).unwrap_or((vline, ""));
        let Ok(count) = count_tok.parse::<usize>() else {
            pp.issues.push(format!(
                "line {vl}: {name}: value count {count_tok:?} is not a number"
            ));
            continue;
        };
        let count = if count > MAX_VALUES {
            pp.issues.push(format!(
                "line {vl}: {name}: {count} values (limit {MAX_VALUES})"
            ));
            MAX_VALUES
        } else {
            count
        };
        let values = if basic_type == 1 {
            let mut v: Vec<f64> = Vec::with_capacity(count.min(4096));
            for t in rest.split_whitespace().take(count) {
                match t.parse::<f64>() {
                    Ok(x) => v.push(x),
                    Err(_) => pp
                        .issues
                        .push(format!("line {vl}: {name}: {t:?} is not a number")),
                }
            }
            if v.len() < count {
                pp.issues
                    .push(format!("line {vl}: {name}: {} of {count} values", v.len()));
            }
            ProcparValues::Real(v)
        } else {
            let v = read_strings(rest, count, &mut lines, &mut pp.issues, vl);
            if v.len() < count {
                pp.issues
                    .push(format!("line {vl}: {name}: {} of {count} strings", v.len()));
            }
            ProcparValues::Text(v)
        };
        let mut enumeration = Vec::new();
        match lines.next() {
            Some((el, eline)) => {
                let eline = eline.trim_start();
                let (n_tok, rest) = eline.split_once(char::is_whitespace).unwrap_or((eline, ""));
                match n_tok.parse::<usize>() {
                    Ok(0) => {}
                    Ok(n) => {
                        let n = n.min(MAX_VALUES);
                        enumeration = if basic_type == 1 {
                            rest.split_whitespace()
                                .take(n)
                                .map(str::to_string)
                                .collect()
                        } else {
                            let mut none = Lines {
                                lines: Vec::new(),
                                pos: 0,
                            };
                            read_strings(rest, n, &mut none, &mut Vec::new(), el)
                        };
                    }
                    Err(_) => pp.issues.push(format!(
                        "line {el}: {name}: enumeration count {n_tok:?} is not a number"
                    )),
                }
            }
            None => pp.issues.push(format!(
                "line {vl}: {name}: file ends before the enumeration line"
            )),
        }
        pp.index.insert(name.clone(), pp.params.len());
        pp.params.push(ProcparParam {
            name,
            subtype,
            basic_type,
            values,
            enumeration,
            active,
        });
    }
    pp
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "sfrq 1 1 1000000000 0 0 2 1 11 1 64\n1 125.6811107 \n0 \nalock 2 2 8 0 0 2 1 0 1 64\n1 \"n\"\n5 \"a\" \"n\" \"s\" \"u\" \"y\" \ndgs 2 2 1023 0 0 4 1 6 1 64\n2 \"1:AXIAL\"\n\"2:NON \\\"AXIAL\\\"\"\n0 \nphase 7 1 3 0 1 2 1 8 1 64\n2 1 2 \n0 \ntemp 1 1 200 -150 0.1 2 1 8 0 64\n1 25 \n0 \n";

    #[test]
    fn values_strings_enumerations() {
        let p = parse_procpar(SAMPLE.as_bytes());
        assert!(p.issues.is_empty(), "{:?}", p.issues);
        assert_eq!(p.real("sfrq"), Some(125.681_110_7));
        assert_eq!(p.text("alock"), Some("n"));
        assert_eq!(
            p.get("alock").unwrap().enumeration,
            ["a", "n", "s", "u", "y"]
        );
        assert_eq!(
            p.get("dgs").unwrap().values,
            ProcparValues::Text(vec!["1:AXIAL".into(), "2:NON \"AXIAL\"".into()])
        );
        assert_eq!(
            p.get("phase").unwrap().values,
            ProcparValues::Real(vec![1.0, 2.0])
        );
        assert!(!p.get("temp").unwrap().active);
        assert_eq!(p.params.len(), 5);
        assert!(p.to_json()["sfrq"]["values"][0].is_number());
    }

    #[test]
    fn multi_line_string_and_damage() {
        let p = parse_procpar(b"text 2 2 256 0 0 2 1 0 1 64\n1 \"line one\nline two\"\n0 \n");
        assert_eq!(p.text("text"), Some("line one\nline two"));
        assert!(p.issues.is_empty(), "{:?}", p.issues);
        let bad = parse_procpar(b"np 7 1 1 2 3 4 5 6 1 64\nx 1\n0\nsw\n");
        assert!(!bad.issues.is_empty());
        let trunc = parse_procpar(b"seqfil 2 2 8 0 0 2 1 11 1 64\n1 \"s2pul");
        assert!(!trunc.issues.is_empty());
        assert_eq!(parse_procpar(b"").params.len(), 0);
    }
}
