//! Bruker ESP and WinEPR data: a `.par` parameter file and a `.spc` data file. Notes:
//! `docs/formats/bruker-epr.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::assurance::{FeatureKind, Scope};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{
    Facts, SeriesChannel, SeriesColumn, SeriesFile, SeriesTable, SeriesTrace,
};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};
use serde_json::json;

use crate::common::{Axis, axis_quantity, us_date_time};
use crate::params::{self, ParFile};

pub(crate) const FORMAT_ID: &str = "bruker-esp";

/// Largest `.spc` read.
const MAX_DATA: u64 = 1 << 30;

/// The `.par` and `.spc` paths of the data set `path` names (either member).
pub(crate) fn members(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let ext = path.extension()?.to_str()?;
    if !(ext.eq_ignore_ascii_case("par") || ext.eq_ignore_ascii_case("spc")) {
        return None;
    }
    let upper = ext.chars().all(|c| c.is_ascii_uppercase());
    Some(if upper {
        (path.with_extension("PAR"), path.with_extension("SPC"))
    } else {
        (path.with_extension("par"), path.with_extension("spc"))
    })
}

/// The parameter file next to `path`, when there is one and it parses.
pub(crate) fn sibling_par(input: &Input) -> Option<ParFile> {
    let (par, spc) = members(input.path())?;
    if !input.fs().is_file(&spc) {
        return None;
    }
    let f = input.fs().open(&par).ok()?;
    let n = f.size().ok()?;
    if n > params::MAX_TEXT {
        return None;
    }
    let mut b = vec![0u8; usize::try_from(n).ok()?];
    f.read_exact_at(0, &mut b).ok()?;
    params::parse_par(&params::text(&b))
}

/// Which writer the parameters say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flavour {
    /// WinEPR / Simfonia (`DOS Format`): little-endian float32.
    WinEpr,
    /// ESP 300/380 and ECS, cw data: big-endian int32.
    EspCw,
    /// ESP pulse or 2D data: big-endian int32.
    EspPulse,
}

pub(crate) struct Parsed {
    pub(crate) file: SeriesFile,
    pub(crate) size: u64,
}

fn count(v: f64, key: &str) -> Result<usize> {
    if v >= 1.0 && v.fract() == 0.0 && v < 1e9 {
        Ok(v as usize)
    } else {
        Err(Error::corrupt(
            FORMAT_ID,
            format!("{key} is {v}, not a point count"),
        ))
    }
}

