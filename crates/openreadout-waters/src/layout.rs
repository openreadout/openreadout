//! Byte layouts of the files in a MassLynx `.raw` directory: the scan index (`_FUNCnnn.IDX`),
//! the value layouts of `_FUNCnnn.DAT`, function blocks (`_FUNCTNS.INF`), per-scan statistics
//! (`_FUNCnnn.STS`), the text files (`_HEADER.TXT`, `_extern.inf`) and the calibration lines.
//! Every accessor is bounds-checked; malformed input yields `None` or an empty list.
//! Vocabulary and derivation: `docs/formats/waters-raw.md`, `docs/provenance/waters-raw.md`.

use std::collections::BTreeMap;

use openreadout_core::bytes::{latin1, le_f32, le_i16, le_i32, le_u16, le_u32, le_u64, until_nul};
use openreadout_core::time::month_from_abbrev;

/// Bytes per `_FUNCnnn.IDX` record in acquisitions that store 32-bit DAT offsets.
pub const IDX_RECORD: usize = 22;
/// Bytes per `_FUNCnnn.IDX` record when the flag field has [`FLAG_WIDE_CALIBRATED`] set
/// (a 64-bit DAT offset follows the 22 known bytes).
pub const IDX_RECORD_WIDE: usize = 30;
/// Flag bit (of the 10-bit field above the value count) seen with 30-byte index records and
/// values stored already calibrated.
pub const FLAG_WIDE_CALIBRATED: u16 = 0x100;
/// Bytes per function block in `_FUNCTNS.INF`.
pub const FUNCTION_BLOCK: usize = 416;
/// Bytes of the `_CHROnnn.DAT` / `_CHROMS.INF` header.
pub const CHRO_HEADER: usize = 0x80;
/// Bytes of the fixed part of an `.STS` header before the field descriptors.
pub const STATS_PREFIX: usize = 32;
/// Bytes per `.STS` field descriptor.
pub const STATS_FIELD: usize = 48;

/// One scan of a function, from `_FUNCnnn.IDX`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScanIndex {
    /// Byte offset of the scan's values in `_FUNCnnn.DAT`.
    pub offset: u64,
    /// Values stored for the scan (low 22 bits of the second word).
    pub count: u32,
    /// High 10 bits of the second word (32 or 288 in the corpus files).
    pub flags: u16,
    /// Total ion current as written in the index (f32).
    pub stored_tic: f32,
    /// Retention time, minutes.
    pub rt_min: f32,
}

/// The record length of an index: 30 bytes when the first record's flag field has
/// [`FLAG_WIDE_CALIBRATED`] set and the length is a whole number of 30-byte records, else 22.
pub fn idx_record_len(b: &[u8]) -> usize {
    let wide = le_u32(b, 4).is_some_and(|w| ((w >> 22) as u16) & FLAG_WIDE_CALIBRATED != 0);
    if wide && b.len().is_multiple_of(IDX_RECORD_WIDE) {
        IDX_RECORD_WIDE
    } else {
        IDX_RECORD
    }
}

/// Record length of `_FUNCnnn.IDX` bytes checked against the DAT file: the length (22 or 30)
/// whose records tile the file and whose offsets step by `count × bytes per value` through a
/// DAT of `dat_len` bytes. 30-byte records occur without flag 0x100 (a SYNAPT G2-Si profile
/// acquisition, flags 0x60), so the flag alone does not decide. Falls back to
/// [`idx_record_len`] when neither or both lengths fit.
pub fn idx_record_len_for(b: &[u8], dat_len: Option<u64>) -> usize {
    let fits = |rl: usize| -> bool {
        if b.is_empty() || !b.len().is_multiple_of(rl) {
            return false;
        }
        let recs = parse_idx(b, rl);
        let total: u64 = recs.iter().map(|r| u64::from(r.count)).sum();
        let Some(dat) = dat_len else {
            return true;
        };
        if total == 0 || dat % total != 0 {
            return false;
        }
        let bpv = dat / total;
        if !matches!(bpv, 2 | 4 | 6 | 8 | 12) {
            return false;
        }
        let mut at = 0u64;
        for r in &recs {
            if r.offset != at {
                return false;
            }
            at = at.saturating_add(u64::from(r.count) * bpv);
        }
        at == dat
    };
    match (fits(IDX_RECORD), fits(IDX_RECORD_WIDE)) {
        (true, false) => IDX_RECORD,
        (false, true) => IDX_RECORD_WIDE,
        _ => idx_record_len(b),
    }
}

