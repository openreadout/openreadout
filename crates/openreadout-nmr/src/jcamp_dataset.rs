//! `Dataset` for JCAMP-DX: one trace per data table (XYDATA, NTUPLES, PEAK TABLE, XYPOINTS).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::Input;
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::jcamp_asdf::{decode_asdf, decode_groups};
use crate::jcamp_parse::{JcampFile, Ldr, normalize_label, parse_affn, parse_jcamp};
use crate::{JCAMP_FORMAT_ID, JcampReader};

/// Largest JCAMP-DX file read into memory.
const MAX_FILE: u64 = 2 << 30;

/// Parsed variable list of a table: `(X++(Y..Y))` or `(XY..XY)`.
#[derive(Debug, Clone, PartialEq)]
enum VarList {
    /// Independent symbol, dependent symbol.
    Increment(String, String),
    /// Symbols of each group.
    Groups(Vec<String>),
}

fn split_symbols(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in s.chars() {
        if c.is_ascii_digit()
            && let Some(last) = out.last_mut()
        {
            last.push(c);
            continue;
        }
        if c.is_ascii_alphabetic() {
            out.push(c.to_string());
        }
    }
    out
}

/// Parse `(X++(Y..Y)), XYDATA` → (list, table kind).
fn parse_var_list(head: &str) -> Option<(VarList, Option<String>)> {
    let head = head.trim();
    let open = head.find('(')?;
    // matching close paren of the outer group
    let mut depth = 0i32;
    let mut close = None;
    for (i, c) in head[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    // tolerate a missing closing parenthesis (`(X++(Y..Y)` is seen in the wild)
    let close = close.unwrap_or(head.len());
    let inner: String = head[open + 1..close]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let kind = head[(close + 1).min(head.len())..]
        .trim_start_matches([',', ' ', '\t'])
        .split_whitespace()
        .next()
        .map(normalize_label)
        .filter(|s| !s.is_empty());
    if let Some((ind, dep)) = inner.split_once("++") {
        let dep = dep.trim_start_matches('(').trim_end_matches(')');
        let dep_sym = dep.split("..").next()?.to_string();
        return Some((VarList::Increment(ind.to_string(), dep_sym), kind));
    }
    let group = inner.split("..").next()?;
    let syms = split_symbols(group);
    if syms.is_empty() {
        return None;
    }
    Some((VarList::Groups(syms), kind))
}

/// A reference to a table record: (block, ldr index).
type TableRef = (usize, usize);

#[derive(Debug, Clone)]
struct ChannelSpec {
    name: String,
    unit: Option<String>,
    factor: f64,
}

#[derive(Debug, Clone)]
enum Layout {
    /// Evenly spaced abscissa; `tables[sweep][channel]`.
    Increment {
        first: f64,
        last: f64,
        npoints: Option<u64>,
        x_factor: f64,
        tables: Vec<Vec<TableRef>>,
        first_y: Option<f64>,
    },
    /// `(XY..XY)` groups; one table per sweep; channels are the group components.
    Groups { tables: Vec<TableRef>, group: usize },
    /// Declared but not decodable (reason).
    Undecodable(String),
}

#[derive(Debug, Clone)]
struct TraceSpec {
    block: usize,
    channels: Vec<ChannelSpec>,
    layout: Layout,
    info: TraceInfo,
}

/// Decoded data of one trace: `sweeps[s][c][i]` plus problems found while decoding.
#[derive(Debug, Clone, Default)]
struct Decoded {
    sweeps: Vec<Vec<Vec<f64>>>,
    findings: Vec<Finding>,
}

/// An opened JCAMP-DX file.
#[derive(Debug)]
pub struct JcampDataset {
    path: PathBuf,
    file: JcampFile,
    traces: Vec<TraceSpec>,
    cache: HashMap<u32, Decoded>,
}

fn quantity(unit: &str) -> &'static str {
    match normalize_label(unit).as_str() {
        "HZ" | "KHZ" | "MHZ" => "frequency",
        "PPM" => "chemical_shift",
        "SECONDS" | "SEC" | "S" | "MS" | "MINUTES" => "time",
        "1CM" | "CM1" | "CM-1" => "wavenumber",
        "NANOMETERS" | "NM" | "MICROMETERS" | "UM" => "wavelength",
        "MZ" => "mass_to_charge",
        _ => "x",
    }
}

fn csv_field(list: Option<&str>, i: usize) -> Option<String> {
    let s = list?.split(',').nth(i)?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn symbol_name(sym: &str) -> String {
    match sym {
        "Y" => "y".into(),
        "R" => "real".into(),
        "I" => "imag".into(),
        other => other.to_ascii_lowercase(),
    }
}

fn put(m: &mut BTreeMap<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(k.into(), v);
    }
}

