//! Acquisition and stored processing parameters of every dimension of a multidimensional NMR
//! data set, per format, and the loader that processes one with [`process_nd`].
//!
//! | value | Bruker (`bruker-nmr`) | Varian (`varian-nmr`) | JEOL (`jeol-jdf`) | Spinsolve (`magritek-spinsolve`) |
//! | --- | --- | --- | --- | --- |
//! | rows, spectral width, carrier | `TD`, `SW_h`, `SFO1` of `acqu2s` (`acqu3s`) | `ni`, `sw1`; carrier from `reffrq1`, `rfl1`, `rfp1` | `y_points`, `y_sweep`, `y_freq` | rows: `nrSteps` |
//! | 0 ppm frequency | `SF` of `proc2s`, else `BF1` of `acqu2s` | `reffrq1` | carrier / (1 + `y_offset`·10⁻⁶) | – |
//! | encoding | `MC2` of `proc2s` (0 QF, 1 QSEQ, 2 TPPI, 3 States, 4 States-TPPI, 5 echo/antiecho), else `FnMODE` − 1, else guessed from the pulse program | `phase` array (1,2: States; 3: TPPI), `f1coef` with 8 values: echo/antiecho | `complex`: States | series |
//! | stored window | `WDW`, `LB`, `GB`, `SSB`, `TM1`, `TM2` | `lb1` | – | – |
//! | stored phases, mode | `PHC0`, `PHC1`, `PH_mod` (0/1 phased, 2 magnitude, 3 power) | −`rp1`, −`lp1` | – | – |
//! | size, first point, points used | `SI`, `FCOR`, `TDeff` | `fn1`/2 | – | – |
//! | linear prediction | `ME_mod`, `NCOEF`, `LPBIN` | – | – | – |
//! | reversal, strip | `REVERSE`, `STSR`/`STSI` (direct) | – | – | – |
//!
//! The direct dimension takes the 1D parameters of [`crate::nmr::source::fid_parameters`] and
//! the same stored values from `procs`. Bruker DSPFVS 11/12 firmware (group delay from the
//! table, with a fractional part): the stored PHC0 refers to a spectrum 180° away from ours, so
//! 180° is added (docs/provenance/nmr-processing.md).
//!
//! **Pseudo-2D series.** A Bruker data set is a series (its F1 is not transformed) when the
//! experiment directory holds a variable list (`difflist`, `vdlist`, `vclist`, `vplist`,
//! `vtlist`, `valist`) and F1 is not a hypercomplex encoding, or when the pulse program is a
//! known relaxation or diffusion sequence (names containing `ledbp`, `stebp`, `dstebp`, `dosy`,
//! `diff`, or starting with `t1ir`, `t1rho`, `cpmg`, `t2`). The list is returned as
//! [`Series`]. Spinsolve `data.2d` files are always series (T1, T2, diffusion, SLIC); the delays
//! come from `minDelay`, `maxDelay`, `delaySpacing` and `nrSteps` when present. Varian arrays
//! other than `phase` are series of the arrayed parameter.
//!
//! **Non-uniform sampling.** Bruker data with `FnTYPE` 2 list the sampled increments in
//! `nuslist`; [`load_processed_nd`] places the measured rows on the full grid
//! ([`place_on_grid`]) with zeros elsewhere and says so in a note. A reconstruction step can
//! fill the grid and call [`process_nd`] on it.

use std::path::Path;

use serde_json::Value;

use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::source::Fs;
use openreadout_core::{Dataset, Error, Result};

use crate::fft::Complex;
use crate::nmr::lp::{LinearPrediction, LpDirection};
use crate::nmr::process::FidParameters;
use crate::nmr::processnd::{
    DimensionParameters, Encoding, NdOptions, Series, SpectrumNd, StoredDimension, StoredPhaseMode,
    process_nd,
};
use crate::nmr::source;
use crate::nmr::window::Window;

/// Everything [`process_nd`] needs about a data set's dimensions.
#[derive(Debug, Clone, Default)]
pub struct NdParameters {
    /// Direct dimension acquisition parameters.
    pub direct: FidParameters,
    /// Direct dimension stored processing.
    pub direct_stored: StoredDimension,
    /// Indirect dimensions, fastest-varying first.
    pub indirect: Vec<DimensionParameters>,
    /// Sampled increments of non-uniformly sampled data (one entry per measured increment, one
    /// index per indirect dimension, fastest first) and the full number of complex points per
    /// indirect dimension.
    pub schedule: Option<(Vec<Vec<usize>>, Vec<usize>)>,
    /// Remarks.
    pub notes: Vec<String>,
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        Value::Array(a) => num(a.first()),
        Value::Object(o) => num(o.get("value").or_else(|| o.get("values"))),
        _ => None,
    }
}

