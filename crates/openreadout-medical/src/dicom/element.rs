//! Reading values out of parsed elements (PS3.5 §6.2: value representations).

use super::parse::{Element, Item, Value};

/// Text of a byte string: UTF-8 when it is valid (ISO_IR 192 and plain ASCII), Latin-1
/// otherwise (ISO_IR 100, the most common other character set).
pub(crate) fn decode_text(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    }
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c == ' ' || c.is_control())
}

/// True for VRs whose value is text.
pub(crate) fn is_text_vr(vr: &[u8; 2]) -> bool {
    matches!(
        vr,
        b"AE"
            | b"AS"
            | b"CS"
            | b"DA"
            | b"DS"
            | b"DT"
            | b"IS"
            | b"LO"
            | b"LT"
            | b"PN"
            | b"SH"
            | b"ST"
            | b"TM"
            | b"UC"
            | b"UI"
            | b"UR"
            | b"UT"
    )
}

impl Element {
    /// The inline value bytes (`None` for sequences and values not kept in memory).
    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        match &self.value {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// The items of a sequence (empty for other elements).
    pub(crate) fn items(&self) -> &[Item] {
        match &self.value {
            Value::Seq(v) => v,
            _ => &[],
        }
    }

    /// The text values (split at `\`, padding removed). Empty for binary VRs.
    pub(crate) fn strings(&self) -> Vec<String> {
        let Some(b) = self.bytes() else {
            return Vec::new();
        };
        if !is_text_vr(&self.vr) {
            return Vec::new();
        }
        let t = decode_text(b);
        // LT, ST and UT are single values that may contain backslashes
        if matches!(&self.vr, b"LT" | b"ST" | b"UT") {
            return vec![t.trim_end_matches([' ', '\0']).to_string()];
        }
        t.split('\\').map(|s| trim(s).to_string()).collect()
    }

    /// The first text value, when not empty.
    pub(crate) fn text(&self) -> Option<String> {
        self.strings().into_iter().next().filter(|s| !s.is_empty())
    }

    /// Numbers: DS/IS text values, or binary US/SS/UL/SL/FL/FD/UV/SV/OF/OD values.
    pub(crate) fn numbers(&self) -> Vec<f64> {
        let Some(b) = self.bytes() else {
            return Vec::new();
        };
        let le = self.little;
        macro_rules! each {
            ($n:literal, $f:expr) => {
                b.chunks_exact($n)
                    .map(|c| {
                        let a: [u8; $n] = c.try_into().unwrap_or([0; $n]);
                        $f(a)
                    })
                    .collect()
            };
        }
        match &self.vr {
            b"DS" | b"IS" => self
                .strings()
                .iter()
                .filter_map(|s| s.trim().parse::<f64>().ok())
                .collect(),
            b"US" | b"OW" => each!(2, |a| f64::from(if le {
                u16::from_le_bytes(a)
            } else {
                u16::from_be_bytes(a)
            })),
            b"SS" => each!(2, |a| f64::from(if le {
                i16::from_le_bytes(a)
            } else {
                i16::from_be_bytes(a)
            })),
            b"UL" | b"OL" => each!(4, |a| f64::from(if le {
                u32::from_le_bytes(a)
            } else {
                u32::from_be_bytes(a)
            })),
            b"SL" => each!(4, |a| f64::from(if le {
                i32::from_le_bytes(a)
            } else {
                i32::from_be_bytes(a)
            })),
            b"FL" | b"OF" => each!(4, |a| f64::from(if le {
                f32::from_le_bytes(a)
            } else {
                f32::from_be_bytes(a)
            })),
            b"FD" | b"OD" => each!(8, |a| if le {
                f64::from_le_bytes(a)
            } else {
                f64::from_be_bytes(a)
            }),
            b"UV" | b"OV" => each!(8, |a| (if le {
                u64::from_le_bytes(a)
            } else {
                u64::from_be_bytes(a)
            }) as f64),
            b"SV" => each!(8, |a| (if le {
                i64::from_le_bytes(a)
            } else {
                i64::from_be_bytes(a)
            }) as f64),
            _ => Vec::new(),
        }
    }

    /// The first number.
    pub(crate) fn number(&self) -> Option<f64> {
        self.numbers().into_iter().next().filter(|v| v.is_finite())
    }

    /// Attribute tags (AT): `(group << 16) | element` values.
    pub(crate) fn tags(&self) -> Vec<u32> {
        let Some(b) = self.bytes() else {
            return Vec::new();
        };
        b.chunks_exact(4)
            .map(|c| {
                let (g, e) = if self.little {
                    (
                        u16::from_le_bytes([c[0], c[1]]),
                        u16::from_le_bytes([c[2], c[3]]),
                    )
                } else {
                    (
                        u16::from_be_bytes([c[0], c[1]]),
                        u16::from_be_bytes([c[2], c[3]]),
                    )
                };
                (u32::from(g) << 16) | u32::from(e)
            })
            .collect()
    }
}

/// Look-ups in an item (a data set or a sequence item).
pub(crate) trait Lookup {
    /// The element with `tag`.
    fn get(&self, tag: u32) -> Option<&Element>;
    /// Its first text value.
    fn text(&self, tag: u32) -> Option<String> {
        self.get(tag).and_then(Element::text)
    }
    /// Its first number.
    fn number(&self, tag: u32) -> Option<f64> {
        self.get(tag).and_then(Element::number)
    }
    /// All its numbers.
    fn numbers(&self, tag: u32) -> Vec<f64> {
        self.get(tag).map(Element::numbers).unwrap_or_default()
    }
    /// Its first number as a non-negative integer that fits `u32`.
    fn uint(&self, tag: u32) -> Option<u32> {
        self.number(tag)
            .filter(|v| *v >= 0.0 && v.fract() == 0.0 && *v <= f64::from(u32::MAX))
            .map(|v| v as u32)
    }
    /// The items of a sequence.
    fn items(&self, tag: u32) -> &[Item];
    /// The first item of a sequence.
    fn first(&self, tag: u32) -> Option<&Item> {
        self.items(tag).first()
    }
}

// On `Vec` (an `Item`), so that the slice's own `get(usize)` does not shadow `get(tag)`.
impl Lookup for Vec<Element> {
    fn get(&self, tag: u32) -> Option<&Element> {
        self.iter().find(|e| e.tag == tag)
    }
    fn items(&self, tag: u32) -> &[Item] {
        self.get(tag).map(Element::items).unwrap_or(&[])
    }
}

/// A value looked up in a functional-group sequence: the per-frame item first, then the
/// shared one (PS3.3 C.7.6.16).
pub(crate) fn functional<'a>(
    per_frame: Option<&'a Item>,
    shared: Option<&'a Item>,
    group: u32,
) -> Option<&'a Item> {
    per_frame
        .and_then(|f| f.first(group))
        .or_else(|| shared.and_then(|s| s.first(group)))
}
