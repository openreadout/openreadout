//! MGF (Mascot generic format) peak lists — the dataset.
//!
//! An MGF file is text: optional `KEY=value` lines that apply to the whole file, then one
//! block per MS/MS spectrum between `BEGIN IONS` and `END IONS`. A block opens with its own
//! `KEY=value` lines (`PEPMASS`, `CHARGE`, `TITLE`, `RTINSECONDS`, `SCANS`, ...) and continues
//! with one peak per line (`m/z intensity [charge]`). Matrix Science publishes the syntax
//! (`docs/formats/mgf.md`). The file holds no index, so `open` reads it once, line by line,
//! and keeps the byte range and the parameters of every block; peaks are parsed when a
//! spectrum is read.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use openreadout_core::model::{CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry};
use openreadout_core::model::{SpectraInfo, Spectrum};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::Fs;
use openreadout_core::{Error, Plane, Result};

pub(crate) const FMT: &str = "mgf";

/// Longest line accepted (bytes); longer lines make the file corrupt.
const MAX_LINE: usize = 16 << 20;
/// Parameters kept per block and in the file header (more are counted, not kept).
const MAX_PARAMS: usize = 1000;
/// Longest parameter value kept (characters); longer values are cut.
const MAX_VALUE: usize = 4096;
/// Spectra listed by `info --view structure`.
const LS_SPECTRA: usize = 1000;
/// Problems kept for `check` and the notes.
const MAX_PROBLEMS: usize = 20;

/// One `BEGIN IONS` ... `END IONS` block, as found by the scan at open.
#[derive(Debug, Clone, Default)]
struct Block {
    /// Byte offset of the `BEGIN IONS` line.
    offset: u64,
    /// Byte offset just past the `END IONS` line.
    end: u64,
    /// The block's parameters in file order, keys upper-cased.
    params: Vec<(String, String)>,
    /// Peak lines counted by the scan.
    peaks: u64,
}

/// Values of a block's parameters, interpreted.
#[derive(Debug, Clone, Default, PartialEq)]
struct Precursors {
    mz: Vec<f64>,
    intensity: Option<f64>,
    charges: Vec<i32>,
}

/// An open MGF file.
#[derive(Debug)]
pub struct MgfDataset {
    path: PathBuf,
    fs: Fs,
    file_len: u64,
    /// File-level parameters (before the first block), keys upper-cased.
    header: Vec<(String, String)>,
    blocks: Vec<Block>,
    /// Lines outside blocks that hold a number: peptide-mass-fingerprint queries.
    pmf_queries: u64,
    /// Comment lines outside blocks.
    comments: u64,
    /// What the scan found wrong (structure problems; peaks are checked by `check`).
    problems: Vec<String>,
    /// The file ends inside a block.
    truncated: bool,
    notes: Vec<String>,
}

/// The upper-cased key and the value of a `KEY=value` line, when it is one: the key starts with
/// a letter or `_` and holds no space (`KEY[0]` indices are allowed).
fn param(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once('=')?;
    let k = k.trim_end();
    let first = k.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_')
        || k.chars().any(char::is_whitespace)
        || !k
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_[]-.".contains(c))
    {
        return None;
    }
    let mut v = v.trim().to_string();
    if v.len() > MAX_VALUE {
        let mut cut = MAX_VALUE;
        while !v.is_char_boundary(cut) {
            cut -= 1;
        }
        v.truncate(cut);
    }
    Some((k.to_ascii_uppercase(), v))
}

fn is_comment(line: &str) -> bool {
    matches!(line.as_bytes().first(), Some(b'#' | b';' | b'!' | b'/'))
}

fn starts_with_number(line: &str) -> bool {
    let b = line.as_bytes();
    let i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    b.get(i).is_some_and(|c| {
        c.is_ascii_digit() || (*c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
    })
}

fn finite(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// One charge such as `2+`, `3-`, `+2`, `2` → signed integer.
fn one_charge(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (digits, sign) = if let Some(d) = s.strip_suffix('+') {
        (d, 1)
    } else if let Some(d) = s.strip_suffix('-') {
        (d, -1)
    } else if let Some(d) = s.strip_prefix('+') {
        (d, 1)
    } else if let Some(d) = s.strip_prefix('-') {
        (d, -1)
    } else {
        (s, 1)
    };
    let n: i32 = digits.trim().parse().ok()?;
    n.checked_mul(sign)
}

/// A `CHARGE` value: one charge or several (`2+ and 3+`, `2+,3+`).
pub(crate) fn parse_charges(s: &str) -> Vec<i32> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case("and"))
        .filter_map(one_charge)
        .collect()
}

