//! Bruker OPUS files (`.0`, `.1`, …): a block directory, parameter blocks and data blocks.
//!
//! Layout and vocabulary: `docs/formats/bruker-opus.md`; provenance:
//! `docs/provenance/bruker-opus.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::OPUS_FORMAT_ID as FMT;
use crate::common::{
    Facts, Le, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues, num, read_at,
};

/// The first four bytes of every OPUS file.
pub(crate) const OPUS_MAGIC: [u8; 4] = [0x0a, 0x0a, 0xfe, 0xfe];
/// Largest parameter or text block decoded (bytes).
const MAX_PARAM_BLOCK: u64 = 4 << 20;
/// Most directory entries read.
const MAX_BLOCKS: u32 = 4096;

/// The six fields of a block's type word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockType {
    /// 1 real part, 2 imaginary part, 3 amplitude (plain data).
    pub(crate) part: u8,
    /// 1 sample, 2 reference, 3 result (ratioed).
    pub(crate) role: u8,
    /// Parameter group: 0 data, 1 data status, 2 instrument, 3 acquisition, 4 FT, 5 display,
    /// 6 optics, 7 GC, 8 library search, 9 communication, 10 sample, 11 lab/process.
    pub(crate) params: u8,
    /// Data kind (low 5 bits) and extra channel number (high bits).
    pub(crate) data: u8,
    /// 0 none, 1 first derivative, 2 second, 3 n-th.
    pub(crate) derivative: u8,
    /// 0 plain, 1 compound information, 2 series (3D), 3 structure, 4 compact, 5 history/report.
    pub(crate) extended: u8,
}

impl BlockType {
    pub(crate) fn of(word: u32) -> Self {
        BlockType {
            part: (word & 3) as u8,
            role: ((word >> 2) & 3) as u8,
            params: ((word >> 4) & 63) as u8,
            data: ((word >> 10) & 127) as u8,
            derivative: ((word >> 17) & 3) as u8,
            extended: ((word >> 19) & 7) as u8,
        }
    }
    fn fields(self) -> [u8; 6] {
        [
            self.part,
            self.role,
            self.params,
            self.data,
            self.derivative,
            self.extended,
        ]
    }
    fn is_directory(self) -> bool {
        self.fields() == [0, 0, 0, 13, 0, 0]
    }
    fn is_history(self) -> bool {
        self.fields() == [0, 0, 0, 0, 0, 5]
    }
    fn is_params(self) -> bool {
        self.params > 0 || self.fields() == [0, 0, 0, 0, 0, 1]
    }
    fn is_data(self) -> bool {
        self.params == 0 && !matches!(self.data, 0 | 13) && !matches!(self.extended, 2 | 5)
    }
    fn is_series(self) -> bool {
        self.params == 0 && !matches!(self.data, 0 | 13) && self.extended == 2
    }
    /// A data block and its data-status block share every field but the parameter group.
    fn pairs_with(self, status: BlockType) -> bool {
        status.params == 1
            && self.part == status.part
            && self.role == status.role
            && self.data == status.data
            && self.derivative == status.derivative
            && self.extended == status.extended
    }
    fn group_name(self) -> &'static str {
        match self.params {
            1 => "data_status",
            2 => "instrument",
            3 => "acquisition",
            4 => "fourier_transform",
            5 => "display",
            6 => "optics",
            7 => "gc",
            8 => "library_search",
            9 => "communication",
            10 => "sample",
            11 => "lab_process",
            0 if self.fields() == [0, 0, 0, 0, 0, 1] => "info",
            _ => "other",
        }
    }
}

/// A parameter value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Param {
    Int(i32),
    Float(f64),
    Text(String),
}

impl Param {
    fn json(&self) -> Value {
        match self {
            Param::Int(i) => json!(i),
            Param::Float(f) => num(*f),
            Param::Text(s) => json!(s),
        }
    }
    fn as_f64(&self) -> Option<f64> {
        match self {
            Param::Int(i) => Some(f64::from(*i)),
            Param::Float(f) => Some(*f),
            Param::Text(s) => s.trim().parse().ok(),
        }
    }
    fn as_text(&self) -> String {
        match self {
            Param::Int(i) => i.to_string(),
            Param::Float(f) => f.to_string(),
            Param::Text(s) => s.clone(),
        }
    }
}

/// Parameter records: a 3-character name (NUL-padded to 4), a 16-bit type (0 int32, 1 float64,
/// 2…4 text), a 16-bit size in 2-byte words, the value. Ends at `END` or the block's end.
pub(crate) fn parse_params(b: &[u8]) -> (Vec<(String, Param)>, bool) {
    let mut out = Vec::new();
    let mut at = 0usize;
    loop {
        let Some(name) = b.bytes_at(at, 3) else {
            return (out, false);
        };
        if name == b"END" {
            return (out, true);
        }
        let (Some(kind), Some(words)) = (b.i16_at(at + 4), b.i16_at(at + 6)) else {
            return (out, false);
        };
        let Ok(words) = usize::try_from(words) else {
            return (out, false);
        };
        let size = words * 2;
        let Some(value) = b.bytes_at(at + 8, size) else {
            return (out, false);
        };
        if !name.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            return (out, false);
        }
        let key = String::from_utf8_lossy(name).to_string();
        let v = match kind {
            0 => match value.i32_at(0) {
                Some(i) => Param::Int(i),
                None => return (out, false),
            },
            1 => match value.f64_at(0) {
                Some(f) => Param::Float(f),
                None => return (out, false),
            },
            _ => Param::Text(crate::common::text_field(value)),
        };
        out.push((key, v));
        at += 8 + size;
    }
}

