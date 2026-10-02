//! Bruker BES3T data sets: a `.DSC` descriptor and a `.DTA` data file (optionally `.XGF`,
//! `.YGF`, `.ZGF` axis files). Notes: `docs/formats/bruker-epr.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::assurance::{FeatureKind, Scope};
use openreadout_core::bytes::Endian;
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{
    Facts, SeriesChannel, SeriesColumn, SeriesFile, SeriesTable, SeriesTrace,
};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};
use serde_json::json;

use crate::common::{Axis, axis_quantity, epr_facts};
use crate::params::{self, Descriptor};

pub(crate) const FORMAT_ID: &str = "bruker-bes3t";

/// Largest `.DTA` read (the largest public data sets are tens of MB).
const MAX_DATA: u64 = 1 << 30;

/// The paths of a BES3T data set, from either member.
pub(crate) fn members(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let ext = path.extension()?.to_str()?;
    let (dsc, dta) = match ext {
        "DSC" | "DTA" => ("DSC", "DTA"),
        "dsc" | "dta" => ("dsc", "dta"),
        e if e.eq_ignore_ascii_case("dsc") || e.eq_ignore_ascii_case("dta") => ("DSC", "DTA"),
        _ => return None,
    };
    Some((path.with_extension(dsc), path.with_extension(dta)))
}

/// A number format of `IRFMT`/`xFMT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Num {
    I8,
    I16,
    I32,
    F32,
    F64,
}

impl Num {
    fn of(code: &str) -> Option<Num> {
        match code.trim().to_ascii_uppercase().as_str() {
            "C" => Some(Num::I8),
            "S" => Some(Num::I16),
            "I" => Some(Num::I32),
            "F" => Some(Num::F32),
            "D" => Some(Num::F64),
            _ => None,
        }
    }
    fn size(self) -> usize {
        match self {
            Num::I8 => 1,
            Num::I16 => 2,
            Num::I32 | Num::F32 => 4,
            Num::F64 => 8,
        }
    }
    fn dtype(self) -> &'static str {
        match self {
            Num::I8 => "int8",
            Num::I16 => "int16",
            Num::I32 => "int32",
            Num::F32 => "float32",
            Num::F64 => "float64",
        }
    }
    fn decode(self, b: &[u8], big: bool) -> f64 {
        let e = if big { Endian::Big } else { Endian::Little };
        match self {
            Num::I8 => b.first().map(|&v| f64::from(i8::from_ne_bytes([v]))),
            Num::I16 => e.i16(b, 0).map(f64::from),
            Num::I32 => e.i32(b, 0).map(f64::from),
            Num::F32 => e.f32(b, 0).map(f64::from),
            Num::F64 => e.f64(b, 0),
        }
        .unwrap_or(f64::NAN)
    }
    fn decode_all(self, b: &[u8], big: bool) -> Vec<f64> {
        b.chunks_exact(self.size())
            .map(|c| self.decode(c, big))
            .collect()
    }
}

fn unsupported(what: impl Into<String>, hint: &str) -> Error {
    Error::unsupported(FORMAT_ID, what.into(), hint)
}

/// Points of a dimension (`XPTS`…), 1 when absent.
fn points(d: &Descriptor, key: &str) -> Result<usize> {
    match d.get(key) {
        None => Ok(1),
        Some(v) => v
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("{key} is `{v}`, not a count"))),
    }
}