/// A retention time or a range `a-b` (seconds) → (start, end of the range).
pub(crate) fn parse_rt(s: &str) -> Option<(f64, Option<f64>)> {
    let s = s.trim();
    if let Some(v) = finite(s) {
        return Some((v, None));
    }
    // a range: split at a '-' that is not a sign or part of an exponent
    let b = s.as_bytes();
    for i in 1..b.len() {
        if b[i] == b'-' && !matches!(b[i - 1], b'e' | b'E' | b'-') {
            let (a, z) = (finite(&s[..i])?, finite(&s[i + 1..])?);
            return Some((a, Some(z)));
        }
    }
    None
}

/// The parameter `key` of `params` (the first one).
fn get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Every `PEPMASS` line of a block: m/z of each, the first one's intensity, and the charges:
/// those printed on the first `PEPMASS` line (the charge of that precursor), else the block's
/// `CHARGE`, else the file's `CHARGE`, which applies to every query that names none.
fn precursors(params: &[(String, String)], header: &[(String, String)]) -> Precursors {
    let mut p = Precursors::default();
    let mut pep_charges = Vec::new();
    for (k, v) in params {
        if k != "PEPMASS" {
            continue;
        }
        let mut words = v.split_whitespace();
        let Some(mz) = words.next().and_then(finite) else {
            continue;
        };
        if p.mz.is_empty() {
            p.intensity = words.next().and_then(finite);
            pep_charges = words.flat_map(parse_charges).collect();
        }
        p.mz.push(mz);
    }
    p.charges = if pep_charges.is_empty() {
        get(params, "CHARGE")
            .or_else(|| get(header, "CHARGE"))
            .map(parse_charges)
            .unwrap_or_default()
    } else {
        pep_charges
    };
    p
}

/// The native id printed in a ProteoWizard-style title (`... NativeID:"controllerType=0 ... scan=17"`).
fn native_id_in(title: &str) -> Option<String> {
    let i = title.find("NativeID:\"")?;
    let rest = &title[i + 10..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// What kind of MGF the parameters say it is (`docs/formats/mgf.md` § Dialects).
fn dialect(header: &[(String, String)], first: Option<&Block>) -> &'static str {
    let has = |k: &str| first.is_some_and(|b| b.params.iter().any(|(x, _)| x == k));
    if has("SPECTRUMID")
        || first
            .and_then(|b| get(&b.params, "ORGANISM"))
            .is_some_and(|o| o.starts_with("GNPS"))
    {
        "spectral library (GNPS)"
    } else if has("FEATURE_ID") {
        "feature export (FEATURE_ID)"
    } else if header
        .iter()
        .any(|(k, _)| matches!(k.as_str(), "SEARCH" | "DB" | "CLE" | "MODS" | "COM"))
    {
        "search submission (file parameters)"
    } else if has("SEQ") {
        "annotated peptide spectra (SEQ)"
    } else {
        "peak list"
    }
}

/// The software the file names as its writer: a `COM` or `USERNAME` file parameter that names
/// OpenMS, a GNPS library record, or a ProteoWizard-style title.
fn writer(header: &[(String, String)], first: Option<&Block>) -> Option<&'static str> {
    if header
        .iter()
        .any(|(k, v)| (k == "COM" || k == "USERNAME") && v.contains("OpenMS"))
    {
        return Some("OpenMS");
    }
    let p = first.map_or(&[][..], |b| b.params.as_slice());
    if get(p, "SPECTRUMID").is_some_and(|s| s.starts_with("CCMSLIB"))
        || get(p, "ORGANISM").is_some_and(|o| o.starts_with("GNPS"))
    {
        return Some("GNPS");
    }
    if get(p, "TITLE").is_some_and(|t| t.contains("NativeID:\"")) {
        return Some("ProteoWizard");
    }
    None
}