/// Descriptive records copied into a trace's `extra` (our name, normalized JCAMP-DX label).
const NOTES: &[(&str, &str)] = &[
    ("title", "TITLE"),
    ("jcamp_version", "JCAMPDX"),
    ("data_type", "DATATYPE"),
    ("data_class", "DATACLASS"),
    ("origin", "ORIGIN"),
    ("owner", "OWNER"),
    ("spectrometer", "SPECTROMETERDATASYSTEM"),
    ("instrument_parameters", "INSTRUMENTPARAMETERS"),
    ("sample_description", "SAMPLEDESCRIPTION"),
    ("compound_name", "CASNAME"),
    ("names", "NAMES"),
    ("molecular_formula", "MOLFORM"),
    ("cas_registry_number", "CASREGISTRYNO"),
    ("date", "DATE"),
    ("time", "TIME"),
    ("long_date", "LONGDATE"),
    ("nucleus", ".OBSERVENUCLEUS"),
    ("solvent", ".SOLVENTNAME"),
    ("pulse_sequence", ".PULSESEQUENCE"),
    ("acquisition_mode", ".ACQUISITIONMODE"),
    ("ms_spectrometer_type", ".SPECTROMETERTYPE"),
    ("ms_inlet", ".INLET"),
    ("ms_ionization_mode", ".IONIZATIONMODE"),
];

const NUMERIC_NOTES: &[(&str, &str)] = &[
    ("observe_frequency_mhz", ".OBSERVEFREQUENCY"),
    ("field_t", ".FIELD"),
    ("scans", ".AVERAGES"),
    ("resolution", "RESOLUTION"),
    ("min_y", "MINY"),
    ("max_y", "MAXY"),
    ("first_y", "FIRSTY"),
];

/// Look a record up in the block, then in its enclosing blocks (compound files put shared
/// notes in the LINK block).
fn lookup<'a>(file: &'a JcampFile, b: usize, key: &str) -> Option<&'a Ldr> {
    let mut cur = Some(b);
    while let Some(i) = cur {
        let blk = &file.blocks[i];
        if let Some(l) = blk.get(key) {
            return Some(l);
        }
        cur = blk.parent.map(|p| p as usize);
    }
    None
}

fn block_notes(file: &JcampFile, b: usize) -> BTreeMap<String, Value> {
    let mut e = BTreeMap::new();
    e.insert("block".into(), json!(b));
    for (name, key) in NOTES {
        let v = if *key == "TITLE" || *key == "DATATYPE" || *key == "DATACLASS" {
            file.blocks[b].get(key)
        } else {
            lookup(file, b, key)
        };
        if let Some(l) = v {
            let t = l.value.trim();
            if !t.is_empty() {
                e.insert((*name).into(), json!(t.replace('\n', " ")));
            }
        }
    }
    for (name, key) in NUMERIC_NOTES {
        put(
            &mut e,
            name,
            file.blocks[b]
                .get(key)
                .and_then(|l| parse_affn(l.head()))
                .map(|v| json!(v)),
        );
    }
    e
}

fn increment_axis(first: f64, last: f64, n: u64, unit: Option<&str>) -> Value {
    let step = if n > 1 {
        (last - first) / (n - 1) as f64
    } else {
        0.0
    };
    json!({
        "quantity": unit.map_or("x", quantity), "unit": unit, "first": first, "last": last,
        "step": step, "size": n,
    })
}

/// Seconds per unit of a time `##XUNITS=` (`SECONDS`, `MS`, `MINUTES`).
fn seconds_per(unit: &str) -> f64 {
    match normalize_label(unit).as_str() {
        "MS" => 1e-3,
        "MINUTES" => 60.0,
        _ => 1.0,
    }
}

fn sample_rate(first: f64, last: f64, n: u64, unit: Option<&str>) -> f64 {
    match unit {
        Some(u) if quantity(u) == "time" && n > 1 && (last - first).abs() > 0.0 => {
            (n - 1) as f64 / ((last - first).abs() * seconds_per(u))
        }
        _ => 0.0,
    }
}

/// `start_s` of a trace whose regular abscissa is time and increases: its first value in
/// seconds, so that `start_s + i / sample_rate_hz` places sample `i` as `extra.axis` does.
fn time_start_s(e: &BTreeMap<String, Value>, rate: f64) -> Option<f64> {
    let a = e.get("axis")?;
    if rate <= 0.0 || a.get("quantity").and_then(Value::as_str) != Some("time") {
        return None;
    }
    let unit = a.get("unit").and_then(Value::as_str)?;
    let first = a.get("first").and_then(Value::as_f64)?;
    let step = a.get("step").and_then(Value::as_f64)?;
    (step > 0.0 && first.is_finite()).then(|| first * seconds_per(unit))
}