/// Parse `_FUNCnnn.IDX` bytes with `record_len`-byte records (22 or 30); a trailing partial
/// record is ignored.
pub fn parse_idx(b: &[u8], record_len: usize) -> Vec<ScanIndex> {
    if record_len < IDX_RECORD {
        return Vec::new();
    }
    b.chunks_exact(record_len)
        .filter_map(|r| {
            let w = le_u32(r, 4)?;
            let offset = if record_len >= IDX_RECORD_WIDE {
                le_u64(r, 22)?
            } else {
                u64::from(le_u32(r, 0)?)
            };
            Some(ScanIndex {
                offset,
                count: w & 0x003F_FFFF,
                flags: (w >> 22) as u16,
                stored_tic: le_f32(r, 8)?,
                rt_min: le_f32(r, 12)?,
            })
        })
        .collect()
}

/// A 4-byte `_FUNCnnn.DAT` value: 22-bit mantissa (low bits) × 2^(exponent − 21), exponent in
/// the high 10 bits. With this scale the values of a scan sum to the index's stored TIC.
pub fn decode_packed32(w: u32) -> f64 {
    let m = f64::from(w & 0x003F_FFFF);
    let e = (w >> 22) as i32;
    m * 2f64.powi(e - 21)
}

/// A 2-byte `_FUNCnnn.DAT` intensity (SIR data): base in the high 13 bits × 4^(low 3 bits).
pub fn decode_packed16(w: u16) -> f64 {
    f64::from(w >> 3) * 4f64.powi(i32::from(w & 7))
}

/// The mass word of a 12-byte value: 27-bit mantissa × 2^(exponent − 27), exponent in the top
/// 5 bits. The result is the stored (uncalibrated, unless flagged) mass.
pub fn decode_mass27(w: u32) -> f64 {
    let m = f64::from(w & 0x07FF_FFFF);
    let e = (w >> 27) as i32;
    m * 2f64.powi(e - 27)
}

/// The intensity word of a 12-byte value: a 22-bit mantissa × 2^(e − 21) with `e` the low 6
/// bits of the top 10; bits 6 and 7 of that field are point flags ([`value12_flags`]), not
/// exponent (a flagged point decoded with all 10 bits reads 2^64 to 2^128 too high).
pub fn decode_intensity12(w: u32) -> f64 {
    let m = f64::from(w & 0x003F_FFFF);
    let e = ((w >> 22) & 0x3F) as i32;
    m * 2f64.powi(e - 21)
}

/// The point flags of a 12-byte value: the top four bits of its intensity word's 10-bit
/// exponent field, as values of that field (0x40, 0x80, 0x100, 0x200). `FLAG_LOCK_MASS_PEAK`
/// (0x40) marks the lock-mass peak of a lock-spray reference scan; 0x80 is set on a few analyte
/// peaks (meaning not known); 0x100 and 0x200 are not seen set.
pub fn value12_flags(b: &[u8]) -> u16 {
    le_u32(b, 0).map_or(0, |w| ((w >> 22) & 0x3C0) as u16)
}

/// 12-byte value flag: the lock-mass peak of a reference scan.
pub const FLAG_LOCK_MASS_PEAK: u16 = 0x40;

/// A 12-byte value: (stored mass, intensity). The third word is not interpreted.
pub fn decode_value12(b: &[u8]) -> Option<(f64, f64)> {
    let b = b.get(..12)?;
    let i = le_u32(b, 0)?;
    let m = le_u32(b, 4)?;
    Some((decode_mass27(m), decode_intensity12(i)))
}