/// One dimension's axis.
fn axis(
    input: &Input,
    d: &Descriptor,
    base: &Path,
    dim: char,
    n: usize,
    big: bool,
    notes: &mut Vec<Finding>,
    members: &mut Vec<PathBuf>,
) -> Result<Axis> {
    let key = |k: &str| format!("{dim}{k}");
    let name = d.get(&key("NAM")).unwrap_or("").to_string();
    let unit = d.get(&key("UNI")).unwrap_or("").to_string();
    let typ = d.get(&key("TYP")).unwrap_or("IDX").to_string();
    let quantity = axis_quantity(&name, &unit);
    match typ.as_str() {
        "IDX" | "NODATA" => {
            let min = d.get(&key("MIN")).and_then(params::leading_number);
            let wid = d.get(&key("WID")).and_then(params::leading_number);
            match (min, wid) {
                (Some(min), Some(wid)) if wid != 0.0 || n <= 1 => Ok(Axis::Linear {
                    first: min,
                    step: if n > 1 { wid / (n - 1) as f64 } else { 0.0 },
                    quantity,
                    unit,
                    label: name,
                }),
                (Some(_), Some(_)) => {
                    notes.push(Finding::warning(
                        "zero_width_axis",
                        format!("{dim} axis has zero width: points are numbered instead"),
                    ));
                    Ok(Axis::points())
                }
                _ if n <= 1 => Ok(Axis::points()),
                _ => Err(Error::corrupt(
                    FORMAT_ID,
                    format!("{dim}TYP IDX without {dim}MIN and {dim}WID"),
                )),
            }
        }
        "IGD" => {
            let ext = if base.extension().is_some_and(|e| e == "dsc") {
                format!("{}gf", dim.to_ascii_lowercase())
            } else {
                format!("{dim}GF")
            };
            let path = base.with_extension(ext);
            let fmt = d
                .get(&key("FMT"))
                .and_then(Num::of)
                .filter(|f| matches!(f, Num::F64 | Num::F32 | Num::I32 | Num::I16))
                .ok_or_else(|| {
                    unsupported(
                        format!(
                            "{dim} axis file in format `{}`",
                            d.get(&key("FMT")).unwrap_or("")
                        ),
                        "Only D, F, I and S axis files are read.",
                    )
                })?;
            let bytes = input.fs().read(&path).map_err(|_| {
                Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "{dim}TYP IGD but the axis file {} is missing",
                        path.file_name()
                            .map_or_else(String::new, |n| n.to_string_lossy().into())
                    ),
                )
            })?;
            let need = n
                .checked_mul(fmt.size())
                .ok_or_else(|| Error::corrupt(FORMAT_ID, "axis too long"))?;
            if bytes.len() < need {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "axis file {dim}GF holds {} bytes; {n} {} values need {need}",
                        bytes.len(),
                        fmt.dtype()
                    ),
                ));
            }
            members.push(path);
            Ok(Axis::Listed {
                values: fmt.decode_all(&bytes[..need], big),
                quantity,
                unit,
                label: name,
            })
        }
        "NTUP" => Err(unsupported(
            format!("{dim} axis of type NTUP"),
            "N-tuple axes are not read.",
        )),
        other => Err(unsupported(
            format!("{dim} axis of type `{other}`"),
            "Only IDX and IGD axes are read.",
        )),
    }
}

/// A parsed data set.
pub(crate) struct Parsed {
    pub(crate) file: SeriesFile,
    pub(crate) size: u64,
}

