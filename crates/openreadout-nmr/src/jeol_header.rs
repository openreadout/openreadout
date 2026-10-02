//! JEOL Delta `.jdf` file header and parameter section. See `docs/formats/jeol-jdf.md`.
//!
//! The first 1360 bytes are a big-endian header (identifier `JEOL.NMR`, byte order of the rest
//! of the file, dimension count, data type and layout, per-axis types, sizes, valid ranges,
//! axis end values and units, text fields, section offsets). The parameter section holds
//! 64-byte records (value, value type, unit, name); the data section holds the samples in the
//! byte order the header names.

use openreadout_core::bytes::{Endian, be_f64, be_u16, be_u32, be_u64, until_nul};

/// Bytes of the fixed header.
pub const JDF_HEADER_BYTES: usize = 1360;
/// Bytes of one parameter record.
pub const JDF_PARAM_BYTES: usize = 64;
/// Most parameter records read.
const MAX_PARAMS: u64 = 100_000;

/// Kind of one axis (header bytes 24..32).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JdfAxisType {
    /// Code 0: the axis is not used.
    None,
    /// Code 1: real values.
    Real,
    /// Code 2: TPPI-encoded.
    Tppi,
    /// Code 3: complex (real and imaginary sections).
    Complex,
    /// Code 4: real in this axis, complex in the direct one (two sections).
    RealComplex,
    /// Code 5: envelope.
    Envelope,
    /// Any other code.
    Other(i8),
}

impl JdfAxisType {
    fn from_code(c: i8) -> Self {
        match c {
            0 => JdfAxisType::None,
            1 => JdfAxisType::Real,
            2 => JdfAxisType::Tppi,
            3 => JdfAxisType::Complex,
            4 => JdfAxisType::RealComplex,
            5 => JdfAxisType::Envelope,
            o => JdfAxisType::Other(o),
        }
    }
    /// Our name.
    pub fn name(self) -> String {
        match self {
            JdfAxisType::None => "none".into(),
            JdfAxisType::Real => "real".into(),
            JdfAxisType::Tppi => "tppi".into(),
            JdfAxisType::Complex => "complex".into(),
            JdfAxisType::RealComplex => "real_complex".into(),
            JdfAxisType::Envelope => "envelope".into(),
            JdfAxisType::Other(c) => format!("code {c}"),
        }
    }
}

/// A unit: SI prefix exponent, power and base unit code (header axis units and parameter units).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JdfUnit {
    /// Prefix as a power of 1000, negated: 1 milli, 2 micro, 3 nano, −1 kilo, −2 mega, 0 none.
    pub prefix: i8,
    /// Power of the unit (1 for plain units).
    pub power: u8,
    /// Base unit code (13 hertz, 26 ppm, 28 second, 4 celsius, 31 tesla, …).
    pub base: i8,
}

impl JdfUnit {
    fn parse(b0: u8, b1: u8) -> Self {
        let nib = (b0 >> 4) as i8;
        JdfUnit {
            prefix: if nib >= 8 { nib - 16 } else { nib },
            power: b0 & 0x0f,
            base: b1 as i8,
        }
    }
    /// Symbol of the base unit, when we know it.
    pub fn base_symbol(self) -> Option<&'static str> {
        Some(match self.base {
            2 => "A",
            4 => "degC",
            6 => "deg",
            7 => "eV",
            10 => "g",
            12 => "H",
            13 => "Hz",
            14 => "K",
            15 => "J",
            16 => "L",
            19 => "m",
            20 => "mol",
            21 => "N",
            22 => "Ohm",
            23 => "Pa",
            24 => "%",
            25 => "points",
            26 => "ppm",
            27 => "rad",
            28 => "s",
            31 => "T",
            32 => "V",
            33 => "W",
            35 => "dB",
            36 => "Da",
            40 => "ppt",
            41 => "ppb",
            _ => return None,
        })
    }
    /// Factor from a value in this unit to the base unit (milli → 1e-3).
    pub fn factor(self) -> f64 {
        10f64.powi(-3 * i32::from(self.prefix))
    }
    /// Unit text (`us`, `Hz`, `ppm`), or `None` for no unit.
    pub fn symbol(self) -> Option<String> {
        if self.base == 0 {
            return None;
        }
        let base = self
            .base_symbol()
            .map_or_else(|| format!("unit{}", self.base), str::to_string);
        let prefix = match self.prefix {
            0 => "",
            1 => "m",
            2 => "u",
            3 => "n",
            4 => "p",
            -1 => "k",
            -2 => "M",
            -3 => "G",
            _ => "?",
        };
        let pow = if self.power > 1 {
            format!("^{}", self.power)
        } else {
            String::new()
        };
        Some(format!("{prefix}{base}{pow}"))
    }
}