/// A 6-byte value (rainbow documentation, "FUNC .DAT 6-byte"): 48 bits read little-endian;
/// from the top, 23 bits mass base, 5 bits mass power (mass = base × 2^(power − 23)), 4 bits
/// intensity power, a signed 16-bit intensity base (intensity = base × 4^power).
pub fn decode_value6(b: &[u8]) -> Option<(f64, f64)> {
    let s = b.get(..6)?;
    let mut v = 0u64;
    for (i, &x) in s.iter().enumerate() {
        v |= u64::from(x) << (8 * i);
    }
    let base_key = (v >> 25) & 0x7F_FFFF;
    let power_key = ((v >> 20) & 0x1F) as i32;
    let power_val = ((v >> 16) & 0xF) as i32;
    let base_val = f64::from((v & 0xFFFF) as u16 as i16);
    Some((
        base_key as f64 * 2f64.powi(power_key - 23),
        base_val * 4f64.powi(power_val),
    ))
}

/// An 8-byte value (rainbow documentation, "FUNC .DAT 8-byte"): 64 bits read little-endian;
/// from the top, 5 bits `x` then a 31-bit fixed-point mass with `x` integer bits; then 6 bits
/// `y`, one bit not interpreted, and a 21-bit intensity scaled by 2^(y − 21).
pub fn decode_value8(b: &[u8]) -> Option<(f64, f64)> {
    let v = le_u64(b, 0)?;
    let x = ((v >> 59) & 0x1F) as i32;
    let mass_bits = (v >> 28) & 0x7FFF_FFFF;
    let y = ((v >> 22) & 0x3F) as i32;
    let val = v & 0x1F_FFFF;
    Some((
        mass_bits as f64 * 2f64.powi(x - 31),
        val as f64 * 2f64.powi(y - 21),
    ))
}

/// One function block of `_FUNCTNS.INF`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBlock {
    /// First 16-bit word (function code; its 0x20 bit is set in negative-ion functions).
    pub code: u16,
    /// Function start and end, minutes.
    pub start_min: f32,
    /// End of the function, minutes.
    pub end_min: f32,
    /// f32 at block offset 0x18: the precursor (set mass) of a quadrupole product-ion
    /// ("daughter") scan function; other function types hold other values here.
    pub set_mass: f32,
    /// Masses at block offset 0xA0: MRM precursors / SIR masses, or the start mass of a
    /// scanning function; zeros dropped.
    pub set_mz: Vec<f32>,
    /// Masses at block offset 0x120: MRM products, or the end mass of a scanning function;
    /// zeros dropped.
    pub second_mz: Vec<f32>,
}

/// Parse the function blocks of `_FUNCTNS.INF`.
pub fn parse_functions(b: &[u8]) -> Vec<FunctionBlock> {
    b.as_chunks::<FUNCTION_BLOCK>()
        .0
        .iter()
        .map(|s| {
            let floats = |off: usize| -> Vec<f32> {
                (0..32)
                    .filter_map(|i| le_f32(s, off + 4 * i))
                    .take_while(|v| *v != 0.0)
                    .collect()
            };
            FunctionBlock {
                code: le_u16(s, 0).unwrap_or(0),
                set_mass: le_f32(s, 0x18).unwrap_or(f32::NAN),
                start_min: le_f32(s, 10).unwrap_or(f32::NAN),
                end_min: le_f32(s, 14).unwrap_or(f32::NAN),
                set_mz: floats(0xA0),
                second_mz: floats(0x120),
            }
        })
        .collect()
}

impl FunctionBlock {
    /// The function type: the low 5 bits of `code` (`FUNCTION_TOF_MS` 0x12, `FUNCTION_TOF_MSMS`
    /// 0x10, `FUNCTION_DAUGHTER` 0x06, `FUNCTION_MRM` 0x09, 0x00 quadrupole full scan, …).
    pub fn function_type(&self) -> u16 {
        self.code & 0x1F
    }
    /// Bit 0x8000 of `code`: the lock-spray reference function.
    pub fn is_reference_code(&self) -> bool {
        self.code & 0x8000 != 0
    }
}

