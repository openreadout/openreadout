//! What the TEXT keywords say about the DATA segment: data type, byte order, mode, parameters,
//! and how to turn DATA bytes into values (FCS 3.1 §3.2.20 and §3.3).

use crate::keywords::{KeywordSet, parse_float, parse_uint};

/// `$DATATYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    /// `I`: unsigned binary integers, `$PnB` bits each, masked by `$PnR`.
    Integer,
    /// `F`: IEEE-754 single precision.
    Float,
    /// `D`: IEEE-754 double precision.
    Double,
    /// `A`: ASCII-encoded numbers, fixed width (`$PnB` characters) or free format (`$PnB/*/`).
    Ascii,
}

impl DataType {
    pub fn from_code(v: &str) -> Option<DataType> {
        match v.trim().to_ascii_uppercase().as_str() {
            "I" => Some(DataType::Integer),
            "F" => Some(DataType::Float),
            "D" => Some(DataType::Double),
            "A" => Some(DataType::Ascii),
            _ => None,
        }
    }

    /// The single-letter code used in the file.
    pub fn code(self) -> &'static str {
        match self {
            DataType::Integer => "I",
            DataType::Float => "F",
            DataType::Double => "D",
            DataType::Ascii => "A",
        }
    }
}

/// `$BYTEORD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ByteOrder {
    /// `1,2,3,4` (or `1,2` in some FCS 2.0 files).
    LittleEndian,
    /// `4,3,2,1` (or `2,1`).
    BigEndian,
    /// Any other permutation (allowed before FCS 3.1); kept verbatim, not decoded.
    Mixed(String),
}

impl ByteOrder {
    pub fn from_value(v: &str) -> ByteOrder {
        let digits: Vec<&str> = v.split(',').map(str::trim).collect();
        let nums: Vec<u32> = digits.iter().filter_map(|d| d.parse().ok()).collect();
        if nums.len() == digits.len() && !nums.is_empty() {
            let ascending = nums.iter().enumerate().all(|(i, &n)| n == i as u32 + 1);
            let descending = nums
                .iter()
                .enumerate()
                .all(|(i, &n)| n == (nums.len() - i) as u32);
            if ascending {
                return ByteOrder::LittleEndian;
            }
            if descending {
                return ByteOrder::BigEndian;
            }
        }
        ByteOrder::Mixed(v.trim().to_string())
    }

    pub fn name(&self) -> &str {
        match self {
            ByteOrder::LittleEndian => "little-endian",
            ByteOrder::BigEndian => "big-endian",
            ByteOrder::Mixed(s) => s,
        }
    }
}

/// `$MODE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `L`: one row per event.
    List,
    /// `C`: correlated multivariate histogram (deprecated in 3.1).
    Correlated,
    /// `U`: uncorrelated univariate histograms (deprecated in 3.1).
    Uncorrelated,
}

impl Mode {
    pub fn from_code(v: &str) -> Option<Mode> {
        match v.trim().to_ascii_uppercase().as_str() {
            "L" => Some(Mode::List),
            "C" => Some(Mode::Correlated),
            "U" => Some(Mode::Uncorrelated),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Mode::List => "L",
            Mode::Correlated => "C",
            Mode::Uncorrelated => "U",
        }
    }
}

/// `$PnB`: the number as written (bits for I/F/D, characters for A), or `*` (free-format ASCII).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldWidth {
    Fixed(u32),
    FreeFormat,
}