/// The scan number a block names: `SCANS` when it holds one whole number.
fn scan_of(params: &[(String, String)]) -> Option<u64> {
    get(params, "SCANS").and_then(|s| s.trim().parse::<u64>().ok())
}

fn ms_level_of(params: &[(String, String)]) -> u32 {
    get(params, "MSLEVEL")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|l| *l > 0)
        .unwrap_or(2)
}

fn polarity_of(params: &[(String, String)]) -> &'static str {
    let mode = get(params, "IONMODE").or_else(|| get(params, "ION_MODE"));
    match mode.map(|m| m.trim().to_ascii_lowercase()) {
        Some(m) if m.starts_with("pos") => "positive",
        Some(m) if m.starts_with("neg") => "negative",
        _ => "unknown",
    }
}

/// One peak line: m/z, intensity and an optional charge; further text is ignored.
fn peak(line: &str) -> std::result::Result<(f64, f64, Option<i32>), String> {
    let mut w = line.split_whitespace();
    let mz = w
        .next()
        .and_then(finite)
        .ok_or_else(|| format!("peak line {line:?}: no m/z"))?;
    let int = w
        .next()
        .and_then(finite)
        .ok_or_else(|| format!("peak line {line:?}: no intensity"))?;
    let z = w.next().and_then(one_charge);
    Ok((mz, int, z))
}

