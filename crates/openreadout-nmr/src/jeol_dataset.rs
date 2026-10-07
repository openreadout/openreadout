//! `Dataset` for a JEOL Delta `.jdf` file: one trace (an FID or a processed spectrum) whose
//! sweeps are the rows of a 2D data set.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::Source;
use openreadout_core::experiment::{Acquisition, Experiment, Origin, Sample};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::jeol_header::{
    JDF_HEADER_BYTES, JdfAxisType, JdfHeader, JdfParam, JdfValue, parse_jdf_params,
};
use crate::{JEOL_FORMAT_ID, JeolReader};

/// Most values decoded by one `read_trace` call.
const MAX_READ_VALUES: u64 = 1 << 28;
/// Largest header + parameter area read at open (parameters follow the header directly in
/// every corpus file and take < 64 KiB).
const MAX_PARAM_AREA: u64 = 64 << 20;
/// Submatrix edge of two-dimensional data (layout code 2).
const EDGE_2D: u64 = 32;
/// Seconds from the Unix epoch to 1990-01-01T00:00:00Z (the epoch of `ACTUAL_START_TIME`,
/// inferred; see the format notes).
const EPOCH_1990: i64 = 631_152_000;

/// How the stored sections map to sweeps and channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    /// Sections of `points[0] × points[1]` values each.
    sections: u64,
    /// True for two-dimensional data (32 × 32 submatrices).
    two_d: bool,
    nx: u64,
    ny: u64,
    x0: u64,
    x1: u64,
    y0: u64,
    y1: u64,
    width: u64,
    little: bool,
}

impl Layout {
    fn samples(&self) -> u64 {
        self.x1 - self.x0 + 1
    }
    fn rows(&self) -> u64 {
        if self.two_d { self.y1 - self.y0 + 1 } else { 1 }
    }
    /// Sweeps per stored row: 2 when the rows alternate the indirect real/imaginary parts.
    fn per_row(&self) -> u64 {
        if self.sections == 4 { 2 } else { 1 }
    }
    fn channels(&self) -> u64 {
        if self.sections == 1 { 1 } else { 2 }
    }
    fn section_values(&self) -> u64 {
        self.nx * self.ny
    }
    /// Value index of (row y, column x) inside one section.
    fn index(&self, y: u64, x: u64) -> u64 {
        if !self.two_d {
            return x;
        }
        let e = EDGE_2D;
        ((y / e) * (self.nx / e) + x / e) * e * e + (y % e) * e + x % e
    }
    fn needed_bytes(&self) -> u64 {
        self.sections * self.section_values() * self.width
    }
}

/// Why data cannot be read.
#[derive(Debug, Clone)]
enum Fault {
    Corrupt(String),
    Unsupported(String),
}

/// An opened `.jdf` file.
#[derive(Debug)]
pub struct JeolDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    file_len: u64,
    header: JdfHeader,
    params: Vec<JdfParam>,
    param_issues: Vec<String>,
    by_name: BTreeMap<String, usize>,
    layout: std::result::Result<Layout, Fault>,
    /// Point-by-point axis values of listed axes (in the axis unit, prefix applied).
    axis_lists: Vec<Option<Vec<f64>>>,
    /// `sample_id` as the context section writes it, uncut.
    context_sample_id: Option<String>,
}

/// Largest axis list read (a list holds one float64 per stored point).
const MAX_LIST_BYTES: u32 = 8 << 20;
/// Largest context section read for the full sample id (the corpus sections take < 64 KiB).
const MAX_CONTEXT_BYTES: u32 = 1 << 20;
/// Bytes of a text parameter's value: longer strings are cut to this in the parameter section.
const TEXT_PARAM_CHARS: usize = 16;

/// The value of the first `sample_id => "…";` (or `=`) line of the context text.
fn context_sample_id(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("sample_id")?;
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("=>")
            .or_else(|| rest.strip_prefix('='))?
            .trim_start()
            .strip_prefix('"')?;
        let v = &rest[..rest.find('"')?];
        Some(v.trim().to_string()).filter(|v| !v.is_empty())
    })
}