/// Parse the data set `path` (either member) names.
pub(crate) fn open(input: &Input) -> Result<Parsed> {
    let path = input.path();
    let (dsc_path, dta_path) = members(path).ok_or_else(|| {
        Error::Usage(format!(
            "{} is not a BES3T file (.DSC or .DTA)",
            path.display()
        ))
    })?;
    let fs = input.fs();
    let dsc_bytes = fs.read(&dsc_path).map_err(|e| Error::io(&dsc_path, e))?;
    if dsc_bytes.len() as u64 > params::MAX_TEXT {
        return Err(Error::corrupt(FORMAT_ID, "descriptor larger than 16 MiB"));
    }
    if !params::is_descriptor(&dsc_bytes) {
        return Err(Error::corrupt(
            FORMAT_ID,
            "the .DSC file does not start with a #DESC layer",
        ));
    }
    let desc = params::parse_descriptor(&params::text(&dsc_bytes));
    let dta_meta = fs
        .metadata(&dta_path)
        .map_err(|e| Error::io(&dta_path, e))?;
    if dta_meta.len() > MAX_DATA {
        return Err(unsupported(
            format!("a {} MiB .DTA file", dta_meta.len() >> 20),
            "Data sets larger than 1 GiB are not read.",
        ));
    }
    let dta = fs.read(&dta_path).map_err(|e| Error::io(&dta_path, e))?;
    let size = dta.len() as u64 + dsc_bytes.len() as u64;
    let mut findings = Vec::new();
    let mut members_used = vec![dsc_path.clone(), dta_path.clone()];

    // what a point holds
    let ikkf: Vec<String> = desc
        .get("IKKF")
        .unwrap_or("REAL")
        .split(',')
        .map(|s| s.trim().to_ascii_uppercase())
        .collect();
    if desc.get("IKKF").is_none() {
        findings.push(Finding::warning(
            "missing_ikkf",
            "no IKKF in the descriptor: the data are read as real",
        ));
    }
    let complex: Vec<bool> = ikkf
        .iter()
        .map(|k| match k.as_str() {
            "REAL" => Ok(false),
            "CPLX" => Ok(true),
            other => Err(unsupported(
                format!("IKKF value `{other}`"),
                "Only REAL and CPLX data are read.",
            )),
        })
        .collect::<Result<_>>()?;
    let irfmt: Vec<&str> = desc
        .get("IRFMT")
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "the descriptor has no IRFMT"))?
        .split(',')
        .map(str::trim)
        .collect();
    if irfmt.len() != complex.len() {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "IKKF lists {} values per point but IRFMT {}",
                complex.len(),
                irfmt.len()
            ),
        ));
    }
    let fmts: Vec<Num> = irfmt
        .iter()
        .map(|c| {
            Num::of(c).ok_or_else(|| match c.to_ascii_uppercase().as_str() {
                "A" => unsupported(
                    "BES3T data stored as ASCII",
                    "ASCII BES3T data sets are not read; export the data as binary from Xepr.",
                ),
                "0" | "N" => unsupported(
                    "a BES3T data set without data",
                    "The descriptor says the data set holds no data.",
                ),
                other => unsupported(
                    format!("IRFMT `{other}`"),
                    "Only C, S, I, F and D data are read.",
                ),
            })
        })
        .collect::<Result<_>>()?;
    if let Some(ii) = desc.get("IIFMT")
        && !ii.eq_ignore_ascii_case(desc.get("IRFMT").unwrap_or(""))
        && complex.iter().any(|c| *c)
    {
        return Err(unsupported(
            format!("IIFMT `{ii}` differing from IRFMT"),
            "Real and imaginary parts in different number formats are not read.",
        ));
    }
    if fmts.windows(2).any(|w| w[0] != w[1]) {
        return Err(unsupported(
            "data values of one point in different number formats",
            "Points whose values have different formats are not read.",
        ));
    }
    let fmt = fmts[0];
    let big = match desc.get("BSEQ") {
        Some("BIG") => true,
        Some("LIT") => false,
        None => {
            findings.push(Finding::warning(
                "missing_bseq",
                "no BSEQ in the descriptor: big-endian assumed",
            ));
            true
        }
        Some(other) => {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("BSEQ `{other}` is neither BIG nor LIT"),
            ));
        }
    };
    let nx = points(&desc, "XPTS")?;
    if desc.get("XPTS").is_none() {
        return Err(Error::corrupt(FORMAT_ID, "the descriptor has no XPTS"));
    }
    let ny = points(&desc, "YPTS")?;
    let nz = points(&desc, "ZPTS")?;
    let reals_per_point: usize = complex.iter().map(|c| if *c { 2 } else { 1 }).sum();
    let n_points = nx
        .checked_mul(ny)
        .and_then(|v| v.checked_mul(nz))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "XPTS × YPTS × ZPTS overflows"))?;
    let need = n_points
        .checked_mul(reals_per_point)
        .and_then(|v| v.checked_mul(fmt.size()))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "data size overflows"))?;
    if dta.len() < need {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "the .DTA file holds {} bytes; {nx} × {ny} × {nz} points of {} × {} need {need}",
                dta.len(),
                reals_per_point,
                fmt.dtype()
            ),
        ));
    }
    if dta.len() > need {
        findings.push(Finding::warning(
            "trailing_data",
            format!(
                "the .DTA file holds {} bytes more than the descriptor describes; they are ignored",
                dta.len() - need
            ),
        ));
    }
    let x = axis(
        input,
        &desc,
        &dsc_path,
        'X',
        nx,
        big,
        &mut findings,
        &mut members_used,
    )?;
    let y = if ny > 1 {
        Some(axis(
            input,
            &desc,
            &dsc_path,
            'Y',
            ny,
            big,
            &mut findings,
            &mut members_used,
        )?)
    } else {
        None
    };
    let z = if nz > 1 {
        Some(axis(
            input,
            &desc,
            &dsc_path,
            'Z',
            nz,
            big,
            &mut findings,
            &mut members_used,
        )?)
    } else {
        None
    };

    // decode: point p (x fastest), values in IKKF order, a complex value as real then imaginary
    let all = fmt.decode_all(&dta[..need], big);
    let title = desc.get("TITL").map_or_else(
        || {
            dsc_path
                .file_stem()
                .map_or_else(|| "spectrum".into(), |s| s.to_string_lossy().to_string())
        },
        str::to_string,
    );
    let names = params::list(desc.get("IRNAM").unwrap_or(""));
    let units = params::list(desc.get("IRUNI").unwrap_or(""));
    let component_names = harmonic_names(&desc, complex.len());
    let mut traces = Vec::new();
    let mut offset = 0usize;
    for (component, cplx) in complex.iter().enumerate() {
        let label = names.get(component).map_or("", String::as_str);
        let unit = units
            .get(component)
            .map(String::as_str)
            .filter(|u| !u.is_empty());
        let mut channels = Vec::new();
        if let Axis::Listed { .. } = x {
            channels.push(SeriesChannel::new(
                x.quantity(),
                x.unit(),
                desc.get("XFMT")
                    .and_then(Num::of)
                    .map_or("float64", Num::dtype),
            ));
        }
        let ch = |name: &str| {
            let mut channel = SeriesChannel::new(name, unit, fmt.dtype());
            if !label.is_empty() {
                channel.extra.insert("label".into(), json!(label));
            }
            channel
        };
        if *cplx {
            channels.push(ch("real"));
            channels.push(ch("imaginary"));
        } else {
            channels.push(ch("intensity"));
        }
        let mut sweeps = Vec::with_capacity(ny * nz);
        for sweep_no in 0..ny * nz {
            let mut sweep: Vec<Vec<f64>> = Vec::new();
            if let Axis::Listed { values, .. } = &x {
                sweep.push(values.clone());
            }
            let mut re = Vec::with_capacity(nx);
            let mut im = Vec::with_capacity(if *cplx { nx } else { 0 });
            for i in 0..nx {
                let at = (sweep_no * nx + i) * reals_per_point + offset;
                re.push(all.get(at).copied().unwrap_or(f64::NAN));
                if *cplx {
                    im.push(all.get(at + 1).copied().unwrap_or(f64::NAN));
                }
            }
            sweep.push(re);
            if *cplx {
                sweep.push(im);
            }
            sweeps.push(sweep);
        }
        offset += if *cplx { 2 } else { 1 };
        let mut extra = BTreeMap::new();
        extra.insert("axis".into(), x.to_json(nx, 0));
        extra.insert("kind".into(), json!("epr_spectrum"));
        extra.insert("data_type".into(), json!("EPR SPECTRUM"));
        if let Some(e) = desc.get("EXPT") {
            extra.insert("experiment".into(), json!(e.to_ascii_lowercase()));
        }
        if complex.len() > 1 {
            extra.insert("component".into(), json!(component));
            if let Some(h) = component_names.as_ref().and_then(|n| n.get(component)) {
                extra.insert("harmonic".into(), json!(h.0));
                extra.insert("harmonic_phase_deg".into(), json!(h.1));
            }
        }
        if let Some(y) = &y {
            extra.insert("sweep_axis".into(), y.to_json(ny, 0));
        }
        if let Some(z) = &z {
            extra.insert("sweep_axis_2".into(), z.to_json(nz, 0));
        }
        if ny * nz > 1 {
            extra.insert("sweep_table".into(), json!(0));
        }
        traces.push(SeriesTrace {
            name: if complex.len() > 1 {
                match component_names.as_ref().and_then(|n| n.get(component)) {
                    Some((harmonic, 0)) => format!("{title} (harmonic {harmonic})"),
                    Some((harmonic, phase)) => {
                        format!("{title} (harmonic {harmonic}, {phase}°)")
                    }
                    None => format!("{title} ({})", component + 1),
                }
            } else {
                title.clone()
            },
            channels,
            sweeps,
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        });
    }

    let mut tables = Vec::new();
    if ny * nz > 1 {
        let mut cols = vec![SeriesColumn::numbers(
            "sweep",
            None,
            (0..ny * nz).map(|s| s as f64).collect(),
        )];
        if let Some(y) = &y {
            let mut c = SeriesColumn::numbers(
                y.quantity(),
                y.unit(),
                (0..ny * nz).map(|s| y.at(s % ny)).collect(),
            );
            c.extra.insert("dimension".into(), json!("y"));
            cols.push(c);
        }
        if let Some(z) = &z {
            let mut c = SeriesColumn::numbers(
                z.quantity(),
                z.unit(),
                (0..ny * nz).map(|s| z.at(s / ny)).collect(),
            );
            c.extra.insert("dimension".into(), json!("z"));
            cols.push(c);
        }
        tables.push(SeriesTable {
            name: "sweeps".into(),
            columns: cols,
            extra: BTreeMap::from([
                ("kind".to_string(), json!("sweep_axis")),
                ("trace".to_string(), json!(0)),
            ]),
        });
    }

    let mut facts = Facts::new();
    epr_facts(&mut facts, &desc);
    let nsweeps = ny * nz;
    let what = format!(
        "{} EPR, {} points{}{}",
        match desc.get("EXPT") {
            Some("CW") => "continuous-wave",
            Some("PLS") => "pulse",
            _ => "",
        },
        nx,
        if nsweeps > 1 {
            format!(" × {nsweeps} sweeps")
        } else {
            String::new()
        },
        if complex.iter().any(|c| *c) {
            ", complex"
        } else {
            ""
        }
    );
    facts.measurement(
        MeasurementKind::Trace,
        (0..traces.len() as u32).collect(),
        what.trim().to_string(),
        Some(match desc.get("EXPT") {
            Some("CW") => "CHMO:0000329",
            Some("PLS") => "CHMO:0000330",
            _ => "CHMO:0000328",
        }),
    );

    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    provenance.insert("tables".into(), Source::PriorArt);
    let mut observations = openreadout_core::assurance::Observations::default();
    observations.feature(
        FeatureKind::SampleLayout,
        format!(
            "{} {}{}",
            if big { "big-endian" } else { "little-endian" },
            fmt.dtype(),
            if complex.iter().any(|c| *c) {
                " complex"
            } else {
                ""
            }
        ),
        &[Scope::Traces],
    );
    let entries = vec![
        LsEntry {
            kind: "metadata".into(),
            name: file_name(&dsc_path),
            offset: None,
            size: Some(dsc_bytes.len() as u64),
            image: None,
            details: json!({"layers": desc.versions}),
        },
        LsEntry {
            kind: "block".into(),
            name: file_name(&dta_path),
            offset: Some(0),
            size: Some(dta.len() as u64),
            image: None,
            details: json!({"points": n_points, "values_per_point": reals_per_point, "dtype": fmt.dtype()}),
        },
    ];
    let file = SeriesFile {
        format_version: desc
            .versions
            .get("DESC")
            .filter(|v| !v.is_empty())
            .map(|v| format!("BES3T {v}")),
        traces,
        tables,
        experiment: Some(facts.build()),
        vendor: json!({ "bes3t": desc.to_json() }),
        entries,
        findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks: vec![format!(
            "the .DTA size checked against XPTS × YPTS × ZPTS × IKKF ({need} bytes)"
        )],
        members: members_used,
    };
    Ok(Parsed { file, size })
}