/// One parameter (column), from the `$Pn*` keywords.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parameter {
    /// `n`, one-based.
    pub number: u32,
    /// `$PnN`.
    pub short_name: String,
    /// `$PnS`.
    pub label: Option<String>,
    /// `$PnB`; `None` when missing or unreadable.
    pub width: Option<FieldWidth>,
    /// `$PnR` as a number.
    pub range: Option<f64>,
    /// `$PnR` exactly as written.
    pub range_text: Option<String>,
    /// `$PnE` `(decades, offset)`; `(0, 0)` is linear.
    pub amplification: Option<[f64; 2]>,
    /// `$PnG`.
    pub gain: Option<f64>,
    /// `$PnV`, volts.
    pub voltage: Option<f64>,
    /// `$PnL`, nanometres (FCS 3.1 allows a list).
    pub wavelengths_nm: Vec<f64>,
    /// `$PnF`.
    pub filter: Option<String>,
    /// `$PnT`.
    pub detector_type: Option<String>,
    /// `$PnO`, milliwatts.
    pub power_mw: Option<f64>,
    /// `$PnP`, percent.
    pub percent_emitted: Option<f64>,
    /// `$PnD` verbatim (`Linear,0,1024` / `Logarithmic,4,0.1`).
    pub display: Option<String>,
    /// `$PnCALIBRATION` verbatim (`f,unit`).
    pub calibration: Option<String>,
    /// `$PnDATATYPE` (FCS 3.2): this measurement's data type when it differs from `$DATATYPE`.
    pub data_type: Option<DataType>,
    /// `$PnDET` (FCS 3.2): detector name.
    pub detector: Option<String>,
    /// `$PnTYPE` (FCS 3.2): measurement type (`Raw_Fluorescence`, `Time`, …).
    pub measurement_type: Option<String>,
    /// `$PnFEATURE` (FCS 3.2): evaluation feature (`Area`, `Height`, `Width`, …).
    pub feature: Option<String>,
    /// `$PnTAG` (FCS 3.2): the dye.
    pub tag: Option<String>,
    /// `$PnANALYTE` (FCS 3.2): the target molecule or process.
    pub analyte: Option<String>,
}

impl Parameter {
    /// Bytes this parameter occupies per event: `$PnB / 8` for I/F/D (byte-aligned widths only),
    /// `$PnB` characters for fixed-width A; `None` for free-format ASCII or an unusable `$PnB`.
    pub fn byte_width(&self, data_type: DataType) -> Option<u64> {
        match (data_type, self.width) {
            (DataType::Ascii, Some(FieldWidth::Fixed(n))) if n > 0 => Some(u64::from(n)),
            (DataType::Ascii, _) => None,
            (_, Some(FieldWidth::Fixed(b))) if b % 8 == 0 && b > 0 => Some(u64::from(b / 8)),
            _ => None,
        }
    }

    /// The `$DATATYPE/I/` bit mask: next power of two at or above `$PnR`, minus one, when that is
    /// narrower than the field; otherwise all ones (FCS 3.1 §3.3).
    pub fn bit_mask(&self) -> u64 {
        let bits = match self.width {
            Some(FieldWidth::Fixed(b)) => b.min(64),
            _ => 64,
        };
        let full = if bits >= 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let Some(r) = self.range else { return full };
        if r.is_nan() || r < 1.0 {
            return full;
        }
        let r = r.ceil();
        if r > (1u64 << 63) as f64 {
            return full;
        }
        let pow = (r as u64).next_power_of_two();
        if pow == 0 || pow > full {
            full
        } else {
            pow - 1
        }
    }
}

/// Storage dtype string for a parameter (NumPy spelling): the smallest unsigned integer type
/// that holds `$PnB` bits for `I`, `float32` for `F`, `float64` for `D` and `A`.
pub fn storage_dtype(data_type: Option<DataType>, width: Option<FieldWidth>) -> &'static str {
    match data_type {
        Some(DataType::Float) => "float32",
        Some(DataType::Double | DataType::Ascii) => "float64",
        Some(DataType::Integer) => match width {
            Some(FieldWidth::Fixed(b)) if b <= 8 => "uint8",
            Some(FieldWidth::Fixed(b)) if b <= 16 => "uint16",
            Some(FieldWidth::Fixed(b)) if b <= 32 => "uint32",
            Some(FieldWidth::Fixed(_)) => "uint64",
            _ => "unknown",
        },
        None => "unknown",
    }
}

