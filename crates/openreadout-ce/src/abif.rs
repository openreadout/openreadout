//! The ABIF container: a 128-byte header pointing at a directory of 28-byte entries, each
//! naming a tagged item (four-character name and a number) and where its data is. Everything
//! is big-endian. Layout from the Applied Biosystems ABIF specification (July 2006); notes in
//! `docs/formats/abif.md`.

use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

pub(crate) const FORMAT_ID: &str = "abif";

/// Size of the header and of one directory entry.
const HEADER_LEN: usize = 128;
const ENTRY_LEN: usize = 28;
/// More directory entries than any instrument writes (the specification says fewer than 1000).
const MAX_ENTRIES: usize = 100_000;

/// True when `head` starts like an ABIF file (signature and a version 1.xx).
pub(crate) fn looks_like(head: &[u8]) -> bool {
    head.len() >= 6
        && head.starts_with(b"ABIF")
        && (u16::from_be_bytes([head[4], head[5]]) / 100).wrapping_sub(1) < 3
}

/// One tagged item.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub(crate) name: [u8; 4],
    pub(crate) number: i32,
    pub(crate) etype: i16,
    pub(crate) count: u32,
    /// The data bytes (empty when the entry points outside the file).
    pub(crate) data: Vec<u8>,
}

impl Item {
    /// `DATA9`, `S/N%1`, …
    pub(crate) fn key(&self) -> String {
        format!("{}{}", name_text(&self.name), self.number)
    }

    /// Values of a 16-bit signed array (element type 4) or unsigned (3).
    pub(crate) fn shorts(&self) -> Option<Vec<f64>> {
        match self.etype {
            4 => Some(
                self.data
                    .chunks_exact(2)
                    .map(|c| f64::from(i16::from_be_bytes([c[0], c[1]])))
                    .collect(),
            ),
            3 => Some(
                self.data
                    .chunks_exact(2)
                    .map(|c| f64::from(u16::from_be_bytes([c[0], c[1]])))
                    .collect(),
            ),
            _ => None,
        }
    }

    /// Numeric values of any numeric element type.
    pub(crate) fn numbers(&self) -> Option<Vec<f64>> {
        let d = &self.data;
        Some(match self.etype {
            1 => d.iter().map(|&b| f64::from(b)).collect(),
            3 | 4 => return self.shorts(),
            5 => d
                .chunks_exact(4)
                .map(|c| f64::from(i32::from_be_bytes([c[0], c[1], c[2], c[3]])))
                .collect(),
            7 => d
                .chunks_exact(4)
                .map(|c| f64::from(f32::from_be_bytes([c[0], c[1], c[2], c[3]])))
                .collect(),
            8 => d
                .chunks_exact(8)
                .map(|c| f64::from_be_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
                .collect(),
            13 => d.iter().map(|&b| f64::from(u8::from(b != 0))).collect(),
            // thumbprint: two 32-bit integers and two bytes
            12 => d
                .chunks_exact(10)
                .flat_map(|c| {
                    [
                        f64::from(i32::from_be_bytes([c[0], c[1], c[2], c[3]])),
                        f64::from(i32::from_be_bytes([c[4], c[5], c[6], c[7]])),
                        f64::from(c[8]),
                        f64::from(c[9]),
                    ]
                })
                .collect(),
            _ => return None,
        })
    }

    /// The first numeric value.
    pub(crate) fn number(&self) -> Option<f64> {
        self.numbers()?.first().copied()
    }

    /// Text of a string item (pString, cString, or a char array).
    pub(crate) fn text(&self) -> Option<String> {
        let bytes: &[u8] = match self.etype {
            18 => {
                let n = usize::from(*self.data.first()?);
                self.data
                    .get(1..1 + n.min(self.data.len().saturating_sub(1)))?
            }
            19 | 2 => {
                let end = self
                    .data
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(self.data.len());
                &self.data[..end]
            }
            _ => return None,
        };
        Some(decode_text(bytes))
    }

    /// A date item: `YYYY-MM-DD`.
    pub(crate) fn date(&self) -> Option<String> {
        if self.etype != 10 || self.data.len() < 4 {
            return None;
        }
        let y = i16::from_be_bytes([self.data[0], self.data[1]]);
        let (m, d) = (self.data[2], self.data[3]);
        ((1..=12).contains(&m) && (1..=31).contains(&d) && y > 0)
            .then(|| format!("{y:04}-{m:02}-{d:02}"))
    }

    /// A time item: `HH:MM:SS` and seconds since midnight. The fourth byte should hold
    /// hundredths of a second, but some Data Collection versions write values above 99 there,
    /// so it is ignored.
    pub(crate) fn time(&self) -> Option<(String, f64)> {
        if self.etype != 11 || self.data.len() < 4 {
            return None;
        }
        let (h, m, s) = (self.data[0], self.data[1], self.data[2]);
        if h > 23 || m > 59 || s > 59 {
            return None;
        }
        Some((
            format!("{h:02}:{m:02}:{s:02}"),
            f64::from(h) * 3600.0 + f64::from(m) * 60.0 + f64::from(s),
        ))
    }

    /// The value as JSON for the vendor tree: text, a number, a short array, or a summary of a
    /// long one.
    fn json(&self) -> Value {
        if let Some(t) = self.text() {
            return json!(t);
        }
        if let Some(d) = self.date() {
            return json!(d);
        }
        if let Some((t, _)) = self.time() {
            return json!(t);
        }
        if let Some(v) = self.numbers() {
            return match v.len() {
                1 => openreadout_core::series::json_num(v[0]),
                n if n <= 64 => json!(v),
                n => json!({"type": type_name(self.etype), "count": n}),
            };
        }
        json!({
            "type": type_name(self.etype),
            "count": self.count,
            "bytes": hex(&self.data[..self.data.len().min(32)]),
        })
    }
}

/// The text of a tag name: printable ASCII as is, other bytes as `\xNN`.
pub(crate) fn name_text(n: &[u8; 4]) -> String {
    n.iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                char::from(b).to_string()
            } else {
                format!("\\x{b:02x}")
            }
        })
        .collect()
}

