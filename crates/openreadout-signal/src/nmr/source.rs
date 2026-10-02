//! Getting an FID with its processing parameters, or a processed spectrum with its ppm axis,
//! out of any NMR reader.
//!
//! Traces are recognised by `extra.kind`: `time_domain` (an FID or `ser`) and anything with a
//! chemical-shift axis (`extra.axis.unit` `ppm`, or `Hz`/`HZ` with an observe frequency, as
//! JCAMP-DX NMR spectra carry). The parameters come from the trace's `extra` and the vendor
//! parameter tree (`Dataset::vendor_metadata`), per format:
//!
//! | parameter | Bruker (`bruker-nmr`) | Varian (`varian-nmr`) | JEOL (`jeol-jdf`) | Spinsolve (`magritek-spinsolve`) |
//! | --- | --- | --- | --- | --- |
//! | spectral width | `SW_h` | `sw` | `X_SWEEP` | `bandwidth` × 1000 |
//! | carrier | `SFO1` | `sfrq` | `X_FREQ` | `b1Freq` + (`lowestFrequency` + width/2)·10⁻⁶ |
//! | 0 ppm frequency | `SF` of the first `pdata/<n>/procs`, else `BF1` | `reffrq`, else from `rfl`/`rfp` | carrier / (1 + `X_OFFSET`·10⁻⁶) | `b1Freq` |
//! | group delay | `GRPDLY` or the DSP table (`group_delay_points`) | none | digital-filter `orders`/`factors` (see `jeol_group_delay`) | none |
//! | stored phases | `PHC0`, `PHC1` | −`rp`, −`lp` | – | `p0Phase`, `p1Phase` (`proc.par`), else −`Phase(p0, p1)` (`processing.script`) |
//! | stored LB, size | `LB`, `SI` | `lb`, `fn`/2 | – | 0 when `filter` is `no`; `nrPnts` × `zf` |
//! | conjugate | no | yes | yes | yes |
//!
//! The conventions (orientation, phase signs) were fixed by reproducing the vendors' own
//! processed spectra and reference shifts; see `docs/provenance/nmr-processing.md`.

use serde_json::Value;

use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::{Dataset, Error, Result};

use crate::fft::Complex;
use crate::nmr::process::{FidParameters, PpmAxis, StoredProcessing};

/// Where a spectrum should come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpectrumSource {
    /// The vendor's processed spectrum when the data set has one, else the FID processed here.
    #[default]
    Auto,
    /// Always process the FID.
    Fid,
    /// Always use a stored processed spectrum.
    Processed,
}

/// Is this trace a time-domain FID?
pub fn is_fid(t: &TraceInfo) -> bool {
    t.extra.get("kind").and_then(Value::as_str) == Some("time_domain")
}

/// The ppm axis of a frequency-domain trace, if it has one.
pub fn spectrum_axis(t: &TraceInfo) -> Option<PpmAxis> {
    let axis = t.extra.get("axis")?;
    let unit = axis.get("unit")?.as_str()?;
    let first = axis.get("first")?.as_f64()?;
    let step = axis.get("step")?.as_f64()?;
    let size = axis
        .get("size")
        .and_then(Value::as_u64)
        .unwrap_or(t.sample_count) as usize;
    let mhz = axis
        .get("spectrometer_frequency_mhz")
        .and_then(Value::as_f64)
        .or_else(|| {
            t.extra
                .get("spectrometer_frequency_mhz")
                .and_then(Value::as_f64)
        })
        .or_else(|| t.extra.get("observe_frequency_mhz").and_then(Value::as_f64));
    match unit.to_ascii_lowercase().as_str() {
        "ppm" => Some(PpmAxis {
            first_ppm: first,
            step_ppm: step,
            size,
            reference_frequency_mhz: mhz.unwrap_or(0.0),
        }),
        "hz" => {
            let f = mhz.filter(|f| *f > 0.0)?;
            Some(PpmAxis {
                first_ppm: first / f,
                step_ppm: step / f,
                size,
                reference_frequency_mhz: f,
            })
        }
        _ => None,
    }
}