/// Read `r` line by line into `line` (without the line ending); returns the bytes consumed,
/// 0 at the end.
fn next_line(r: &mut dyn BufRead, buf: &mut Vec<u8>) -> Result<usize> {
    buf.clear();
    let n = r
        .take(MAX_LINE as u64 + 1)
        .read_until(b'\n', buf)
        .map_err(|e| Error::corrupt(FMT, format!("read error: {e}")))?;
    if buf.len() > MAX_LINE {
        return Err(Error::corrupt(
            FMT,
            format!("a line longer than {MAX_LINE} bytes: this is not an MGF peak list"),
        ));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    Ok(n)
}

impl MgfDataset {
    /// Open `path` in the namespace `fs`: one pass over the file to find every block.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let file_len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        let f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let mut r = BufReader::with_capacity(1 << 20, f);
        let mut ds = MgfDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            file_len,
            header: Vec::new(),
            blocks: Vec::new(),
            pmf_queries: 0,
            comments: 0,
            problems: Vec::new(),
            truncated: false,
            notes: Vec::new(),
        };
        let mut buf = Vec::new();
        let mut at = 0u64;
        let mut cur: Option<Block> = None;
        let mut extra_params = 0u64;
        let mut line_no = 0u64;
        let problem = |ds: &mut MgfDataset, p: String| {
            if ds.problems.len() < MAX_PROBLEMS {
                ds.problems.push(p);
            }
        };
        loop {
            let start = at;
            let n = next_line(&mut r, &mut buf)?;
            if n == 0 {
                break;
            }
            at += n as u64;
            line_no += 1;
            let text = String::from_utf8_lossy(&buf);
            let line = text.trim_start_matches('\u{feff}').trim();
            if line.is_empty() {
                continue;
            }
            match &mut cur {
                None => {
                    if line.eq_ignore_ascii_case("BEGIN IONS") {
                        cur = Some(Block {
                            offset: start,
                            ..Block::default()
                        });
                    } else if line.eq_ignore_ascii_case("END IONS") {
                        problem(
                            &mut ds,
                            format!("line {line_no}: END IONS without BEGIN IONS"),
                        );
                    } else if is_comment(line) {
                        ds.comments += 1;
                    } else if let Some(kv) = param(line) {
                        if ds.header.len() < MAX_PARAMS {
                            ds.header.push(kv);
                        }
                    } else if starts_with_number(line) {
                        ds.pmf_queries += 1;
                    } else if ds.blocks.is_empty() && line_no <= 3 {
                        // the first lines are neither parameters nor blocks: not MGF
                        return Err(Error::corrupt(
                            FMT,
                            format!(
                                "line {line_no} is not an MGF line ({:?})",
                                line.chars().take(60).collect::<String>()
                            ),
                        ));
                    } else {
                        problem(
                            &mut ds,
                            format!("line {line_no}: not an MGF line outside a block"),
                        );
                    }
                }
                Some(b) => {
                    if line.eq_ignore_ascii_case("END IONS") {
                        b.end = at;
                        ds.blocks.push(std::mem::take(b));
                        cur = None;
                    } else if line.eq_ignore_ascii_case("BEGIN IONS") {
                        let ofs = b.offset;
                        problem(
                            &mut ds,
                            format!(
                                "block at byte {ofs}: BEGIN IONS before its END IONS (the block is dropped)"
                            ),
                        );
                        cur = Some(Block {
                            offset: start,
                            ..Block::default()
                        });
                    } else if starts_with_number(line) {
                        b.peaks += 1;
                    } else if let Some(kv) = param(line) {
                        if b.params.len() < MAX_PARAMS {
                            b.params.push(kv);
                        } else {
                            extra_params += 1;
                        }
                    } else if is_comment(line) {
                        // comments are not allowed in a block; they carry no data
                    } else {
                        let ofs = b.offset;
                        problem(
                            &mut ds,
                            format!(
                                "line {line_no} (block at byte {ofs}): neither a parameter nor a peak"
                            ),
                        );
                    }
                }
            }
        }
        if let Some(b) = cur {
            ds.truncated = true;
            ds.notes.push(format!(
                "the file ends inside the block that starts at byte {} (no END IONS): that block is left out",
                b.offset
            ));
        }
        if ds.blocks.is_empty() && ds.header.is_empty() && ds.pmf_queries == 0 {
            return Err(Error::corrupt(FMT, "no BEGIN IONS block and no parameters"));
        }
        if extra_params > 0 {
            ds.notes.push(format!(
                "{extra_params} block parameters past the first {MAX_PARAMS} of their block are not kept"
            ));
        }
        if ds.pmf_queries > 0 {
            ds.notes.push(format!(
                "{} peptide-mass-fingerprint lines outside blocks are counted (`extra.pmf_queries`), not read as spectra",
                ds.pmf_queries
            ));
        }
        if !ds.problems.is_empty() {
            ds.notes.push(format!(
                "structure problems: {}",
                ds.problems
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        Ok(ds)
    }

    /// The spectrum of block `i` without its peaks.
    #[allow(clippy::many_single_char_names)]
    fn interpret(&self, i: usize) -> Result<Spectrum> {
        let b = self.blocks.get(i).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum index {i} out of range (the file has {} spectra)",
                self.blocks.len()
            ))
        })?;
        let p = &b.params;
        let pre = precursors(p, &self.header);
        let title = get(p, "TITLE");
        let mut sp = Spectrum {
            index: i as u64,
            scan_number: scan_of(p).unwrap_or(i as u64 + 1),
            ms_level: ms_level_of(p),
            polarity: polarity_of(p).into(),
            // Mascot accepts centroided peak lists only (docs/formats/mgf.md)
            centroided: true,
            precursor_mz: pre.mz.first().copied(),
            precursor_intensity: pre.intensity,
            precursor_charge: (pre.charges.len() == 1).then(|| pre.charges[0]),
            native_id: title.and_then(native_id_in),
            ..Spectrum::default()
        };
        if let Some(v) = get(p, "RTINSECONDS") {
            match parse_rt(v) {
                Some((a, z)) => {
                    sp.rt_s = Some(a);
                    if let Some(z) = z {
                        sp.extra.insert("rt_range_s".into(), json!([a, z]));
                    }
                }
                None => {
                    sp.extra.insert("rt_unparsed".into(), json!(v));
                }
            }
        }
        if let Some(t) = title {
            sp.extra.insert("title".into(), json!(t));
        }
        if let Some(s) = get(p, "SCANS") {
            sp.extra.insert("scans".into(), json!(s));
        }
        if pre.charges.len() > 1 {
            sp.extra.insert("charges".into(), json!(pre.charges));
        }
        if pre.mz.len() > 1 {
            sp.extra.insert("precursors".into(), json!(pre.mz));
        }
        let params: serde_json::Map<String, Value> =
            p.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        sp.extra.insert("params".into(), Value::Object(params));
        Ok(sp)
    }

    /// The text of block `i`.
    fn block_text(&self, i: usize) -> Result<String> {
        let b = &self.blocks[i];
        let len = usize::try_from(b.end.saturating_sub(b.offset))
            .map_err(|_| Error::corrupt(FMT, "block too large"))?;
        let f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let mut bytes = vec![0u8; len];
        f.read_exact_at(b.offset, &mut bytes)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// Spectrum `i` with its peaks, in file order.
    pub fn spectrum(&self, i: usize) -> Result<Spectrum> {
        let mut sp = self.interpret(i)?;
        let text = self.block_text(i)?;
        let offset = self.blocks[i].offset;
        let mut charges: Vec<Option<i32>> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if !starts_with_number(line) {
                continue;
            }
            let (mz, int, z) = peak(line)
                .map_err(|e| Error::corrupt_at(FMT, offset, format!("spectrum {i}: {e}")))?;
            sp.mz.push(mz);
            sp.intensity.push(int as f32);
            charges.push(z);
        }
        if charges.iter().any(Option::is_some) {
            sp.extra.insert("peak_charges".into(), json!(charges));
        }
        Ok(sp)
    }

    fn spectra_info(&self) -> SpectraInfo {
        let mut levels: BTreeMap<u32, u64> = BTreeMap::new();
        let mut charge_states: BTreeMap<String, u64> = BTreeMap::new();
        let mut pol = BTreeSet::new();
        let mut rt: Option<[f64; 2]> = None;
        let (mut points, mut with_rt, mut chimeric) = (0u64, 0u64, 0u64);
        let mut keys: BTreeMap<String, u64> = BTreeMap::new();
        for b in &self.blocks {
            *levels.entry(ms_level_of(&b.params)).or_default() += 1;
            pol.insert(polarity_of(&b.params));
            points += b.peaks;
            let pre = precursors(&b.params, &self.header);
            if pre.mz.len() > 1 {
                chimeric += 1;
            }
            let label = match pre.charges.as_slice() {
                [] => "none".to_string(),
                [c] => c.to_string(),
                _ => "several".to_string(),
            };
            *charge_states.entry(label).or_default() += 1;
            if let Some((a, z)) = get(&b.params, "RTINSECONDS").and_then(parse_rt) {
                with_rt += 1;
                let z = z.unwrap_or(a);
                rt = Some(rt.map_or([a, z], |[lo, hi]| [lo.min(a), hi.max(z)]));
            }
            for (k, _) in &b.params {
                *keys.entry(k.clone()).or_default() += 1;
            }
        }
        let first = self.blocks.first();
        let mut extra = BTreeMap::new();
        extra.insert("ms_level_counts".into(), json!(levels));
        extra.insert("polarities".into(), json!(pol));
        extra.insert("total_points".into(), json!(points));
        extra.insert("charge_states".into(), json!(charge_states));
        extra.insert("spectra_with_rt".into(), json!(with_rt));
        extra.insert("chimeric_spectra".into(), json!(chimeric));
        extra.insert("block_parameters".into(), json!(keys));
        extra.insert("dialect".into(), json!(dialect(&self.header, first)));
        if let Some(w) = writer(&self.header, first) {
            extra.insert("writer".into(), json!(w));
        }
        if !self.header.is_empty() {
            let h: serde_json::Map<String, Value> = self
                .header
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            extra.insert("file_parameters".into(), Value::Object(h));
        }
        if self.pmf_queries > 0 {
            extra.insert("pmf_queries".into(), json!(self.pmf_queries));
        }
        SpectraInfo {
            index: 0,
            name: first
                .and_then(|b| get(&b.params, "RAWFILE"))
                .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f).to_string()),
            scan_count: self.blocks.len() as u64,
            ms_levels: levels.keys().copied().collect(),
            rt_range_s: rt,
            instrument: None,
            extra,
        }
    }

    fn run_check(&self, deep: bool) -> CheckReport {
        let mut rep = CheckReport::new(self.path.display().to_string(), FMT);
        rep.performed("found every BEGIN IONS / END IONS block and its parameters");
        if self.truncated {
            rep.push(Finding::error(
                "truncated",
                "the file ends inside a block (no END IONS): interrupted write or copy",
            ));
        }
        for p in &self.problems {
            rep.push(Finding::warning("structure", p.clone()));
        }
        let no_pepmass = self
            .blocks
            .iter()
            .filter(|b| precursors(&b.params, &[]).mz.is_empty())
            .count();
        if no_pepmass > 0 {
            rep.push(Finding::warning(
                "no_pepmass",
                format!(
                    "{no_pepmass} blocks have no usable PEPMASS line, which every MS/MS query needs"
                ),
            ));
        }
        if !deep {
            return rep;
        }
        rep.performed("parsed every peak line (m/z and intensity numbers) and compared the count with the scan");
        let mut bad = 0usize;
        for i in 0..self.blocks.len() {
            match self.spectrum(i) {
                Ok(sp) if sp.mz.len() as u64 == self.blocks[i].peaks => {}
                Ok(sp) => {
                    bad += 1;
                    if bad == 1 {
                        rep.push(
                            Finding::error(
                                "peak_count",
                                format!(
                                    "spectrum {i}: {} peaks parsed, {} peak lines counted",
                                    sp.mz.len(),
                                    self.blocks[i].peaks
                                ),
                            )
                            .at(self.blocks[i].offset),
                        );
                    }
                }
                Err(e) => {
                    bad += 1;
                    if bad == 1 {
                        rep.push(
                            Finding::error("bad_peak", e.to_string()).at(self.blocks[i].offset),
                        );
                    }
                }
            }
        }
        if bad > 1 {
            rep.push(Finding::error(
                "bad_peak",
                format!("{bad} spectra failed to parse in total"),
            ));
        }
        rep
    }
}

