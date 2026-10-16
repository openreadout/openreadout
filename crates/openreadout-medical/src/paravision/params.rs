//! ParaVision parameter files (`visu_pars`, `method`, `acqp`, `reco`, `subject`): JCAMP-DX
//! style records `##$Name=value`, with array sizes in brackets (`( 2 )` then the values on
//! the next lines), `<strings>`, structures in parentheses and the run-length form
//! `@n*(value)`. The syntax was learnt from brukerapi (MIT) and the corpus files.

use serde_json::{Map, Value};

/// One element of a parameter value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Atom {
    Number(f64),
    Text(String),
    /// A bare word (an enumeration value such as `littleEndian`).
    Word(String),
    Struct(Vec<Atom>),
}

/// A parameter: its declared dimensions (empty for scalars) and its elements.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Param {
    pub(crate) dims: Vec<usize>,
    pub(crate) items: Vec<Atom>,
}

impl Param {
    pub(crate) fn numbers(&self) -> Vec<f64> {
        self.items
            .iter()
            .filter_map(|a| match a {
                Atom::Number(v) => Some(*v),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn number(&self) -> Option<f64> {
        self.numbers().into_iter().next()
    }

    /// The text of the first string or word.
    pub(crate) fn text(&self) -> Option<String> {
        self.items.iter().find_map(|a| match a {
            Atom::Text(s) | Atom::Word(s) => Some(s.clone()),
            _ => None,
        })
    }

    /// Every string or word.
    pub(crate) fn texts(&self) -> Vec<String> {
        self.items
            .iter()
            .filter_map(|a| match a {
                Atom::Text(s) | Atom::Word(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn structs(&self) -> Vec<&Vec<Atom>> {
        self.items
            .iter()
            .filter_map(|a| match a {
                Atom::Struct(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn to_json(&self) -> Value {
        fn atom(a: &Atom) -> Value {
            match a {
                Atom::Number(v) => crate::util::num(*v),
                Atom::Text(s) | Atom::Word(s) => Value::from(s.clone()),
                Atom::Struct(v) => Value::Array(v.iter().map(atom).collect()),
            }
        }
        // strings declared with a length dimension (`( 65 )`) are one value
        if self.items.len() == 1 && !self.dims.is_empty() {
            return atom(&self.items[0]);
        }
        if self.dims.is_empty() && self.items.len() == 1 {
            return atom(&self.items[0]);
        }
        Value::Array(self.items.iter().map(atom).collect())
    }
}

/// A parsed parameter file: records in file order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Params {
    pub(crate) records: Vec<(String, Param)>,
}

impl Params {
    pub(crate) fn get(&self, name: &str) -> Option<&Param> {
        self.records.iter().find(|(k, _)| k == name).map(|(_, p)| p)
    }
    pub(crate) fn number(&self, name: &str) -> Option<f64> {
        self.get(name).and_then(Param::number)
    }
    pub(crate) fn numbers(&self, name: &str) -> Vec<f64> {
        self.get(name).map(Param::numbers).unwrap_or_default()
    }
    pub(crate) fn text(&self, name: &str) -> Option<String> {
        self.get(name)
            .and_then(Param::text)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    pub(crate) fn to_json(&self, withheld: &[&str]) -> Value {
        let mut m = Map::new();
        for (k, p) in &self.records {
            let v = if withheld.contains(&k.as_str()) {
                Value::from("withheld")
            } else {
                p.to_json()
            };
            m.insert(k.clone(), v);
        }
        Value::Object(m)
    }
}

/// Most elements one parameter may expand to (`@n*` runs included).
const MAX_ITEMS: usize = 1 << 20;

struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lexer<'_> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn string(&mut self) -> String {
        // at '<'; a backslash escapes the next character
        self.i += 1;
        let mut out = Vec::new();
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c == b'\\' && self.i + 1 < self.s.len() && matches!(self.s[self.i + 1], b'<' | b'>') {
                out.push(self.s[self.i + 1]);
                self.i += 2;
                continue;
            }
            self.i += 1;
            if c == b'>' {
                break;
            }
            out.push(c);
        }
        let t = String::from_utf8_lossy(&out).to_string();
        // `<\` + newline + `>` is how ParaVision writes an empty multi-line string
        if t.trim() == "\\" {
            String::new()
        } else {
            t
        }
    }

    fn word(&mut self) -> String {
        let start = self.i;
        while self.i < self.s.len()
            && !self.s[self.i].is_ascii_whitespace()
            && !matches!(self.s[self.i], b',' | b'(' | b')' | b'<')
        {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).to_string()
    }

    /// Atoms until the end, or until `)` when inside a structure.
    fn atoms(&mut self, depth: u32, out: &mut Vec<Atom>) {
        loop {
            self.skip_ws();
            if self.i >= self.s.len() || out.len() > MAX_ITEMS {
                return;
            }
            match self.s[self.i] {
                b')' => {
                    self.i += 1;
                    if depth > 0 {
                        return;
                    }
                }
                b'(' => {
                    self.i += 1;
                    let mut inner = Vec::new();
                    if depth < 8 {
                        self.atoms(depth + 1, &mut inner);
                    }
                    out.push(Atom::Struct(inner));
                }
                b'<' => out.push(Atom::Text(self.string())),
                b'@' => {
                    // @n*(value): n copies
                    self.i += 1;
                    let w = self.word();
                    let n: usize = w.trim_end_matches('*').parse().unwrap_or(0);
                    self.skip_ws();
                    let mut one = Vec::new();
                    if self.s.get(self.i) == Some(&b'(') {
                        self.i += 1;
                        self.atoms(depth + 1, &mut one);
                    }
                    let n = n.min(MAX_ITEMS.saturating_sub(out.len()));
                    for _ in 0..n {
                        out.extend(one.iter().cloned());
                    }
                }
                _ => {
                    let w = self.word();
                    if w.is_empty() {
                        self.i += 1;
                        continue;
                    }
                    match w.parse::<f64>() {
                        Ok(v) => out.push(Atom::Number(v)),
                        Err(_) => out.push(Atom::Word(w)),
                    }
                }
            }
        }
    }
}

/// Parse a parameter file. Never fails: unreadable records are skipped.
pub(crate) fn parse(bytes: &[u8]) -> Params {
    let text = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    };
    let mut records: Vec<(String, String)> = Vec::new();
    let mut open_angle = 0i64;
    for line in text.lines() {
        if open_angle == 0 && line.starts_with("$$") {
            continue;
        }
        if open_angle == 0
            && let Some(rest) = line.strip_prefix("##")
        {
            if let Some((k, v)) = rest.split_once('=') {
                records.push((k.trim_start_matches('$').trim().to_string(), v.to_string()));
            }
        } else if let Some((_, v)) = records.last_mut() {
            v.push('\n');
            v.push_str(line);
        }
        let last = records.last().map_or("", |(_, v)| v.as_str());
        let _ = last;
        for b in line.bytes() {
            match b {
                b'<' => open_angle += 1,
                b'>' => open_angle = (open_angle - 1).max(0),
                _ => {}
            }
        }
    }
    let mut out = Params::default();
    for (k, v) in records {
        if k == "END" {
            break;
        }
        out.records.push((k, value(&v)));
    }
    out
}

/// Parse a record's value text.
fn value(v: &str) -> Param {
    let t = v.trim_start();
    let mut dims = Vec::new();
    let mut body = t;
    if t.starts_with('(')
        && let Some(close) = t.find(')')
    {
        let inside = &t[1..close];
        let rest = &t[close + 1..];
        let sizes: Option<Vec<usize>> = inside
            .split(',')
            .map(|p| p.trim().parse::<usize>().ok())
            .collect();
        // a size bracket is followed by the values; a bracket alone is a structure value
        if let Some(sz) = sizes
            && !rest.trim().is_empty()
        {
            dims = sz;
            body = rest;
        }
    }
    let mut lx = Lexer {
        s: body.as_bytes(),
        i: 0,
    };
    let mut items = Vec::new();
    lx.atoms(0, &mut items);
    Param { dims, items }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes_strings_structs_and_runs() {
        let f = b"##TITLE=Parameter List\n##$VisuCoreSize=( 2 )\n128 96\n$$ comment\n##$VisuCoreUnits=( 2, 65 )\n<mm> <mm>\n##$VisuFGOrderDesc=( 2 )\n(2, <FG_ECHO>, <Map Type>, 0, 2) (5, <FG_SLICE>, <>, 2, 2)\n##$VisuCoreByteOrder=littleEndian\n##$VisuCoreDataOffs=( 4 )\n@3*(0) 1.5\n##$VisuStudyDescription=( 2048 )\n<\\\n>\n##$Pair=(1, 1)\n##END=\n";
        let p = parse(f);
        assert_eq!(p.numbers("VisuCoreSize"), vec![128.0, 96.0]);
        assert_eq!(p.get("VisuCoreUnits").unwrap().texts(), vec!["mm", "mm"]);
        let fg = p.get("VisuFGOrderDesc").unwrap().structs();
        assert_eq!(fg.len(), 2);
        assert_eq!(fg[1][1], Atom::Text("FG_SLICE".into()));
        assert_eq!(p.text("VisuCoreByteOrder").as_deref(), Some("littleEndian"));
        assert_eq!(p.numbers("VisuCoreDataOffs"), vec![0.0, 0.0, 0.0, 1.5]);
        assert_eq!(p.text("VisuStudyDescription"), None);
        assert_eq!(p.get("Pair").unwrap().structs().len(), 1);
    }

    #[test]
    fn hostile_runs_are_capped() {
        let p = parse(b"##$X=( 3 )\n@99999999999*(1)\n");
        assert!(p.numbers("X").len() <= MAX_ITEMS);
    }
}