/// One directory entry.
#[derive(Debug, Clone)]
struct Block {
    index: usize,
    word: u32,
    ty: BlockType,
    offset: u64,
    size: u64,
}

type Params = BTreeMap<String, Param>;

/// A data block paired with its status parameters.
struct DataPair {
    data: Block,
    status: Params,
    status_index: usize,
}

/// What a data block holds, in our words: (trace name stem, y channel name, y unit, JCAMP data type).
fn data_kind(
    ty: BlockType,
    dxu: Option<&str>,
) -> (String, &'static str, Option<&'static str>, &'static str) {
    let role = match ty.role {
        1 => "sample ",
        2 => "reference ",
        _ => "",
    };
    let ir = if dxu == Some("NM") {
        "UV/VIS SPECTRUM"
    } else {
        "INFRARED SPECTRUM"
    };
    let (stem, y, unit, dtype) = match ty.data % 32 {
        1 => (format!("{role}single channel"), "single_beam", None, ir),
        2 => (
            format!("{role}interferogram"),
            "interferogram",
            None,
            "INFRARED INTERFEROGRAM",
        ),
        3 => (format!("{role}phase"), "phase", None, ir),
        4 => (format!("{role}absorbance"), "absorbance", Some("AU"), ir),
        5 => (format!("{role}transmittance"), "transmittance", None, ir),
        6 => (format!("{role}Kubelka-Munk"), "kubelka_munk", None, ir),
        7 => (format!("{role}trace"), "intensity", None, ir),
        8 => (
            format!("{role}GC interferograms"),
            "interferogram",
            None,
            "INFRARED INTERFEROGRAM",
        ),
        9 => (format!("{role}GC spectra"), "single_beam", None, ir),
        10 => (
            format!("{role}Raman"),
            "raman_intensity",
            None,
            "RAMAN SPECTRUM",
        ),
        11 => (format!("{role}emission"), "emission", None, ir),
        12 => (format!("{role}reflectance"), "reflectance", None, ir),
        14 => (format!("{role}power"), "power", None, ir),
        15 => (
            format!("{role}log reflectance"),
            "log_reflectance",
            None,
            ir,
        ),
        16 => (format!("{role}ATR"), "atr", None, ir),
        17 => (format!("{role}photoacoustic"), "photoacoustic", None, ir),
        18 => (
            format!("{role}transmittance (arithmetic result)"),
            "transmittance",
            None,
            ir,
        ),
        19 => (
            format!("{role}absorbance (arithmetic result)"),
            "absorbance",
            Some("AU"),
            ir,
        ),
        22 => (format!("{role}match"), "match", None, ir),
        k => (format!("{role}data kind {k}"), "intensity", None, ir),
    };
    let mut stem = stem.trim().to_string();
    if ty.data >= 32 {
        stem.push_str(&format!(" (channel {})", ty.data / 32 + 1));
    }
    match ty.derivative {
        1 => stem.push_str(", first derivative"),
        2 => stem.push_str(", second derivative"),
        3 => stem.push_str(", n-th derivative"),
        _ => {}
    }
    match ty.part {
        1 => stem.push_str(", real part"),
        2 => stem.push_str(", imaginary part"),
        _ => {}
    }
    if ty.extended == 4 {
        stem.push_str(" (compact)");
    }
    (stem, y, unit, dtype)
}

/// x quantity and unit of a `DXU` code.
fn x_axis(dxu: Option<&str>) -> (&'static str, Option<&'static str>) {
    match dxu {
        Some("WN") => ("wavenumber", Some("1/cm")),
        Some("MI") => ("wavelength", Some("µm")),
        Some("NM") => ("wavelength", Some("nm")),
        Some("LGW") => ("log_wavenumber", None),
        Some("MIN") => ("time", Some("min")),
        Some("PNT") => ("points", None),
        _ => ("x", None),
    }
}