/// JEOL axis titles (`Proton`, `Carbon13`, `Silicon29`) in the usual order (`1H`, `13C`,
/// `29Si`). Names we do not know are returned unchanged.
pub fn jeol_nucleus(domain: &str) -> String {
    let d = domain.trim();
    match d.to_ascii_lowercase().as_str() {
        "proton" | "hydrogen1" => return "1H".into(),
        "deuterium" | "hydrogen2" => return "2H".into(),
        "tritium" | "hydrogen3" => return "3H".into(),
        _ => {}
    }
    let split = d.find(|c: char| c.is_ascii_digit());
    let Some(i) = split.filter(|&i| i > 0 && d[i..].chars().all(|c| c.is_ascii_digit())) else {
        return d.to_string();
    };
    let (name, mass) = (d[..i].to_ascii_lowercase(), &d[i..]);
    let sym = match name.as_str() {
        "hydrogen" => "H",
        "helium" => "He",
        "lithium" => "Li",
        "boron" => "B",
        "carbon" => "C",
        "nitrogen" => "N",
        "oxygen" => "O",
        "fluorine" => "F",
        "sodium" => "Na",
        "magnesium" => "Mg",
        "aluminum" | "aluminium" => "Al",
        "silicon" => "Si",
        "phosphorus" | "phosphorous" => "P",
        "sulfur" | "sulphur" => "S",
        "chlorine" => "Cl",
        "potassium" => "K",
        "calcium" => "Ca",
        "vanadium" => "V",
        "cobalt" => "Co",
        "copper" => "Cu",
        "zinc" => "Zn",
        "gallium" => "Ga",
        "selenium" => "Se",
        "bromine" => "Br",
        "rubidium" => "Rb",
        "yttrium" => "Y",
        "rhodium" => "Rh",
        "silver" => "Ag",
        "cadmium" => "Cd",
        "tin" => "Sn",
        "tellurium" => "Te",
        "xenon" => "Xe",
        "cesium" | "caesium" => "Cs",
        "tungsten" => "W",
        "platinum" => "Pt",
        "mercury" => "Hg",
        "thallium" => "Tl",
        "lead" => "Pb",
        _ => return d.to_string(),
    };
    format!("{mass}{sym}")
}

fn layout_of(h: &JdfHeader, file_len: u64) -> std::result::Result<Layout, Fault> {
    let width = h
        .value_bytes()
        .ok_or_else(|| Fault::Unsupported(format!("data type code {}", h.data_type)))?;
    let types = &h.axis_types;
    let (two_d, sections) = match (h.data_format, h.dimensions) {
        (1, 1) => match types[0] {
            JdfAxisType::Real => (false, 1),
            JdfAxisType::Complex => (false, 2),
            t => return Err(Fault::Unsupported(format!("1D axis type {}", t.name()))),
        },
        (2, 2) => match (types[0], types[1]) {
            (JdfAxisType::Real, JdfAxisType::Real) => (true, 1),
            (JdfAxisType::Complex, JdfAxisType::Real)
            | (JdfAxisType::RealComplex, JdfAxisType::RealComplex) => (true, 2),
            (JdfAxisType::Complex, JdfAxisType::Complex) => (true, 4),
            (a, b) => {
                return Err(Fault::Unsupported(format!(
                    "2D axis types {} / {}",
                    a.name(),
                    b.name()
                )));
            }
        },
        (_, d) => {
            return Err(Fault::Unsupported(format!(
                "layout {} with {d} dimensions (one_d and two_d are read)",
                h.format_name()
            )));
        }
    };
    let dim = |k: usize| -> std::result::Result<(u64, u64, u64), Fault> {
        let n = u64::from(h.points[k]);
        let (a, b) = (u64::from(h.valid_start[k]), u64::from(h.valid_stop[k]));
        if n == 0 || a > b || b >= n {
            return Err(Fault::Corrupt(format!(
                "dimension {}: {n} points, valid range {a}..={b}",
                k + 1
            )));
        }
        Ok((n, a, b))
    };
    let (nx, x0, x1) = dim(0)?;
    let (ny, y0, y1) = if two_d { dim(1)? } else { (1, 0, 0) };
    if two_d && (nx % EDGE_2D != 0 || ny % EDGE_2D != 0) {
        return Err(Fault::Corrupt(format!(
            "2D data of {nx} × {ny} points is not a whole number of 32 × 32 submatrices"
        )));
    }
    let l = Layout {
        sections,
        two_d,
        nx,
        ny,
        x0,
        x1,
        y0,
        y1,
        width,
        little: h.little_endian,
    };
    let need = nx
        .checked_mul(ny)
        .and_then(|v| v.checked_mul(sections))
        .and_then(|v| v.checked_mul(width))
        .ok_or_else(|| Fault::Corrupt("data size overflows".into()))?;
    if u64::from(h.data_start) >= file_len && need > 0 {
        return Err(Fault::Corrupt(format!(
            "data section starts at {} but the file has {file_len} bytes (truncated)",
            h.data_start
        )));
    }
    Ok(l)
}