impl Dataset for MgfDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: descriptor(),
            format_version: None,
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![self.spectra_info()],
            traces: Vec::new(),
            plane_count: 0,
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h: serde_json::Map<String, Value> = self
            .header
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        Ok(json!({
            "file_parameters": h,
            "blocks": self.blocks.len(),
            "comment_lines": self.comments,
            "pmf_queries": self.pmf_queries,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        for k in [
            "spectra[0].scan_count",
            "spectra[0].rt_range_s",
            "spectra[0].extra.charge_states",
            "spectra[0].extra.file_parameters",
            "spectra[0].extra.total_points",
        ] {
            m.insert(k.into(), Source::Spec);
        }
        // MS/MS unless MSLEVEL says otherwise (a convention of library exports)
        m.insert("spectra[0].ms_levels".into(), Source::Spec);
        m.insert("spectra[0].extra.dialect".into(), Source::Inferred);
        m.insert("spectra[0].extra.writer".into(), Source::Inferred);
        m.insert("spectra[0].extra.polarities".into(), Source::Inferred);
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (i, b) in self.blocks.iter().enumerate().take(LS_SPECTRA) {
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: get(&b.params, "TITLE").map_or_else(|| format!("block {i}"), str::to_string),
                offset: Some(b.offset),
                size: Some(b.end.saturating_sub(b.offset)),
                image: None,
                details: json!({"index": i, "peaks": b.peaks}),
            });
        }
        if self.blocks.len() > LS_SPECTRA {
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: format!("… {} more spectra", self.blocks.len() - LS_SPECTRA),
                offset: None,
                size: None,
                image: None,
                details: json!({"listed": LS_SPECTRA, "total": self.blocks.len()}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FMT,
            "image planes",
            "MGF holds MS/MS peak lists, not images: use `openreadout scans` or `openreadout spectrum`.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        Ok(self.run_check(true))
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        Ok(self.run_check(false))
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        if run != 0 {
            return Ok(None);
        }
        Ok(Some(
            self.blocks.iter().map(|b| ms_level_of(&b.params)).collect(),
        ))
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} out of range (an MGF file holds one list of spectra)"
            )));
        }
        let start = usize::try_from(first).unwrap_or(usize::MAX);
        for i in start..self.blocks.len() {
            let mut h = openreadout_core::ScanHeader::from(self.interpret(i)?);
            h.point_count = Some(self.blocks[i].peaks);
            if !visit(h) {
                break;
            }
        }
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "run {index} out of range (an MGF file holds one list of spectra)"
            )));
        }
        let i = usize::try_from(spectrum)
            .map_err(|_| Error::Usage("spectrum index too large".into()))?;
        self.spectrum(i)
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        if run != 0 {
            return Ok(None);
        }
        if let Some(i) = self
            .blocks
            .iter()
            .position(|b| scan_of(&b.params) == Some(scan_number))
        {
            return Ok(Some(i as u64));
        }
        if self.blocks.iter().all(|b| scan_of(&b.params).is_none()) {
            // without SCANS, spectra are numbered by position from 1
            let i = scan_number
                .checked_sub(1)
                .filter(|i| *i < self.blocks.len() as u64);
            return i.map(Some).ok_or_else(|| {
                Error::Usage(format!(
                    "no spectrum {scan_number}: the file has {} (numbered from 1 by position, as no block has SCANS)",
                    self.blocks.len()
                ))
            });
        }
        Err(Error::Usage(format!("no block with SCANS={scan_number}")))
    }
}