/// The date fields of the header: 7 bits years since 1990, 4 bits month, 5 bits day.
fn jdf_date(bytes: &[u8]) -> Option<String> {
    let bits = be_u16(bytes, 0)?;
    let (year, month, day) = (1990 + (bits >> 9), (bits >> 5) & 0x0f, bits & 0x1f);
    ((1..=12).contains(&month) && (1..=31).contains(&day))
        .then(|| format!("{year:04}-{month:02}-{day:02}"))
}

fn text(b: &[u8]) -> String {
    crate::text::decode_text(until_nul(b)).0.trim().to_string()
}

/// The fixed header.
#[derive(Debug, Clone, PartialEq)]
pub struct JdfHeader {
    /// True when the parameters and data are little-endian.
    pub little_endian: bool,
    /// Major and minor version of the file layout.
    pub version: (u8, u16),
    /// Number of dimensions (header byte 12).
    pub dimensions: u8,
    /// Data type code (2 bits): 0 float64, 1 float32.
    pub data_type: u8,
    /// Layout code (6 bits): 1 one-dimensional, 2 two-dimensional (32-point submatrices), 3…8
    /// higher, 12…14 small submatrices.
    pub data_format: u8,
    /// Instrument code (header byte 15).
    pub instrument: i8,
    /// Axis type per dimension, direct dimension first.
    pub axis_types: Vec<JdfAxisType>,
    /// Axis unit per dimension.
    pub units: Vec<JdfUnit>,
    /// Title.
    pub title: String,
    /// Per dimension: true when the axis values are listed point by point (the list section),
    /// false when the axis is a linear range from `axis_start` to `axis_stop`.
    pub axis_listed: Vec<bool>,
    /// Offset of each dimension's axis list.
    pub list_start: Vec<u32>,
    /// Bytes of each dimension's axis list (big-endian float64 values).
    pub list_length: Vec<u32>,
    /// Stored points per dimension (padded to the submatrix size).
    pub points: Vec<u32>,
    /// First valid point per dimension.
    pub valid_start: Vec<u32>,
    /// Last valid point per dimension.
    pub valid_stop: Vec<u32>,
    /// Axis value of the first stored point per dimension.
    pub axis_start: Vec<f64>,
    /// Axis value of the last stored point per dimension.
    pub axis_stop: Vec<f64>,
    /// Creation date (`YYYY-MM-DD`).
    pub created: Option<String>,
    /// Revision date (`YYYY-MM-DD`).
    pub revised: Option<String>,
    /// Computer name.
    pub node_name: String,
    /// Site (usually the spectrometer name).
    pub site: String,
    /// Author (login).
    pub author: String,
    /// Comment.
    pub comment: String,
    /// Axis title per dimension (`Proton`, `Carbon13`).
    pub axis_titles: Vec<String>,
    /// Base frequency per dimension (MHz).
    pub base_frequency: Vec<f64>,
    /// Zero point per dimension.
    pub zero_point: Vec<f64>,
    /// Reversed flag per dimension.
    pub reversed: Vec<bool>,
    /// Offset and length of the parameter section.
    pub param_start: u32,
    /// Bytes of the parameter section.
    pub param_length: u32,
    /// Offset of the data section.
    pub data_start: u32,
    /// Bytes of the data section.
    pub data_length: u64,
    /// Offset of the context section (the experiment text the spectrometer ran; it follows
    /// the data section).
    pub context_start: u64,
    /// Bytes of the context section.
    pub context_length: u32,
    /// Total file size the header declares.
    pub total_size: u64,
}