impl JeolDataset {
    /// Open a `.jdf` file: header and parameters only.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut head = vec![0u8; JDF_HEADER_BYTES];
        let n = f.read(&mut head).map_err(|e| Error::io(path, e))?;
        if n < 8 || &head[..8] != b"JEOL.NMR" {
            return Err(Error::UnknownFormat {
                path: path.to_path_buf(),
            });
        }
        let header = JdfHeader::parse(&head[..n]).ok_or_else(|| {
            Error::corrupt_at(
                JEOL_FORMAT_ID,
                n as u64,
                format!("file has {n} bytes; the header needs {JDF_HEADER_BYTES} (truncated)"),
            )
        })?;
        // read header + parameter section
        let pend = u64::from(header.param_start)
            .saturating_add(u64::from(header.param_length))
            .min(file_len)
            .min(MAX_PARAM_AREA);
        let mut area = vec![0u8; usize::try_from(pend).unwrap_or(0)];
        f.seek(SeekFrom::Start(0)).map_err(|e| Error::io(path, e))?;
        let got = f.read(&mut area).map_err(|e| Error::io(path, e))?;
        let mut filled = got;
        while filled < area.len() {
            let k = f
                .read(&mut area[filled..])
                .map_err(|e| Error::io(path, e))?;
            if k == 0 {
                break;
            }
            filled += k;
        }
        area.truncate(filled);
        let (params, mut param_issues) = parse_jdf_params(&area, &header);
        if u64::from(header.param_start) + u64::from(header.param_length) > MAX_PARAM_AREA {
            param_issues.push("parameter section extends past 64 MiB: not read in full".into());
        }
        let by_name = params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.to_ascii_lowercase(), i))
            .collect();
        let layout = layout_of(&header, file_len);
        let mut axis_lists = Vec::new();
        for k in 0..header.axis_listed.len() {
            let (start, len) = (header.list_start[k], header.list_length[k]);
            let list = if header.axis_listed[k]
                && len > 0
                && len <= MAX_LIST_BYTES
                && u64::from(start) + u64::from(len) <= file_len
            {
                let mut buf = vec![0u8; len as usize];
                f.seek(SeekFrom::Start(u64::from(start)))
                    .map_err(|e| Error::io(path, e))?;
                f.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
                let factor = header.units[k].factor();
                Some(
                    buf.as_chunks::<8>()
                        .0
                        .iter()
                        .map(|&a| f64::from_be_bytes(a) * factor)
                        .collect(),
                )
            } else {
                None
            };
            axis_lists.push(list);
        }
        // The context section (experiment text) keeps strings the 16-byte text parameters cut.
        let (cs, cl) = (header.context_start, header.context_length);
        let context_sample_id = if cl > 0
            && cl <= MAX_CONTEXT_BYTES
            && cs
                .checked_add(u64::from(cl))
                .is_some_and(|end| end <= file_len)
        {
            let mut buf = vec![0u8; cl as usize];
            f.seek(SeekFrom::Start(cs))
                .and_then(|_| f.read_exact(&mut buf))
                .ok()
                .and_then(|()| context_sample_id(&crate::text::decode_text(&buf).0))
        } else {
            None
        };
        Ok(JeolDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            file_len,
            header,
            params,
            param_issues,
            by_name,
            layout,
            axis_lists,
            context_sample_id,
        })
    }

    /// The sample id: the context section's value when it begins with the (16-byte) parameter
    /// or there is no parameter, else the parameter; and whether it is the context's value.
    fn sample_id(&self) -> Option<(String, bool)> {
        let param = self.text("sample_id");
        match (&self.context_sample_id, param) {
            (Some(full), Some(p)) if full.starts_with(p) => Some((full.clone(), true)),
            (Some(full), None) => Some((full.clone(), true)),
            (_, Some(p)) => Some((p.to_string(), false)),
            (None, None) => None,
        }
    }

    /// The parameter fills its 16 bytes and no longer value was found: it may be cut.
    fn sample_id_may_be_cut(&self) -> bool {
        self.sample_id()
            .is_some_and(|(id, full)| !full && id.chars().count() >= TEXT_PARAM_CHARS)
    }

    /// A parameter by name (case-insensitive).
    pub fn param(&self, name: &str) -> Option<&JdfParam> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|&i| &self.params[i])
    }

    /// The parsed header.
    pub fn header(&self) -> &JdfHeader {
        &self.header
    }

    fn text(&self, name: &str) -> Option<&str> {
        self.param(name).and_then(JdfParam::text)
    }
    /// A number in the base unit of the parameter (prefix applied).
    fn base(&self, name: &str) -> Option<f64> {
        let p = self.param(name)?;
        Some(p.number()? * p.unit.factor())
    }
    fn is_time_domain(&self) -> bool {
        self.header.units.first().is_some_and(|u| u.base == 28)
    }

    fn axis(&self, k: usize, first_index: u64, size: u64) -> Option<Value> {
        let h = &self.header;
        let n = u64::from(*h.points.get(k)?);
        let unit = h.units.get(k)?;
        if unit.base == 0 {
            return None;
        }
        let (quantity, u) = match unit.base {
            28 => ("time", "s"),
            26 => ("chemical_shift", "ppm"),
            13 => ("frequency", "Hz"),
            _ => ("other", unit.base_symbol().unwrap_or("")),
        };
        if let Some(Some(list)) = self.axis_lists.get(k) {
            let lo = usize::try_from(first_index)
                .unwrap_or(usize::MAX)
                .min(list.len());
            let hi = lo
                .saturating_add(usize::try_from(size).unwrap_or(0))
                .min(list.len());
            let vals = &list[lo..hi];
            return Some(json!({
                "quantity": quantity, "unit": u, "listed": true, "size": size,
                "first": vals.first(), "last": vals.last(), "values_table": "axis_list",
            }));
        }
        if n < 2 {
            return None;
        }
        let factor = unit.factor();
        let (start, stop) = (h.axis_start[k] * factor, h.axis_stop[k] * factor);
        let step = (stop - start) / (n - 1) as f64;
        let first = start + step * first_index as f64;
        Some(json!({
            "quantity": quantity, "unit": u, "first": first, "step": step,
            "last": first + step * (size.saturating_sub(1)) as f64, "size": size,
        }))
    }

    fn trace_info(&self) -> TraceInfo {
        let h = &self.header;
        let time = self.is_time_domain();
        let l = self.layout.as_ref().ok();
        let dtype = h.dtype();
        let names: &[&str] = match l.map(Layout::channels) {
            Some(1) => &["real"],
            _ => &["real", "imag"],
        };
        let channels = names
            .iter()
            .enumerate()
            .map(|(i, n)| SignalChannelInfo {
                index: i as u32,
                name: (*n).into(),
                unit: None,
                dtype: dtype.into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            })
            .collect();
        let samples = l.map_or(0, Layout::samples);
        let sweeps = l.map_or(1, |l| l.rows() * l.per_row());
        let mut e = BTreeMap::new();
        let put = |e: &mut BTreeMap<String, Value>, k: &str, v: Option<Value>| {
            if let Some(v) = v {
                e.insert(k.to_string(), v);
            }
        };
        e.insert(
            "kind".into(),
            json!(if time {
                "time_domain"
            } else {
                "processed_spectrum"
            }),
        );
        put(&mut e, "axis", l.and_then(|l| self.axis(0, l.x0, samples)));
        let domain = self
            .text("x_domain")
            .map(str::to_string)
            .or_else(|| h.axis_titles.first().cloned().filter(|s| !s.is_empty()));
        put(
            &mut e,
            "nucleus",
            domain.as_deref().map(|d| json!(jeol_nucleus(d))),
        );
        put(&mut e, "domain", domain.map(|d| json!(d)));
        put(
            &mut e,
            "spectrometer_frequency_mhz",
            self.base("x_freq").map(|v| json!(v / 1e6)),
        );
        put(
            &mut e,
            "carrier_offset_ppm",
            self.base("x_offset").map(|v| json!(v)),
        );
        let sw = self.base("x_sweep").filter(|v| *v > 0.0);
        put(&mut e, "spectral_width_hz", sw.map(|v| json!(v)));
        // A FID is sampled every 1/X_SWEEP s (X_ACQ_DURATION = X_POINTS / X_SWEEP). When the
        // header's axis range implies another step, the time axis follows the sampling rate and
        // the header's range is kept as `header_axis`.
        if time
            && let (Some(sw), Some(axis)) = (sw, e.get("axis").cloned())
            && let (Some(first), Some(step), Some(size)) = (
                axis.get("first").and_then(Value::as_f64),
                axis.get("step").and_then(Value::as_f64),
                axis.get("size").and_then(Value::as_u64),
            )
            && ((step * sw) - 1.0).abs() > 1e-6
        {
            let dwell = 1.0 / sw;
            e.insert(
                "axis".into(),
                json!({
                    "quantity": "time", "unit": "s", "first": first, "step": dwell,
                    "last": first + dwell * size.saturating_sub(1) as f64, "size": size,
                    "step_from": "X_SWEEP",
                }),
            );
            e.insert("header_axis".into(), axis);
        }
        if let (Some(sw), Some(f)) = (sw, self.base("x_freq").filter(|v| *v > 0.0)) {
            e.insert("spectral_width_ppm".into(), json!(sw / f * 1e6));
        }
        let int = |k: &str| {
            self.param(k)
                .and_then(JdfParam::number)
                .filter(|v| v.fract() == 0.0)
                .map(|v| json!(v as i64))
        };
        put(&mut e, "time_domain_size", int("x_points"));
        put(&mut e, "scans", int("scans"));
        put(&mut e, "total_scans", int("total_scans"));
        put(
            &mut e,
            "pulse_program",
            self.text("experiment").map(|s| json!(s)),
        );
        put(&mut e, "solvent", self.text("solvent").map(|s| json!(s)));
        if let Some(p) = self.param("temp_get")
            && let Some(v) = p.number()
        {
            match p.unit.base {
                4 => {
                    e.insert("temperature_c".into(), json!(v));
                    e.insert("temperature_k".into(), json!(v + 273.15));
                }
                14 => {
                    e.insert("temperature_k".into(), json!(v));
                }
                _ => {}
            }
        }
        put(
            &mut e,
            "field_strength_t",
            self.base("field_strength").map(|v| json!(v)),
        );
        put(&mut e, "sample_id", self.sample_id().map(|(s, _)| json!(s)));
        put(
            &mut e,
            "sample_id_truncated",
            self.sample_id_may_be_cut().then(|| json!(true)),
        );
        put(
            &mut e,
            "title",
            Some(h.title.clone())
                .filter(|s| !s.is_empty())
                .map(|s| json!(s)),
        );
        put(
            &mut e,
            "comment",
            Some(h.comment.clone())
                .filter(|s| !s.is_empty())
                .map(|s| json!(s)),
        );
        put(
            &mut e,
            "operator",
            Some(h.author.clone())
                .filter(|s| !s.is_empty())
                .map(|s| json!(s)),
        );
        put(
            &mut e,
            "site",
            Some(h.site.clone())
                .filter(|s| !s.is_empty())
                .map(|s| json!(s)),
        );
        put(
            &mut e,
            "instrument",
            self.text("inst_model_number").map(|s| json!(s)),
        );
        put(
            &mut e,
            "instrument_serial",
            self.text("inst_serial_number").map(|s| json!(s)),
        );
        put(&mut e, "console", h.instrument_name().map(|s| json!(s)));
        put(
            &mut e,
            "software_version",
            self.text("version").map(|s| json!(s)),
        );
        // JEOL spectrometer software is Delta; the file names no program (inferred)
        if self.text("version").is_some() {
            e.insert("software".into(), json!("Delta"));
        }
        put(&mut e, "sampling", self.text("sampling").map(|s| json!(s)));
        if let Some(t) = self.param("actual_start_time").and_then(JdfParam::number)
            && t > 0.0
            && t < 4e9
        {
            e.insert(
                "acquired_at".into(),
                json!(openreadout_core::time::unix_to_iso8601(
                    EPOCH_1990 + t as i64,
                    0
                )),
            );
        }
        put(&mut e, "created_on", h.created.clone().map(|s| json!(s)));
        put(&mut e, "revised_on", h.revised.clone().map(|s| json!(s)));
        e.insert("data_format".into(), json!(h.format_name()));
        e.insert(
            "axis_types".into(),
            json!(h.axis_types.iter().map(|t| t.name()).collect::<Vec<_>>()),
        );
        e.insert("stored_points".into(), json!(h.points));
        e.insert("sample_type".into(), json!(dtype));
        e.insert(
            "byte_order".into(),
            json!(if h.little_endian {
                "little-endian"
            } else {
                "big-endian"
            }),
        );
        if h.dimensions >= 2 {
            let mut d = BTreeMap::new();
            d.insert("dimension".to_string(), json!(2));
            let dom = self
                .text("y_domain")
                .map(str::to_string)
                .or_else(|| h.axis_titles.get(1).cloned().filter(|s| !s.is_empty()));
            put(
                &mut d,
                "nucleus",
                dom.as_deref().map(|x| json!(jeol_nucleus(x))),
            );
            put(&mut d, "domain", dom.map(|x| json!(x)));
            put(
                &mut d,
                "points",
                self.param("y_points")
                    .and_then(JdfParam::number)
                    .map(|v| json!(v as i64)),
            );
            put(
                &mut d,
                "spectral_width_hz",
                self.base("y_sweep").map(|v| json!(v)),
            );
            put(
                &mut d,
                "spectrometer_frequency_mhz",
                self.base("y_freq").map(|v| json!(v / 1e6)),
            );
            put(
                &mut d,
                "encoding",
                h.axis_types.get(1).map(|t| json!(t.name())),
            );
            if let Some(l) = l {
                put(&mut d, "axis", self.axis(1, l.y0, l.rows()));
            }
            e.insert("indirect_dimensions".into(), json!([d]));
        }
        TraceInfo {
            index: 0,
            name: Some(if time { "fid" } else { "spectrum" }.into()),
            sample_rate_hz: if time { sw.unwrap_or(0.0) } else { 0.0 },
            sample_count: samples,
            sweep_count: u32::try_from(sweeps).unwrap_or(u32::MAX),
            channels,
            start_s: time.then_some(0.0),
            extra: e,
        }
    }
}