/// Function type (low 5 bits of the block code): time-of-flight MS survey / parent scan.
pub const FUNCTION_TOF_MS: u16 = 0x12;
/// Function type: time-of-flight MS/MS (fragments of a selected precursor).
pub const FUNCTION_TOF_MSMS: u16 = 0x10;
/// Function type: quadrupole product-ion ("daughter") scan of the block's set mass.
pub const FUNCTION_DAUGHTER: u16 = 0x06;
/// Function type: multiple reaction monitoring.
pub const FUNCTION_MRM: u16 = 0x09;
/// Function type: photodiode-array (UV/Vis) detector; 6-byte values hold (wavelength in nm,
/// signed absorbance count).
pub const FUNCTION_PDA: u16 = 0x0C;

/// `$$ Key: value` lines of `_HEADER.TXT`, in order.
pub fn parse_header_txt(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let l = l.trim_end_matches('\r');
            let rest = l.strip_prefix("$$")?;
            let (k, v) = rest.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// How a calibration polynomial is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CalibrationKind {
    /// `T0`: m = Σ cᵢ mᵢ (polynomial in the stored mass).
    Linear,
    /// `T1`: √m = Σ cᵢ (√stored)ⁱ (polynomial in the square root of the stored mass).
    SquareRoot,
}

/// A `Cal Function n` line of `_HEADER.TXT`.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    /// Coefficients c₀, c₁, … in order.
    pub coefficients: Vec<f64>,
    /// How they are applied (`T0` / `T1`).
    pub kind: CalibrationKind,
}

impl Calibration {
    /// Parse `c0,c1,…,Tn`; `None` for an empty or unknown line.
    pub fn parse(text: &str) -> Option<Calibration> {
        let parts: Vec<&str> = text
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        let (last, coef) = parts.split_last()?;
        let kind = match *last {
            "T0" => CalibrationKind::Linear,
            "T1" => CalibrationKind::SquareRoot,
            _ => return None,
        };
        let coefficients: Vec<f64> = coef
            .iter()
            .map(|c| c.parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect::<Option<_>>()?;
        (!coefficients.is_empty()).then_some(Calibration { coefficients, kind })
    }

    /// True when the polynomial is the identity (0, 1).
    #[allow(clippy::float_cmp)] // exact 0 and 1 coefficients, as written in the file
    pub fn is_identity(&self) -> bool {
        self.coefficients
            .iter()
            .enumerate()
            .all(|(i, &c)| if i == 1 { c == 1.0 } else { c == 0.0 })
    }

    fn poly(&self, x: f64) -> f64 {
        let mut acc = 0.0;
        let mut p = 1.0;
        for &c in &self.coefficients {
            acc += c * p;
            p *= x;
        }
        acc
    }

    /// Calibrated mass of a stored mass (double precision).
    pub fn apply(&self, raw: f64) -> f64 {
        match self.kind {
            CalibrationKind::Linear => self.poly(raw),
            CalibrationKind::SquareRoot => {
                let s = self.poly(raw.max(0.0).sqrt());
                s * s
            }
        }
    }
}

/// One field of an `.STS` file, described by the file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatField {
    /// Numeric id in the descriptor.
    pub id: u16,
    /// Storage type: 0 = 1 byte, 1 = i16, 2 = i32, 3 = f32.
    pub kind: u16,
    /// Byte offset in each record.
    pub offset: usize,
    /// The descriptor's text name (as written in the file).
    pub name: String,
}

/// The layout of an `.STS` file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatsLayout {
    /// Bytes before the first record.
    pub header_len: usize,
    /// Bytes per record (one record per scan).
    pub record_len: usize,
    /// Field descriptors.
    pub fields: Vec<StatField>,
}