/// True when the start of a file looks like MGF: within its first lines, a `BEGIN IONS` line,
/// or `KEY=value` file parameters followed by one.
pub(crate) fn looks_like_mgf(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&head[..head.len().min(64 << 10)]);
    let mut lines = 0;
    for raw in text.lines() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || is_comment(line) {
            continue;
        }
        if line.eq_ignore_ascii_case("BEGIN IONS") {
            return true;
        }
        if param(line).is_none() {
            return false;
        }
        lines += 1;
        if lines > 200 {
            return false;
        }
    }
    false
}

pub(crate) fn descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: FMT.into(),
        name: "MGF (Mascot generic format) peak lists".into(),
        vendor: "Matrix Science open text format (written by many converters)".into(),
        extensions: vec!["mgf".into(), "mgf.gz".into()],
        family: "mass-spectrometry".into(),
        can_read: true,
        can_write: false,
        confidence: crate::assurance::MGF.confidence,
        known_gaps: vec![
            "MGF holds processed peak lists (usually centroided MS/MS), not raw scans; spectra are reported centroided".into(),
            "Peptide-mass-fingerprint lines outside BEGIN IONS blocks are counted, not read as spectra".into(),
            "Polarity is known only when a block names it (IONMODE / ION_MODE); otherwise `unknown`".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        assert_eq!(parse_charges("2+ and 3+"), vec![2, 3]);
        assert_eq!(parse_charges("2+,3+"), vec![2, 3]);
        assert_eq!(parse_charges("1-"), vec![-1]);
        assert_eq!(parse_charges("3"), vec![3]);
        assert_eq!(parse_charges(""), Vec::<i32>::new());
        assert_eq!(parse_rt("95-97"), Some((95.0, Some(97.0))));
        assert_eq!(parse_rt("1.5e-3"), Some((0.0015, None)));
        assert_eq!(parse_rt("x"), None);
        assert_eq!(
            param("pepmass=983.6 1234"),
            Some(("PEPMASS".into(), "983.6 1234".into()))
        );
        assert_eq!(param("846.60 73"), None);
        assert!(peak("72.04 101 # b1").is_ok());
        assert!(peak("72.04").is_err());
        assert_eq!(
            native_id_in(r#"a.1.1.2 File:"a.raw", NativeID:"controllerType=0 scan=1""#),
            Some("controllerType=0 scan=1".into())
        );
    }

    #[test]
    fn sniffing() {
        assert!(looks_like_mgf(
            b"COM=x\nCHARGE=2+\n\nBEGIN IONS\nPEPMASS=1\n"
        ));
        assert!(looks_like_mgf(b"# comment\nBEGIN IONS\n"));
        assert!(!looks_like_mgf(b"<?xml version='1.0'?>\n"));
        assert!(!looks_like_mgf(b"a=b\n"));
    }

    #[test]
    fn chimeric_precursors() {
        let p = vec![
            ("PEPMASS".to_string(), "983.6 1234 2+".to_string()),
            ("PEPMASS".to_string(), "500.2".to_string()),
        ];
        let pre = precursors(&p, &[]);
        assert_eq!(pre.mz, vec![983.6, 500.2]);
        assert_eq!(pre.intensity, Some(1234.0));
        assert_eq!(pre.charges, vec![2]);
    }
}