fn text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.trim().trim_matches(['<', '>']).to_string()),
        Value::Array(a) => text(a.first()),
        Value::Object(o) => text(o.get("value").or_else(|| o.get("values"))),
        other => Some(other.to_string()),
    }
}

/// Bruker `MC2` (or `FnMODE` − 1) to an encoding.
pub fn bruker_encoding(code: i64) -> Option<Encoding> {
    Some(match code {
        0 => Encoding::Complex,
        1 => Encoding::Sequential,
        2 => Encoding::Tppi,
        3 => Encoding::States,
        4 => Encoding::StatesTppi,
        5 => Encoding::EchoAntiecho,
        _ => return None,
    })
}

/// A Bruker window from `WDW` and its parameters (`p`: a `procs`-like map).
pub fn bruker_window(
    p: &serde_json::Map<String, Value>,
    notes: &mut Vec<String>,
) -> Option<Window> {
    let g = |k: &str| num(p.get(k));
    let wdw = g("WDW")? as i64;
    let ssb = g("SSB").unwrap_or(0.0);
    let offset = if ssb >= 2.0 { 1.0 / ssb } else { 0.0 };
    Some(match wdw {
        0 => Window::None,
        1 => Window::Exponential {
            line_broadening_hz: g("LB").unwrap_or(0.0),
        },
        2 => Window::LorentzToGauss {
            line_broadening_hz: g("LB").unwrap_or(0.0),
            gaussian_position: g("GB").unwrap_or(0.0),
        },
        3 => Window::Sine { offset, power: 1.0 },
        4 => Window::Sine { offset, power: 2.0 },
        5 => {
            let (a, b) = (g("TM1").unwrap_or(0.0), g("TM2").unwrap_or(1.0));
            notes.push("stored trapezoid window: TM1/TM2 read as fractions of the acquisition time (not verified on vendor spectra)".into());
            Window::Trapezoid { rise: a, fall: b }
        }
        7 | 8 => {
            notes.push("stored sinc window: centre SSB and width GB as this crate defines them (not verified on vendor spectra)".into());
            Window::Sinc {
                center: g("SSB").unwrap_or(0.0).clamp(0.0, 1.0),
                width: g("GB").unwrap_or(1.0),
                power: if wdw == 8 { 2.0 } else { 1.0 },
            }
        }
        9 | 10 => {
            if wdw == 10 {
                notes.push("stored window TRAFS done as TRAF (Traficante)".into());
            }
            Window::Traficante {
                line_broadening_hz: g("LB").unwrap_or(0.0),
            }
        }
        other => {
            notes.push(format!(
                "stored window function code WDW {other} is not known; no window used"
            ));
            Window::None
        }
    })
}