/// UTF-8 when the bytes are valid UTF-8, else Latin-1, with trailing blanks removed.
pub(crate) fn decode_text(b: &[u8]) -> String {
    let s = match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    };
    s.trim_end_matches([' ', '\0']).to_string()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Our name for an element type.
pub(crate) fn type_name(t: i16) -> &'static str {
    match t {
        1 => "byte",
        2 => "char",
        3 => "word",
        4 => "short",
        5 => "long",
        6 => "rational",
        7 => "float",
        8 => "double",
        9 => "bcd",
        10 => "date",
        11 => "time",
        12 => "thumbprint",
        13 => "bool",
        14 => "point",
        15 => "rect",
        16 => "vpoint",
        17 => "vrect",
        18 => "pstring",
        19 => "cstring",
        20 => "tag",
        128 | 256 | 384 => "compressed",
        t if t >= 1024 => "user",
        _ => "unknown",
    }
}

/// Bytes per element of the element types whose size is fixed.
fn element_size(t: i16) -> Option<u64> {
    Some(match t {
        1 | 2 | 13 | 18 | 19 => 1,
        3 | 4 => 2,
        5 | 7 | 10 | 11 | 14 => 4,
        6 | 8 | 15 | 16 | 20 => 8,
        12 => 10,
        17 => 16,
        _ => return None,
    })
}

/// The parsed container.
#[derive(Debug, Default)]
pub(crate) struct Abif {
    pub(crate) version: u16,
    pub(crate) items: Vec<Item>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) entries: Vec<LsEntry>,
}

impl Abif {
    /// The item `name` number `number`.
    pub(crate) fn get(&self, name: &str, number: i32) -> Option<&Item> {
        let n = name.as_bytes();
        self.items
            .iter()
            .find(|i| i.number == number && i.name.as_slice() == n)
    }

    /// Text of an item, blank as `None`.
    pub(crate) fn text(&self, name: &str, number: i32) -> Option<String> {
        self.get(name, number)
            .and_then(Item::text)
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }

    /// First number of an item.
    pub(crate) fn number(&self, name: &str, number: i32) -> Option<f64> {
        self.get(name, number).and_then(Item::number)
    }

    /// The vendor tree: every item by key (`DATA9`, `SMPL1`), in directory order.
    pub(crate) fn vendor(&self) -> Value {
        let mut m = serde_json::Map::new();
        for i in &self.items {
            m.entry(i.key()).or_insert_with(|| i.json());
        }
        Value::Object(m)
    }
}

fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    b.get(at..at + 4)
        .map(|s| i32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be_i16(b: &[u8], at: usize) -> Option<i16> {
    b.get(at..at + 2).map(|s| i16::from_be_bytes([s[0], s[1]]))
}

/// Parse the container of a whole file held in memory.
pub(crate) fn parse(b: &[u8]) -> Result<Abif> {
    if b.len() < HEADER_LEN.min(34) || !b.starts_with(b"ABIF") {
        return Err(Error::corrupt(
            FORMAT_ID,
            "the file does not start with the ABIF signature",
        ));
    }
    let version = u16::from_be_bytes([b[4], b[5]]);
    if !(1..=3).contains(&(version / 100)) {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("ABIF major version {}", version / 100),
            "ABIF versions 1.xx to 3.xx are read (every Applied Biosystems instrument from the 310 to the 3500 and SeqStudio).",
        ));
    }
    let bad =
        |what: &str| Error::corrupt(FORMAT_ID, format!("the header's directory entry: {what}"));
    let count = be_i32(b, 18).ok_or_else(|| bad("cut short"))?;
    let dir_off = be_i32(b, 26).ok_or_else(|| bad("cut short"))?;
    let count = usize::try_from(count).map_err(|_| bad("a negative entry count"))?;
    let dir_off = usize::try_from(dir_off).map_err(|_| bad("a negative directory offset"))?;
    if count > MAX_ENTRIES {
        return Err(bad(&format!(
            "{count} entries, more than any instrument writes"
        )));
    }
    let dir_len = count
        .checked_mul(ENTRY_LEN)
        .ok_or_else(|| bad("entry count overflows"))?;
    let dir_end = dir_off
        .checked_add(dir_len)
        .ok_or_else(|| bad("directory offset overflows"))?;
    let mut out = Abif {
        version,
        ..Abif::default()
    };
    out.entries.push(LsEntry {
        kind: "header".into(),
        name: "ABIF header".into(),
        offset: Some(0),
        size: Some(HEADER_LEN as u64),
        image: None,
        details: json!({"version": version, "entries": count}),
    });
    let dir = b.get(dir_off..dir_end).ok_or_else(|| {
        Error::corrupt_at(
            FORMAT_ID,
            dir_off as u64,
            format!(
                "the directory of {count} entries at byte {dir_off} runs past the end of the file ({} bytes): the file is cut short",
                b.len()
            ),
        )
    })?;
    out.entries.push(LsEntry {
        kind: "directory".into(),
        name: "tag directory".into(),
        offset: Some(dir_off as u64),
        size: Some(dir_len as u64),
        image: None,
        details: json!({"entries": count}),
    });
    for k in 0..count {
        let e = &dir[k * ENTRY_LEN..(k + 1) * ENTRY_LEN];
        let name = [e[0], e[1], e[2], e[3]];
        let number = be_i32(e, 4).unwrap_or(0);
        let etype = be_i16(e, 8).unwrap_or(0);
        let raw_count = be_i32(e, 12).unwrap_or(0);
        let size = be_i32(e, 16).unwrap_or(0);
        let off = be_i32(e, 20).unwrap_or(0);
        let key = format!("{}{number}", name_text(&name));
        let (Ok(count), Ok(size)) = (u32::try_from(raw_count), usize::try_from(size)) else {
            out.findings.push(Finding::error(
                "bad_entry",
                format!("entry {k} ({key}): a negative element count or data size"),
            ));
            continue;
        };
        let (data, offset) = if size <= 4 {
            (e[20..20 + size].to_vec(), None)
        } else {
            let Ok(o) = usize::try_from(off) else {
                out.findings.push(Finding::error(
                    "bad_entry",
                    format!("entry {k} ({key}): a negative data offset"),
                ));
                continue;
            };
            match o.checked_add(size).and_then(|end| b.get(o..end)) {
                Some(d) => (d.to_vec(), Some(o as u64)),
                None => {
                    out.findings.push(Finding::error(
                        "data_out_of_file",
                        format!(
                            "{key}: {size} bytes at byte {o} run past the end of the file ({} bytes)",
                            b.len()
                        ),
                    ));
                    continue;
                }
            }
        };
        if let Some(es) = element_size(etype)
            && u64::from(count).checked_mul(es) != Some(size as u64)
        {
            out.findings.push(Finding::warning(
                "size_mismatch",
                format!(
                    "{key}: {count} elements of type {} but {size} bytes of data",
                    type_name(etype)
                ),
            ));
        }
        if let Some(o) = offset {
            out.entries.push(LsEntry {
                kind: "item".into(),
                name: key,
                offset: Some(o),
                size: Some(size as u64),
                image: None,
                details: json!({"type": type_name(etype), "count": count}),
            });
        }
        out.items.push(Item {
            name,
            number,
            etype,
            count,
            data,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file of the given items (name, number, type, count, data).
    pub(crate) fn build(items: &[(&str, i32, i16, u32, Vec<u8>)]) -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        let mut dir: Vec<u8> = Vec::new();
        for (name, number, etype, count, bytes) in items {
            let mut e = Vec::new();
            let mut n = [b' '; 4];
            n[..name.len().min(4)].copy_from_slice(&name.as_bytes()[..name.len().min(4)]);
            e.extend_from_slice(&n);
            e.extend_from_slice(&number.to_be_bytes());
            e.extend_from_slice(&etype.to_be_bytes());
            e.extend_from_slice(&1i16.to_be_bytes());
            e.extend_from_slice(&(*count as i32).to_be_bytes());
            e.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
            if bytes.len() <= 4 {
                let mut v = [0u8; 4];
                v[..bytes.len()].copy_from_slice(bytes);
                e.extend_from_slice(&v);
            } else {
                let off = 128 + data.len();
                e.extend_from_slice(&(off as i32).to_be_bytes());
                data.extend_from_slice(bytes);
            }
            e.extend_from_slice(&0i32.to_be_bytes());
            dir.extend_from_slice(&e);
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"ABIF");
        f.extend_from_slice(&101u16.to_be_bytes());
        f.extend_from_slice(b"tdir");
        f.extend_from_slice(&1i32.to_be_bytes());
        f.extend_from_slice(&1023i16.to_be_bytes());
        f.extend_from_slice(&28i16.to_be_bytes());
        f.extend_from_slice(&(items.len() as i32).to_be_bytes());
        f.extend_from_slice(&(dir.len() as i32).to_be_bytes());
        f.extend_from_slice(&((128 + data.len()) as i32).to_be_bytes());
        f.extend_from_slice(&0i32.to_be_bytes());
        f.resize(128, 0);
        f.extend_from_slice(&data);
        f.extend_from_slice(&dir);
        f
    }

    #[test]
    fn reads_items_inline_and_at_offsets() {
        let f = build(&[
            ("SMPL", 1, 18, 5, b"\x04abcd".to_vec()),
            ("DATA", 1, 4, 3, vec![0, 1, 0xff, 0xff, 0, 7]),
            ("Dye#", 1, 4, 1, vec![0, 4]),
            ("RUND", 1, 10, 1, vec![0x07, 0xd9, 12, 13]),
            ("RUNT", 1, 11, 1, vec![9, 56, 53, 0]),
        ]);
        let a = parse(&f).unwrap();
        assert_eq!(a.version, 101);
        assert_eq!(a.text("SMPL", 1).as_deref(), Some("abcd"));
        assert_eq!(
            a.get("DATA", 1).unwrap().shorts().unwrap(),
            vec![1.0, -1.0, 7.0]
        );
        assert_eq!(a.number("Dye#", 1), Some(4.0));
        assert_eq!(
            a.get("RUND", 1).unwrap().date().as_deref(),
            Some("2009-12-13")
        );
        assert_eq!(a.get("RUNT", 1).unwrap().time().unwrap().0, "09:56:53");
        assert!(a.findings.is_empty());
        assert_eq!(a.vendor()["SMPL1"], json!("abcd"));
    }

    #[test]
    fn damaged_files_are_errors_not_panics() {
        let f = build(&[("DATA", 1, 4, 3, vec![0, 1, 0, 2, 0, 3])]);
        // every truncation is an error or a finding
        for n in 0..f.len() {
            let _ = parse(&f[..n]);
        }
        assert!(parse(b"ABIF").is_err());
        assert!(parse(b"NOPE and more bytes than the header needs").is_err());
        let mut v2 = f.clone();
        v2[4..6].copy_from_slice(&401u16.to_be_bytes());
        assert!(matches!(parse(&v2), Err(Error::Unsupported { .. })));
        // the directory points past the end
        let cut = &f[..f.len() - 4];
        assert!(parse(cut).is_err());
    }
}
