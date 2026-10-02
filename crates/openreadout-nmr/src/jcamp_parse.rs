//! JCAMP-DX structure: labeled data records (LDRs) grouped into (possibly nested) blocks.
//! Implemented from the IUPAC protocols (JCAMP-DX 4.24 §4, §6.1, §11; NMR 1993; 5.01);
//! see `docs/formats/jcamp-dx.md`.

use openreadout_core::model::Finding;

use crate::text::{decode_text, lines_with_offsets, split_comment};

/// One labeled data record: `##LABEL= value` plus continuation lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Ldr {
    /// Label as written, without `##` and `=` (e.g. `DATA TYPE`, `$AQ_mod`, `.OBSERVE FREQUENCY`).
    pub label: String,
    /// Normalized label: upper case, spaces, dashes, slashes and underscores removed (§4.4).
    pub key: String,
    /// Text after `=` and every continuation line, joined with `\n`, `$$` comments removed.
    pub value: String,
    /// Line number (1-based) of the `##` line.
    pub line: usize,
    /// Byte offset of the `##` line.
    pub offset: u64,
    /// Byte offset just past the record's last line.
    pub end: u64,
}

impl Ldr {
    /// The value's first line, trimmed.
    pub fn head(&self) -> &str {
        self.value.split('\n').next().unwrap_or("").trim()
    }
    /// The value after its first line (the table body of `##XYDATA=` and friends).
    pub fn body(&self) -> &str {
        self.value.split_once('\n').map_or("", |(_, b)| b)
    }
}

/// A block: `##TITLE=` … `##END=`.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub index: u32,
    /// Enclosing block (a LINK block of a compound file).
    pub parent: Option<u32>,
    pub depth: u32,
    pub ldrs: Vec<Ldr>,
    pub offset: u64,
    pub end: u64,
    /// True when its `##END=` was found.
    pub closed: bool,
}

impl Block {
    /// First record with this normalized key.
    pub fn get(&self, key: &str) -> Option<&Ldr> {
        self.ldrs.iter().find(|l| l.key == key)
    }
    /// Every record with this normalized key.
    pub fn all(&self, key: &str) -> Vec<&Ldr> {
        self.ldrs.iter().filter(|l| l.key == key).collect()
    }
    /// First line of a record's value, trimmed; `None` when absent or empty.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).map(Ldr::head).filter(|s| !s.is_empty())
    }
    /// A numeric record (first AFFN number of its value).
    pub fn number(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(|l| parse_affn(l.head()))
    }
    /// `##TITLE=`.
    pub fn title(&self) -> Option<&str> {
        self.text("TITLE")
    }
    /// `##DATA TYPE=`, upper case.
    pub fn data_type(&self) -> Option<String> {
        self.text("DATATYPE").map(str::to_ascii_uppercase)
    }
    /// `##DATA CLASS=`, normalized like a label (`PEAK TABLE` → `PEAKTABLE`).
    pub fn data_class(&self) -> Option<String> {
        self.text("DATACLASS").map(normalize_label)
    }
    /// `##DATA TYPE= LINK`.
    pub fn is_link(&self) -> bool {
        self.data_type().is_some_and(|t| t.trim() == "LINK")
    }
}

/// A parsed JCAMP-DX file.
#[derive(Debug, Clone, Default)]
pub struct JcampFile {
    pub blocks: Vec<Block>,
    /// Structural problems (unbalanced `##TITLE=`/`##END=`, text outside blocks, bad labels).
    pub issues: Vec<Finding>,
    /// True when the text is not UTF-8 and was read as Latin-1.
    pub latin1: bool,
    pub len: u64,
}

/// Normalize a label or class name: upper case; spaces, dashes, slashes, underscores removed.
pub fn normalize_label(s: &str) -> String {
    s.trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '-' | '/' | '_'))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Parse the first AFFN number of `s` (§4.5.3), ignoring anything after it.