/// Read the `$Pn*` keywords for parameters `1..=count`.
pub fn read_parameters(kw: &KeywordSet, count: u32) -> Vec<Parameter> {
    (1..=count)
        .map(|n| {
            let get = |suffix: &str| kw.get(&format!("$P{n}{suffix}"));
            let text = |suffix: &str| {
                get(suffix)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            };
            let width = get("B").and_then(|v| {
                let t = v.trim();
                if t == "*" {
                    Some(FieldWidth::FreeFormat)
                } else {
                    t.parse::<u32>().ok().map(FieldWidth::Fixed)
                }
            });
            let amplification = get("E").and_then(|v| {
                let parts: Vec<f64> = v.split(',').filter_map(parse_float).collect();
                (parts.len() == 2).then(|| [parts[0], parts[1]])
            });
            Parameter {
                number: n,
                short_name: get("N").map(|s| s.trim().to_string()).unwrap_or_default(),
                label: text("S"),
                width,
                range: get("R").and_then(parse_float),
                range_text: get("R").map(|s| s.trim().to_string()),
                amplification,
                gain: get("G").and_then(parse_float),
                voltage: get("V").and_then(parse_float),
                wavelengths_nm: get("L")
                    .map(|v| v.split(',').filter_map(parse_float).collect())
                    .unwrap_or_default(),
                filter: text("F"),
                detector_type: text("T"),
                power_mw: get("O").and_then(parse_float),
                percent_emitted: get("P").and_then(parse_float),
                display: text("D"),
                calibration: text("CALIBRATION"),
                data_type: get("DATATYPE").and_then(DataType::from_code),
                detector: text("DET"),
                measurement_type: text("TYPE"),
                feature: text("FEATURE"),
                tag: text("TAG"),
                analyte: text("ANALYTE"),
            }
        })
        .collect()
}

/// A spillover (or compensation) matrix from `$SPILLOVER`, `SPILL` or `$COMP`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpilloverMatrix {
    /// The keyword it came from.
    pub keyword: String,
    /// Parameter names (`$PnN`) heading the rows and columns; empty when the value lists none.
    pub parameters: Vec<String>,
    /// Row-major `n × n` coefficients.
    pub values: Vec<Vec<f64>>,
}

/// Parse `n,name1..namen,v11..vnn` (or `n,v11..vnn` when `names_optional` and no names are present).
pub fn parse_spillover(
    keyword: &str,
    value: &str,
    names_optional: bool,
) -> Option<SpilloverMatrix> {
    let toks: Vec<&str> = value.split(',').map(str::trim).collect();
    let n = parse_uint(toks.first()?)? as usize;
    if n == 0 || n > 4096 {
        return None;
    }
    let nn = n.checked_mul(n)?;
    let (names, nums): (Vec<String>, &[&str]) = if toks.len() == 1 + n + nn {
        (
            toks[1..=n].iter().map(|s| (*s).to_string()).collect(),
            &toks[1 + n..],
        )
    } else if names_optional && toks.len() == 1 + nn {
        (Vec::new(), &toks[1..])
    } else {
        return None;
    };
    let vals: Vec<f64> = nums.iter().filter_map(|s| parse_float(s)).collect();
    if vals.len() != nn {
        return None;
    }
    Some(SpilloverMatrix {
        keyword: keyword.to_string(),
        parameters: names,
        values: vals.chunks(n).map(<[f64]>::to_vec).collect(),
    })
}