/// Parse an `.STS` header; `None` when the header does not describe itself consistently.
pub fn parse_stats_layout(b: &[u8]) -> Option<StatsLayout> {
    let header_len = le_u16(b, 0)? as usize;
    let record_len = le_u16(b, 4)? as usize;
    let n = le_u16(b, 6)? as usize;
    if record_len == 0 || header_len < STATS_PREFIX || header_len > b.len() {
        return None;
    }
    let mut fields = Vec::with_capacity(n);
    for i in 0..n {
        let d = b.get(STATS_PREFIX + STATS_FIELD * i..STATS_PREFIX + STATS_FIELD * (i + 1))?;
        let id = le_u16(d, 0)?;
        let kind = le_u16(d, 2)?;
        let offset = le_u16(d, 4)? as usize;
        let text = &d[6..38];
        let name = latin1(until_nul(text));
        if offset.checked_add(stat_size(kind))? > record_len {
            return None;
        }
        fields.push(StatField {
            id,
            kind,
            offset,
            name: name.trim().to_string(),
        });
    }
    Some(StatsLayout {
        header_len,
        record_len,
        fields,
    })
}

fn stat_size(kind: u16) -> usize {
    match kind {
        0 => 1,
        1 => 2,
        _ => 4,
    }
}

impl StatsLayout {
    /// Number of whole records in a file of `len` bytes.
    pub fn record_count(&self, len: usize) -> usize {
        len.saturating_sub(self.header_len) / self.record_len
    }
    /// The field named `name` (case-insensitive).
    pub fn field(&self, name: &str) -> Option<&StatField> {
        self.fields
            .iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
    }
    /// The field with descriptor id `id` (`STAT_SET_MASS` = 77 is `Set Mass` in every corpus
    /// file, and the only field, unnamed, of files the MassLynx SDK writes).
    pub fn field_by_id(&self, id: u16) -> Option<&StatField> {
        self.fields.iter().find(|f| f.id == id)
    }
    /// Value of field `f` in record `scan` of the file bytes `b`.
    pub fn value(&self, b: &[u8], scan: usize, f: &StatField) -> Option<f64> {
        let at = self
            .header_len
            .checked_add(scan.checked_mul(self.record_len)?)?
            .checked_add(f.offset)?;
        match f.kind {
            0 => b.get(at).map(|&v| f64::from(v)),
            1 => le_i16(b, at).map(f64::from),
            2 => le_i32(b, at).map(f64::from),
            3 => le_f32(b, at).map(f64::from),
            _ => None,
        }
    }
}

/// `.STS` field id of `Set Mass` (the MS/MS precursor).
pub const STAT_SET_MASS: u16 = 77;

/// What `_extern.inf` says about one function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExternFunction {
    /// The function's type text (`TOF MS FUNCTION`, `TOF MSMS FUNCTION`, `REFERENCE`, …).
    pub kind: Option<String>,
    /// `ES+` → `positive`, `ES-` → `negative`.
    pub polarity: Option<String>,
    /// `Data Format` (`Centroid`, `Continuum`).
    pub data_format: Option<String>,
    /// Key-value lines of the function's section.
    pub parameters: Vec<(String, String)>,
}

/// What `_extern.inf` says about the acquisition.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExternInfo {
    /// Per function number.
    pub functions: BTreeMap<u32, ExternFunction>,
    /// A `Polarity` line outside any function section.
    pub polarity: Option<String>,
    /// `Created by Masslynx v4.1` → `4.1`; `Created by 4.2 SCN1028` → `4.2 SCN1028`.
    pub software_version: Option<String>,
    /// `Lock Mass` of the lock-spray section.
    pub lock_mass: Option<f64>,
    /// `ADC Pushes Per IMS Increment`: pusher periods per drift bin.
    pub pushes_per_drift_bin: Option<u32>,
    /// A function section says `UseSONARMode TRUE`: the drift-resolved bins of this
    /// acquisition are quadrupole steps (SONAR), not ion-mobility drift times.
    pub sonar: bool,
}

fn polarity_word(v: &str) -> Option<&'static str> {
    let v = v.trim();
    if v.ends_with('+') {
        Some("positive")
    } else if v.ends_with('-') {
        Some("negative")
    } else {
        None
    }
}