/// `DAT` + `TIM` as ISO 8601: `11/05/2020` or `2012/11/09`, `08:59:44.322 (GMT+12)`.
pub(crate) fn opus_time(dat: &str, tim: &str) -> Option<String> {
    let d: Vec<&str> = dat.trim().split('/').collect();
    if d.len() != 3 {
        return None;
    }
    let (y, m, day) = if d[0].len() == 4 {
        (d[0], d[1], d[2])
    } else {
        (d[2], d[1], d[0])
    };
    let (y, m, day): (u32, u32, u32) = (y.parse().ok()?, m.parse().ok()?, day.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || !(1900..=2200).contains(&y) {
        return None;
    }
    let tim = tim.trim();
    let (clock, zone) = match tim.split_once('(') {
        Some((c, z)) => (c.trim(), Some(z.trim_end_matches(')').trim())),
        None => (tim, None),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let (hh, mm): (u32, u32) = (parts[0].parse().ok()?, parts[1].parse().ok()?);
    let sec: f64 = parts[2].parse().ok()?;
    if hh > 23 || mm > 59 || !(0.0..61.0).contains(&sec) {
        return None;
    }
    let secs = if parts[2].contains('.') {
        let frac = parts[2].split('.').nth(1).unwrap_or("");
        format!("{:02}.{frac}", sec.trunc() as u32)
    } else {
        format!("{:02}", sec as u32)
    };
    let offset = zone.and_then(|z| {
        let z = z.strip_prefix("GMT").or_else(|| z.strip_prefix("UTC"))?;
        if z.is_empty() {
            return Some("Z".to_string());
        }
        let (sign, rest) = z.split_at(1);
        if sign != "+" && sign != "-" {
            return None;
        }
        let minutes = if let Some((h, mi)) = rest.split_once(':') {
            h.parse::<u32>().ok()? * 60 + mi.parse::<u32>().ok()?
        } else {
            let h: f64 = rest.parse().ok()?;
            (h * 60.0).round() as u32
        };
        (minutes <= 14 * 60).then(|| format!("{sign}{:02}:{:02}", minutes / 60, minutes % 60))
    });
    Some(format!(
        "{y:04}-{m:02}-{day:02}T{hh:02}:{mm:02}:{secs}{}",
        offset.unwrap_or_default()
    ))
}

/// Our names for the parameters we normalize: (parameter, our key, unit for experiment).
const NORMALIZED: &[(&str, &str)] = &[
    ("RES", "resolution_cm1"),
    ("LWN", "laser_wavenumber_cm1"),
    ("APF", "apodization"),
    ("ZFF", "zero_filling_factor"),
    ("PHZ", "phase_correction"),
    ("PHR", "phase_resolution_cm1"),
    ("BMS", "beamsplitter"),
    ("SRC", "source"),
    ("DTC", "detector"),
    ("APT", "aperture"),
    ("ACC", "accessory"),
    ("VEL", "scanner_velocity"),
    ("CHN", "measurement_channel"),
    ("INS", "instrument"),
    ("SRN", "instrument_serial"),
    ("VSN", "instrument_firmware"),
    ("SNM", "sample_name"),
    ("SFM", "sample_form"),
    ("CNM", "operator"),
    ("EXP", "experiment"),
    ("PLF", "result_spectrum"),
    ("HFW", "range_high_cm1"),
    ("LFW", "range_low_cm1"),
    ("DUR", "duration_s"),
];

/// Common apodization codes and the functions they name.
fn apodization_name(code: &str) -> Option<&'static str> {
    Some(match code {
        "BX" => "boxcar",
        "TR" => "triangular",
        "B3" => "Blackman-Harris 3-term",
        "B4" => "Blackman-Harris 4-term",
        "HG" => "Happ-Genzel",
        "NBW" => "Norton-Beer weak",
        "NBM" => "Norton-Beer medium",
        "NBS" => "Norton-Beer strong",
        "TP" => "trapezoidal",
        _ => return None,
    })
}

/// Parse an OPUS file's directory and parameter blocks.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let head = read_at(f, path, 0, 24, file_len)?;
    if head.len() < 24 || head[..4] != OPUS_MAGIC {
        return Err(Error::corrupt(
            FMT,
            "not an OPUS file: the file does not start with 0A 0A FE FE or is shorter than its 24-byte header",
        ));
    }
    let version = head.f64_at(4).unwrap_or(f64::NAN);
    let dir_start = u64::from(head.u32_at(12).unwrap_or(0));
    let max_blocks = head.u32_at(16).unwrap_or(0);
    let used_blocks = head.u32_at(20).unwrap_or(0);
    if dir_start < 24 || dir_start >= file_len || max_blocks == 0 {
        return Err(Error::corrupt_at(
            FMT,
            12,
            format!(
                "directory offset {dir_start} or size {max_blocks} is not valid for a {file_len}-byte file"
            ),
        ));
    }
    let slots = max_blocks.min(MAX_BLOCKS);
    let dir = read_at(f, path, dir_start, u64::from(slots) * 12, file_len)?;
    let mut blocks = Vec::new();
    let mut findings = Vec::new();
    for i in 0..(dir.len() / 12) {
        let at = i * 12;
        let (Some(word), Some(words), Some(offset)) =
            (dir.u32_at(at), dir.i32_at(at + 4), dir.i32_at(at + 8))
        else {
            break;
        };
        if offset <= 0 {
            break;
        }
        let Ok(words) = u64::try_from(words) else {
            findings.push(Finding::error(
                "bad_block_size",
                format!("directory entry {i}: negative block size"),
            ));
            continue;
        };
        let b = Block {
            index: i,
            word,
            ty: BlockType::of(word),
            offset: offset as u64,
            size: words * 4,
        };
        if b.offset.saturating_add(b.size) > file_len {
            findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "block {i} (type {:?}) at {} of {} bytes runs past the end of the file ({file_len} bytes)",
                        b.ty.fields(),
                        b.offset,
                        b.size
                    ),
                )
                .at(b.offset),
            );
        }
        blocks.push(b);
    }
    if blocks.is_empty() {
        return Err(Error::corrupt_at(
            FMT,
            dir_start,
            "the block directory is empty",
        ));
    }
    if used_blocks as usize != blocks.len() {
        findings.push(Finding::info(
            "block_count",
            format!(
                "the header declares {used_blocks} blocks, the directory lists {}",
                blocks.len()
            ),
        ));
    }

    // Parameter blocks (small): decode them all.
    let mut param_blocks: Vec<(usize, Params, bool)> = Vec::new(); // (block index, params, terminated)
    let mut history = None;
    for (k, b) in blocks.iter().enumerate() {
        if b.offset.saturating_add(b.size) > file_len {
            continue;
        }
        if b.ty.is_params() && b.size <= MAX_PARAM_BLOCK {
            let bytes = read_at(f, path, b.offset, b.size, file_len)?;
            let (list, done) = parse_params(&bytes);
            if !done {
                findings.push(
                    Finding::warning(
                        "unterminated_parameters",
                        format!("parameter block {} ends without an END record", b.index),
                    )
                    .at(b.offset),
                );
            }
            param_blocks.push((k, list.into_iter().collect(), done));
        } else if b.ty.is_history() && b.size <= MAX_PARAM_BLOCK {
            let bytes = read_at(f, path, b.offset, b.size, file_len)?;
            let text: Vec<String> = bytes
                .split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !text.is_empty() {
                history = Some(text.join("\n"));
            }
        }
    }

    // Pair data blocks with data-status blocks.
    let statuses: Vec<(usize, &Params)> = param_blocks
        .iter()
        .filter(|(k, _, _)| blocks[*k].ty.params == 1)
        .map(|(k, p, _)| (*k, p))
        .collect();
    let data: Vec<usize> = (0..blocks.len())
        .filter(|&k| blocks[k].ty.is_data() || blocks[k].ty.is_series())
        .collect();
    let mut used = vec![false; blocks.len()];
    let mut pairs: Vec<DataPair> = Vec::new();
    let mut unpaired = Vec::new();
    // First pass: unique type matches; then min/max disambiguation; then file order.
    let candidates: Vec<(usize, Vec<usize>)> = data
        .iter()
        .map(|&d| {
            (
                d,
                statuses
                    .iter()
                    .filter(|(s, _)| blocks[d].ty.pairs_with(blocks[*s].ty))
                    .map(|(s, _)| *s)
                    .collect(),
            )
        })
        .collect();
    let status_of = |s: usize| -> Params {
        param_blocks
            .iter()
            .find(|(k, _, _)| *k == s)
            .map(|(_, p, _)| p.clone())
            .unwrap_or_default()
    };
    let mut pending = Vec::new();
    for (d, c) in &candidates {
        if c.len() == 1 {
            pairs.push(DataPair {
                data: blocks[*d].clone(),
                status: status_of(c[0]),
                status_index: c[0],
            });
            used[c[0]] = true;
        } else if c.is_empty() {
            unpaired.push(*d);
        } else {
            pending.push((*d, c.clone()));
        }
    }
    for (d, c) in pending {
        let free: Vec<usize> = c.iter().copied().filter(|s| !used[*s]).collect();
        // prefer the status whose MNY/MXY equal the block's own extremes
        let b = &blocks[d];
        let mut chosen = None;
        if b.ty.is_data() && b.size <= 256 << 20 {
            let bytes = read_at(f, path, b.offset, b.size, file_len)?;
            for &s in &free {
                let p = status_of(s);
                if extremes_match(&bytes, &p, b.ty.extended == 4) {
                    chosen = Some(s);
                    break;
                }
            }
        }
        let chosen = chosen.or_else(|| free.first().copied());
        match chosen {
            Some(s) => {
                used[s] = true;
                pairs.push(DataPair {
                    data: b.clone(),
                    status: status_of(s),
                    status_index: s,
                });
            }
            None => unpaired.push(d),
        }
    }

    // Parameters by role: sample/result blocks (role 0) and reference blocks (role 2).
    let mut sample_params: Params = BTreeMap::new();
    let mut reference_params: Params = BTreeMap::new();
    for (k, p, _) in &param_blocks {
        let ty = blocks[*k].ty;
        if ty.params <= 1 && ty.fields() != [0, 0, 0, 0, 0, 1] {
            continue;
        }
        let target = if ty.role == 2 {
            &mut reference_params
        } else {
            &mut sample_params
        };
        for (name, v) in p {
            target.entry(name.clone()).or_insert_with(|| v.clone());
        }
    }

    let mut parsed = Parsed {
        format_version: Some(format!("{version}")),
        findings,
        ..Parsed::default()
    };

    // Order: result spectra, then sample, then reference; plain data before interferograms and phases.
    pairs.sort_by_key(|p| {
        let t = p.data.ty;
        let role = match t.role {
            3 => 0,
            1 => 1,
            2 => 2,
            _ => 3,
        };
        let kind = match t.data % 32 {
            2 | 8 => 2,
            3 => 3,
            _ => 1,
        };
        // a later copy of the same kind of block supersedes an earlier one (OPUS appends the
        // processed spectrum after the original): the later one comes first
        (
            role,
            kind,
            t.extended,
            t.derivative,
            t.part,
            std::cmp::Reverse(p.data.offset),
        )
    });
    let opus_version = history.as_deref().and_then(history_version);
    for p in &pairs {
        match make_set(f, path, file_len, p, &sample_params, &reference_params) {
            Ok((set, table)) => {
                let index = parsed.sets.len() as u32;
                if let Some(mut t) = table {
                    t.trace = index;
                    parsed.tables.push(t);
                }
                let mut set = set;
                if let Some(v) = &opus_version {
                    set.extra.insert("software_version".into(), json!(v));
                }
                parsed.sets.push(set);
            }
            Err(e) => parsed.findings.push(
                Finding::error(
                    "bad_data_block",
                    format!("data block {} not decoded: {e}", p.data.index),
                )
                .at(p.data.offset),
            ),
        }
    }
    if !unpaired.is_empty() {
        parsed.notes.push(format!(
            "{} data block(s) without a data-status block are listed by `info --view structure`, not decoded",
            unpaired.len()
        ));
    }
    if parsed.sets.is_empty() {
        parsed
            .notes
            .push("no data block could be paired with its data-status block".into());
    }

    // Facts for the experiment.
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "Bruker", "format");
    if let Some(v) = sample_params.get("SNM") {
        Facts::text(
            &mut facts.sample_name,
            &v.as_text(),
            "sample parameters SNM",
        );
    }
    if let Some(v) = sample_params.get("CNM") {
        Facts::text(&mut facts.operator, &v.as_text(), "sample parameters CNM");
    }
    if let Some(v) = sample_params.get("INS") {
        Facts::text(&mut facts.model, &v.as_text(), "instrument parameters INS");
    }
    if let Some(v) = sample_params.get("SRN") {
        Facts::text(&mut facts.serial, &v.as_text(), "instrument parameters SRN");
    }
    Facts::text(&mut facts.software, "OPUS", "format");
    if let Some(v) = &opus_version {
        Facts::text(
            &mut facts.software_version,
            v,
            "history block (\"Version … Build\")",
        );
    }
    if let Some(v) = sample_params.get("EXP") {
        Facts::text(
            &mut facts.method_name,
            &v.as_text(),
            "sample parameters EXP",
        );
    }
    if let Some(p) = pairs.first() {
        let st = &p.status;
        if let (Some(d), Some(t)) = (st.get("DAT"), st.get("TIM"))
            && let Some(iso) = opus_time(&d.as_text(), &t.as_text())
        {
            Facts::text(&mut facts.started_at, &iso, "data status DAT, TIM");
        }
    }
    let pa = Source::PriorArt;
    let inf = Source::Inferred;
    if let Some(v) = sample_params.get("RES").and_then(Param::as_f64) {
        facts.number(
            "resolution",
            v,
            Some("cm⁻¹"),
            "acquisition parameters RES",
            inf,
        );
    }
    if let Some(v) = sample_params.get("NSS").and_then(Param::as_f64) {
        facts.number("scans", v, None, "acquisition parameters NSS", inf);
    }
    if let Some(v) = reference_params
        .get("NSR")
        .or_else(|| sample_params.get("NSR"))
        .and_then(Param::as_f64)
    {
        facts.number(
            "background_scans",
            v,
            None,
            "reference acquisition parameters NSR",
            inf,
        );
    }
    if let Some(v) = sample_params.get("APF") {
        let code = v.as_text();
        let words = apodization_name(&code).map_or(code.clone(), |n| format!("{n} ({code})"));
        facts.word("apodization", &words, "FT parameters APF", inf);
    }
    if let Some(v) = sample_params.get("ZFF").and_then(Param::as_f64) {
        facts.number("zero_filling_factor", v, None, "FT parameters ZFF", inf);
    }
    if let Some(v) = sample_params.get("LWN").and_then(Param::as_f64) {
        facts.number(
            "laser_wavenumber",
            v,
            Some("cm⁻¹"),
            "instrument parameters LWN",
            inf,
        );
    }
    for (k, ours) in [
        ("BMS", "beamsplitter"),
        ("SRC", "source"),
        ("DTC", "detector"),
        ("APT", "aperture"),
        ("ACC", "accessory"),
        ("PHZ", "phase_correction"),
    ] {
        if let Some(v) = sample_params.get(k) {
            facts.word(ours, &v.as_text(), &format!("parameters {k}"), inf);
        }
    }
    let _ = pa;
    parsed.facts = facts;

    // Vendor tree and listing.
    let mut vb = Vec::new();
    for b in &blocks {
        let mut o = Map::new();
        o.insert("index".into(), json!(b.index));
        o.insert("type".into(), json!(b.ty.fields()));
        o.insert("type_word".into(), json!(format!("{:#010x}", b.word)));
        o.insert("offset".into(), json!(b.offset));
        o.insert("size".into(), json!(b.size));
        if let Some((_, p, _)) = param_blocks
            .iter()
            .find(|(k, _, _)| blocks[*k].index == b.index)
        {
            o.insert("group".into(), json!(b.ty.group_name()));
            o.insert(
                "parameters".into(),
                Value::Object(p.iter().map(|(k, v)| (k.clone(), v.json())).collect()),
            );
        }
        vb.push(Value::Object(o));
    }
    parsed.vendor = json!({
        "header": {"version": num(version), "directory_offset": dir_start, "directory_slots": max_blocks, "blocks": used_blocks},
        "blocks": vb,
        "history": history,
    });
    for b in &blocks {
        let kind = if b.ty.is_directory() {
            "directory"
        } else if b.ty.is_history() {
            "history"
        } else if b.ty.is_params() {
            "parameters"
        } else if b.ty.is_series() {
            "data_series"
        } else if b.ty.is_data() {
            "data"
        } else {
            "block"
        };
        let trace = pairs
            .iter()
            .position(|p| p.data.index == b.index || blocks[p.status_index].index == b.index);
        parsed.entries.push(LsEntry {
            kind: kind.into(),
            name: if b.ty.is_params() {
                b.ty.group_name().to_string()
            } else if b.ty.is_data() || b.ty.is_series() {
                data_kind(b.ty, None).0
            } else {
                kind.to_string()
            },
            offset: Some(b.offset),
            size: Some(b.size),
            image: None,
            details: json!({"index": b.index, "type": b.ty.fields(), "trace": trace}),
        });
    }
    for (k, v) in [
        ("format_version", pa),
        ("traces[].sample_count", pa),
        ("traces[].sweep_count", pa),
        ("traces[].extra.axis", pa),
        ("traces[].channels[].scale", pa),
        ("traces[].extra.y_quantity", pa),
        ("traces[].extra.data_type", inf),
        ("traces[].extra.acquired_at", inf),
        ("traces[].extra.resolution_cm1", inf),
        ("traces[].extra.scans", inf),
        ("traces[].extra.laser_wavenumber_cm1", inf),
        ("traces[].extra.apodization", inf),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

/// True when the stored extremes (× `CSF`) over `NPT` points equal the status block's `MNY`/`MXY`.
fn extremes_match(bytes: &[u8], p: &Params, compact: bool) -> bool {
    let npt = p.get("NPT").and_then(Param::as_f64).unwrap_or(0.0);
    let csf = p.get("CSF").and_then(Param::as_f64).unwrap_or(1.0);
    let (Some(mny), Some(mxy)) = (
        p.get("MNY").and_then(Param::as_f64),
        p.get("MXY").and_then(Param::as_f64),
    ) else {
        return false;
    };
    let n = npt as usize;
    if n == 0 || bytes.len() < n * 4 {
        return false;
    }
    let start = if compact { bytes.len() - n * 4 } else { 0 };
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for c in bytes[start..start + n * 4].as_chunks::<4>().0 {
        let v = f64::from(f32::from_le_bytes(*c)) * csf;
        lo = lo.min(v);
        hi = hi.max(v);
    }
    (lo - mny).abs() <= 1e-6 * mny.abs().max(1e-12)
        && (hi - mxy).abs() <= 1e-6 * mxy.abs().max(1e-12)
}

fn make_set(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    p: &DataPair,
    sample: &Params,
    reference: &Params,
) -> Result<(SpectrumSet, Option<SpectrumTable>)> {
    let st = &p.status;
    let npt = st
        .get("NPT")
        .and_then(Param::as_f64)
        .filter(|v| *v >= 1.0 && *v <= 1e9)
        .ok_or_else(|| Error::corrupt(FMT, "data status NPT missing or not positive"))?
        as u64;
    let fxv = st.get("FXV").and_then(Param::as_f64).unwrap_or(f64::NAN);
    let lxv = st.get("LXV").and_then(Param::as_f64).unwrap_or(f64::NAN);
    let csf = st
        .get("CSF")
        .and_then(Param::as_f64)
        .filter(|v| v.is_finite() && *v != 0.0)
        .unwrap_or(1.0);
    let dxu = st.get("DXU").map(Param::as_text);
    let stored = match st.get("DPF").and_then(Param::as_f64) {
        Some(v) if (v - 2.0).abs() < 0.5 => Stored::I32,
        _ => Stored::F32,
    };
    let ty = p.data.ty;
    let (name, y, y_unit, dtype) = data_kind(ty, dxu.as_deref());
    let (xq, xu) = x_axis(dxu.as_deref());
    let params = if ty.role == 2 { reference } else { sample };
    let mut extra: BTreeMap<String, Value> = BTreeMap::new();
    let role = match ty.role {
        1 => "sample",
        2 => "reference",
        3 => "result",
        _ => "other",
    };
    extra.insert("spectrum_role".into(), json!(role));
    extra.insert("block_type".into(), json!(ty.fields()));
    if let (Some(d), Some(t)) = (st.get("DAT"), st.get("TIM"))
        && let Some(iso) = opus_time(&d.as_text(), &t.as_text())
    {
        extra.insert("acquired_at".into(), json!(iso));
    }
    for (k, ours) in NORMALIZED {
        let v = if *k == "RES" || *k == "APF" || *k == "ZFF" || *k == "PHZ" || *k == "PHR" {
            params.get(*k).or_else(|| sample.get(*k))
        } else {
            params.get(*k).or_else(|| sample.get(*k))
        };
        if let Some(v) = v {
            match v {
                Param::Text(s) if s.trim().is_empty() => {}
                Param::Text(s) if *k == "ZFF" && s.trim().parse::<f64>().is_ok() => {
                    extra.insert((*ours).into(), num(s.trim().parse().unwrap_or(f64::NAN)));
                }
                Param::Text(s) => {
                    extra.insert((*ours).into(), json!(s.trim()));
                }
                other => {
                    extra.insert((*ours).into(), other.json());
                }
            }
        }
    }
    if let Some(Value::String(code)) = extra.get("apodization").cloned()
        && let Some(n) = apodization_name(&code)
    {
        extra.insert("apodization_name".into(), json!(n));
    }
    let scans = if ty.role == 2 {
        params.get("NSR").or_else(|| sample.get("NSR"))
    } else {
        params.get("NSS")
    };
    if let Some(v) = scans.and_then(Param::as_f64) {
        extra.insert("scans".into(), json!(v as i64));
    }
    if ty.role != 2
        && let Some(v) = reference.get("NSR").and_then(Param::as_f64)
    {
        extra.insert("background_scans".into(), json!(v as i64));
    }
    if let Some(v) = st.get("MNY").and_then(Param::as_f64) {
        extra.insert("stored_y_min".into(), num(v));
    }
    if let Some(v) = st.get("MXY").and_then(Param::as_f64) {
        extra.insert("stored_y_max".into(), num(v));
    }
    if let Some(d) = &dxu {
        extra.insert("x_units_code".into(), json!(d));
    }
    extra.insert("data_block".into(), json!(p.data.index));

    let block_end = p.data.offset.saturating_add(p.data.size);
    if ty.is_series() {
        let head = read_at(f, path, p.data.offset, 24, file_len)?;
        let (Some(n), Some(first), Some(data_size), Some(info_size)) = (
            head.i32_at(4),
            head.i32_at(8),
            head.i32_at(12),
            head.i32_at(16),
        ) else {
            return Err(Error::corrupt_at(
                FMT,
                p.data.offset,
                "series block header is truncated",
            ));
        };
        let (n, first, data_size, info_size) = (
            u64::try_from(n).unwrap_or(0),
            u64::try_from(first).unwrap_or(0),
            u64::try_from(data_size).unwrap_or(0),
            u64::try_from(info_size).unwrap_or(0),
        );
        if data_size < npt * 4 || n == 0 {
            return Err(Error::corrupt_at(
                FMT,
                p.data.offset,
                format!("series block: {n} spectra of {data_size} bytes cannot hold {npt} points"),
            ));
        }
        let stride = data_size + info_size;
        // spectra actually stored (the block may end early)
        let room = p.data.size.saturating_sub(first) / stride.max(1);
        let count = n.min(room).min(u64::from(u32::MAX)) as u32;
        if count == 0 {
            return Err(Error::corrupt_at(
                FMT,
                p.data.offset,
                "series block holds no complete spectrum",
            ));
        }
        let base = p.data.offset + first;
        // per-spectrum records follow each spectrum: 8 int32, 4 float64, 2 int32, 7 float64,
        // 2 int32, 2 float64 (scans … start and end time)
        let mut scans = Vec::new();
        let mut start_t = Vec::new();
        let mut end_t = Vec::new();
        if info_size >= 152 {
            for k in 0..u64::from(count) {
                let at = base + k * stride + data_size;
                let r = read_at(f, path, at, 152, file_len)?;
                scans.push(r.i32_at(0).map_or(f64::NAN, f64::from));
                start_t.push(r.f64_at(136).unwrap_or(f64::NAN));
                end_t.push(r.f64_at(144).unwrap_or(f64::NAN));
            }
        }
        extra.insert("series".into(), json!(true));
        let table = (!scans.is_empty()).then(|| SpectrumTable {
            name: format!("{name} spectra"),
            trace: 0,
            columns: vec![
                ("spectrum".into(), None, (0..count).map(f64::from).collect()),
                ("scans".into(), None, scans),
                // units not established (no public series file)
                ("start_time".into(), None, start_t),
                ("end_time".into(), None, end_t),
            ],
        });
        return Ok((
            SpectrumSet {
                name,
                x_quantity: xq,
                x_unit: xu.map(str::to_string),
                x: XValues::Regular {
                    first: fxv,
                    last: lxv,
                },
                y_name: y.into(),
                y_unit: y_unit.map(str::to_string),
                points: npt,
                count,
                rows: Rows::Strided {
                    first: base,
                    stride,
                },
                stored,
                scale: csf,
                data_type: dtype,
                extra,
            },
            table,
        ));
    }
    let need = npt * 4;
    if p.data.size < need {
        return Err(Error::corrupt_at(
            FMT,
            p.data.offset,
            format!(
                "data block of {} bytes cannot hold NPT = {npt} values",
                p.data.size
            ),
        ));
    }
    let start = if ty.extended == 4 {
        block_end - need
    } else {
        p.data.offset
    };
    Ok((
        SpectrumSet {
            name,
            x_quantity: xq,
            x_unit: xu.map(str::to_string),
            x: XValues::Regular {
                first: fxv,
                last: lxv,
            },
            y_name: y.into(),
            y_unit: y_unit.map(str::to_string),
            points: npt,
            count: 1,
            rows: Rows::Listed(vec![start]),
            stored,
            scale: csf,
            data_type: dtype,
            extra,
        },
        None,
    ))
}

/// The OPUS version that wrote the history block: the token after the first `Version ` that is
/// followed by ` Build` (`Version 8.5(SP1) Build: 8, 7, 10 20200710` → `8.5(SP1)`).
pub(crate) fn history_version(history: &str) -> Option<String> {
    let mut rest = history;
    while let Some(i) = rest.find("Version ") {
        let after = &rest[i + "Version ".len()..];
        let end = after.find(" Build").filter(|&e| e > 0 && e <= 24)?;
        let v = after[..end].trim();
        if v.starts_with(|c: char| c.is_ascii_digit()) && !v.contains(char::is_whitespace) {
            return Some(v.to_string());
        }
        rest = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_versions() {
        assert_eq!(
            history_version("Operator:Admin\tVersion 8.1 Build: 8, 1, 29 20180416\tx").as_deref(),
            Some("8.1")
        );
        assert_eq!(
            history_version("Version 8.5(SP1) Build: 8, 7, 10 20200710").as_deref(),
            Some("8.5(SP1)")
        );
        assert_eq!(history_version("Version unknown; no build"), None);
    }

    #[test]
    fn block_type_fields() {
        // absorbance result: part 3, role 3, data 4
        let t = BlockType::of(0x4000_100f);
        assert_eq!(t.fields(), [3, 3, 0, 4, 0, 0]);
        assert!(t.is_data());
        let s = BlockType::of(0x4000_101f);
        assert!(t.pairs_with(s));
        assert!(BlockType::of(0x0000_3400).is_directory());
        assert!(BlockType::of(0x4068_0000).is_history());
        assert_eq!(BlockType::of(0x0070_100f).extended, 6);
    }

    #[test]
    fn times() {
        assert_eq!(
            opus_time("11/05/2020", "08:59:44.322 (GMT+12)").as_deref(),
            Some("2020-05-11T08:59:44.322+12:00")
        );
        assert_eq!(
            opus_time("2012/11/09", "11:09:33 (GMT-6)").as_deref(),
            Some("2012-11-09T11:09:33-06:00")
        );
        assert_eq!(
            opus_time("03/05/2019", "13:34:44.641 (GMT-4)").as_deref(),
            Some("2019-05-03T13:34:44.641-04:00")
        );
        assert_eq!(opus_time("x", "y"), None);
        assert_eq!(opus_time("31/13/2020", "00:00:00"), None);
    }

    #[test]
    fn params_records() {
        let mut b = Vec::new();
        b.extend_from_slice(b"NPT\0");
        b.extend_from_slice(&0i16.to_le_bytes());
        b.extend_from_slice(&2i16.to_le_bytes());
        b.extend_from_slice(&42i32.to_le_bytes());
        b.extend_from_slice(b"RES\0");
        b.extend_from_slice(&1i16.to_le_bytes());
        b.extend_from_slice(&4i16.to_le_bytes());
        b.extend_from_slice(&4.0f64.to_le_bytes());
        b.extend_from_slice(b"BMS\0");
        b.extend_from_slice(&2i16.to_le_bytes());
        b.extend_from_slice(&2i16.to_le_bytes());
        b.extend_from_slice(b"KBr\0");
        b.extend_from_slice(b"END\0\0\0\0\0");
        let (p, done) = parse_params(&b);
        assert!(done);
        assert_eq!(p[0], ("NPT".into(), Param::Int(42)));
        assert_eq!(p[1], ("RES".into(), Param::Float(4.0)));
        assert_eq!(p[2], ("BMS".into(), Param::Text("KBr".into())));
        // truncated in the middle of a value: no panic, not terminated
        let (p, done) = parse_params(&b[..20]);
        assert!(!done);
        assert_eq!(p.len(), 1);
    }
}
