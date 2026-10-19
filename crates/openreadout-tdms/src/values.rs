//! Channel values: reading the pieces a channel's values sit in, converting each stored type
//! to f64, and the scaling chains that DAQmx and other writers describe in `NI_Scale[i]_…`
//! properties (`docs/formats/ni-tdms.md` § Scaling).

use std::collections::BTreeMap;

use openreadout_core::source::SourceFile;

use crate::parse::{Object, Piece, Prop, T_BOOL, T_STRING, T_TIMESTAMP, timestamp};

/// Bytes read at most in one go.
const READ_BLOCK: u64 = 4 << 20;

/// The DAQmx raw types seen in the corpus (`docs/formats/ni-tdms.md` § DAQmx raw data): code →
/// (value bytes, the plain data type it is read as). Other codes are not read.
pub(crate) fn daqmx_type(code: u32) -> Option<(u64, u32)> {
    match code {
        3 => Some((2, 2)),
        _ => None,
    }
}

/// The plain data type and value size a piece stores.
fn piece_type(p: &Piece) -> Option<(u32, u64)> {
    if p.dtype & 0x8000_0000 != 0 && p.dtype != u32::MAX {
        let (sz, t) = daqmx_type(p.dtype & 0x7fff_ffff)?;
        Some((t, sz))
    } else {
        Some((p.dtype, crate::parse::type_size(p.dtype)?))
    }
}

/// One stored value as f64 (timestamps as seconds since 1970).
fn value(b: &[u8], t: u32, big: bool) -> Option<f64> {
    let mut a = [0u8; 8];
    let n = b.len().min(8);
    a[..n].copy_from_slice(&b[..n]);
    if big && n > 1 && t != T_TIMESTAMP {
        a[..n].reverse();
    }
    Some(match t {
        1 => f64::from(a[0] as i8),
        2 => f64::from(i16::from_le_bytes([a[0], a[1]])),
        3 => f64::from(i32::from_le_bytes([a[0], a[1], a[2], a[3]])),
        4 => i64::from_le_bytes(a) as f64,
        5 => f64::from(a[0]),
        6 => f64::from(u16::from_le_bytes([a[0], a[1]])),
        7 => f64::from(u32::from_le_bytes([a[0], a[1], a[2], a[3]])),
        8 => u64::from_le_bytes(a) as f64,
        9 | 0x19 => f64::from(f32::from_le_bytes([a[0], a[1], a[2], a[3]])),
        10 | 0x1A => f64::from_le_bytes(a),
        T_BOOL => f64::from(u8::from(a[0] != 0)),
        T_TIMESTAMP => timestamp(b, big),
        _ => return None,
    })
}

/// Whether the values of `o` can be read as numbers.
pub(crate) fn numeric(o: &Object) -> bool {
    o.problem.is_none()
        && !o.digital_line
        && o.dtype.is_some_and(|t| t != T_STRING)
        && o.pieces.iter().all(|p| {
            piece_type(p).is_some_and(|(t, _)| !matches!(t, 11 | 0x1B | 0x08_000C | 0x10_000D))
        })
}

/// Raw values `first .. first + n` of channel `o`.
pub(crate) fn raw(f: &SourceFile, o: &Object, first: u64, n: u64) -> Result<Vec<f64>, String> {
    let mut out = Vec::with_capacity(n.min(1 << 24) as usize);
    let mut skip = first;
    let mut want = n;
    for p in &o.pieces {
        if want == 0 {
            break;
        }
        if skip >= p.count {
            skip -= p.count;
            continue;
        }
        let (t, sz) = piece_type(p).ok_or("a value type the reader does not decode")?;
        let take = (p.count - skip).min(want);
        let start =
            p.at.checked_add(skip.checked_mul(p.stride).ok_or("offset overflow")?)
                .ok_or("offset overflow")?;
        // read in blocks of whole strides
        let per_block = (READ_BLOCK / p.stride.max(1)).max(1);
        let mut done = 0u64;
        while done < take {
            let k = per_block.min(take - done);
            let span = (k - 1) * p.stride + sz;
            let mut buf = vec![0u8; span as usize];
            f.read_exact_at(start + done * p.stride, &mut buf)
                .map_err(|e| format!("values at byte {}: {e}", start + done * p.stride))?;
            for i in 0..k {
                let at = (i * p.stride) as usize;
                let v = value(&buf[at..at + sz as usize], t, p.big_endian)
                    .ok_or("a value type the reader does not decode")?;
                out.push(v);
            }
            done += k;
        }
        skip = 0;
        want -= take;
    }
    Ok(out)
}

/// A scaling step of the `NI_Scale[i]` chain.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Scale {
    Linear { slope: f64, intercept: f64 },
    Polynomial(Vec<f64>),
}