impl JcampDataset {
    /// Open and parse a JCAMP-DX file (tables are decoded lazily).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
        if meta.len() > MAX_FILE {
            return Err(Error::unsupported(
                JCAMP_FORMAT_ID,
                format!("a {} byte JCAMP-DX file", meta.len()),
                "Files over 2 GiB are not read.",
            ));
        }
        let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
        let file = parse_jcamp(&bytes);
        if file.blocks.is_empty() {
            return Err(Error::corrupt(
                JCAMP_FORMAT_ID,
                "no ##TITLE= block found: not a JCAMP-DX file or empty",
            ));
        }
        let mut ds = JcampDataset {
            path: path.to_path_buf(),
            file,
            traces: Vec::new(),
            cache: HashMap::new(),
        };
        ds.build_traces();
        Ok(ds)
    }

    fn build_traces(&mut self) {
        let mut specs = Vec::new();
        for b in 0..self.file.blocks.len() {
            let blk = &self.file.blocks[b];
            if blk.is_link() {
                continue;
            }
            if let Some(nt) = blk.ldrs.iter().position(|l| l.key == "NTUPLES") {
                specs.push(self.ntuples_spec(b, nt));
                continue;
            }
            let xy: Vec<usize> = blk
                .ldrs
                .iter()
                .enumerate()
                .filter(|(_, l)| l.key == "XYDATA")
                .map(|(i, _)| i)
                .collect();
            if !xy.is_empty() {
                specs.push(self.xydata_spec(b, &xy));
            }
            for key in ["PEAKTABLE", "XYPOINTS"] {
                for (i, l) in blk.ldrs.iter().enumerate() {
                    if l.key == key {
                        specs.push(self.groups_spec(b, i));
                    }
                }
            }
        }
        for (i, s) in specs.iter_mut().enumerate() {
            s.info.index = i as u32;
        }
        self.traces = specs;
    }

    fn xydata_spec(&self, b: usize, xy: &[usize]) -> TraceSpec {
        let blk = &self.file.blocks[b];
        let mut e = block_notes(&self.file, b);
        e.insert("kind".into(), json!("xydata"));
        let first = blk.number("FIRSTX");
        let last = blk.number("LASTX");
        let npoints = blk
            .number("NPOINTS")
            .filter(|v| *v >= 0.0)
            .map(|v| v as u64);
        let x_factor = blk.number("XFACTOR").unwrap_or(1.0);
        let y_factor = blk.number("YFACTOR").unwrap_or(1.0);
        let xunit = blk.text("XUNITS").map(str::to_string);
        let yunit = blk.text("YUNITS").map(str::to_string);
        let mut channels = Vec::new();
        let mut refs = Vec::new();
        let mut bad = None;
        for &i in xy {
            match parse_var_list(blk.ldrs[i].head()) {
                Some((VarList::Increment(_, dep), _)) => {
                    channels.push(ChannelSpec {
                        name: symbol_name(&dep),
                        unit: yunit.clone(),
                        factor: y_factor,
                    });
                    refs.push((b, i));
                }
                Some((VarList::Groups(_), _)) | None => {
                    bad = Some(format!(
                        "##XYDATA= variable list {:?} is not (X++(Y..Y))",
                        blk.ldrs[i].head()
                    ));
                }
            }
        }
        let layout = match (bad, first, last) {
            (Some(m), _, _) => Layout::Undecodable(m),
            (None, Some(first), Some(last)) => Layout::Increment {
                first,
                last,
                npoints,
                x_factor,
                tables: vec![refs],
                first_y: blk.number("FIRSTY"),
            },
            _ => Layout::Undecodable("##FIRSTX= or ##LASTX= is missing".into()),
        };
        let n = npoints.unwrap_or(0);
        if let Layout::Increment { first, last, .. } = &layout {
            e.insert(
                "axis".into(),
                increment_axis(*first, *last, n, xunit.as_deref()),
            );
        }
        let rate = match &layout {
            Layout::Increment { first, last, .. } => {
                sample_rate(*first, *last, n, xunit.as_deref())
            }
            _ => 0.0,
        };
        self.finish_spec(b, channels, layout, n, 1, rate, e)
    }

    fn groups_spec(&self, b: usize, i: usize) -> TraceSpec {
        let blk = &self.file.blocks[b];
        let ldr = &blk.ldrs[i];
        let mut e = block_notes(&self.file, b);
        e.insert(
            "kind".into(),
            json!(if ldr.key == "PEAKTABLE" {
                "peak_table"
            } else {
                "xypoints"
            }),
        );
        let xunit = blk.text("XUNITS").map(str::to_string);
        let yunit = blk.text("YUNITS").map(str::to_string);
        let x_factor = blk.number("XFACTOR").unwrap_or(1.0);
        let y_factor = blk.number("YFACTOR").unwrap_or(1.0);
        match parse_var_list(ldr.head()) {
            Some((VarList::Groups(syms), _)) => {
                let channels: Vec<ChannelSpec> = syms
                    .iter()
                    .enumerate()
                    .map(|(k, s)| ChannelSpec {
                        name: s.to_ascii_lowercase(),
                        unit: match k {
                            0 => xunit.clone(),
                            1 => yunit.clone(),
                            _ => None,
                        },
                        factor: match k {
                            0 => x_factor,
                            1 => y_factor,
                            _ => 1.0,
                        },
                    })
                    .collect();
                let n = blk
                    .number("NPOINTS")
                    .filter(|v| *v >= 0.0)
                    .map(|v| v as u64)
                    .or_else(|| {
                        decode_groups(ldr.body(), syms.len())
                            .ok()
                            .map(|(c, _)| c[0].len() as u64)
                    })
                    .unwrap_or(0);
                e.insert(
                    "axis".into(),
                    json!({"quantity": xunit.as_deref().map_or("x", quantity), "unit": xunit, "irregular": true, "channel": 0}),
                );
                let group = syms.len();
                self.finish_spec(
                    b,
                    channels,
                    Layout::Groups {
                        tables: vec![(b, i)],
                        group,
                    },
                    n,
                    1,
                    0.0,
                    e,
                )
            }
            _ => self.finish_spec(
                b,
                Vec::new(),
                Layout::Undecodable(format!("unsupported variable list {:?}", ldr.head())),
                0,
                1,
                0.0,
                e,
            ),
        }
    }

    #[allow(clippy::many_single_char_names)]
    fn ntuples_spec(&self, b: usize, nt: usize) -> TraceSpec {
        let blk = &self.file.blocks[b];
        let mut e = block_notes(&self.file, b);
        e.insert("kind".into(), json!("ntuples"));
        e.insert("ntuples".into(), json!(blk.ldrs[nt].head()));
        // header records between ##NTUPLES= and the first ##PAGE=
        let mut hdr: HashMap<&str, &str> = HashMap::new();
        let mut pages: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, l) in blk.ldrs.iter().enumerate().skip(nt + 1) {
            if l.key == "ENDNTUPLES" {
                break;
            }
            if l.key == "PAGE" {
                pages.push((l.head().to_string(), Vec::new()));
                continue;
            }
            match pages.last_mut() {
                Some((_, v)) => v.push(i),
                None => {
                    hdr.entry(l.key.as_str()).or_insert(l.value.as_str());
                }
            }
        }
        let field = |k: &str| hdr.get(k).map(|v| v.replace('\n', " "));
        let symbols: Vec<String> = field("SYMBOL")
            .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
            .unwrap_or_default();
        let names = field("VARNAME");
        let units = field("UNITS");
        let factors = field("FACTOR");
        let firsts = field("FIRST");
        let lasts = field("LAST");
        let dims = field("VARDIM");
        let idx = |sym: &str| symbols.iter().position(|s| s.eq_ignore_ascii_case(sym));
        let num_at = |list: &Option<String>, i: usize| {
            csv_field(list.as_deref(), i).and_then(|s| parse_affn(&s))
        };
        e.insert("symbols".into(), json!(symbols));
        if let Some(n) = &names {
            e.insert(
                "variables".into(),
                json!(
                    n.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                ),
            );
        }
        // classify pages
        let mut keyed: Vec<(String, String, VarList, usize)> = Vec::new(); // (page var, page value, list, table ldr)
        let mut problems = Vec::new();
        for (page, ldrs) in &pages {
            let (pvar, pval) = page
                .split_once('=')
                .map_or((String::new(), page.trim().to_string()), |(a, v)| {
                    (a.trim().to_string(), v.trim().to_string())
                });
            let Some(&t) = ldrs.iter().find(|&&i| blk.ldrs[i].key == "DATATABLE") else {
                problems.push(format!("page {page:?} has no ##DATA TABLE="));
                continue;
            };
            match parse_var_list(blk.ldrs[t].head()) {
                Some((vl, _)) => keyed.push((pvar, pval, vl, t)),
                None => problems.push(format!(
                    "page {page:?}: unreadable variable list {:?}",
                    blk.ldrs[t].head()
                )),
            }
        }
        if keyed.is_empty() {
            problems.push("no decodable pages".into());
        }
        let all_increment = keyed.iter().all(|k| matches!(k.2, VarList::Increment(..)));
        let all_groups = keyed.iter().all(|k| matches!(k.2, VarList::Groups(..)));
        let mut rate = 0.0;
        let (channels, layout, n, sweeps) = if !problems.is_empty() && keyed.is_empty() {
            (Vec::new(), Layout::Undecodable(problems.join("; ")), 0, 1)
        } else if all_increment {
            let VarList::Increment(ind, _) = &keyed[0].2 else {
                unreachable!()
            };
            let ind = ind.clone();
            // channels: distinct dependent symbols in order; sweeps: distinct page values unless
            // the page variable is the page number and each page holds a different symbol
            let mut chan_syms: Vec<String> = Vec::new();
            for k in &keyed {
                if let VarList::Increment(_, d) = &k.2
                    && !chan_syms.contains(d)
                {
                    chan_syms.push(d.clone());
                }
            }
            let by_symbol = keyed.len() == chan_syms.len();
            let mut sweep_keys: Vec<String> = Vec::new();
            if by_symbol {
                sweep_keys.push(String::new());
            } else {
                for k in &keyed {
                    if !sweep_keys.contains(&k.1) {
                        sweep_keys.push(k.1.clone());
                    }
                }
            }
            let mut tables =
                vec![vec![(usize::MAX, usize::MAX); chan_syms.len()]; sweep_keys.len()];
            for k in &keyed {
                let VarList::Increment(_, d) = &k.2 else {
                    continue;
                };
                let c = chan_syms.iter().position(|s| s == d).unwrap_or(0);
                let s = if by_symbol {
                    0
                } else {
                    sweep_keys.iter().position(|s| s == &k.1).unwrap_or(0)
                };
                tables[s][c] = (b, k.3);
            }
            let complete = tables.iter().flatten().all(|t| t.0 != usize::MAX);
            let ix = idx(&ind);
            let first = ix.and_then(|i| num_at(&firsts, i));
            let last = ix.and_then(|i| num_at(&lasts, i));
            let dim = ix
                .and_then(|i| num_at(&dims, i))
                .filter(|v| *v >= 0.0)
                .map(|v| v as u64);
            let xunit = ix.and_then(|i| csv_field(units.as_deref(), i));
            let channels: Vec<ChannelSpec> = chan_syms
                .iter()
                .map(|s| {
                    let i = idx(s);
                    ChannelSpec {
                        name: i
                            .and_then(|i| csv_field(names.as_deref(), i))
                            .unwrap_or_else(|| symbol_name(s)),
                        unit: i.and_then(|i| csv_field(units.as_deref(), i)),
                        factor: i.and_then(|i| num_at(&factors, i)).unwrap_or(1.0),
                    }
                })
                .collect();
            let layout = match (complete, first, last) {
                (false, _, _) => Layout::Undecodable(
                    "NTUPLES pages do not form a complete sweep × channel grid".into(),
                ),
                (true, Some(first), Some(last)) => Layout::Increment {
                    first,
                    last,
                    npoints: dim,
                    x_factor: ix.and_then(|i| num_at(&factors, i)).unwrap_or(1.0),
                    tables,
                    first_y: None,
                },
                _ => Layout::Undecodable(format!(
                    "##FIRST=/##LAST= give no value for the independent variable {ind}"
                )),
            };
            let n = dim.unwrap_or(0);
            if let (Some(f), Some(l)) = (first, last) {
                e.insert("axis".into(), increment_axis(f, l, n, xunit.as_deref()));
                rate = sample_rate(f, l, n, xunit.as_deref());
            }
            if !by_symbol {
                let pvar = keyed[0].0.clone();
                let values: Vec<Value> = sweep_keys
                    .iter()
                    .map(|v| parse_affn(v).map_or_else(|| json!(v), |f| json!(f)))
                    .collect();
                let unit = idx(&pvar).and_then(|i| csv_field(units.as_deref(), i));
                e.insert(
                    "sweep_axis".into(),
                    json!({"variable": pvar, "unit": unit, "values": values}),
                );
            }
            let sweeps = sweep_keys.len() as u32;
            (channels, layout, n, sweeps)
        } else if all_groups {
            let VarList::Groups(syms) = &keyed[0].2 else {
                unreachable!()
            };
            let group = syms.len();
            let channels: Vec<ChannelSpec> = syms
                .iter()
                .map(|s| {
                    let i = idx(s);
                    ChannelSpec {
                        name: i
                            .and_then(|i| csv_field(names.as_deref(), i))
                            .unwrap_or_else(|| s.to_ascii_lowercase()),
                        unit: i.and_then(|i| csv_field(units.as_deref(), i)),
                        factor: i.and_then(|i| num_at(&factors, i)).unwrap_or(1.0),
                    }
                })
                .collect();
            let tables: Vec<TableRef> = keyed.iter().map(|k| (b, k.3)).collect();
            let counts: Vec<u64> = tables
                .iter()
                .map(|&(_, t)| {
                    decode_groups(blk.ldrs[t].body(), group).map_or(0, |(c, _)| c[0].len() as u64)
                })
                .collect();
            let n = counts.iter().copied().max().unwrap_or(0);
            if counts.iter().any(|&c| c != n) {
                // per-sweep lengths, read by `openreadout_core::trace::sweep_samples`
                e.insert("sweep_sample_counts".into(), json!(counts));
            }
            let values: Vec<Value> = keyed
                .iter()
                .map(|k| parse_affn(&k.1).map_or_else(|| json!(k.1), |f| json!(f)))
                .collect();
            e.insert(
                "sweep_axis".into(),
                json!({"variable": keyed[0].0, "values": values}),
            );
            e.insert(
                "axis".into(),
                json!({"irregular": true, "channel": 0, "unit": channels.first().and_then(|c| c.unit.clone())}),
            );
            e.insert(
                "note".into(),
                json!("pages may hold different numbers of points: sample_count is the largest, extra.sweep_sample_counts the per-page counts when they differ"),
            );
            let sweeps = tables.len() as u32;
            (channels, Layout::Groups { tables, group }, n, sweeps)
        } else {
            (
                Vec::new(),
                Layout::Undecodable("NTUPLES pages mix (X++(Y..Y)) and (XY..XY) tables".into()),
                0,
                1,
            )
        };
        if !problems.is_empty() {
            e.insert("problems".into(), json!(problems));
        }
        self.finish_spec(b, channels, layout, n, sweeps, rate, e)
    }

    fn finish_spec(
        &self,
        b: usize,
        channels: Vec<ChannelSpec>,
        layout: Layout,
        n: u64,
        sweeps: u32,
        rate: f64,
        mut e: BTreeMap<String, Value>,
    ) -> TraceSpec {
        if let Layout::Undecodable(m) = &layout {
            e.insert("undecodable".into(), json!(m));
        }
        let info = TraceInfo {
            index: 0,
            name: self.file.blocks[b].title().map(str::to_string),
            sample_rate_hz: rate,
            sample_count: n,
            sweep_count: sweeps,
            channels: channels
                .iter()
                .enumerate()
                .map(|(i, c)| SignalChannelInfo {
                    index: i as u32,
                    name: c.name.clone(),
                    unit: c.unit.clone(),
                    dtype: "float64".into(),
                    scale: c.factor,
                    offset: 0.0,
                    extra: BTreeMap::new(),
                })
                .collect(),
            start_s: time_start_s(&e, rate),
            extra: e,
        };
        TraceSpec {
            block: b,
            channels,
            layout,
            info,
        }
    }

    fn decode(&self, t: &TraceSpec) -> Result<Decoded> {
        let mut out = Decoded::default();
        let title = self.file.blocks[t.block]
            .title()
            .unwrap_or("untitled")
            .to_string();
        match &t.layout {
            Layout::Undecodable(m) => {
                return Err(Error::unsupported(
                    JCAMP_FORMAT_ID,
                    format!("block {} ({title}): {m}", t.block),
                    "See `openreadout self formats` for known gaps; `info --view full` shows the raw records.",
                ));
            }
            Layout::Increment {
                first,
                last,
                npoints,
                x_factor,
                tables,
                first_y,
            } => {
                for (s, row) in tables.iter().enumerate() {
                    let mut chans = Vec::new();
                    for (c, &(b, l)) in row.iter().enumerate() {
                        let ldr = &self.file.blocks[b].ldrs[l];
                        let tab = decode_asdf(ldr.body()).map_err(|m| {
                            Error::corrupt_at(
                                JCAMP_FORMAT_ID,
                                ldr.offset,
                                format!(
                                    "block {} ({title}), ##{}= line {}: {m}",
                                    t.block, ldr.label, ldr.line
                                ),
                            )
                        })?;
                        for (line, prev, got) in &tab.y_check_failures {
                            out.findings.push(
                                Finding::error(
                                    "y_check",
                                    format!(
                                        "block {} ({title}), ##{}= table line {line}: DIF check value {got} != previous ordinate {prev}",
                                        t.block, ldr.label
                                    ),
                                )
                                .at(ldr.offset),
                            );
                        }
                        let n = tab.y.len() as u64;
                        if let Some(np) = npoints
                            && *np != n
                        {
                            out.findings.push(
                                Finding::error(
                                    if n < *np { "truncated" } else { "npoints_mismatch" },
                                    format!(
                                        "block {} ({title}), ##{}= sweep {s}: {n} values decoded, {np} declared",
                                        t.block, ldr.label
                                    ),
                                )
                                .at(ldr.offset),
                            );
                        }
                        // X-sequence checks (§5.8.1): half a step of tolerance
                        let np = npoints.unwrap_or(n);
                        if np > 1 {
                            let step = (last - first) / (np - 1) as f64;
                            let bad = tab
                                .x_checks
                                .iter()
                                .filter(|(x, res, i)| {
                                    let want = first + step * *i as f64;
                                    let tol =
                                        step.abs() * 0.5 + res * x_factor.abs() + want.abs() * 1e-9;
                                    (x * x_factor - want).abs() > tol
                                })
                                .count();
                            if bad > 0 {
                                out.findings.push(Finding::warning(
                                    "x_sequence",
                                    format!(
                                        "block {} ({title}), ##{}=: {bad} of {} line abscissas differ from FIRSTX + i·step by more than half a step",
                                        t.block, ldr.label, tab.x_checks.len()
                                    ),
                                ));
                            }
                        }
                        let f = t.channels[c].factor;
                        let vals: Vec<f64> = tab.y.iter().map(|v| v * f).collect();
                        if s == 0
                            && c == 0
                            && let (Some(fy), Some(&v0)) = (first_y, vals.first())
                            // one quantization step (the factor) of tolerance: writers round FIRSTY
                            && (fy - v0).abs() > f.abs() + 1e-6 * fy.abs().max(v0.abs())
                        {
                            out.findings.push(Finding::warning(
                                "firsty_mismatch",
                                format!("block {} ({title}): ##FIRSTY= {fy} but the first ordinate × YFACTOR is {v0}", t.block),
                            ));
                        }
                        chans.push(vals);
                    }
                    out.sweeps.push(chans);
                }
            }
            Layout::Groups { tables, group } => {
                for &(b, l) in tables {
                    let ldr = &self.file.blocks[b].ldrs[l];
                    let (cols, non_numeric) = decode_groups(ldr.body(), *group).map_err(|m| {
                        Error::corrupt_at(
                            JCAMP_FORMAT_ID,
                            ldr.offset,
                            format!(
                                "block {} ({title}), ##{}= line {}: {m}",
                                t.block, ldr.label, ldr.line
                            ),
                        )
                    })?;
                    if non_numeric > 0 {
                        out.findings.push(Finding::info(
                            "non_numeric",
                            format!("block {} ({title}), ##{}=: {non_numeric} non-numeric components returned as NaN", t.block, ldr.label),
                        ));
                    }
                    let blk = &self.file.blocks[b];
                    if tables.len() == 1
                        && let Some(np) = blk.number("NPOINTS")
                        && np as usize != cols[0].len()
                    {
                        out.findings.push(Finding::error(
                            if (cols[0].len() as f64) < np {
                                "truncated"
                            } else {
                                "npoints_mismatch"
                            },
                            format!(
                                "block {} ({title}): {} groups decoded, ##NPOINTS= {np}",
                                t.block,
                                cols[0].len()
                            ),
                        ));
                    }
                    let chans = cols
                        .into_iter()
                        .enumerate()
                        .map(|(c, col)| {
                            let f = t.channels.get(c).map_or(1.0, |ch| ch.factor);
                            col.into_iter().map(|v| v * f).collect()
                        })
                        .collect();
                    out.sweeps.push(chans);
                }
            }
        }
        Ok(out)
    }

    fn decoded(&mut self, index: u32) -> Result<&Decoded> {
        if !self.cache.contains_key(&index) {
            let t = self.traces.get(index as usize).ok_or_else(|| {
                Error::Usage(format!(
                    "trace index {index} out of range (0..{})",
                    self.traces.len()
                ))
            })?;
            let d = self.decode(t)?;
            self.cache.insert(index, d);
        }
        Ok(&self.cache[&index])
    }
}