/// Bruker stored processing of one dimension (`procs`, `proc2s`, …). `rows_per_point`: 1 for
/// the direct dimension's `TDeff` in complex points is handled by the caller.
fn bruker_stored(
    p: &serde_json::Map<String, Value>,
    source: String,
    spectrum_stored: bool,
    direct: bool,
    notes: &mut Vec<String>,
    label: &str,
) -> StoredDimension {
    let g = |k: &str| num(p.get(k));
    let mut s = StoredDimension {
        source: Some(source),
        size: g("SI").filter(|v| *v >= 1.0).map(|v| v as usize),
        phase0_deg: g("PHC0"),
        phase1_deg: g("PHC1"),
        first_point_factor: g("FCOR"),
        spectrum_stored,
        ..StoredDimension::default()
    };
    let mut wnotes = Vec::new();
    s.window = bruker_window(p, &mut wnotes);
    notes.extend(wnotes.into_iter().map(|n| format!("{label}: {n}")));
    s.phase_mode = match g("PH_mod").map(|v| v as i64) {
        Some(2) => Some(StoredPhaseMode::Magnitude),
        Some(3) => Some(StoredPhaseMode::Power),
        Some(0 | 1) => Some(StoredPhaseMode::Phased),
        _ => None,
    };
    let tdeff = g("TDeff").filter(|v| *v >= 2.0).map(|v| v as usize);
    s.time_domain_points = if direct { tdeff.map(|v| v / 2) } else { tdeff };
    let me = g("ME_mod").unwrap_or(0.0) as i64;
    let ncoef = g("NCOEF").unwrap_or(0.0).max(0.0) as usize;
    let lpbin = g("LPBIN").unwrap_or(0.0).max(0.0) as usize;
    if me != 0 {
        let direction = match me {
            1 | 2 => Some(LpDirection::Forward),
            3 | 4 => Some(LpDirection::Backward),
            5 | 6 => Some(LpDirection::MirrorForward),
            _ => None,
        };
        match direction {
            Some(d) if ncoef > 0 => {
                let points = if d == LpDirection::Backward {
                    // the number of points TopSpin replaces is not stored; NCOEF is used
                    notes.push(format!("{label}: backward linear prediction replaces {ncoef} points (the stored count is not known)"));
                    ncoef
                } else if direct {
                    lpbin / 2
                } else {
                    lpbin
                };
                if me % 2 == 1 {
                    notes.push(format!(
                        "{label}: real linear prediction (ME_mod {me}) done on the complex data"
                    ));
                }
                s.linear_prediction = Some(LinearPrediction {
                    direction: d,
                    coefficients: ncoef,
                    points,
                });
            }
            _ => notes.push(format!(
                "{label}: stored linear prediction ME_mod {me} not understood; not done"
            )),
        }
    }
    s.reverse = text(p.get("REVERSE")).is_some_and(|t| t.eq_ignore_ascii_case("yes") || t == "1");
    if direct {
        let (a, b) = (g("STSR"), g("STSI"));
        if let (Some(a), Some(b)) = (a, b)
            && b >= 1.0
            && s.size.is_some_and(|n| (b as usize) < n)
        {
            s.strip = Some((a.max(0.0) as usize, b as usize));
        }
    }
    let bc = g("BC_mod").unwrap_or(0.0) as i64;
    if bc >= 5 {
        notes.push(format!("{label}: the stored processing filtered the solvent signal (BC_mod {bc}); that is not done here"));
    }
    s
}

/// Values of a Bruker variable list: one number per line, `s`/`m`/`u` suffixes scale delays
/// to seconds.
pub fn parse_list(text: &str) -> Vec<f64> {
    text.lines()
        .filter_map(|l| {
            let w = l.split_whitespace().next()?;
            if w.starts_with('#') {
                return None;
            }
            let (body, mult) = match w.chars().last() {
                Some('s' | 'S') => (&w[..w.len() - 1], 1.0),
                Some('m') => (&w[..w.len() - 1], 1e-3),
                Some('u') => (&w[..w.len() - 1], 1e-6),
                _ => (w, 1.0),
            };
            body.parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .map(|v| v * mult)
        })
        .collect()
}

const LISTS: [(&str, Option<&str>); 6] = [
    ("difflist", Some("G/cm")),
    ("vdlist", Some("s")),
    ("vclist", None),
    ("vplist", Some("s")),
    ("vtlist", Some("K")),
    ("valist", Some("dB")),
];

fn bruker_series(dir: &Path) -> Option<Series> {
    let fs = Fs::local();
    for (name, unit) in LISTS {
        let p = dir.join(name);
        if let Ok(t) = fs.read_to_string(&p) {
            return Some(Series {
                name: name.into(),
                values: parse_list(&t),
                unit: unit.map(str::to_string),
                source: Some(name.into()),
            });
        }
    }
    None
}

fn pseudo_pulse_program(pp: &str) -> bool {
    let p = pp.to_ascii_lowercase();
    ["ledbp", "stebp", "dstebp", "dosy", "diff"]
        .iter()
        .any(|k| p.contains(k))
        || ["t1ir", "t1rho", "cpmg", "t2"]
            .iter()
            .any(|k| p.starts_with(k))
}

fn trace(info: &FileInfo, index: u32) -> Result<&TraceInfo> {
    info.traces
        .iter()
        .find(|t| t.index == index)
        .ok_or_else(|| Error::Usage(format!("trace {index} out of range")))
}