pub(crate) fn open(input: &Input) -> Result<Parsed> {
    let (par_path, spc_path) = members(input.path()).ok_or_else(|| {
        Error::Usage(format!(
            "{} is not a .par/.spc file",
            input.path().display()
        ))
    })?;
    let fs = input.fs();
    let par_bytes = fs.read(&par_path).map_err(|e| Error::io(&par_path, e))?;
    if par_bytes.len() as u64 > params::MAX_TEXT {
        return Err(Error::corrupt(
            FORMAT_ID,
            "parameter file larger than 16 MiB",
        ));
    }
    let p = params::parse_par(&params::text(&par_bytes)).ok_or_else(|| {
        Error::corrupt(
            FORMAT_ID,
            "the .par file does not hold ESP/WinEPR parameters",
        )
    })?;
    let meta = fs
        .metadata(&spc_path)
        .map_err(|e| Error::io(&spc_path, e))?;
    if meta.len() > MAX_DATA {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("a {} MiB .spc file", meta.len() >> 20),
            "Data files larger than 1 GiB are not read.",
        ));
    }
    let spc = fs.read(&spc_path).map_err(|e| Error::io(&spc_path, e))?;
    let mut findings = Vec::new();

    let mut flavour = if p.get("DOS").is_some() {
        Flavour::WinEpr
    } else {
        Flavour::EspCw
    };
    let (mut complex, mut two_d) = (false, false);
    if let Some(jss) = p.number("JSS") {
        if jss < 0.0 || jss.fract() != 0.0 || jss > f64::from(u32::MAX) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("JSS is {jss}, not a flag word"),
            ));
        }
        let flags = jss as u32;
        complex = flags & (1 << 4) != 0;
        two_d = flags & (1 << 12) != 0;
    }
    let mut nx: usize = 1024;
    let mut ny: usize = 1;
    let mut nx_from = "the default of 1024 points";
    if two_d {
        if let Some(v) = p.number("SSX") {
            nx = count(v, "SSX")?;
            if complex {
                nx /= 2;
            }
            nx_from = "SSX";
            if flavour == Flavour::EspCw {
                flavour = Flavour::EspPulse;
            }
        }
        if let Some(v) = p.number("SSY") {
            ny = count(v, "SSY")?;
            if flavour == Flavour::EspCw {
                flavour = Flavour::EspPulse;
            }
        }
    }
    if let Some(v) = p.number("ANZ") {
        let anz = count(v, "ANZ")?;
        if two_d {
            if nx.checked_mul(ny) != Some(anz) {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("2D data: SSX × SSY ({nx} × {ny}) is not ANZ ({anz})"),
                ));
            }
        } else {
            if flavour == Flavour::EspCw {
                flavour = Flavour::EspPulse;
            }
            nx = if complex { anz / 2 } else { anz };
            nx_from = "ANZ";
        }
    }
    if let Some(v) = p.number("RES") {
        nx = count(v, "RES")?;
        nx_from = "RES";
    }
    if let Some(v) = p.number("REY") {
        ny = count(v, "REY")?;
    }
    if let Some(v) = p.number("XPLS") {
        nx = count(v, "XPLS")?;
        nx_from = "XPLS";
    }
    if !two_d && ny > 1 {
        // a slice of 2D data saved by WinEPR: REY names the original size
        ny = 1;
    }
    let size = 4usize;
    let per_point = if complex { 2 } else { 1 };
    let need = nx
        .checked_mul(ny)
        .and_then(|v| v.checked_mul(per_point))
        .and_then(|v| v.checked_mul(size))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "data size overflows"))?;
    if spc.len() < need {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "the .spc file holds {} bytes; {nx} × {ny} points ({nx_from}) need {need}",
                spc.len()
            ),
        ));
    }
    if spc.len() > need {
        findings.push(Finding::warning(
            "trailing_data",
            format!(
                "the .spc file holds {} bytes more than the parameters describe; they are ignored",
                spc.len() - need
            ),
        ));
    }
    let values: Vec<f64> = spc[..need]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&a| match flavour {
            Flavour::WinEpr => f64::from(f32::from_le_bytes(a)),
            _ => f64::from(i32::from_be_bytes(a)),
        })
        .collect();

    // the abscissa
    let unit = p.get("JUN").unwrap_or("G").trim().to_string();
    let jex = p.get("JEX").unwrap_or("field-sweep").trim().to_string();
    let (hcf, hsw, gst, gsi) = (
        p.number("HCF"),
        p.number("HSW"),
        p.number("GST"),
        p.number("GSI"),
    );
    let (xxlb, xxwi, xylb, xywi) = (
        p.number("XXLB"),
        p.number("XXWI"),
        p.number("XYLB"),
        p.number("XYWI"),
    );
    let linear =
        |first: f64, width: f64, quantity: &'static str, unit: &str, label: &str| Axis::Linear {
            first,
            step: if nx > 1 { width / (nx - 1) as f64 } else { 0.0 },
            quantity,
            unit: unit.to_string(),
            label: label.to_string(),
        };
    let field = axis_quantity("field", &unit);
    let (x, x_from) = if jex == "Time-Sweep" {
        let rct = p.number("RCT").unwrap_or(1.0);
        (
            Axis::Linear {
                first: 0.0,
                step: rct / 1e3,
                quantity: "time",
                unit: "s".into(),
                label: String::new(),
            },
            "RCT (conversion time, ms)",
        )
    } else if jex == "ENDOR" {
        match (gst, gsi) {
            (Some(a), Some(w)) => (linear(a, w, "frequency", "MHz", ""), "GST and GSI"),
            _ => (Axis::points(), "no range in the parameters"),
        }
    } else if let (Some(a), Some(w), Some(_), Some(_)) = (xxlb, xxwi, xylb, xywi) {
        (linear(a, w, field, &unit, ""), "XXLB and XXWI")
    } else if let (Some(_), Some(_), Some(a), Some(w)) = (hcf, hsw, gst, gsi) {
        (linear(a, w, field, &unit, ""), "GST and GSI")
    } else if let (Some(c), Some(w)) = (hcf, hsw) {
        (linear(c - w / 2.0, w, field, &unit, ""), "HCF ± HSW/2")
    } else if let (Some(a), Some(w)) = (gst, gsi) {
        (linear(a, w, field, &unit, ""), "GST and GSI")
    } else if let (Some(a), Some(w)) = (xxlb, xxwi) {
        (linear(a, w, field, &unit, ""), "XXLB and XXWI")
    } else {
        findings.push(Finding::warning(
            "no_axis",
            "the parameters give no field range: points are numbered",
        ));
        (Axis::points(), "no range in the parameters")
    };
    let y = if ny > 1 {
        match (xylb, xywi) {
            (Some(a), Some(w)) => Some(Axis::Linear {
                first: a,
                step: w / (ny - 1) as f64,
                quantity: match p.get("JEY").unwrap_or("") {
                    "mw-power-sweep" => "attenuation",
                    _ => "y",
                },
                unit: String::new(),
                label: p.get("JEY").unwrap_or("").to_string(),
            }),
            _ => Some(Axis::points()),
        }
    } else {
        None
    };

    let title = p.get("JCO").filter(|s| !s.trim().is_empty()).map_or_else(
        || {
            par_path
                .file_stem()
                .map_or_else(|| "spectrum".into(), |s| s.to_string_lossy().to_string())
        },
        str::to_string,
    );
    let dtype = if flavour == Flavour::WinEpr {
        "float32"
    } else {
        "int32"
    };
    let mut channels = Vec::new();
    if complex {
        channels.push(SeriesChannel::new("real", None, dtype));
        channels.push(SeriesChannel::new("imaginary", None, dtype));
    } else {
        channels.push(SeriesChannel::new("intensity", None, dtype));
    }
    let mut sweeps = Vec::with_capacity(ny);
    for s in 0..ny {
        let mut re = Vec::with_capacity(nx);
        let mut im = Vec::new();
        for i in 0..nx {
            let at = (s * nx + i) * per_point;
            re.push(values.get(at).copied().unwrap_or(f64::NAN));
            if complex {
                im.push(values.get(at + 1).copied().unwrap_or(f64::NAN));
            }
        }
        let mut sw = vec![re];
        if complex {
            sw.push(im);
        }
        sweeps.push(sw);
    }
    let mut extra = BTreeMap::new();
    extra.insert("axis".into(), x.to_json(nx, 0));
    extra.insert("axis_from".into(), json!(x_from));
    extra.insert("kind".into(), json!("epr_spectrum"));
    extra.insert("data_type".into(), json!("EPR SPECTRUM"));
    extra.insert("experiment".into(), json!(jex));
    if let Some(y) = &y {
        extra.insert("sweep_axis".into(), y.to_json(ny, 0));
        extra.insert("sweep_table".into(), json!(0));
    }
    let trace = SeriesTrace {
        name: title.clone(),
        channels,
        sweeps,
        sample_rate_hz: 0.0,
        start_s: None,
        extra,
    };
    let mut tables = Vec::new();
    if let Some(y) = &y {
        tables.push(SeriesTable {
            name: "sweeps".into(),
            columns: vec![
                SeriesColumn::numbers("sweep", None, (0..ny).map(|s| s as f64).collect()),
                SeriesColumn::numbers(y.quantity(), y.unit(), (0..ny).map(|s| y.at(s)).collect()),
            ],
            extra: BTreeMap::from([
                ("kind".to_string(), json!("sweep_axis")),
                ("trace".to_string(), json!(0)),
            ]),
        });
    }

    let mut f = Facts::new();
    f.set("instrument.vendor", "Bruker", "the file format")
        .instrument_kind("CHMO:0002253");
    f.set(
        "instrument.software",
        if flavour == Flavour::WinEpr {
            "WinEPR"
        } else {
            "ESP"
        },
        "`DOS Format` in the parameter file (WinEPR) or its absence (ESP)",
    );
    f.set(
        "acquisition.operator",
        p.get("JON").unwrap_or(""),
        ".par JON",
    );
    f.set(
        "acquisition.comment",
        p.get("JCO").unwrap_or(""),
        ".par JCO",
    );
    if let Some(s) = p.get("JDA").and_then(|d| us_date_time(d, p.get("JTM"))) {
        f.set(
            "acquisition.started_at",
            &s,
            ".par JDA and JTM (month/day/year, local time)",
        );
    }
    let technique = if jex == "ENDOR" || flavour == Flavour::EspPulse {
        "CHMO:0000328"
    } else {
        "CHMO:0000329"
    };
    f.technique(technique, ".par JEX and the data layout");
    if let Some(v) = p.number("MF") {
        f.number("microwave_frequency", v, "GHz", ".par MF (GHz)");
    }
    if let Some(v) = p.number("MP") {
        f.number("microwave_power", v, "mW", ".par MP (mW)");
    }
    if let Some(v) = p.number("RMA") {
        f.number("modulation_amplitude", v, "G", ".par RMA (G)");
    }
    if let Some(v) = p.number("RRG") {
        f.plain("receiver_gain", v, ".par RRG");
    }
    if let Some(v) = p.number("RCT") {
        f.number("conversion_time", v, "ms", ".par RCT (ms)");
    }
    if let Some(v) = p.number("RTC") {
        f.number("time_constant", v, "ms", ".par RTC (ms)");
    }
    if let Some(v) = p.number("HCF") {
        f.number("center_field", v, "G", ".par HCF (G)");
    }
    if let Some(v) = p.number("HSW") {
        f.number("sweep_width", v, "G", ".par HSW (G)");
    }
    if let Some(v) = p.number("JSD") {
        f.plain("scans", v, ".par JSD");
    }
    if let Some(v) = p.number("TE").filter(|t| *t > 0.0) {
        f.number("temperature", v, "K", ".par TE (K)");
    }
    f.measurement(
        MeasurementKind::Trace,
        vec![0],
        format!(
            "EPR ({jex}), {nx} points{}{}",
            if ny > 1 {
                format!(" × {ny} sweeps")
            } else {
                String::new()
            },
            if complex { ", complex" } else { "" }
        ),
        Some(technique),
    );

    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    let mut observations = openreadout_core::assurance::Observations::default();
    observations.feature(
        FeatureKind::Writer,
        match flavour {
            Flavour::WinEpr => "WinEPR",
            Flavour::EspCw => "ESP cw",
            Flavour::EspPulse => "ESP pulse/2D",
        },
        &[Scope::Traces],
    );
    let size = par_bytes.len() as u64 + spc.len() as u64;
    let entries = vec![
        LsEntry {
            kind: "metadata".into(),
            name: name(&par_path),
            offset: None,
            size: Some(par_bytes.len() as u64),
            image: None,
            details: json!({"keys": p.entries.len()}),
        },
        LsEntry {
            kind: "block".into(),
            name: name(&spc_path),
            offset: Some(0),
            size: Some(spc.len() as u64),
            image: None,
            details: json!({"points": nx * ny, "values_per_point": per_point, "dtype": dtype, "byte_order": if flavour == Flavour::WinEpr {"little"} else {"big"}}),
        },
    ];
    let file = SeriesFile {
        format_version: Some(
            match flavour {
                Flavour::WinEpr => "WinEPR",
                _ => "ESP",
            }
            .into(),
        ),
        traces: vec![trace],
        tables,
        experiment: Some(f.build()),
        vendor: json!({ "esp_parameters": p.to_json() }),
        entries,
        findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks: vec![format!(
            "the .spc size checked against the point count ({need} bytes)"
        )],
        members: vec![par_path.clone(), spc_path.clone()],
    };
    Ok(Parsed { file, size })
}

fn name(p: &Path) -> String {
    p.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().to_string())
}