impl Dataset for JcampDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut notes = Vec::new();
        if self
            .file
            .issues
            .iter()
            .any(|f| f.severity == openreadout_core::model::Severity::Error)
        {
            notes.push(
                "structural problems found (e.g. a block without ##END=); run `check`".into(),
            );
        }
        let skipped: Vec<String> = self
            .file
            .blocks
            .iter()
            .filter(|b| !b.is_link() && !self.traces.iter().any(|t| t.block == b.index as usize))
            .map(|b| {
                format!(
                    "block {} ({}): no decodable table{}",
                    b.index,
                    b.title().unwrap_or("untitled"),
                    b.data_type()
                        .map(|t| format!(", DATA TYPE {t}"))
                        .unwrap_or_default()
                )
            })
            .collect();
        notes.extend(skipped);
        notes.push("values are table values × the declared factor (##YFACTOR= or NTUPLES ##FACTOR=); a regular abscissa is described in extra.axis".into());
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file.len,
            format: JcampReader.descriptor(),
            format_version: self
                .file
                .blocks
                .first()
                .and_then(|b| b.text("JCAMPDX"))
                .map(|v| v.split_whitespace().next().unwrap_or(v).to_string()),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: self.traces.iter().map(|t| t.info.clone()).collect(),
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let blocks: Vec<Value> = self
            .file
            .blocks
            .iter()
            .map(|b| {
                let ldrs: Vec<Value> = b
                    .ldrs
                    .iter()
                    .map(|l| {
                        let is_table = matches!(
                            l.key.as_str(),
                            "XYDATA"
                                | "DATATABLE"
                                | "PEAKTABLE"
                                | "XYPOINTS"
                                | "PEAKASSIGNMENTS"
                                | "RADATA"
                        );
                        let v = if is_table {
                            format!("{} ({} data lines)", l.head(), l.body().lines().count())
                        } else {
                            l.value.trim().to_string()
                        };
                        json!([l.label, v])
                    })
                    .collect();
                json!({"index": b.index, "parent": b.parent, "title": b.title(), "records": ldrs})
            })
            .collect();
        Ok(json!({ "blocks": blocks }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Spec),
            ("traces[].name", Source::Spec),
            ("traces[].sample_count", Source::Spec),
            ("traces[].sweep_count", Source::Inferred),
            ("traces[].sample_rate_hz", Source::Spec),
            ("traces[].start_s", Source::Inferred),
            ("traces[].channels[].name", Source::Spec),
            ("traces[].channels[].unit", Source::Spec),
            ("traces[].channels[].scale", Source::Spec),
            ("traces[].extra.axis", Source::Spec),
            ("traces[].extra.axis[irregular]", Source::Inferred),
            ("traces[].extra.sweep_axis", Source::Inferred),
            ("traces[].extra.nucleus", Source::Spec),
            ("traces[].extra.observe_frequency_mhz", Source::Spec),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for b in &self.file.blocks {
            out.push(LsEntry {
                kind: "block".into(),
                name: b.title().unwrap_or("untitled").to_string(),
                offset: Some(b.offset),
                size: Some(b.end.saturating_sub(b.offset)),
                image: None,
                details: json!({
                    "index": b.index, "parent": b.parent, "data_type": b.data_type(),
                    "data_class": b.text("DATACLASS"), "records": b.ldrs.len(), "closed": b.closed,
                }),
            });
            for l in &b.ldrs {
                if matches!(
                    l.key.as_str(),
                    "XYDATA"
                        | "DATATABLE"
                        | "PEAKTABLE"
                        | "XYPOINTS"
                        | "PEAKASSIGNMENTS"
                        | "RADATA"
                ) {
                    out.push(LsEntry {
                        kind: "table".into(),
                        name: format!("##{}= {}", l.label, l.head()),
                        offset: Some(l.offset),
                        size: Some(l.end.saturating_sub(l.offset)),
                        image: None,
                        details: json!({"block": b.index, "line": l.line, "data_lines": l.body().lines().count()}),
                    });
                }
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            JCAMP_FORMAT_ID,
            "image planes",
            "JCAMP-DX files hold spectra (traces), not images: use `openreadout trace`, `openreadout export --to csv`, or the openreadout_trace MCP tool.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let d = self.decoded(index)?;
        let row = d.sweeps.get(sweep as usize).ok_or_else(|| {
            Error::Usage(format!(
                "sweep {sweep} out of range (trace {index} has {} sweeps)",
                d.sweeps.len()
            ))
        })?;
        let n = row.first().map_or(0, Vec::len) as u64;
        if first_sample > n {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({n} samples)"
            )));
        }
        let a = first_sample as usize;
        let b = (first_sample + max_samples.min(n - first_sample)) as usize;
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels: row
                .iter()
                .map(|c| c.get(a..b.min(c.len())).unwrap_or(&[]).to_vec())
                .collect(),
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), JCAMP_FORMAT_ID);
        r.performed("block structure: every ##TITLE= has an ##END=, no text outside blocks, labels well formed");
        r.performed("required labels per data block (##JCAMP-DX=, ##DATA TYPE=; ##FIRSTX=/##LASTX=/##NPOINTS= for XYDATA)");
        r.performed("every table decodes: ASDF (AFFN/PAC/SQZ/DIF/DUP) or AFFN groups; DIF Y-value check-points; X-sequence check-points");
        r.performed("decoded point counts against ##NPOINTS= / ##VAR_DIM=; ##FIRSTY= against the first ordinate");
        for f in &self.file.issues {
            r.push(f.clone());
        }
        if self.file.latin1 {
            r.push(Finding::info(
                "non_utf8",
                "file is not UTF-8; read as Latin-1",
            ));
        }
        for b in &self.file.blocks {
            if b.get("JCAMPDX").is_none() && b.get("JCAMPCS").is_none() {
                r.push(Finding::warning(
                    "missing_label",
                    format!(
                        "block {} ({}) has no ##JCAMP-DX=",
                        b.index,
                        b.title().unwrap_or("untitled")
                    ),
                ));
            }
            if b.get("DATATYPE").is_none() && b.get("JCAMPCS").is_none() {
                r.push(Finding::warning(
                    "missing_label",
                    format!("block {} has no ##DATA TYPE=", b.index),
                ));
            }
            if b.get("XYDATA").is_some() {
                for k in ["FIRSTX", "LASTX", "NPOINTS"] {
                    if b.get(k).is_none() {
                        r.push(Finding::error(
                            "missing_label",
                            format!("block {} has ##XYDATA= but no ##{k}=", b.index),
                        ));
                    }
                }
            }
        }
        for i in 0..self.traces.len() {
            let t = &self.traces[i];
            if let Layout::Undecodable(m) = &t.layout {
                r.push(Finding::warning(
                    "undecodable",
                    format!("trace {i} (block {}): {m}", t.block),
                ));
                continue;
            }
            let declared = t.info.sample_count;
            let multi = t.info.sweep_count > 1;
            match self.decode(&self.traces[i].clone()) {
                Ok(d) => {
                    for f in d.findings {
                        r.push(f);
                    }
                    if let Layout::Increment { .. } = &self.traces[i].layout
                        && multi
                    {
                        for (s, sw) in d.sweeps.iter().enumerate() {
                            let n = sw.first().map_or(0, Vec::len) as u64;
                            if declared > 0 && n != declared {
                                r.push(Finding::error(
                                    if n < declared {
                                        "truncated"
                                    } else {
                                        "npoints_mismatch"
                                    },
                                    format!(
                                        "trace {i} sweep {s}: {n} values, ##VAR_DIM= {declared}"
                                    ),
                                ));
                            }
                        }
                    }
                }
                Err(e) => r.push(Finding::error(
                    if matches!(e, Error::Unsupported { .. }) {
                        "undecodable"
                    } else {
                        "bad_table"
                    },
                    e.to_string(),
                )),
            }
        }
        if self.traces.is_empty() {
            r.push(Finding::warning(
                "no_data",
                "no decodable data table in any block",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_lists() {
        assert_eq!(
            parse_var_list("(X++(Y..Y))"),
            Some((VarList::Increment("X".into(), "Y".into()), None))
        );
        assert_eq!(
            parse_var_list("(X++(R..R)), XYDATA   $$ x"),
            Some((
                VarList::Increment("X".into(), "R".into()),
                Some("XYDATA".into())
            ))
        );
        assert_eq!(
            parse_var_list("(F2++(Y..Y)), PROFILE"),
            Some((
                VarList::Increment("F2".into(), "Y".into()),
                Some("PROFILE".into())
            ))
        );
        assert_eq!(
            parse_var_list("(XY..XY), PEAKS"),
            Some((
                VarList::Groups(vec!["X".into(), "Y".into()]),
                Some("PEAKS".into())
            ))
        );
        assert_eq!(
            parse_var_list("(XYW..XYW)"),
            Some((
                VarList::Groups(vec!["X".into(), "Y".into(), "W".into()]),
                None
            ))
        );
    }
}