/// Decode `rows` events from fixed-width DATA bytes into column-major f64 values.
/// `bytes` must hold exactly `rows × event_width` bytes.
pub fn decode_fixed(
    bytes: &[u8],
    rows: usize,
    data_type: DataType,
    order: &ByteOrder,
    params: &[Parameter],
) -> Result<Vec<Vec<f64>>, String> {
    let widths: Vec<usize> = params
        .iter()
        .map(|p| {
            p.byte_width(p.data_type.unwrap_or(data_type))
                .map(|w| w as usize)
                .ok_or_else(|| format!("parameter {} has no byte-aligned $PnB", p.number))
        })
        .collect::<Result<_, _>>()?;
    let event: usize = widths.iter().sum();
    if bytes.len() != rows * event {
        return Err(format!(
            "expected {} bytes for {rows} events, got {}",
            rows * event,
            bytes.len()
        ));
    }
    let little = match order {
        ByteOrder::LittleEndian => true,
        ByteOrder::BigEndian => false,
        ByteOrder::Mixed(s) => return Err(format!("byte order {s} is not decoded")),
    };
    let masks: Vec<u64> = params.iter().map(Parameter::bit_mask).collect();
    let mut cols: Vec<Vec<f64>> = params.iter().map(|_| Vec::with_capacity(rows)).collect();
    let mut pos = 0usize;
    for _ in 0..rows {
        for (c, &w) in widths.iter().enumerate() {
            let field = &bytes[pos..pos + w];
            pos += w;
            let v = match params[c].data_type.unwrap_or(data_type) {
                DataType::Integer => {
                    if w > 8 {
                        return Err(format!(
                            "integer parameter {} wider than 64 bits",
                            params[c].number
                        ));
                    }
                    let mut x: u64 = 0;
                    if little {
                        for (i, &b) in field.iter().enumerate() {
                            x |= u64::from(b) << (8 * i);
                        }
                    } else {
                        for &b in field {
                            x = (x << 8) | u64::from(b);
                        }
                    }
                    (x & masks[c]) as f64
                }
                DataType::Float => {
                    let a: [u8; 4] = field
                        .try_into()
                        .map_err(|_| format!("$DATATYPE/F/ needs $P{}B/32/", params[c].number))?;
                    f64::from(if little {
                        f32::from_le_bytes(a)
                    } else {
                        f32::from_be_bytes(a)
                    })
                }
                DataType::Double => {
                    let a: [u8; 8] = field
                        .try_into()
                        .map_err(|_| format!("$DATATYPE/D/ needs $P{}B/64/", params[c].number))?;
                    if little {
                        f64::from_le_bytes(a)
                    } else {
                        f64::from_be_bytes(a)
                    }
                }
                DataType::Ascii => parse_ascii_number(field).ok_or_else(|| {
                    format!(
                        "parameter {}: {:?} is not a number",
                        params[c].number,
                        String::from_utf8_lossy(field)
                    )
                })?,
            };
            cols[c].push(v);
        }
    }
    Ok(cols)
}

fn parse_ascii_number(field: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(field).ok()?.trim();
    if s.is_empty() {
        return Some(0.0);
    }
    s.parse::<f64>().ok()
}