/// The detection harmonics of a multi-harmonic data set: when the signal channel enables exactly
/// as many harmonics as a point holds values, value `k` is the `k`-th enabled one in the order
/// 1st, 1st at 90°, 2nd, 2nd at 90°, … (an inference from the device parameters; see the notes).
fn harmonic_names(d: &Descriptor, n: usize) -> Option<Vec<(u32, u32)>> {
    if n < 2 {
        return None;
    }
    let mut out = Vec::new();
    for (h, word) in [(1, "1st"), (2, "2nd"), (3, "3rd"), (4, "4th"), (5, "5th")] {
        for (suffix, phase) in [("", 0), ("90", 90)] {
            if d.device(None, &format!("Enable{word}Harm{suffix}")) == Some("True") {
                out.push((h, phase));
            }
        }
    }
    (out.len() == n).then_some(out)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().to_string())
}

/// True when the `.DSC` next to `path` exists and is a descriptor.
pub(crate) fn has_descriptor(input: &Input) -> bool {
    let Some((dsc, _)) = members(input.path()) else {
        return false;
    };
    input
        .fs()
        .open(&dsc)
        .ok()
        .and_then(|f| {
            let mut b = vec![0u8; 512.min(f.size().ok()? as usize)];
            f.read_exact_at(0, &mut b).ok()?;
            Some(params::is_descriptor(&b))
        })
        .unwrap_or(false)
}