impl JdfHeader {
    /// Parse the fixed header; `None` when the identifier is not `JEOL.NMR` or the buffer is
    /// shorter than the header.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < JDF_HEADER_BYTES || &b[..8] != b"JEOL.NMR" {
            return None;
        }
        let u32at = |o: usize| be_u32(b, o).unwrap_or(0);
        let u64at = |o: usize| be_u64(b, o).unwrap_or(0);
        let f64at = |o: usize| be_f64(b, o).unwrap_or(0.0);
        let dims = b[12];
        let nd = usize::from(dims.clamp(1, 8));
        let axis_types: Vec<JdfAxisType> = (0..8)
            .map(|i| JdfAxisType::from_code(b[24 + i] as i8))
            .collect();
        Some(JdfHeader {
            little_endian: b[8] == 1,
            version: (b[9], be_u16(b, 10).unwrap_or(0)),
            dimensions: dims,
            data_type: b[14] >> 6,
            data_format: b[14] & 0x3f,
            instrument: b[15] as i8,
            axis_types: axis_types[..nd].to_vec(),
            units: (0..nd)
                .map(|i| JdfUnit::parse(b[32 + 2 * i], b[33 + 2 * i]))
                .collect(),
            title: text(&b[48..172]),
            axis_listed: (0..nd)
                .map(|i| {
                    let byte = b[172 + i / 2];
                    let nib = if i % 2 == 0 { byte >> 4 } else { byte & 0x0f };
                    matches!(nib, 1 | 3)
                })
                .collect(),
            list_start: (0..nd).map(|i| u32at(1220 + 4 * i)).collect(),
            list_length: (0..nd).map(|i| u32at(1252 + 4 * i)).collect(),
            points: (0..nd).map(|i| u32at(176 + 4 * i)).collect(),
            valid_start: (0..nd).map(|i| u32at(208 + 4 * i)).collect(),
            valid_stop: (0..nd).map(|i| u32at(240 + 4 * i)).collect(),
            axis_start: (0..nd).map(|i| f64at(272 + 8 * i)).collect(),
            axis_stop: (0..nd).map(|i| f64at(336 + 8 * i)).collect(),
            created: jdf_date(&b[400..404]),
            revised: jdf_date(&b[404..408]),
            node_name: text(&b[408..424]),
            site: text(&b[424..552]),
            author: text(&b[552..680]),
            comment: text(&b[680..808]),
            axis_titles: (0..nd)
                .map(|i| text(&b[808 + 32 * i..840 + 32 * i]))
                .collect(),
            base_frequency: (0..nd).map(|i| f64at(1064 + 8 * i)).collect(),
            zero_point: (0..nd).map(|i| f64at(1128 + 8 * i)).collect(),
            reversed: (0..nd).map(|i| b[1192 + i] != 0).collect(),
            param_start: u32at(1212),
            param_length: u32at(1216),
            data_start: u32at(1284),
            data_length: u64at(1288),
            context_start: u64at(1296),
            context_length: u32at(1304),
            total_size: u64at(1320),
        })
    }

    /// Bytes per stored value, or `None` for a reserved data type.
    pub fn value_bytes(&self) -> Option<u64> {
        match self.data_type {
            0 => Some(8),
            1 => Some(4),
            _ => None,
        }
    }

    /// `float64` / `float32`.
    pub fn dtype(&self) -> &'static str {
        if self.data_type == 1 {
            "float32"
        } else {
            "float64"
        }
    }

    /// Name of the layout code.
    pub fn format_name(&self) -> String {
        match self.data_format {
            1 => "one_d".into(),
            2 => "two_d".into(),
            3 => "three_d".into(),
            4 => "four_d".into(),
            12 => "small_two_d".into(),
            13 => "small_three_d".into(),
            14 => "small_four_d".into(),
            c => format!("code {c}"),
        }
    }

    /// Name of the instrument code (as tabulated by nmrglue).
    pub fn instrument_name(&self) -> Option<&'static str> {
        Some(match self.instrument {
            1 => "GSX",
            2 => "Alpha",
            3 => "Eclipse",
            8 => "Gemini",
            9 => "Unity",
            10 => "Aspect",
            11 => "UX",
            13 => "Lambda",
            23 => "AMX",
            24 => "DMX",
            25 => "ECA",
            26 => "Alice",
            _ => return None,
        })
    }
}

/// Value of one parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum JdfValue {
    /// Type 0: up to 16 characters.
    Text(String),
    /// Type 1: 32-bit integer.
    Integer(i32),
    /// Type 2: float64.
    Float(f64),
    /// Type 3: two float64 (real, imaginary).
    Complex(f64, f64),
    /// Type 4: 32-bit integer marking an infinite value.
    Infinity(i32),
    /// Any other type code.
    Unknown(i32),
}

/// One parameter record.
#[derive(Debug, Clone, PartialEq)]
pub struct JdfParam {
    /// Name as written (trailing blanks removed).
    pub name: String,
    /// Value as stored.
    pub value: JdfValue,
    /// Power of ten the stored value is multiplied by.
    pub scaler: i16,
    /// First unit of the value.
    pub unit: JdfUnit,
}