/// Index of the trace to use: `requested` if given (checked), else the first stored spectrum
/// (for `Auto`/`Processed`) or the first FID.
pub fn choose_trace(
    info: &FileInfo,
    requested: Option<u32>,
    source: SpectrumSource,
) -> Result<u32> {
    if let Some(i) = requested {
        let t = info.traces.iter().find(|t| t.index == i).ok_or_else(|| {
            Error::Usage(format!(
                "trace {i} out of range (file has {} traces)",
                info.traces.len()
            ))
        })?;
        if !is_fid(t) && spectrum_axis(t).is_none() {
            return Err(Error::unsupported(
                "nmr-processing",
                format!("trace {i}: neither an FID nor a spectrum with a ppm axis"),
                "Run `openreadout info <file>` and pick a trace whose extra.kind is time_domain or that has a ppm axis.",
            ));
        }
        return Ok(i);
    }
    let fid = info.traces.iter().find(|t| is_fid(t)).map(|t| t.index);
    let spec = info
        .traces
        .iter()
        .find(|t| !is_fid(t) && spectrum_axis(t).is_some())
        .map(|t| t.index);
    let pick = match source {
        SpectrumSource::Auto => spec.or(fid),
        SpectrumSource::Fid => fid,
        SpectrumSource::Processed => spec,
    };
    pick.ok_or_else(|| {
        let what = match source {
            SpectrumSource::Fid => "an FID (time-domain trace)",
            SpectrumSource::Processed => "a processed spectrum",
            SpectrumSource::Auto => "an FID or a processed spectrum",
        };
        Error::unsupported(
            "nmr-processing",
            format!("this {} file has no {what}", info.format.name),
            "NMR processing and peak picking need an NMR FID or spectrum (Bruker, Varian, JEOL, JCAMP-DX); `openreadout info <file>` lists the traces. Use `--from auto` to take whichever exists.",
        )
    })
}

/// Read every sample of sweep `sweep` of trace `index`, all channels.
fn read_all(ds: &mut dyn Dataset, t: &TraceInfo, sweep: u32) -> Result<Vec<Vec<f64>>> {
    let tr = ds.read_trace(t.index, sweep, 0, t.sample_count)?;
    Ok(tr.channels)
}

/// Read a stored spectrum's real part (first channel) and axis.
pub fn load_spectrum(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    index: u32,
) -> Result<(Vec<f64>, PpmAxis)> {
    let t = trace(info, index)?;
    let axis = spectrum_axis(t).ok_or_else(|| {
        Error::unsupported(
            "nmr-processing",
            format!("trace {index} has no ppm axis"),
            "Pick a processed spectrum trace (see `openreadout info`).",
        )
    })?;
    let mut ch = read_all(ds, t, 0)?;
    if ch.is_empty() {
        return Err(Error::corrupt(
            "nmr-processing",
            format!("trace {index} has no channels"),
        ));
    }
    let mut y = ch.swap_remove(0);
    y.truncate(axis.size);
    let axis = PpmAxis {
        size: y.len(),
        ..axis
    };
    Ok((y, axis))
}