fn extra_dims(t: &TraceInfo) -> Vec<serde_json::Map<String, Value>> {
    t.extra
        .get("indirect_dimensions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|d| d.as_object().cloned()).collect())
        .unwrap_or_default()
}

fn bruker(t: &TraceInfo, vendor: &Value, info: &FileInfo, out: &mut NdParameters) -> Result<()> {
    let acq = vendor.get("acquisition").and_then(Value::as_object);
    let param = |file: &str| -> Option<serde_json::Map<String, Value>> {
        acq?.get(file)?.get("parameters")?.as_object().cloned()
    };
    let acqus = param("acqus").unwrap_or_default();
    // the lowest procno with proc2s (else the lowest with procs)
    let pdata = vendor.get("pdata").and_then(Value::as_object);
    let mut procnos: Vec<String> = pdata
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    procnos.sort_by_key(|k| k.parse::<u64>().unwrap_or(u64::MAX));
    let proc_file = |no: &str, f: &str| -> Option<serde_json::Map<String, Value>> {
        pdata?
            .get(no)?
            .get(f)?
            .get("parameters")?
            .as_object()
            .cloned()
    };
    let procno = procnos
        .iter()
        .find(|n| proc_file(n, "proc2s").is_some())
        .or_else(|| procnos.first())
        .cloned();
    let stored_name = procno.as_ref().map(|n| format!("pdata/{n}"));
    let spectrum_stored = stored_name.as_ref().is_some_and(|s| {
        info.traces
            .iter()
            .any(|x| x.name.as_deref() == Some(s.as_str()))
    });
    if let Some(no) = &procno
        && let Some(p) = proc_file(no, "procs")
    {
        out.direct_stored = bruker_stored(
            &p,
            format!("pdata/{no}/procs"),
            spectrum_stored,
            true,
            &mut out.notes,
            "direct dimension",
        );
    }
    // DSPFVS 11/12: stored phases 180° away (provenance log 2026-11-14)
    let fv = num(acqus.get("DSPFVS")).unwrap_or(0.0) as i64;
    let grp = num(acqus.get("GRPDLY")).unwrap_or(-1.0);
    let g = out.direct.group_delay_points;
    if matches!(fv, 11 | 12) && grp < 0.0 && g.fract() > 0.0 {
        if let Some(p0) = out.direct_stored.phase0_deg.as_mut() {
            *p0 += 180.0;
            out.notes.push(format!("DSPFVS {fv} firmware: 180° added to the stored direct-dimension phase (the convention measured on such data sets)"));
        }
    }
    let pulprog = text(acqus.get("PULPROG")).unwrap_or_default();
    let dir = vendor
        .get("directory")
        .and_then(Value::as_str)
        .map(Path::new);
    let dims = extra_dims(t);
    let names = ["acqu2s", "acqu3s", "acqu4s"];
    for (k, d) in dims.iter().enumerate() {
        let file = d
            .get("parameter_file")
            .and_then(Value::as_str)
            .unwrap_or(names.get(k).copied().unwrap_or("acqu2s"))
            .to_string();
        let a = param(&file).unwrap_or_default();
        let pname = format!("proc{}s", k + 2);
        let p = procno.as_ref().and_then(|n| proc_file(n, &pname));
        let label = format!("F{}", dims.len() - k);
        let rows = num(d.get("time_domain_size"))
            .or_else(|| num(a.get("TD")))
            .unwrap_or(0.0)
            .max(0.0) as usize;
        let mc2 = p.as_ref().and_then(|p| num(p.get("MC2"))).map(|v| v as i64);
        let fnmode = num(a.get("FnMODE")).map(|v| v as i64).filter(|v| *v >= 1);
        let mut encoding = fnmode
            .and_then(|f| bruker_encoding(f - 1))
            .or_else(|| mc2.and_then(bruker_encoding));
        if let (Some(m), Some(f)) = (
            mc2.and_then(bruker_encoding),
            fnmode.and_then(|f| bruker_encoding(f - 1)),
        ) && m != f
        {
            // the stored spectrum was made with MC2
            out.notes.push(format!(
                "{label}: FnMODE says {f:?}, the stored processing used {m:?}; {m:?} is used"
            ));
            encoding = Some(m);
        }
        let encoding = encoding.unwrap_or_else(|| {
            let pp = pulprog.to_ascii_lowercase();
            let e = if pp.contains("qf") {
                Encoding::Complex
            } else if pp.contains("et") || pp.contains("ea") {
                Encoding::EchoAntiecho
            } else {
                Encoding::StatesTppi
            };
            out.notes.push(format!(
                "{label}: no FnMODE or MC2; encoding {e:?} guessed from the pulse program {pulprog:?}"
            ));
            e
        });
        let mut dim = DimensionParameters {
            nucleus: d.get("nucleus").and_then(Value::as_str).map(str::to_string),
            spectral_width_hz: num(d.get("spectral_width_hz"))
                .or_else(|| num(a.get("SW_h")))
                .unwrap_or(0.0),
            carrier_frequency_mhz: num(d.get("spectrometer_frequency_mhz"))
                .or_else(|| num(a.get("SFO1")))
                .unwrap_or(0.0),
            reference_frequency_mhz: p
                .as_ref()
                .and_then(|p| num(p.get("SF")))
                .filter(|v| *v > 0.0)
                .or_else(|| num(a.get("BF1")))
                .unwrap_or(0.0),
            rows,
            encoding,
            conjugate: false,
            series: None,
            stored: StoredDimension::default(),
        };
        if let Some(p) = &p {
            dim.stored = bruker_stored(
                p,
                format!("pdata/{}/{pname}", procno.as_deref().unwrap_or("1")),
                spectrum_stored,
                false,
                &mut out.notes,
                &label,
            );
        }
        // pseudo-2D (only the dimension next to the direct one, 2D data)
        if k == 0 && dims.len() == 1 {
            let list = dir.and_then(bruker_series);
            let hyper = encoding.rows_per_point() == 2 || encoding == Encoding::Tppi;
            if (list.is_some() && !hyper) || pseudo_pulse_program(&pulprog) {
                dim.encoding = Encoding::Series;
                dim.series = Some(list.unwrap_or_else(|| Series {
                    name: "row".into(),
                    values: (0..rows).map(|i| i as f64).collect(),
                    unit: None,
                    source: None,
                }));
                out.notes.push(format!(
                    "pseudo-2D series ({}): F2 transformed, F1 kept as rows",
                    if pulprog.is_empty() {
                        "variable list"
                    } else {
                        pulprog.as_str()
                    }
                ));
            }
        }
        out.indirect.push(dim);
    }
    // non-uniform sampling
    if let Some(nus) = t.extra.get("non_uniform_sampling") {
        let full = num(nus.get("full_time_domain_size"));
        out.notes.push(format!(
            "non-uniformly sampled ({} increments of {}): missing increments are zero, no reconstruction",
            num(nus.get("nuslist_entries")).unwrap_or(0.0),
            full.unwrap_or(0.0)
        ));
    }
    Ok(())
}

fn varian(t: &TraceInfo, vendor: &Value, out: &mut NdParameters) {
    let pp = vendor.get("procpar");
    let get = |k: &str| num(pp.and_then(|p| p.get(k)).and_then(|v| v.get("values")));
    let gets = |k: &str| {
        pp.and_then(|p| p.get(k))
            .and_then(|v| v.get("values"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<f64>>())
            .unwrap_or_default()
    };
    out.direct_stored = StoredDimension {
        source: Some("procpar".into()),
        size: get("fn").filter(|v| *v >= 2.0).map(|v| (v / 2.0) as usize),
        phase0_deg: get("rp").map(|v| -v),
        phase1_deg: get("lp").map(|v| -v),
        window: get("lb")
            .filter(|v| *v != 0.0)
            .map(|lb| Window::Exponential {
                line_broadening_hz: lb,
            }),
        ..StoredDimension::default()
    };
    let array = pp
        .and_then(|p| p.get("array"))
        .and_then(|v| v.get("values"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let dims = extra_dims(t);
    let suffix = ["1", "2", "3"];
    for (k, d) in dims.iter().enumerate() {
        let s = suffix.get(k).copied().unwrap_or("1");
        let label = format!("F{}", dims.len() - k);
        let phase: Vec<f64> = d
            .get("phase_values")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        let coef = gets(&format!("f{s}coef"));
        let encoding = if coef.len() == 8 {
            Encoding::EchoAntiecho
        } else if phase.len() >= 2 {
            Encoding::States
        } else if phase.first() == Some(&3.0) {
            Encoding::Tppi
        } else {
            Encoding::Complex
        };
        if coef.len() == 8 {
            out.notes.push(format!(
                "{label}: f{s}coef {coef:?} read as echo/antiecho pairs (not verified on vendor spectra)"
            ));
        }
        let ni = num(d.get("increments")).unwrap_or(0.0).max(0.0) as usize;
        let sw = num(d.get("spectral_width_hz")).unwrap_or(0.0);
        let rf = |k: &str| get(&format!("{k}{s}"));
        let reference = rf("reffrq").filter(|v| *v > 0.0);
        let carrier = reference
            .map(|r| r + ((rf("rfp").unwrap_or(0.0) - rf("rfl").unwrap_or(0.0)) + sw / 2.0) * 1e-6);
        if reference.is_none() {
            out.notes.push(format!(
                "{label}: no reffrq{s}; the ppm axis of this dimension is not referenced"
            ));
        }
        let mut stored = StoredDimension {
            source: Some("procpar".into()),
            size: rf("fn").filter(|v| *v >= 2.0).map(|v| (v / 2.0) as usize),
            phase0_deg: rf("rp").map(|v| -v),
            phase1_deg: rf("lp").map(|v| -v),
            ..StoredDimension::default()
        };
        if let Some(lb) = rf("lb").filter(|v| *v != 0.0) {
            stored.window = Some(Window::Exponential {
                line_broadening_hz: lb,
            });
        }
        let rows = ni * encoding.rows_per_point();
        out.indirect.push(DimensionParameters {
            nucleus: d.get("nucleus").and_then(Value::as_str).map(str::to_string),
            spectral_width_hz: sw,
            carrier_frequency_mhz: carrier.unwrap_or(0.0),
            reference_frequency_mhz: reference.unwrap_or(0.0),
            rows,
            encoding,
            // VnmrJ's F1 runs the other way from Bruker's (the NMRPipe conversion of the
            // nmrglue test sets: no reversal for States, `REV` after the TPPI transform)
            conjugate: true,
            series: None,
            stored,
        });
    }
    if dims.is_empty() && !array.is_empty() && array != "phase" {
        let rows = t.sweep_count as usize;
        let name = array.split(',').next().unwrap_or("").trim().to_string();
        let values = gets(&name);
        out.indirect.push(DimensionParameters {
            rows,
            encoding: Encoding::Series,
            series: Some(Series {
                name: name.clone(),
                values,
                unit: None,
                source: Some(format!("procpar {name}")),
            }),
            ..DimensionParameters::default()
        });
        out.notes.push(format!(
            "arrayed experiment ({array}): each row processed as a spectrum"
        ));
    }
    if dims.len() > 1 {
        out.notes.push("Varian data with more than one indirect dimension: rows are taken in the order phase, phase2, …, then increments, as for Bruker; not verified on vendor spectra".into());
    }
}

fn jeol(t: &TraceInfo, vendor: &Value, out: &mut NdParameters) {
    let pr = vendor.get("parameters").and_then(Value::as_object);
    // parameter names are case-insensitive in JEOL files
    let base = |k: &str| -> Option<f64> {
        let p = pr?
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(k))
            .map(|(_, v)| v)?;
        let v = num(p.get("value"))?;
        let e = p.get("power_of_ten").and_then(Value::as_i64).unwrap_or(0);
        Some(v * 10f64.powi(e.clamp(-30, 30) as i32))
    };
    for d in extra_dims(t) {
        let enc = d.get("encoding").and_then(Value::as_str).unwrap_or("");
        let encoding = match enc {
            "complex" => Encoding::States,
            "tppi" => Encoding::Tppi,
            _ => Encoding::Complex,
        };
        let points = num(d.get("points")).unwrap_or(0.0).max(0.0) as usize;
        let carrier = num(d.get("spectrometer_frequency_mhz")).unwrap_or(0.0);
        let off = base("y_offset").unwrap_or(0.0);
        out.indirect.push(DimensionParameters {
            nucleus: d.get("nucleus").and_then(Value::as_str).map(str::to_string),
            spectral_width_hz: num(d.get("spectral_width_hz")).unwrap_or(0.0),
            carrier_frequency_mhz: carrier,
            reference_frequency_mhz: carrier / (1.0 + off * 1e-6),
            rows: points * encoding.rows_per_point(),
            encoding,
            // the COSY diagonal of nmrxiv-s200 runs high-high to low-low without conjugation
            conjugate: false,
            series: None,
            stored: StoredDimension::default(),
        });
    }
    if t.extra.get("sampling").and_then(Value::as_str) == Some("Non Uniform") {
        out.notes.push("non-uniformly sampled JEOL data: the rows are processed as if uniformly spaced (no reconstruction); the spectrum is distorted".into());
    }
}

fn spinsolve(t: &TraceInfo, vendor: &Value, out: &mut NdParameters) {
    let acq = vendor.get("acqu_par");
    let a = |k: &str| num(acq.and_then(|a| a.get(k)));
    let rows = t.sweep_count as usize;
    if rows <= 1 {
        return;
    }
    let mut series = Series {
        name: "row".into(),
        values: (0..rows).map(|i| i as f64).collect(),
        unit: None,
        source: None,
    };
    if let (Some(lo), Some(hi)) = (a("minDelay"), a("maxDelay")) {
        let n = a("nrSteps").map_or(rows, |v| v as usize).max(1);
        let log = acq
            .and_then(|x| x.get("delaySpacing"))
            .and_then(Value::as_str)
            .is_some_and(|s| s.eq_ignore_ascii_case("log"));
        let vals: Vec<f64> = (0..n)
            .map(|i| {
                let f = if n > 1 {
                    i as f64 / (n - 1) as f64
                } else {
                    0.0
                };
                if log && lo > 0.0 && hi > 0.0 {
                    lo * (hi / lo).powf(f)
                } else {
                    lo + (hi - lo) * f
                }
            })
            .collect();
        series = Series {
            name: "delays".into(),
            values: vals,
            unit: Some("ms".into()),
            source: Some("acqu.par minDelay, maxDelay, delaySpacing, nrSteps".into()),
        };
        out.notes.push("delays computed from minDelay/maxDelay/delaySpacing (the spacing rule is inferred, not verified)".into());
    }
    out.indirect.push(DimensionParameters {
        rows,
        encoding: Encoding::Series,
        series: Some(series),
        ..DimensionParameters::default()
    });
}

/// The parameters of every dimension of FID trace `index`, from headers only.
pub fn nd_parameters(ds: &dyn Dataset, info: &FileInfo, index: u32) -> Result<NdParameters> {
    let t = trace(info, index)?.clone();
    let (direct, stored1d, notes) = source::fid_parameters(ds, info, index)?;
    let mut out = NdParameters {
        direct,
        notes: notes
            .into_iter()
            .filter(|n| {
                !n.starts_with("stored window function code")
                    && !n.starts_with("the stored processing used linear prediction")
            })
            .collect(),
        ..NdParameters::default()
    };
    out.direct_stored = StoredDimension {
        source: stored1d.source.clone(),
        size: stored1d.size,
        window: stored1d.line_broadening_hz.map(|lb| Window::Exponential {
            line_broadening_hz: lb,
        }),
        phase0_deg: stored1d.phase0_deg,
        phase1_deg: stored1d.phase1_deg,
        first_point_factor: stored1d.first_point_factor,
        time_domain_points: stored1d.time_domain_points,
        spectrum_stored: stored1d.spectrum_stored,
        ..StoredDimension::default()
    };
    let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
    match info.format.id.as_str() {
        "bruker-nmr" => bruker(&t, &vendor, info, &mut out)?,
        "varian-nmr" => varian(&t, &vendor, &mut out),
        "jeol-jdf" => jeol(&t, &vendor, &mut out),
        "magritek-spinsolve" => spinsolve(&t, &vendor, &mut out),
        _ => {}
    }
    Ok(out)
}

/// Read every row (sweep) of FID trace `index`.
pub fn read_rows(ds: &mut dyn Dataset, info: &FileInfo, index: u32) -> Result<Vec<Vec<Complex>>> {
    let t = trace(info, index)?;
    (0..t.sweep_count)
        .map(|s| source::read_fid(ds, info, index, s))
        .collect()
}

/// Place the rows of non-uniformly sampled data on the full grid.
///
/// `schedule[j]` holds the complex-point index of measured increment `j` along each indirect
/// dimension (fastest first, as Bruker `nuslist` lists them); `rows` holds, for each increment
/// in schedule order, `Π rows_per_point` rows (both parts of each hypercomplex pair, fastest
/// dimension's pair fastest). `full[k]` is the full number of complex points of indirect
/// dimension `k`. Grid points that were not measured are zero. The result can be passed to
/// [`process_nd`] with `indirect[k].rows = full[k] · rows_per_point`.
pub fn place_on_grid(
    rows: &[Vec<Complex>],
    schedule: &[Vec<usize>],
    indirect: &[DimensionParameters],
    full: &[usize],
) -> Result<Vec<Vec<Complex>>> {
    if full.len() != indirect.len() {
        return Err(Error::Usage(
            "place_on_grid: one full size per indirect dimension is needed".into(),
        ));
    }
    let rpp: Vec<usize> = indirect
        .iter()
        .map(|d| d.encoding.rows_per_point())
        .collect();
    let block: usize = rpp.iter().product();
    let dims_rows: Vec<usize> = full.iter().zip(&rpp).map(|(f, r)| f * r).collect();
    let total: usize = dims_rows
        .iter()
        .try_fold(1usize, |a, d| a.checked_mul(*d))
        .ok_or_else(|| Error::Usage("place_on_grid: grid too large".into()))?;
    if total > crate::nmr::processnd::MAX_ND_POINTS {
        return Err(Error::Usage("place_on_grid: grid too large".into()));
    }
    let len = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut grid = vec![vec![Complex::default(); len]; total];
    for (j, inc) in schedule.iter().enumerate() {
        if inc.len() != indirect.len() || inc.iter().zip(full).any(|(i, f)| i >= f) {
            continue;
        }
        for b in 0..block {
            // part index along each dimension
            let mut rem = b;
            let mut target = 0usize;
            let mut stride = 1usize;
            for k in 0..indirect.len() {
                let part = rem % rpp[k];
                rem /= rpp[k];
                target += (inc[k] * rpp[k] + part) * stride;
                stride *= dims_rows[k];
            }
            if let Some(src) = rows.get(j * block + b) {
                grid[target][..src.len()].copy_from_slice(src);
            }
        }
    }
    Ok(grid)
}

/// The `nuslist` of a Bruker data set, if the reader lists one as a table.
fn read_schedule(ds: &mut dyn Dataset, info: &FileInfo) -> Option<Vec<Vec<usize>>> {
    let t = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("nuslist"))?;
    let tab = ds.read_table(t.index, 0, t.row_count).ok()?;
    let n = tab.columns.first().map_or(0, Vec::len);
    let rows: Vec<Vec<usize>> = (0..n)
        .map(|r| {
            tab.columns
                .iter()
                .filter_map(|c| c.get(r))
                .map(|v| {
                    if v.is_finite() {
                        v.max(0.0) as usize
                    } else {
                        0
                    }
                })
                .collect()
        })
        .collect();
    (!rows.is_empty()).then_some(rows)
}

/// Process FID trace `trace` of a multidimensional data set with `opts` (module docs): read
/// the parameters ([`nd_parameters`]) and all rows, place non-uniformly sampled rows on the
/// full grid, and run [`process_nd`]. The notes of every step end up in `record.notes`.
pub fn load_processed_nd(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    trace_index: u32,
    opts: &NdOptions,
) -> Result<SpectrumNd> {
    let mut p = nd_parameters(ds, info, trace_index)?;
    if p.indirect.is_empty() {
        return Err(Error::unsupported(
            "nmr-processing",
            "multidimensional processing of a 1D FID",
            "This FID has no indirect dimension; process it as a 1D spectrum (`analyze nmr-peaks`, `--process`).",
        ));
    }
    let mut rows = read_rows(ds, info, trace_index)?;
    let t = trace(info, trace_index)?;
    if let Some(nus) = t.extra.get("non_uniform_sampling")
        && let Some(schedule) = read_schedule(ds, info)
    {
        let full_rows = num(nus.get("full_time_domain_size")).unwrap_or(0.0) as usize;
        if p.indirect.len() == 1 && full_rows > 0 {
            let rpp = p.indirect[0].encoding.rows_per_point();
            let full = vec![full_rows / rpp];
            rows = place_on_grid(&rows, &schedule, &p.indirect, &full)?;
            p.indirect[0].rows = full_rows;
            p.schedule = Some((schedule, full));
        } else {
            p.notes.push("non-uniform sampling of more than one indirect dimension: rows processed as measured".into());
        }
    }
    let mut s = process_nd(&rows, &p.direct, &p.direct_stored, &p.indirect, opts)?;
    let mut notes = p.notes;
    notes.append(&mut s.record.notes);
    s.record.notes = notes;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_codes() {
        assert_eq!(parse_list("2.254\n2.899\n"), vec![2.254, 2.899]);
        let v = parse_list("10m\n1s\n50u\n0.5\n# x\n");
        assert_eq!(v.len(), 4);
        assert!((v[0] - 0.01).abs() < 1e-12 && (v[2] - 5e-5).abs() < 1e-15);
        assert_eq!(bruker_encoding(5), Some(Encoding::EchoAntiecho));
        assert_eq!(bruker_encoding(9), None);
        assert!(pseudo_pulse_program("ledbpgp2s"));
        assert!(pseudo_pulse_program("t1irpg"));
        assert!(!pseudo_pulse_program("hsqcedetgpsisp2.3"));
        let mut n = Vec::new();
        let m: serde_json::Map<String, Value> =
            serde_json::from_value(serde_json::json!({"WDW": 4, "SSB": 2})).unwrap();
        assert_eq!(
            bruker_window(&m, &mut n),
            Some(Window::Sine {
                offset: 0.5,
                power: 2.0
            })
        );
    }

    #[test]
    fn grid_placement() {
        let d = DimensionParameters {
            rows: 8,
            encoding: Encoding::States,
            ..DimensionParameters::default()
        };
        let rows: Vec<Vec<Complex>> = (0..4)
            .map(|i| vec![Complex::new(i as f64 + 1.0, 0.0); 2])
            .collect();
        // increments 0 and 2 measured, of 4
        let g = place_on_grid(&rows, &[vec![0], vec![2]], &[d], &[4]).unwrap();
        assert_eq!(g.len(), 8);
        assert_eq!(g[0][0].re, 1.0);
        assert_eq!(g[1][0].re, 2.0);
        assert_eq!(g[2][0].re, 0.0);
        assert_eq!(g[4][0].re, 3.0);
        assert_eq!(g[5][0].re, 4.0);
    }
}