impl JeolDataset {
    /// Listed axes (non-uniform sampling schedules) with the valid rows of their dimension.
    fn listed(&self) -> Vec<(usize, &Vec<f64>, u64, u64)> {
        let h = &self.header;
        self.axis_lists
            .iter()
            .enumerate()
            .filter_map(|(k, l)| {
                let l = l.as_ref()?;
                let a = u64::from(h.valid_start[k]).min(l.len() as u64);
                let b = (u64::from(h.valid_stop[k]) + 1).min(l.len() as u64).max(a);
                Some((k, l, a, b))
            })
            .collect()
    }

    fn table_infos(&self) -> Vec<openreadout_core::model::TableInfo> {
        use openreadout_core::model::{ColumnInfo, TableInfo};
        self.listed()
            .into_iter()
            .enumerate()
            .map(|(i, (k, _, a, b))| {
                let unit = match self.header.units[k].base {
                    28 => Some("s".to_string()),
                    26 => Some("ppm".to_string()),
                    13 => Some("Hz".to_string()),
                    _ => None,
                };
                TableInfo {
                    index: i as u32,
                    name: Some("axis_list".into()),
                    row_count: b - a,
                    columns: vec![
                        ColumnInfo {
                            index: 0,
                            name: "point".into(),
                            dtype: "uint32".into(),
                            ..ColumnInfo::default()
                        },
                        ColumnInfo {
                            index: 1,
                            name: "value".into(),
                            dtype: "float64".into(),
                            unit,
                            ..ColumnInfo::default()
                        },
                    ],
                    extra: BTreeMap::from([
                        ("kind".to_string(), json!("axis_values")),
                        ("dimension".to_string(), json!(k + 1)),
                        ("domain".to_string(), json!(self.header.axis_titles.get(k))),
                    ]),
                }
            })
            .collect()
    }
}