/// Parse `_extern.inf`: `Function Parameters - Function N - TYPE` and
/// `Instrument Parameters - Function N:` sections, polarity lines, the lock mass.
pub fn parse_extern(text: &str) -> ExternInfo {
    let mut out = ExternInfo::default();
    let mut cur: Option<u32> = None;
    // A `Function Parameters` section runs to the next function header: its `[ACQUISITION]`,
    // `[PARENT MS SURVEY]`, ... subsections belong to it (MSe ramps are listed there).
    let mut in_function_params = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Function Parameters - Function") {
            let rest = rest.trim();
            let (num, kind) = rest.split_once(" - ").unwrap_or((rest, ""));
            cur = num.trim().trim_end_matches(':').trim().parse().ok();
            in_function_params = cur.is_some();
            if let Some(func) = cur {
                let entry = out.functions.entry(func).or_default();
                if !kind.trim().is_empty() {
                    entry.kind = Some(kind.trim().to_string());
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Instrument Parameters - Function") {
            cur = rest.trim().trim_end_matches(':').trim().parse().ok();
            in_function_params = false;
            if let Some(func) = cur {
                out.functions.entry(func).or_default();
            }
            continue;
        }
        if line.starts_with('[') && !in_function_params {
            cur = None;
        }
        // `Created by Masslynx v4.1` (MassLynx 4.1) or `Created by 4.2 SCN1028` (4.2)
        if let Some(v) = line.strip_prefix("Created by") {
            let v = v.trim();
            let v = if v.len() >= 8 && v[..8].eq_ignore_ascii_case("masslynx") {
                v[8..].trim()
            } else {
                v
            };
            let v = v.trim_start_matches(['v', 'V']).trim();
            if v.starts_with(|c: char| c.is_ascii_digit()) {
                out.software_version = Some(v.to_string());
            }
            continue;
        }
        let (k, v) = match line.split_once('\t') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };
        if k.eq_ignore_ascii_case("Lock Mass") {
            out.lock_mass = v.parse().ok();
        }
        if k.eq_ignore_ascii_case("ADC Pushes Per IMS Increment") {
            out.pushes_per_drift_bin = v.parse().ok();
        }
        if k.eq_ignore_ascii_case("UseSONARMode") && v.eq_ignore_ascii_case("TRUE") {
            out.sonar = true;
        }
        match cur {
            Some(func) => {
                let entry = out.functions.entry(func).or_default();
                if k == "Polarity" && entry.polarity.is_none() {
                    entry.polarity = polarity_word(v).map(str::to_string);
                }
                if k == "Data Format" {
                    entry.data_format = Some(v.to_string());
                }
                if entry.parameters.len() < 256 {
                    entry.parameters.push((k.to_string(), v.to_string()));
                }
            }
            None => {
                if k == "Polarity" && out.polarity.is_none() {
                    out.polarity = polarity_word(v).map(str::to_string);
                }
            }
        }
    }
    out
}

/// Parse `_CHROMS.INF`: header (u16 header length, u16 channel count, u16 record length), then
/// one record per channel: u32, NUL-terminated description, `$CC$,...,unit`.
/// Returns (description, unit, conversion string) per channel.
pub fn parse_chroms(b: &[u8]) -> Vec<(Option<String>, Option<String>, Option<String>)> {
    let hlen = le_u16(b, 0).map_or(CHRO_HEADER, |v| v as usize);
    let count = le_u16(b, 2).unwrap_or(0) as usize;
    let rlen = le_u16(b, 4).unwrap_or(0) as usize;
    if rlen < 5 {
        return Vec::new();
    }
    (0..count)
        .filter_map(|i| {
            let rec = b.get(hlen + i * rlen..hlen + (i + 1) * rlen)?;
            let text = &rec[4..];
            let mut parts = text.split(|&c| c == 0).filter(|p| !p.is_empty());
            let desc = parts.next().map(latin1);
            let conv = parts.next().map(latin1);
            let unit = conv
                .as_deref()
                .filter(|c| c.starts_with("$CC$"))
                .and_then(|c| c.rsplit(',').next())
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .map(str::to_string);
            Some((desc, unit, conv))
        })
        .collect()
}