/// Decode free-format ASCII DATA (`$PnB/*/`): values separated by space, tab, comma, CR or LF,
/// runs of separators counting as one. Returns `total_rows × params` values column-major.
pub fn decode_free_ascii(
    bytes: &[u8],
    total_rows: usize,
    params: usize,
) -> Result<Vec<Vec<f64>>, String> {
    let mut cols: Vec<Vec<f64>> = (0..params)
        .map(|_| Vec::with_capacity(total_rows))
        .collect();
    let needed = total_rows * params;
    let mut n = 0usize;
    for tok in bytes
        .split(|b| matches!(b, b' ' | b'\t' | b',' | b'\r' | b'\n'))
        .filter(|t| !t.is_empty())
    {
        if n == needed {
            break;
        }
        let v = parse_ascii_number(tok)
            .ok_or_else(|| format!("{:?} is not a number", String::from_utf8_lossy(tok)))?;
        cols[n % params].push(v);
        n += 1;
    }
    if n < needed {
        return Err(format!(
            "free-format ASCII DATA holds {n} values, {needed} expected"
        ));
    }
    Ok(cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(bits: u32, range: f64) -> Parameter {
        Parameter {
            number: 1,
            width: Some(FieldWidth::Fixed(bits)),
            range: Some(range),
            ..Parameter::default()
        }
    }

    #[test]
    fn masks_follow_next_power_of_two() {
        assert_eq!(p(16, 1024.0).bit_mask(), 1023);
        assert_eq!(p(16, 1000.0).bit_mask(), 1023);
        assert_eq!(p(16, 65536.0).bit_mask(), 0xffff);
        assert_eq!(p(32, 4_294_967_296.0).bit_mask(), 0xffff_ffff);
        assert_eq!(p(32, 11_209_599.0).bit_mask(), (1 << 24) - 1);
        assert_eq!(p(8, 65536.0).bit_mask(), 0xff);
        assert_eq!(p(32, 0.0).bit_mask(), 0xffff_ffff);
    }

    #[test]
    fn byte_orders() {
        assert_eq!(ByteOrder::from_value("1,2,3,4"), ByteOrder::LittleEndian);
        assert_eq!(ByteOrder::from_value("1,2"), ByteOrder::LittleEndian);
        assert_eq!(ByteOrder::from_value("4,3,2,1"), ByteOrder::BigEndian);
        assert_eq!(ByteOrder::from_value(" 2,1 "), ByteOrder::BigEndian);
        assert_eq!(
            ByteOrder::from_value("3,4,1,2"),
            ByteOrder::Mixed("3,4,1,2".into())
        );
    }

    #[test]
    fn decode_integers_masks_and_24_bit() {
        let params = vec![p(16, 1024.0), p(24, 30000.0)];
        // big-endian: 0xFC05 masked to 0x005 → 5; 24-bit 0x012345 masked by 32767 → 0x2345
        let bytes = [0xFC, 0x05, 0x01, 0x23, 0x45];
        let cols =
            decode_fixed(&bytes, 1, DataType::Integer, &ByteOrder::BigEndian, &params).unwrap();
        assert_eq!(cols, vec![vec![5.0], vec![f64::from(0x2345)]]);
        let le = [0x05, 0xFC, 0x45, 0x23, 0x01];
        let cols =
            decode_fixed(&le, 1, DataType::Integer, &ByteOrder::LittleEndian, &params).unwrap();
        assert_eq!(cols, vec![vec![5.0], vec![f64::from(0x2345)]]);
    }

    #[test]
    fn decode_floats_and_doubles() {
        let params = vec![p(32, 262_144.0)];
        let mut b = Vec::new();
        b.extend_from_slice(&1.5f32.to_be_bytes());
        b.extend_from_slice(&(-2.25f32).to_be_bytes());
        let cols = decode_fixed(&b, 2, DataType::Float, &ByteOrder::BigEndian, &params).unwrap();
        assert_eq!(cols, vec![vec![1.5, -2.25]]);
        let params = vec![p(64, 1.0)];
        let cols = decode_fixed(
            &7.125f64.to_le_bytes(),
            1,
            DataType::Double,
            &ByteOrder::LittleEndian,
            &params,
        )
        .unwrap();
        assert_eq!(cols, vec![vec![7.125]]);
        assert!(
            decode_fixed(
                &[0; 4],
                1,
                DataType::Double,
                &ByteOrder::LittleEndian,
                &[p(32, 1.0)]
            )
            .is_err()
        );
    }

    #[test]
    fn ascii_fixed_and_free() {
        let params = vec![p(4, 0.0), p(3, 0.0)];
        let cols = decode_fixed(
            b"0012 34 999  7",
            2,
            DataType::Ascii,
            &ByteOrder::LittleEndian,
            &params,
        )
        .unwrap();
        assert_eq!(cols, vec![vec![12.0, 999.0], vec![34.0, 7.0]]);
        let cols = decode_free_ascii(b"1,3,, ,3\n4\t5  6", 3, 2).unwrap();
        assert_eq!(cols, vec![vec![1.0, 3.0, 5.0], vec![3.0, 4.0, 6.0]]);
        assert!(decode_free_ascii(b"1 2 3", 2, 2).is_err());
    }

    #[test]
    fn spillover_spec_examples() {
        let m = parse_spillover("$SPILLOVER", "2,FL1-A,FL2-A,1.0,0.1,0.03,1.0", false).unwrap();
        assert_eq!(m.parameters, vec!["FL1-A", "FL2-A"]);
        assert_eq!(m.values, vec![vec![1.0, 0.1], vec![0.03, 1.0]]);
        let m = parse_spillover(
            "$SPILLOVER",
            "3,FL2-A,FL1-A,FL3-A,1.0,0.03,0.2,0.1,1.0,0.0,0.05,0,1.0",
            false,
        )
        .unwrap();
        assert_eq!(m.values[2], vec![0.05, 0.0, 1.0]);
        assert!(parse_spillover("$SPILLOVER", "2,FL1-A,FL2-A,1.0", false).is_none());
        let m = parse_spillover("$COMP", "2,1,0.5,0.25,1", true).unwrap();
        assert!(m.parameters.is_empty());
    }
}