impl Dataset for JeolDataset {
    fn read_table(
        &mut self,
        index: u32,
        first: u64,
        max: u64,
    ) -> Result<openreadout_core::model::Table> {
        let listed = self.listed();
        let (_, list, a, b) = listed.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} does not exist ({} tables)",
                listed.len()
            ))
        })?;
        let rows = b - a;
        if first > rows {
            return Err(Error::Usage(format!(
                "first row {first} is past the end ({rows} rows)"
            )));
        }
        let end = first.saturating_add(max).min(rows);
        let idx: Vec<f64> = (a + first..a + end).map(|p| p as f64).collect();
        let vals: Vec<f64> = list[(a + first) as usize..(a + end) as usize].to_vec();
        Ok(openreadout_core::model::Table {
            table: index,
            first_row: first,
            columns: vec![idx, vals],
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        let mut e = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Inferred,
            from: from.to_string(),
        };
        let mut s = Sample::default();
        if let Some((id, full)) = self.sample_id() {
            let from = if full {
                "context sample_id"
            } else {
                "parameter sample_id"
            };
            s.id = Some(id);
            s.source_field = Some(from.into());
            e.provenance.insert("sample.id".into(), origin(from));
        }
        let title = self.header.title.trim();
        if !title.is_empty() && s.id.as_deref() != Some(title) {
            s.name = Some(title.to_string());
            e.provenance
                .insert("sample.name".into(), origin("header title"));
        }
        if s != Sample::default() {
            e.sample = Some(s);
        }
        let author = self.header.author.trim();
        if !author.is_empty() {
            e.acquisition = Some(Acquisition {
                operator: Some(author.to_string()),
                ..Acquisition::default()
            });
            e.provenance
                .insert("acquisition.operator".into(), origin("header author"));
        }
        (!e.is_empty()).then_some(e)
    }

    fn info(&self) -> Result<FileInfo> {
        let mut notes = Vec::new();
        match &self.layout {
            Err(Fault::Unsupported(m)) => notes.push(format!("data not decoded: unsupported {m}")),
            Err(Fault::Corrupt(m)) => notes.push(format!("data not readable: {m}")),
            Ok(l) => {
                let need = u64::from(self.header.data_start).saturating_add(l.needed_bytes());
                if need > self.file_len {
                    notes.push(format!(
                        "the data section needs bytes up to {need} but the file has {}; run `check`",
                        self.file_len
                    ));
                }
                if l.sections == 4 {
                    notes.push("2D complex data: sweeps alternate the real (even) and imaginary (odd) parts of the indirect dimension; channels are the direct dimension's real and imaginary parts".into());
                }
            }
        }
        notes.push("values are returned as stored (nmrglue returns the complex conjugate: real − i·imag); only the valid point range of each dimension is returned".into());
        if !self.param_issues.is_empty() {
            notes.push("the parameter section has problems; run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: JeolReader.descriptor(),
            format_version: Some(format!(
                "JDF {}.{}{}",
                self.header.version.0,
                self.header.version.1,
                self.text("version")
                    .map(|v| format!(", Delta {v}"))
                    .unwrap_or_default()
            )),
            images: Vec::new(),
            tables: self.table_infos(),
            spectra: Vec::new(),
            traces: vec![self.trace_info()],
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        let mut params = serde_json::Map::new();
        for p in &self.params {
            let v = match &p.value {
                JdfValue::Text(s) => json!(s.trim()),
                JdfValue::Integer(i) | JdfValue::Infinity(i) => json!(i),
                JdfValue::Float(f) if f.is_finite() => json!(f),
                JdfValue::Float(f) => json!(f.to_string()),
                JdfValue::Complex(a, b) => json!([a, b]),
                JdfValue::Unknown(c) => json!(format!("value type {c}")),
            };
            let mut m = serde_json::Map::new();
            m.insert("value".into(), v);
            if let Some(u) = p.unit.symbol() {
                m.insert("unit".into(), json!(u));
            }
            if p.scaler != 0 {
                m.insert("power_of_ten".into(), json!(p.scaler));
            }
            params.insert(p.name.clone(), Value::Object(m));
        }
        Ok(json!({
            "header": {
                "version": format!("{}.{}", h.version.0, h.version.1),
                "byte_order": if h.little_endian { "little-endian" } else { "big-endian" },
                "dimensions": h.dimensions, "data_type": h.dtype(), "data_format": h.format_name(),
                "instrument_code": h.instrument,
                "axis_types": h.axis_types.iter().map(|t| t.name()).collect::<Vec<_>>(),
                "units": h.units.iter().map(|u| u.symbol()).collect::<Vec<_>>(),
                "title": h.title, "points": h.points, "valid_start": h.valid_start, "valid_stop": h.valid_stop,
                "axis_start": h.axis_start, "axis_stop": h.axis_stop,
                "created": h.created, "revised": h.revised, "node_name": h.node_name, "site": h.site,
                "author": h.author, "comment": h.comment, "axis_titles": h.axis_titles,
                "base_frequency": h.base_frequency, "zero_point": h.zero_point, "reversed": h.reversed,
                "param_start": h.param_start, "param_length": h.param_length,
                "data_start": h.data_start, "data_length": h.data_length,
                "context_start": h.context_start, "context_length": h.context_length,
                "total_size": h.total_size,
            },
            "parameters": params,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("traces[0].sample_count", Source::PriorArt),
            ("traces[0].sweep_count", Source::PriorArt),
            ("traces[0].sample_rate_hz", Source::PriorArt),
            ("traces[0].channels[].dtype", Source::PriorArt),
            ("traces[0].extra.axis", Source::Inferred),
            ("traces[0].extra.nucleus", Source::Inferred),
            (
                "traces[0].extra.spectrometer_frequency_mhz",
                Source::PriorArt,
            ),
            ("traces[0].extra.spectral_width_hz", Source::PriorArt),
            ("traces[0].extra.scans", Source::Inferred),
            ("traces[0].extra.pulse_program", Source::Inferred),
            ("traces[0].extra.solvent", Source::Inferred),
            ("traces[0].extra.temperature_c", Source::Inferred),
            ("traces[0].extra.acquired_at", Source::Inferred),
            ("traces[0].extra.created_on", Source::Inferred),
            ("traces[0].extra.operator", Source::Inferred),
            ("traces[0].extra.instrument", Source::Inferred),
            ("traces[0].extra.indirect_dimensions", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let h = &self.header;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "header".into(),
            offset: Some(0),
            size: Some(JDF_HEADER_BYTES as u64),
            image: None,
            details: json!({"version": format!("{}.{}", h.version.0, h.version.1), "dimensions": h.dimensions}),
        }];
        out.push(LsEntry {
            kind: "parameters".into(),
            name: "parameters".into(),
            offset: Some(u64::from(h.param_start)),
            size: Some(u64::from(h.param_length)),
            image: None,
            details: json!({"records": self.params.len(), "issues": self.param_issues.len()}),
        });
        let sections = self.layout.as_ref().map_or(0, |l| l.sections);
        out.push(LsEntry {
            kind: "data".into(),
            name: "data".into(),
            offset: Some(u64::from(h.data_start)),
            size: Some(h.data_length),
            image: None,
            details: json!({"sections": sections, "sample_type": h.dtype(), "format": h.format_name()}),
        });
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            JEOL_FORMAT_ID,
            "image planes",
            "JEOL NMR data are traces (FIDs and spectra), not images: use `openreadout trace`, `openreadout export --format csv`, or the openreadout_trace MCP tool.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace index {index} out of range (0..1)"
            )));
        }
        let l = match &self.layout {
            Ok(l) => *l,
            Err(Fault::Corrupt(m)) => return Err(Error::corrupt(JEOL_FORMAT_ID, m.clone())),
            Err(Fault::Unsupported(m)) => {
                return Err(Error::unsupported(
                    JEOL_FORMAT_ID,
                    m.clone(),
                    "Only 1D and 2D (32-point submatrix) layouts with real/complex axes are decoded; `info --view full` shows the header and parameters.",
                ));
            }
        };
        let sweeps = l.rows() * l.per_row();
        if u64::from(sweep) >= sweeps {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({sweeps} sweeps)"
            )));
        }
        let n = l.samples();
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_READ_VALUES);
        let row = l.y0 + u64::from(sweep) / l.per_row();
        let secs: Vec<u64> = match l.sections {
            1 => vec![0],
            2 => vec![0, 1],
            _ => {
                let k = (u64::from(sweep) % 2) * 2;
                vec![k, k + 1]
            }
        };
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let base = u64::from(self.header.data_start);
        let mut channels = Vec::new();
        for s in secs {
            let sec_base = base + s * l.section_values() * l.width;
            let mut col: Vec<f64> = Vec::with_capacity(count as usize);
            let mut x = l.x0 + first;
            let end = x + count;
            while x < end {
                let run = if l.two_d {
                    (EDGE_2D - x % EDGE_2D).min(end - x)
                } else {
                    end - x
                };
                let off = sec_base + l.index(row, x) * l.width;
                let len = run * l.width;
                if off.saturating_add(len) > self.file_len {
                    return Err(Error::corrupt_at(
                        JEOL_FORMAT_ID,
                        off,
                        format!(
                            "sweep {sweep} needs bytes up to {} but the file has {} (truncated)",
                            off.saturating_add(len),
                            self.file_len
                        ),
                    ));
                }
                f.seek(SeekFrom::Start(off))
                    .map_err(|e| Error::io(&self.path, e))?;
                let mut buf = vec![0u8; len as usize];
                f.read_exact(&mut buf)
                    .map_err(|e| Error::io(&self.path, e))?;
                if l.width == 8 {
                    col.extend(buf.as_chunks::<8>().0.iter().map(|&a| {
                        if l.little {
                            f64::from_le_bytes(a)
                        } else {
                            f64::from_be_bytes(a)
                        }
                    }));
                } else {
                    col.extend(buf.as_chunks::<4>().0.iter().map(|&a| {
                        f64::from(if l.little {
                            f32::from_le_bytes(a)
                        } else {
                            f32::from_be_bytes(a)
                        })
                    }));
                }
                x += run;
            }
            channels.push(col);
        }
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), JEOL_FORMAT_ID);
        r.performed("header: identifier, dimensions, data type and layout, valid point ranges");
        r.performed("parameter section: record size, record count against the section length");
        r.performed("data section: sections × points × value size fit in the file");
        let h = &self.header;
        match &self.layout {
            Err(Fault::Unsupported(m)) => r.push(Finding::error("unsupported_layout", m.clone())),
            Err(Fault::Corrupt(m)) => {
                let at = if m.contains("truncated") {
                    Some(self.file_len)
                } else {
                    None
                };
                let mut f = Finding::error(
                    if at.is_some() {
                        "truncated"
                    } else {
                        "bad_header"
                    },
                    m.clone(),
                );
                if let Some(a) = at {
                    f = f.at(a);
                }
                r.push(f);
            }
            Ok(l) => {
                let need = u64::from(h.data_start).saturating_add(l.needed_bytes());
                if need > self.file_len {
                    r.push(
                        Finding::error(
                            "truncated",
                            format!(
                                "the data section needs bytes up to {need} ({} sections of {} × {} {} values) but the file has {}",
                                l.sections, l.nx, l.ny, h.dtype(), self.file_len
                            ),
                        )
                        .at(self.file_len),
                    );
                }
                if l.needed_bytes() > h.data_length {
                    r.push(Finding::warning(
                        "size_mismatch",
                        format!(
                            "the header gives the data section {} bytes; the points need {}",
                            h.data_length,
                            l.needed_bytes()
                        ),
                    ));
                }
            }
        }
        if h.total_size != 0 && h.total_size > self.file_len {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the header declares {} bytes; the file has {}",
                        h.total_size, self.file_len
                    ),
                )
                .at(self.file_len),
            );
        } else if h.total_size != 0 && h.total_size < self.file_len {
            r.push(Finding::warning(
                "extra_bytes",
                format!(
                    "{} bytes after the declared end of the file",
                    self.file_len - h.total_size
                ),
            ));
        }
        for m in &self.param_issues {
            r.push(Finding::warning("parameter_section", m.clone()));
        }
        if self.params.is_empty() {
            r.push(Finding::warning(
                "missing_parameters",
                "no parameter records",
            ));
        } else if self.param("x_sweep").is_none() {
            r.push(Finding::warning(
                "missing_parameter",
                "no X_SWEEP parameter: no sampling rate",
            ));
        }
        if let Some(h) = self.trace_info().extra.get("header_axis") {
            r.push(Finding::info(
                "axis_rate_mismatch",
                format!(
                    "the header's time axis ({} to {} s) does not step by 1/X_SWEEP; the axis follows the sampling rate",
                    h.get("first").unwrap_or(&Value::Null),
                    h.get("last").unwrap_or(&Value::Null)
                ),
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuclei() {
        assert_eq!(jeol_nucleus("Proton"), "1H");
        assert_eq!(jeol_nucleus("Carbon13"), "13C");
        assert_eq!(jeol_nucleus("Silicon29"), "29Si");
        assert_eq!(jeol_nucleus("Nitrogen15"), "15N");
        assert_eq!(jeol_nucleus("Unobtainium7"), "Unobtainium7");
    }

    #[test]
    fn submatrix_index() {
        let l = Layout {
            sections: 1,
            two_d: true,
            nx: 64,
            ny: 64,
            x0: 0,
            x1: 63,
            y0: 0,
            y1: 63,
            width: 8,
            little: true,
        };
        assert_eq!(l.index(0, 0), 0);
        assert_eq!(l.index(0, 31), 31);
        assert_eq!(l.index(0, 32), 1024);
        assert_eq!(l.index(1, 0), 32);
        assert_eq!(l.index(32, 0), 2048);
        assert_eq!(l.index(33, 33), 3 * 1024 + 32 + 1);
    }

    #[test]
    fn sample_id_from_context_text() {
        let text = "header\n    filename        => \"20230816 Zheng Rui Qi MHSWJ-15.81_proton\";\n    sample_id       => \"20230816 Zheng Rui Qi MHSWJ-15.81\";\nend header;\n";
        assert_eq!(
            context_sample_id(text).as_deref(),
            Some("20230816 Zheng Rui Qi MHSWJ-15.81")
        );
        assert_eq!(
            context_sample_id("sample_id = \"a b\";").as_deref(),
            Some("a b")
        );
        assert_eq!(context_sample_id("sample_id => \"\";"), None);
        assert_eq!(context_sample_id("sample_id => \"unterminated"), None);
        assert_eq!(context_sample_id("sample_idx => \"no\";"), None);
    }
}