impl JdfParam {
    /// The value as a number times 10^`scaler` (numbers only).
    pub fn number(&self) -> Option<f64> {
        let v = match self.value {
            JdfValue::Integer(i) | JdfValue::Infinity(i) => f64::from(i),
            JdfValue::Float(f) => f,
            _ => return None,
        };
        let v = v * 10f64.powi(i32::from(self.scaler));
        v.is_finite().then_some(v)
    }
    /// The value as text (strings only), trimmed.
    pub fn text(&self) -> Option<&str> {
        match &self.value {
            JdfValue::Text(s) => Some(s.trim()).filter(|s| !s.is_empty()),
            _ => None,
        }
    }
}

/// Parse the parameter section: a 16-byte head (record size, first and last index, total
/// size) and the records `first..=last`. The second value lists problems.
pub fn parse_jdf_params(file: &[u8], h: &JdfHeader) -> (Vec<JdfParam>, Vec<String>) {
    let mut issues = Vec::new();
    let start = h.param_start as usize;
    let end = start
        .saturating_add(h.param_length as usize)
        .min(file.len());
    let Some(sec) = file.get(start..end).filter(|s| s.len() >= 16) else {
        issues.push(format!(
            "parameter section at {} (+{} bytes) is outside the file",
            h.param_start, h.param_length
        ));
        return (Vec::new(), issues);
    };
    let e = if h.little_endian {
        Endian::Little
    } else {
        Endian::Big
    };
    let u32at = |o: usize| e.u32(sec, o).unwrap_or(0);
    let (size, low, high) = (u32at(0), u32at(4), u32at(8));
    if size as usize != JDF_PARAM_BYTES {
        issues.push(format!("parameter record size {size} (expected 64)"));
        return (Vec::new(), issues);
    }
    let count = u64::from(high)
        .saturating_sub(u64::from(low))
        .saturating_add(1);
    let fit = ((sec.len() - 16) / JDF_PARAM_BYTES) as u64;
    if count > fit {
        issues.push(format!(
            "{count} parameter records declared, {fit} fit in the section"
        ));
    }
    let mut out = Vec::new();
    for i in 0..count.min(fit).min(MAX_PARAMS) {
        let o = 16 + i as usize * JDF_PARAM_BYTES;
        let r = &sec[o..o + JDF_PARAM_BYTES];
        let scaler = e.i16(r, 4).unwrap_or(0);
        let unit = JdfUnit::parse(r[6], r[7]);
        let vt = e.i32(r, 32).unwrap_or(0);
        let raw = &r[16..32];
        let value = match vt {
            0 => JdfValue::Text(text(raw)),
            1 => JdfValue::Integer(e.i32(raw, 0).unwrap_or(0)),
            2 => JdfValue::Float(e.f64(raw, 0).unwrap_or(0.0)),
            3 => JdfValue::Complex(e.f64(raw, 0).unwrap_or(0.0), e.f64(raw, 8).unwrap_or(0.0)),
            4 => JdfValue::Infinity(e.i32(raw, 0).unwrap_or(0)),
            o => JdfValue::Unknown(o),
        };
        let name = text(&r[36..64]);
        if name.is_empty() {
            continue;
        }
        out.push(JdfParam {
            name,
            value,
            scaler,
            unit,
        });
    }
    (out, issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_dates() {
        let us = JdfUnit::parse(0x21, 28);
        assert_eq!(us.symbol().as_deref(), Some("us"));
        assert!((us.factor() - 1e-6).abs() < 1e-18);
        let mhz = JdfUnit::parse(0xe1, 13);
        assert_eq!(mhz.symbol().as_deref(), Some("MHz"));
        assert_eq!(JdfUnit::parse(0, 0).symbol(), None);
        // 0x3afb: 29 years, month 7, day 27
        assert_eq!(
            jdf_date(&[0x3a, 0xfb, 0x7f, 0xca]).as_deref(),
            Some("2019-07-27")
        );
        assert_eq!(jdf_date(&[0, 0, 0, 0]), None);
    }

    #[test]
    fn short_or_foreign_headers() {
        assert!(JdfHeader::parse(b"JEOL.NMR").is_none());
        let mut b = vec![0u8; JDF_HEADER_BYTES];
        assert!(JdfHeader::parse(&b).is_none());
        b[..8].copy_from_slice(b"JEOL.NMR");
        b[12] = 1;
        b[14] = 1;
        let h = JdfHeader::parse(&b).unwrap();
        assert_eq!(h.dimensions, 1);
        assert_eq!(h.format_name(), "one_d");
        assert_eq!(h.value_bytes(), Some(8));
        let (p, issues) = parse_jdf_params(&b, &h);
        assert!(p.is_empty() && !issues.is_empty());
    }
}