/// `16-Jun-2021` + `22:01:20` → `2021-06-16T22:01:20`.
pub fn header_datetime(date: &str, time: &str) -> Option<String> {
    let parts: Vec<&str> = date.trim().split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let day: u32 = parts[0].parse().ok()?;
    let month = month_from_abbrev(parts[1].get(..3)?)?;
    let mut year: i32 = parts[2].parse().ok()?;
    if year < 100 {
        year += 2000;
    }
    let hms: Vec<u32> = time
        .trim()
        .split(':')
        .map(|x| x.trim().parse().ok())
        .collect::<Option<_>>()?;
    let (hour, minute, second) = (
        *hms.first()?,
        hms.get(1).copied().unwrap_or(0),
        hms.get(2).copied().unwrap_or(0),
    );
    if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
    ))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn packed_values() {
        // 0x021a3f1e: exponent 8, mantissa 0x1a3f1e → 1720094 × 2^-13
        assert!((decode_packed32(0x021A_3F1E) - 1_720_094.0 / 8192.0).abs() < 1e-9);
        assert_eq!(decode_packed32(0x0090_0000), 2.0);
        // 13924 = 3481 × 4^1 (rainbow's worked example)
        assert_eq!(decode_packed16((3481 << 3) | 1), 13924.0);
        // MTBLS7290 first peak: mass word 0x3643d360 → 50.1199…
        let m = decode_mass27(0x3643_D360);
        assert!((m - 105_108_320.0 / 2_097_152.0).abs() < 1e-12);
        let mut v = [0u8; 12];
        v[..4].copy_from_slice(&0x0090_0000u32.to_le_bytes());
        v[4..8].copy_from_slice(&0x3643_D360u32.to_le_bytes());
        assert_eq!(decode_value12(&v), Some((m, 2.0)));
        assert_eq!(decode_value12(&v[..8]), None);
    }

    #[test]
    fn rainbow_examples() {
        // 6-byte: 141.932 = 4650831 × 2^(8 − 23), 1229 = 1229 × 4^0
        let v: u64 = (4_650_831u64 << 25) + (8 << 20) + 1229;
        let (m, i) = decode_value6(&v.to_le_bytes()[..6]).unwrap();
        assert_eq!(m, 4_650_831.0 / 32768.0);
        assert_eq!(i, 1229.0);
        // 8-byte: x = 8, 163 + fraction; y = 18, intensity 142528.375
        let mass_bits: u64 = (163u64 << 23) + 3_078_619; // 0.367 × 2^23 ≈ 3078619
        let val: u64 = 1_140_227; // 142528.375 × 2^3
        let v: u64 = (8u64 << 59) + (mass_bits << 28) + (18 << 22) + val;
        let (m, i) = decode_value8(&v.to_le_bytes()).unwrap();
        assert!((m - 163.367).abs() < 1e-4);
        assert_eq!(i, 142_528.375);
    }

    #[test]
    fn idx_records() {
        let mut rec = vec![0u8; 22];
        rec[4..8].copy_from_slice(&0x0800_0005u32.to_le_bytes());
        rec[12..16].copy_from_slice(&1.5f32.to_le_bytes());
        assert_eq!(idx_record_len(&rec), IDX_RECORD);
        let s = parse_idx(&rec, IDX_RECORD);
        assert_eq!(s[0].count, 5);
        assert_eq!(s[0].flags, 32);
        assert_eq!(s[0].rt_min, 1.5);
        let mut wide = vec![0u8; 60];
        wide[4..8].copy_from_slice(&0x4800_0003u32.to_le_bytes());
        wide[34..38].copy_from_slice(&0x4800_0002u32.to_le_bytes());
        wide[52..60].copy_from_slice(&36u64.to_le_bytes());
        assert_eq!(idx_record_len(&wide), IDX_RECORD_WIDE);
        let s = parse_idx(&wide, IDX_RECORD_WIDE);
        assert_eq!(s.len(), 2);
        assert_eq!(s[1].offset, 36);
        assert_eq!(s[1].flags & FLAG_WIDE_CALIBRATED, FLAG_WIDE_CALIBRATED);
        assert!(parse_idx(&wide, 4).is_empty());
    }

    #[test]
    fn calibration() {
        let c = Calibration::parse("-4.4e-3,1.000164492671626e0,6.47e-6,T1").unwrap();
        assert_eq!(c.kind, CalibrationKind::SquareRoot);
        let s = 50.0f64.sqrt();
        let want = (-4.4e-3 + 1.000_164_492_671_626 * s + 6.47e-6 * s * s).powi(2);
        assert!((c.apply(50.0) - want).abs() < 1e-12);
        let id = Calibration::parse("0.0,1.0,T0").unwrap();
        assert!(id.is_identity());
        assert_eq!(id.apply(123.25), 123.25);
        assert!(Calibration::parse(",T0").is_none());
        assert!(Calibration::parse("1,2,T9").is_none());
        assert!(Calibration::parse("x,T0").is_none());
    }

    #[test]
    fn stats_and_text() {
        let mut b = vec![0u8; 32 + 48 * 2 + 2 * 8];
        b[0..2].copy_from_slice(&(32u16 + 96).to_le_bytes());
        b[2..4].copy_from_slice(&1u16.to_le_bytes());
        b[4..6].copy_from_slice(&8u16.to_le_bytes());
        b[6..8].copy_from_slice(&2u16.to_le_bytes());
        let d0 = 32;
        b[d0..d0 + 2].copy_from_slice(&77u16.to_le_bytes());
        b[d0 + 2..d0 + 4].copy_from_slice(&3u16.to_le_bytes());
        b[d0 + 6..d0 + 14].copy_from_slice(b"Set Mass");
        let d1 = 32 + 48;
        b[d1 + 2..d1 + 4].copy_from_slice(&1u16.to_le_bytes());
        b[d1 + 4..d1 + 6].copy_from_slice(&4u16.to_le_bytes());
        b[d1 + 6..d1 + 10].copy_from_slice(b"Cone");
        let r1 = 128 + 8;
        b[r1..r1 + 4].copy_from_slice(&1223.5f32.to_le_bytes());
        b[r1 + 4..r1 + 6].copy_from_slice(&60i16.to_le_bytes());
        let layout = parse_stats_layout(&b).unwrap();
        assert_eq!(layout.record_count(b.len()), 2);
        let set_mass = layout.field("set mass").unwrap().clone();
        assert_eq!(layout.value(&b, 1, &set_mass), Some(1223.5));
        let cone = layout.field("Cone").unwrap().clone();
        assert_eq!(layout.value(&b, 1, &cone), Some(60.0));
        assert_eq!(layout.value(&b, 2, &cone), None);
        let header =
            parse_header_txt("$$ Version: 01.00\r\n$$ Acquired Date: 16-Jun-2021\r\njunk\n");
        assert_eq!(header[0], ("Version".into(), "01.00".into()));
        assert_eq!(header.len(), 2);
        assert_eq!(
            header_datetime("16-Jun-2021", "22:01:20").as_deref(),
            Some("2021-06-16T22:01:20")
        );
        assert_eq!(header_datetime("16-Foo-2021", "22:01:20"), None);
        let ext = parse_extern(
            "Created by Masslynx v4.1\nPolarity\tES-\nFunction Parameters - Function 2 - TOF MSMS FUNCTION\nData Format\t\t\tCentroid\n[LOCK SPRAY]\nLock Mass\t\t785.842650\nInstrument Parameters - Function 3:\nPolarity\tES+\n",
        );
        assert_eq!(ext.polarity.as_deref(), Some("negative"));
        assert_eq!(ext.software_version.as_deref(), Some("4.1"));
        assert_eq!(ext.functions[&2].kind.as_deref(), Some("TOF MSMS FUNCTION"));
        assert_eq!(ext.functions[&2].data_format.as_deref(), Some("Centroid"));
        assert_eq!(ext.functions[&3].polarity.as_deref(), Some("positive"));
        assert_eq!(ext.lock_mass, Some(785.842_65));
    }
}