fn trace(info: &FileInfo, index: u32) -> Result<&TraceInfo> {
    info.traces
        .iter()
        .find(|t| t.index == index)
        .ok_or_else(|| Error::Usage(format!("trace {index} out of range")))
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

fn extra_f64(t: &TraceInfo, key: &str) -> Option<f64> {
    t.extra.get(key).and_then(Value::as_f64)
}

/// Bruker: parameters of the lowest-numbered `pdata/<n>/procs` in the vendor tree.
fn bruker_procs(vendor: &Value) -> Option<(String, &serde_json::Map<String, Value>)> {
    let pdata = vendor.get("pdata")?.as_object()?;
    let mut keys: Vec<&String> = pdata.keys().collect();
    keys.sort_by_key(|k| k.parse::<u64>().unwrap_or(u64::MAX));
    keys.into_iter().find_map(|k| {
        let p = pdata.get(k)?.get("procs")?.get("parameters")?.as_object()?;
        Some((k.clone(), p))
    })
}

/// JEOL Delta digital-filter group delay in output points, from the `orders` and `factors`
/// parameter strings (inferred from corpus files, see the provenance log): `orders` holds the
/// number of filter stages `s` followed by the `s` filter orders, `factors` the `s` decimation
/// factors, whitespace-separated (`"2 54 73"`, `"8  2"`). Stage `i` delays by
/// `(order_i − 1)/2` of its input samples, which is that many output samples divided by the
/// product of its own and all later decimation factors.
pub fn jeol_group_delay(orders: &str, factors: &str) -> Option<f64> {
    let parse = |s: &str| -> Option<Vec<f64>> {
        s.split_whitespace()
            .map(|w| w.parse::<f64>().ok().filter(|v| v.is_finite()))
            .collect()
    };
    let o = parse(orders)?;
    let fac = parse(factors)?;
    let (&stages, ord) = o.split_first()?;
    if stages < 1.0 || stages.fract() != 0.0 {
        return None;
    }
    let stages = stages as usize;
    if ord.len() != stages || fac.len() != stages || fac.iter().any(|&x| x < 1.0) {
        return None;
    }
    let mut delay = 0.0;
    for i in 0..stages {
        let down: f64 = fac[i..].iter().product();
        delay += (ord[i] - 1.0) / 2.0 / down;
    }
    Some(delay)
}

/// Processing parameters of the FID trace `index`, from headers only (trace `extra` and the
/// vendor parameter tree), plus notes about conventions that may not hold.
pub fn fid_parameters(
    ds: &dyn Dataset,
    info: &FileInfo,
    index: u32,
) -> Result<(FidParameters, StoredProcessing, Vec<String>)> {
    let t = trace(info, index)?.clone();
    if !is_fid(&t) {
        return Err(Error::unsupported(
            "nmr-processing",
            format!("trace {index} is not an FID"),
            "Pick the time-domain trace (extra.kind = time_domain) or use `--from processed`.",
        ));
    }
    if t.channels.len() < 2 {
        return Err(Error::unsupported(
            "nmr-processing",
            "real-only (sequential / single-channel) FIDs",
            "Only complex (quadrature) FIDs are processed; use the vendor's processed spectrum (`--from processed`) if there is one.",
        ));
    }
    let vendor = ds.vendor_metadata().unwrap_or(Value::Null);
    let mut notes = Vec::new();
    let nucleus = t
        .extra
        .get("nucleus")
        .and_then(Value::as_str)
        .map(str::to_string);
    let sw = extra_f64(&t, "spectral_width_hz");
    let carrier = extra_f64(&t, "spectrometer_frequency_mhz");
    let mut stored = StoredProcessing::default();
    let (reference, group_delay, conjugate) = match info.format.id.as_str() {
        "bruker-nmr" => {
            let acq = vendor.pointer("/acquisition/acqus/parameters");
            let bf1 =
                extra_f64(&t, "base_frequency_mhz").or_else(|| num(acq.and_then(|a| a.get("BF1"))));
            let procs = bruker_procs(&vendor);
            let sf = procs
                .as_ref()
                .and_then(|(_, p)| num(p.get("SF")))
                .filter(|v| *v > 0.0);
            if let Some((procno, p)) = &procs {
                stored.source = Some(format!("pdata/{procno}/procs"));
                stored.size = num(p.get("SI")).filter(|v| *v >= 1.0).map(|v| v as usize);
                let wdw = num(p.get("WDW")).unwrap_or(1.0) as i64;
                stored.line_broadening_hz = match wdw {
                    0 => Some(0.0),
                    _ => num(p.get("LB")),
                };
                if wdw > 1 {
                    notes.push(format!("stored window function code WDW {wdw} is not exponential; its LB is used as an exponential line broadening"));
                }
                stored.phase0_deg = num(p.get("PHC0"));
                stored.phase1_deg = num(p.get("PHC1"));
                stored.first_point_factor = num(p.get("FCOR"));
                stored.time_domain_points = num(p.get("TDeff"))
                    .filter(|v| *v >= 2.0)
                    .map(|v| (v / 2.0) as usize);
                let name = format!("pdata/{procno}");
                stored.spectrum_stored = info
                    .traces
                    .iter()
                    .any(|x| x.name.as_deref() == Some(name.as_str()));
            }
            let me_mod = procs
                .as_ref()
                .and_then(|(_, p)| num(p.get("ME_mod")))
                .unwrap_or(0.0);
            if me_mod != 0.0 {
                notes.push(format!("the stored processing used linear prediction (ME_mod {me_mod}); it is not done here, so the spectrum differs from the vendor's at the start of the FID (broad baseline, first-point artefacts)"));
            }
            (
                sf.or(bf1),
                extra_f64(&t, "group_delay_points").unwrap_or(0.0),
                false,
            )
        }
        "varian-nmr" => {
            let pp = vendor.get("procpar");
            let get = |k: &str| num(pp.and_then(|p| p.get(k)).and_then(|v| v.get("values")));
            stored.source = Some("procpar".into());
            // VnmrJ phases rotate the other way: our φ = −rp, −lp (rp verified on corpus
            // spectra; lp not reproduced, see docs/formats/nmr-processing.md)
            stored.phase0_deg = get("rp").map(|v| -v);
            stored.phase1_deg = get("lp").map(|v| -v);
            stored.line_broadening_hz = get("lb");
            stored.size = get("fn").filter(|v| *v >= 2.0).map(|v| (v / 2.0) as usize);
            let reffrq = get("reffrq").filter(|v| *v > 0.0);
            let reference = reffrq.or_else(|| {
                let (c, s) = (carrier?, sw?);
                let (rfl, rfp) = (get("rfl")?, get("rfp")?);
                Some(c - ((rfp - rfl) + s / 2.0) * 1e-6)
            });
            (reference, 0.0, true)
        }
        "jeol-jdf" => {
            let pr = vendor.get("parameters");
            let text = |k: &str| {
                pr.and_then(|p| p.get(k))
                    .and_then(|v| v.get("value"))
                    .map(|v| match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
            };
            let off = extra_f64(&t, "carrier_offset_ppm").unwrap_or(0.0);
            let gd = match (text("orders"), text("factors")) {
                (Some(o), Some(f)) => jeol_group_delay(&o, &f).unwrap_or_else(|| {
                    notes.push(format!("digital filter orders {o:?} / factors {f:?} not understood; no group delay removed"));
                    0.0
                }),
                _ => 0.0,
            };
            (carrier.map(|c| c / (1.0 + off * 1e-6)), gd, true)
        }
        "magritek-spinsolve" => {
            // conventions fixed on the Spinsolve software's own processed spectra
            // (docs/provenance/magritek-spinsolve.md): conjugated FID, spectrum × e^{+i·p0} with
            // p0 from `Phase(p0, p1)` in processing.script (= −p0Phase of proc.par)
            let acq = vendor.get("acqu_par");
            let a_num = |k: &str| num(acq.and_then(|a| a.get(k)));
            let a_text = |k: &str| acq.and_then(|a| a.get(k)).and_then(Value::as_str);
            let pair = |k: &str| {
                let v = vendor.get(k)?.as_array()?;
                Some((v.first()?.as_f64()?, v.get(1)?.as_f64()?))
            };
            let phases = pair("proc_phase_deg")
                .map(|p| ("proc.par", p))
                .or_else(|| pair("script_phase_deg").map(|(a, b)| ("processing.script", (-a, -b))));
            if let Some((src, (p0, p1))) = phases {
                stored.source = Some(src.into());
                stored.phase0_deg = Some(p0);
                stored.phase1_deg = Some(p1);
                if p1 != 0.0 {
                    notes.push(format!("stored first-order phase {p1} deg: its sign and pivot were not verified on vendor spectra (only zero-order phases were)"));
                }
            }
            let n = a_num("nrPnts").filter(|v| *v >= 1.0);
            let zf = a_num("zf").filter(|v| *v >= 1.0).unwrap_or(1.0);
            stored.size = n.map(|n| (n * zf) as usize);
            match a_text("filter") {
                Some(f) if f.eq_ignore_ascii_case("no") => stored.line_broadening_hz = Some(0.0),
                Some(f) if f.eq_ignore_ascii_case("yes") => {
                    let lb = a_text("filterType")
                        .and_then(|t| t.strip_prefix("exp:"))
                        .and_then(|v| v.trim().parse::<f64>().ok());
                    if let Some(lb) = lb {
                        stored.line_broadening_hz = Some(lb);
                        notes.push(format!("stored filter exp:{lb} used as an exponential line broadening of {lb} Hz (the unit was not verified on vendor spectra)"));
                    }
                }
                _ => {}
            }
            (extra_f64(&t, "reference_frequency_mhz"), 0.0, true)
        }
        other => {
            return Err(Error::unsupported(
                "nmr-processing",
                format!("FID processing for {other} files"),
                "FID processing knows Bruker, Varian/Agilent, JEOL and Magritek Spinsolve parameters; for other files use a processed spectrum.",
            ));
        }
    };
    let params = FidParameters {
        spectral_width_hz: sw.unwrap_or(0.0),
        carrier_frequency_mhz: carrier.unwrap_or(0.0),
        reference_frequency_mhz: reference.unwrap_or(0.0),
        group_delay_points: group_delay,
        conjugate,
        nucleus,
    };
    Ok((params, stored, notes))
}

/// Read sweep `sweep` of the FID trace `index` as complex points.
pub fn read_fid(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    index: u32,
    sweep: u32,
) -> Result<Vec<Complex>> {
    let t = trace(info, index)?;
    if sweep >= t.sweep_count {
        return Err(Error::Usage(format!(
            "sweep {sweep} out of range (trace {index} has {} sweeps)",
            t.sweep_count
        )));
    }
    let ch = read_all(ds, t, sweep)?;
    if ch.len() < 2 {
        return Err(Error::corrupt(
            "nmr-processing",
            format!("trace {index} returned fewer than two channels"),
        ));
    }
    Ok(ch[0]
        .iter()
        .zip(&ch[1])
        .map(|(&re, &im)| Complex::new(re, im))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jeol_delay_from_orders_and_factors() {
        let d = jeol_group_delay("2 54 73", "8  2").unwrap();
        assert!((d - (26.5 / 16.0 + 36.0 / 2.0)).abs() < 1e-12);
        assert!(jeol_group_delay("", "").is_none());
        assert!(jeol_group_delay("2 54", "8 2").is_none());
        assert!(jeol_group_delay("2 54 73", "8").is_none());
        assert!(jeol_group_delay("2 54 73", "8 0").is_none());
        assert!(jeol_group_delay("x", "8").is_none());
        assert!(jeol_group_delay("1.5 3", "2").is_none());
    }

    #[test]
    fn numbers_from_vendor_values() {
        assert_eq!(num(Some(&serde_json::json!(["1.5"]))), Some(1.5));
        assert_eq!(num(Some(&serde_json::json!({"value": 2}))), Some(2.0));
        assert_eq!(num(Some(&serde_json::json!("x"))), None);
        assert_eq!(num(None), None);
    }
}