/// The scaling a channel's properties describe: the chain from the last scale back to the raw
/// values, applied in that order. `Ok(None)`: the values are stored scaled. `Err`: a scale type
/// the reader does not apply.
pub(crate) fn scaling(props: &BTreeMap<String, Prop>) -> Result<Option<Vec<Scale>>, String> {
    let status = props
        .get("NI_Scaling_Status")
        .and_then(Prop::as_str)
        .unwrap_or("");
    if !status.eq_ignore_ascii_case("unscaled") {
        return Ok(None);
    }
    let Some(n) = props
        .get("NI_Number_Of_Scales")
        .and_then(Prop::as_f64)
        .filter(|n| *n >= 1.0 && *n < 1000.0)
    else {
        return Ok(None);
    };
    let mut chain = Vec::new();
    let mut idx = n as i64 - 1;
    let mut guard = 0;
    while idx >= 0 {
        guard += 1;
        if guard > 64 {
            return Err("the scaling chain loops".into());
        }
        let key = |k: &str| format!("NI_Scale[{idx}]_{k}");
        let Some(kind) = props.get(&key("Scale_Type")).and_then(Prop::as_str) else {
            // no scale under this index: the raw values (the DAQmx format-changing scaler)
            break;
        };
        let f = |k: &str| props.get(&key(k)).and_then(Prop::as_f64);
        let (scale, source) = match kind {
            "Linear" => (
                Scale::Linear {
                    slope: f("Linear_Slope").ok_or("a linear scale without a slope")?,
                    intercept: f("Linear_Y_Intercept").unwrap_or(0.0),
                },
                f("Linear_Input_Source"),
            ),
            "Polynomial" => {
                let m = f("Polynomial_Coefficients_Size")
                    .filter(|m| *m >= 0.0 && *m <= 64.0)
                    .ok_or("a polynomial scale without its coefficient count")?
                    as usize;
                let mut c = Vec::with_capacity(m);
                for j in 0..m {
                    c.push(
                        f(&format!("Polynomial_Coefficients[{j}]"))
                            .ok_or("a polynomial scale with a missing coefficient")?,
                    );
                }
                (Scale::Polynomial(c), f("Polynomial_Input_Source"))
            }
            other => return Err(format!("the {other} scale type")),
        };
        chain.push(scale);
        idx = source.map_or(-1, |s| s as i64);
    }
    chain.reverse();
    Ok((!chain.is_empty()).then_some(chain))
}

/// Apply a scaling chain to raw values.
pub(crate) fn apply(chain: &[Scale], v: &mut [f64]) {
    for s in chain {
        match s {
            Scale::Linear { slope, intercept } => {
                for x in v.iter_mut() {
                    *x = *x * slope + intercept;
                }
            }
            Scale::Polynomial(c) => {
                for x in v.iter_mut() {
                    let mut acc = 0.0;
                    for k in c.iter().rev() {
                        acc = acc * *x + k;
                    }
                    *x = acc;
                }
            }
        }
    }
}

/// String values `first .. first + n` of channel `o` (offset arrays then text, per chunk).
pub(crate) fn strings(
    f: &SourceFile,
    o: &Object,
    first: u64,
    n: u64,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut skip = first;
    let mut want = n;
    for &(offs_at, count, text_at, big) in &o.strings {
        if want == 0 {
            break;
        }
        if skip >= count {
            skip -= count;
            continue;
        }
        if count > 1 << 24 {
            return Err(format!("{count} strings in one chunk"));
        }
        let mut offs = vec![0u8; (count * 4) as usize];
        f.read_exact_at(offs_at, &mut offs)
            .map_err(|e| format!("string offsets at byte {offs_at}: {e}"))?;
        let end = |i: usize| {
            let a = [
                offs[4 * i],
                offs[4 * i + 1],
                offs[4 * i + 2],
                offs[4 * i + 3],
            ];
            u64::from(if big {
                u32::from_be_bytes(a)
            } else {
                u32::from_le_bytes(a)
            })
        };
        let take = (count - skip).min(want);
        for i in skip..skip + take {
            let i = i as usize;
            let a = if i == 0 { 0 } else { end(i - 1) };
            let b = end(i);
            if b < a || b - a > 1 << 24 {
                return Err("string offsets out of order".into());
            }
            let mut s = vec![0u8; (b - a) as usize];
            f.read_exact_at(text_at + a, &mut s)
                .map_err(|e| format!("string at byte {}: {e}", text_at + a))?;
            out.push(String::from_utf8_lossy(&s).into_owned());
        }
        skip = 0;
        want -= take;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_chains() {
        let mut p = BTreeMap::new();
        p.insert("NI_Scaling_Status".into(), Prop::Str("unscaled".into()));
        p.insert("NI_Number_Of_Scales".into(), Prop::Int(2));
        p.insert("NI_Scale[1]_Scale_Type".into(), Prop::Str("Linear".into()));
        p.insert("NI_Scale[1]_Linear_Slope".into(), Prop::Float(2.0));
        p.insert("NI_Scale[1]_Linear_Y_Intercept".into(), Prop::Float(1.0));
        p.insert("NI_Scale[1]_Linear_Input_Source".into(), Prop::Int(0));
        let c = scaling(&p).unwrap().unwrap();
        let mut v = vec![3.0];
        apply(&c, &mut v);
        assert_eq!(v, vec![7.0]);
        p.insert(
            "NI_Scale[1]_Scale_Type".into(),
            Prop::Str("Thermocouple".into()),
        );
        assert!(scaling(&p).is_err());
        p.insert("NI_Scaling_Status".into(), Prop::Str("scaled".into()));
        assert_eq!(scaling(&p).unwrap(), None);
    }

    #[test]
    fn values_by_type() {
        assert_eq!(value(&[0xff, 0xff], 2, false), Some(-1.0));
        assert_eq!(value(&[0x01, 0x00], 6, true), Some(256.0));
        assert_eq!(value(&1.5f32.to_le_bytes(), 9, false), Some(1.5));
        assert_eq!(value(&[1], T_BOOL, false), Some(1.0));
    }
}