pub fn parse_affn(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let end = t
        .char_indices()
        .find(|&(i, c)| {
            !(c.is_ascii_digit()
                || c == '.'
                || ((c == '+' || c == '-')
                    && (i == 0 || matches!(t.as_bytes()[i - 1], b'e' | b'E')))
                || c == 'e'
                || c == 'E')
        })
        .map_or(t.len(), |(i, _)| i);
    let tok = &t[..end];
    if tok.bytes().any(|b| b.is_ascii_digit()) {
        tok.parse::<f64>().ok()
    } else {
        None
    }
}

/// Does the head of a file start with `##TITLE=` (after blanks and a byte-order mark)?
pub(crate) fn looks_like_jcamp(head: &[u8]) -> bool {
    let head = head.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(head);
    let start = head
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(head.len());
    let rest = &head[start..];
    let Some(rest) = rest.strip_prefix(b"##") else {
        return false;
    };
    let Some(eq) = rest.iter().take(64).position(|&b| b == b'=') else {
        return false;
    };
    normalize_label(&String::from_utf8_lossy(&rest[..eq])) == "TITLE"
}

/// Parse a JCAMP-DX file into blocks. Never fails: problems are collected in `issues`.
pub fn parse_jcamp(bytes: &[u8]) -> JcampFile {
    let (text, latin1) = decode_text(bytes);
    let bom = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let mut file = JcampFile {
        latin1,
        len: bytes.len() as u64,
        ..JcampFile::default()
    };
    let lines = lines_with_offsets(&text);
    // byte offsets: exact for UTF-8 input; approximate (character offsets) for Latin-1 fallback
    let mut stack: Vec<usize> = Vec::new();
    let mut current: Option<(usize, Ldr)> = None;
    let mut outside = 0usize;
    let finish = |file: &mut JcampFile, cur: &mut Option<(usize, Ldr)>| {
        if let Some((b, ldr)) = cur.take() {
            file.blocks[b].ldrs.push(ldr);
        }
    };
    for (n, (off, raw)) in lines.iter().enumerate() {
        let off = off + bom;
        let line_end = off + raw.len() as u64;
        let (content, _) = split_comment(raw);
        let trimmed = content.trim_start();
        if let Some(rest) = trimmed.strip_prefix("##") {
            finish(&mut file, &mut current);
            let (label, value) = match rest.split_once('=') {
                Some(lv) => lv,
                // `##END` without `=` closes a block unambiguously (some writers omit the `=`)
                None if normalize_label(rest) == "END" => {
                    file.issues.push(
                        Finding::info(
                            "end_without_equals",
                            format!("line {}: `##END` without `=` read as ##END=", n + 1),
                        )
                        .at(off),
                    );
                    (rest, "")
                }
                None => {
                    file.issues.push(
                        Finding::warning("bad_label", format!("line {}: `##` without `=`", n + 1))
                            .at(off),
                    );
                    continue;
                }
            };
            let key = normalize_label(label);
            if key == "TITLE" {
                let parent = stack.last().copied();
                let depth = stack.len() as u32;
                file.blocks.push(Block {
                    index: file.blocks.len() as u32,
                    parent: parent.map(|p| p as u32),
                    depth,
                    ldrs: Vec::new(),
                    offset: off,
                    end: line_end,
                    closed: false,
                });
                stack.push(file.blocks.len() - 1);
            }
            if key == "END" {
                match stack.pop() {
                    Some(b) => {
                        file.blocks[b].closed = true;
                        file.blocks[b].end = line_end;
                        for &p in &stack {
                            file.blocks[p].end = line_end;
                        }
                    }
                    None => file.issues.push(
                        Finding::warning(
                            "extra_end",
                            format!("line {}: ##END= without an open block", n + 1),
                        )
                        .at(off),
                    ),
                }
                continue;
            }
            let Some(&b) = stack.last() else {
                outside += 1;
                continue;
            };
            current = Some((
                b,
                Ldr {
                    label: label.trim().to_string(),
                    key,
                    value: value.to_string(),
                    line: n + 1,
                    offset: off,
                    end: line_end,
                },
            ));
            for &p in &stack {
                file.blocks[p].end = line_end;
            }
        } else if let Some((_, ldr)) = current.as_mut() {
            ldr.value.push('\n');
            ldr.value.push_str(content);
            ldr.end = line_end;
            for &p in &stack {
                file.blocks[p].end = line_end;
            }
        } else if !trimmed.trim().is_empty() {
            outside += 1;
        }
    }
    finish(&mut file, &mut current);
    if outside > 0 {
        file.issues.push(Finding::warning(
            "outside_block",
            format!("{outside} line(s) outside any ##TITLE= … ##END= block were ignored"),
        ));
    }
    for &b in &stack {
        file.issues.push(
            Finding::error(
                "truncated",
                format!(
                    "block {} ({}) has no ##END= (truncated file?)",
                    b,
                    file.blocks[b].title().unwrap_or("untitled")
                ),
            )
            .at(file.blocks[b].offset),
        );
    }
    if file.blocks.is_empty() {
        file.issues
            .push(Finding::error("no_blocks", "no ##TITLE= block found"));
    }
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_without_equals_closes_the_block() {
        let t = b"##TITLE= ms peaks\n##JCAMP-DX= 4.24\n##DATA TYPE= MASS SPECTRUM\n##PEAK TABLE= (XY..XY)\n100,5 101,7\n##END\n";
        let f = parse_jcamp(t);
        assert_eq!(f.blocks.len(), 1);
        assert!(f.blocks[0].closed);
        assert!(
            f.issues
                .iter()
                .all(|i| i.code != "bad_label"
                    && i.severity != openreadout_core::model::Severity::Error),
            "{:?}",
            f.issues
        );
        assert!(f.issues.iter().any(|i| i.code == "end_without_equals"));
    }

    #[test]
    fn labels_and_numbers() {
        assert_eq!(normalize_label("Data Type"), "DATATYPE");
        assert_eq!(normalize_label("JCAMP-DX"), "JCAMPDX");
        assert_eq!(normalize_label("VAR_NAME"), "VARNAME");
        assert_eq!(normalize_label(".OBSERVE FREQUENCY"), ".OBSERVEFREQUENCY");
        assert_eq!(parse_affn(" 0.2403850E+05, 1"), Some(24038.5));
        assert_eq!(parse_affn("-1.5e-3 $$"), Some(-0.0015));
        assert_eq!(parse_affn("12 COUNTS"), Some(12.0));
        assert_eq!(parse_affn("abc"), None);
    }

    #[test]
    fn nested_blocks() {
        let t = b"##TITLE= link\n##JCAMP-DX= 5.0\n##DATA TYPE= LINK\n##BLOCKS= 1\n##TITLE= child\n##DATA TYPE= NMR SPECTRUM\n##XYDATA= (X++(Y..Y))\n1 2 3 $$ c\n2 4\n##END=\n##END=\n";
        let f = parse_jcamp(t);
        assert!(f.issues.is_empty(), "{:?}", f.issues);
        assert_eq!(f.blocks.len(), 2);
        assert!(f.blocks[0].is_link());
        assert_eq!(f.blocks[1].parent, Some(0));
        let xy = f.blocks[1].get("XYDATA").unwrap();
        assert_eq!(xy.head(), "(X++(Y..Y))");
        assert_eq!(xy.body(), "1 2 3 \n2 4");
        assert!(f.blocks.iter().all(|b| b.closed));
        assert!(looks_like_jcamp(b"\xEF\xBB\xBF  ##Title= x"));
        assert!(!looks_like_jcamp(b"##DATA TYPE= x"));
    }

    #[test]
    fn unterminated_block() {
        let f = parse_jcamp(b"##TITLE= t\n##XYDATA= (X++(Y..Y))\n1 2\n");
        assert!(f.issues.iter().any(|i| i.code == "truncated"));
    }
}
