//! Mass spectrometry: spectra against the depositor's conversion, chromatograms rebuilt from
//! them, SRM traces, timsTOF frames.

use std::fmt::Write as _;
use std::path::Path;

use openreadout_core::Registry;

use crate::oracle::{OracleChromatogram, OracleSpectra, OracleTdf};
use crate::{covered, rel_close, xxh3_f32, xxh3_f64};

pub(crate) fn xxh3_bytes(b: &[u8]) -> String {
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(b))
}

/// Compare decoded timsTOF frames and the m/z / 1/K0 conversions with timsrust's.
pub(crate) fn check_tdf(path: &Path, o: &OracleTdf) -> Result<String, String> {
    let mut ds = openreadout_bruker_tims::TimsDataset::open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let ids: Vec<i64> = ds.frame_records().iter().map(|f| f.id).collect();
    let mut problems = Vec::new();
    let mut ok = 0usize;
    for f in &o.frames {
        let Some(i) = ids.iter().position(|&x| x == f.id) else {
            problems.push(format!("frame {} missing", f.id));
            continue;
        };
        let fr = match ds.read_frame(i) {
            Ok(fr) => fr,
            Err(e) => {
                problems.push(format!("frame {}: {e}", f.id));
                continue;
            }
        };
        let mut bad = Vec::new();
        if fr.scan_count() != f.scans {
            bad.push(format!("scans {} != {}", fr.scan_count(), f.scans));
        }
        if fr.tof_indices.len() != f.peaks {
            bad.push(format!("peaks {} != {}", fr.tof_indices.len(), f.peaks));
        }
        let ob: Vec<u8> = fr
            .scan_offsets
            .iter()
            .flat_map(|&v| (v as u64).to_le_bytes())
            .collect();
        let tb: Vec<u8> = fr
            .tof_indices
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let ib: Vec<u8> = fr
            .intensities
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        for (what, want, got) in [
            ("scan offsets", &f.xxh3_scan_offsets, xxh3_bytes(&ob)),
            ("TOF indices", &f.xxh3_tof, xxh3_bytes(&tb)),
            ("intensities", &f.xxh3_intensity, xxh3_bytes(&ib)),
        ] {
            if want.as_ref().is_some_and(|w| *w != got) {
                bad.push(format!("{what} hash differs"));
            }
        }
        if f.first_tof
            .is_some_and(|t| fr.tof_indices.first() != Some(&t))
        {
            bad.push(format!("first TOF {:?}", fr.tof_indices.first()));
        }
        if f.first_intensity
            .is_some_and(|t| fr.intensities.first() != Some(&t))
        {
            bad.push(format!("first intensity {:?}", fr.intensities.first()));
        }
        if bad.is_empty() {
            ok += 1;
        } else if problems.len() < 8 {
            problems.push(format!("frame {}: {}", f.id, bad.join(", ")));
        }
    }
    let conv = ds.conversions();
    let mut conv_ok = 0usize;
    for &(t, want) in &o.mz_samples {
        match conv.map(|c| c.mz(f64::from(t))) {
            Some(got) if got.to_bits() == want.to_bits() => conv_ok += 1,
            got => problems.push(format!("m/z of TOF {t}: {got:?} != {want}")),
        }
    }
    for &(s, want) in &o.mobility_samples {
        match conv.and_then(|c| c.mobility(f64::from(s))) {
            Some(got) if got.to_bits() == want.to_bits() => conv_ok += 1,
            got => problems.push(format!("1/K0 of scan {s}: {got:?} != {want}")),
        }
    }
    let summary = format!(
        "{ok}/{} frames bit-exact (offsets, TOF, intensity); {conv_ok} conversion samples bit-exact",
        o.frames.len()
    );
    if problems.is_empty() {
        Ok(summary)
    } else {
        Err(format!("{summary}; {}", problems.join("; ")))
    }
}

/// Compare every scan against the depositor's conversion. Metadata must match exactly (rt
/// within 1e-3 s, precursor m/z within 1e-6 relative); peak lists are "exact" when both array
/// hashes match, "close" when the point count matches and the summaries (first five points,
/// m/z range, intensity sum) agree within 1e-6 (m/z) / 1e-4 (intensity) relative.
pub(crate) fn check_spectra(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    o: &OracleSpectra,
) -> Result<String, String> {
    use openreadout_core::SpectrumView;
    let mut problems: Vec<String> = Vec::new();
    let ours = info.spectra.first().map_or(0, |s| s.scan_count);
    // Scan numbers map to our zero-based indices through the first scan's number.
    let first_scan = if o.by_index {
        0
    } else {
        ds.read_spectrum_view(0, 0, SpectrumView::Primary)
            .map_err(|e| format!("first spectrum: {e}"))?
            .scan_number
    };
    // An export may hold fewer scans than the file: by scan number, or (by position) a leading
    // part whose native ids then have to line up with ours (a converter that drops an
    // unfinished last cycle).
    let subset = o.scans.len() as u64 != ours
        && if o.by_index {
            // native ids when the export has them (mzXML has none: then the per-scan times,
            // levels and precursors compared below show whether the positions line up)
            let ids = o.scans.iter().any(|s| s.native_id.is_some());
            (o.scans.len() as u64) < ours
                && o.scans
                    .iter()
                    .all(|s| s.index < ours && (s.native_id.is_some() || !ids))
        } else {
            o.scans
                .iter()
                .all(|s| s.scan_number >= first_scan && s.scan_number - first_scan < ours)
        };
    if ours != o.scan_count && !subset {
        problems.push(format!("scan count {ours} != oracle {}", o.scan_count));
    }
    let (mut exact, mut close, mut differ) = (0usize, 0usize, 0usize);
    let mut meta_bad = 0usize;
    let mut first_diffs: Vec<String> = Vec::new();
    let mut meta_diffs: Vec<String> = Vec::new();
    let mut close_scans: Vec<u64> = Vec::new();
    let note = |msg: String, v: &mut Vec<String>| {
        if v.len() < 8 {
            v.push(msg);
        }
    };
    for s in &o.scans {
        let view = if s.centroided == Some(true) {
            SpectrumView::Centroid
        } else {
            SpectrumView::Primary
        };
        let at = if o.by_index {
            s.index
        } else {
            s.scan_number - first_scan
        };
        let sp = match ds.read_spectrum_view(0, at, view) {
            Ok(sp) => sp,
            Err(e) => {
                problems.push(format!("scan {}: read failed: {e}", s.scan_number));
                break;
            }
        };
        let mut m = Vec::new();
        // An export whose native ids carry no `scan=N` (Agilent `scanId=N`) numbers its scans
        // by position; compare the native ids instead.
        if let (Some(theirs), Some(ours)) = (&s.native_id, &sp.native_id)
            && !theirs.contains("scan=")
        {
            if theirs != ours && o.native_id_not_compared.is_none() {
                m.push(format!("native id {ours} != {theirs}"));
            }
        } else if sp.scan_number != s.scan_number {
            m.push(format!(
                "scan number {} != {}",
                sp.scan_number, s.scan_number
            ));
        }
        if sp.ms_level != s.ms_level {
            m.push(format!("ms level {} != {}", sp.ms_level, s.ms_level));
        }
        if let Some(p) = &s.polarity
            && &sp.polarity != p
        {
            m.push(format!("polarity {} != {p}", sp.polarity));
        }
        // (an export of vendor-picked peaks is centroided whatever the file stores)
        if let Some(c) = s.centroided
            && sp.centroided != c
            && o.peaks_not_compared.is_none()
        {
            m.push(format!("centroided {} != {c}", sp.centroided));
        }
        if let Some(rt) = s.rt_s
            && sp
                .rt_s
                .is_none_or(|o| (o - rt).abs() > s.rt_tolerance_s.unwrap_or(0.0).max(1e-3) + 1e-9)
        {
            m.push(format!("rt {:?} != {rt}", sp.rt_s));
        }
        if let Some(f) = &s.filter
            && sp.scan_filter.as_deref() != Some(f.as_str())
        {
            m.push(format!("filter {:?} != {f:?}", sp.scan_filter));
        }
        if let Some(p) = s.precursor_mz
            && o.precursor_not_compared.is_none()
            && !sp
                .precursor_mz
                .into_iter()
                // an MS^n or multiplexed scan lists every precursor in `extra.precursors`;
                // exports differ in which one they name first
                .chain(
                    sp.extra
                        .get("precursors")
                        .and_then(|v| v.as_array())
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_f64),
                )
                .any(|q| rel_close(q, p, o.precursor_tolerance.unwrap_or(1e-6)))
        {
            m.push(format!("precursor {:?} != {p}", sp.precursor_mz));
        }
        if let Some(pos) = &s.position {
            let ours: Option<Vec<f64>> = sp
                .extra
                .get("position")
                .and_then(|v| serde_json::from_value(v.clone()).ok());
            // pyimzML reports z = 1 when the file records no z: compare what both have
            let same = ours.as_ref().is_some_and(|o| {
                o.len() >= 2 && o.iter().zip(pos).all(|(a, b)| (a - b).abs() < 1e-9)
            });
            if !same {
                m.push(format!("position {ours:?} != {pos:?}"));
            }
        }
        if let Some(im) = s.inverse_reduced_mobility
            && o.mz_not_compared.is_none()
            && !sp
                .inverse_reduced_mobility
                .is_some_and(|q| rel_close(q, im, 1e-12))
        {
            m.push(format!("1/K0 {:?} != {im}", sp.inverse_reduced_mobility));
        }
        if let Some(c) = s.precursor_charge
            && c != 0
            && sp.precursor_charge != Some(c)
        {
            m.push(format!("charge {:?} != {c}", sp.precursor_charge));
        }
        if let Some(act) = &s.activation
            && !act.is_null()
            && o.precursor_not_compared.is_none()
        {
            let theirs = act.to_string().to_ascii_lowercase();
            let theirs = if theirs.contains("beam-type")
                || theirs.contains("hcd")
                || theirs.contains("higher energy")
            {
                "hcd"
            } else if theirs.contains("collision-induced") || theirs.contains("cid") {
                "cid"
            } else {
                "other"
            };
            let from_field = sp.activation.as_deref().map(|a| match a {
                "HCD" => "hcd",
                "CID" => "cid",
                _ => "other",
            });
            let ours = from_field.unwrap_or_else(|| {
                sp.scan_filter
                    .as_deref()
                    .and_then(|f| f.split('@').nth(1))
                    .map_or("none", |t| {
                        if t.starts_with("hcd") {
                            "hcd"
                        } else if t.starts_with("cid") {
                            "cid"
                        } else {
                            "other"
                        }
                    })
            });
            // an MS^n scan (`extra.precursors`) may be named after any of its stages
            let staged = sp.extra.contains_key("precursors")
                && sp
                    .scan_filter
                    .as_deref()
                    .is_some_and(|f| f.contains(&format!("@{theirs}")));
            if ours != theirs && !staged {
                m.push(format!("activation {ours} != {theirs}"));
            }
        }
        if o.peaks_not_compared.is_some() {
            if let Some(t) = s.total_ion_current
                && !sp.total_ion_current.is_some_and(|x| rel_close(x, t, 1e-9))
            {
                m.push(format!("TIC {:?} != {t}", sp.total_ion_current));
            }
            if let Some(b) = s.base_peak_mz
                && !sp.base_peak_mz.is_some_and(|x| rel_close(x, b, 1e-9))
            {
                m.push(format!("base peak m/z {:?} != {b}", sp.base_peak_mz));
            }
        }
        if !m.is_empty() {
            meta_bad += 1;
            note(
                format!("scan {}: {}", s.scan_number, m.join(", ")),
                &mut meta_diffs,
            );
        }
        if o.peaks_not_compared.is_some() {
            continue;
        }
        // Peaks. A 32-bit export rounded m/z to f32; compare our values rounded the same way.
        let mz: Vec<f64> = if s.mz_bits == Some(32) {
            sp.mz.iter().map(|&x| f64::from(x as f32)).collect()
        } else {
            sp.mz.clone()
        };
        if mz.len() == s.n_peaks
            && (o.mz_not_compared.is_some() || xxh3_f64(&mz) == s.xxh3_mz)
            && xxh3_f32(&sp.intensity) == s.xxh3_intensity
        {
            exact += 1;
            continue;
        }
        let sum: f64 = sp.intensity.iter().map(|&v| f64::from(v)).sum();
        let summary_ok = mz.len() == s.n_peaks
            && rel_close(sum, s.sum_intensity, 1e-4)
            && s.mz_min
                .is_none_or(|v| mz.first().is_some_and(|&x| rel_close(x, v, 1e-6)))
            && s.mz_max
                .is_none_or(|v| mz.last().is_some_and(|&x| rel_close(x, v, 1e-6)))
            && s.first_peaks.iter().enumerate().all(|(k, [pm, pi])| {
                mz.get(k).is_some_and(|&x| rel_close(x, *pm, 1e-6))
                    && sp
                        .intensity
                        .get(k)
                        .is_some_and(|&y| rel_close(f64::from(y), *pi, 1e-4))
            });
        if summary_ok {
            close += 1;
            if close_scans.len() < 5 {
                close_scans.push(s.scan_number);
            }
        } else {
            differ += 1;
            note(
                format!(
                    "scan {}: {} points (oracle {}), intensity sum {sum} (oracle {})",
                    s.scan_number,
                    mz.len(),
                    s.n_peaks,
                    s.sum_intensity
                ),
                &mut first_diffs,
            );
        }
    }
    let mut summary = match &o.peaks_not_compared {
        Some(why) => format!(
            "{} spectra: peaks not compared ({why}); metadata, TIC and base-peak m/z mismatches {meta_bad}",
            o.scans.len()
        ),
        None => format!(
            "{} spectra: peaks {exact} exact, {close} within tolerance, {differ} differ; metadata mismatches {meta_bad}",
            o.scans.len()
        ),
    };
    if subset {
        let _ = write!(
            summary,
            " (the export holds {} of our {ours} scans)",
            o.scans.len()
        );
    }
    if !close_scans.is_empty() {
        let _ = write!(summary, " (within-tolerance scans include {close_scans:?})");
    }
    if problems.is_empty() && differ == 0 && meta_bad == 0 {
        Ok(summary)
    } else {
        problems.insert(0, summary);
        problems.extend(meta_diffs.into_iter().take(5));
        problems.extend(first_diffs.into_iter().take(5));
        Err(problems.join("; "))
    }
}

/// Rebuild every chromatogram of the export from our spectra: the TIC from each scan's total
/// ion current, an SRM trace from the peak inside the product window of each scan whose
/// precursor and polarity match. A trace is "exact" when its point count and intensity hash
/// match and every time agrees within 1e-9 min; "exact on its signal" when only the export's
/// zero-intensity padding differs and the non-zero (time, intensity) points hash identically.
pub(crate) fn check_chromatograms(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    traces: &[OracleChromatogram],
    product_tolerance: Option<f64>,
) -> Result<String, String> {
    use openreadout_core::SpectrumView;
    let n = info.spectra.first().map_or(0, |s| s.scan_count);
    let mut scans = Vec::with_capacity(n as usize);
    for i in 0..n {
        scans.push(
            ds.read_spectrum_view(0, i, SpectrumView::Primary)
                .map_err(|e| format!("scan index {i}: {e}"))?,
        );
    }
    // A trace of ours by name (`TIC`, `BPC`): (time in minutes, intensity) points.
    let mut named = |name: &str| -> Option<Vec<(f64, f32)>> {
        let ti = info.traces.iter().find(|t| {
            // a multi-sample file names its first sample's traces `TIC (sample 1)`
            let n = t.name.as_deref().unwrap_or("");
            (n == name || n == format!("{name} (sample 1)")) && t.channels.len() == 2
        })?;
        let tr = ds.read_trace(ti.index, 0, 0, ti.sample_count).ok()?;
        Some(
            tr.channels[0]
                .iter()
                .zip(&tr.channels[1])
                .map(|(&t, &v)| (t / 60.0, v as f32))
                .collect(),
        )
    };
    let tic_trace = named("TIC");
    let bpc_trace = named("BPC");
    let (mut exact, mut padded, mut problems) = (0usize, 0usize, Vec::new());
    for t in traces {
        // Candidate rebuilds, tried in order: the TIC from the spectra, then our TIC trace
        // (formats whose spectra split a cycle, e.g. MRM groups per precursor); the BPC from
        // our BPC trace.
        let candidates: Vec<Vec<(f64, f32)>> = match t.kind.as_str() {
            "tic" => std::iter::once(
                scans
                    .iter()
                    .filter_map(|s| {
                        Some((s.rt_s? / 60.0, s.total_ion_current.unwrap_or(0.0) as f32))
                    })
                    .collect(),
            )
            .chain(tic_trace.clone())
            .collect(),
            "bpc" => match &bpc_trace {
                Some(b) => vec![b.clone()],
                None => continue,
            },
            "srm" => {
                let (Some(q1), Some(q3)) = (t.precursor_mz, t.product_mz) else {
                    problems.push(format!("{}: no precursor/product", t.id));
                    continue;
                };
                let extra = product_tolerance.unwrap_or(1e-4);
                let lo = t.product_lower_offset.unwrap_or(0.0) + extra;
                let hi = t.product_upper_offset.unwrap_or(0.0) + extra;
                // ProteoWizard names scheduled (dynamic MRM) traces `... start=S end=E` (minutes):
                // two transitions with the same Q1/Q3 in different windows are separate traces.
                let window = |key: &str| {
                    t.id.split_whitespace()
                        .find_map(|w| w.strip_prefix(key))
                        .and_then(|v| v.parse::<f64>().ok())
                };
                // (an equal start and end is no window: older Analyst methods store a dwell
                // time where scheduled ones store the expected retention time)
                let (start, end) = match (window("start="), window("end=")) {
                    (Some(a), Some(b)) if a.to_bits() == b.to_bits() => (None, None),
                    w => w,
                };
                let points = scans
                    .iter()
                    .filter(|s| {
                        // an export that records no polarity for its traces matches either
                        (s.polarity == t.polarity || t.polarity == "unknown")
                            && s.precursor_mz.is_some_and(|p| (p - q1).abs() <= 1e-3)
                            && t.collision_energy.is_none_or(|ce| {
                                s.collision_energy.is_none_or(|x| (x - ce).abs() <= 1e-6)
                            })
                            && s.rt_s.is_some_and(|t| {
                                start.is_none_or(|a| t / 60.0 >= a - 1e-6)
                                    && end.is_none_or(|b| t / 60.0 <= b + 1e-6)
                            })
                    })
                    .filter_map(|s| {
                        s.mz.iter()
                            .position(|&m| m >= q3 - lo && m <= q3 + hi)
                            .map(|k| {
                                let method = s
                                    .extra
                                    .get("scan_method")
                                    .and_then(serde_json::Value::as_i64);
                                (s.rt_s.unwrap_or(f64::NAN) / 60.0, s.intensity[k], method)
                            })
                    })
                    .collect::<Vec<_>>();
                // Two transitions with the same Q1, Q3 and energy in overlapping windows
                // interleave: the scans' method id (MassHunter's `scan_method`) tells them apart.
                let mut methods: Vec<i64> = points.iter().filter_map(|p| p.2).collect();
                methods.sort_unstable();
                methods.dedup();
                let all = points.iter().map(|p| (p.0, p.1)).collect();
                std::iter::once(all)
                    .chain(methods.iter().filter(|_| methods.len() > 1).map(|m| {
                        points
                            .iter()
                            .filter(|p| p.2 == Some(*m))
                            .map(|p| (p.0, p.1))
                            .collect()
                    }))
                    .collect()
            }
            _ => continue,
        };
        // Some(true): exact; Some(false): exact on the non-zero points; None: differs.
        // A 32-bit export rounded the times to f32: compare ours rounded the same way too.
        let matches = |points: &[(f64, f32)]| -> Option<bool> {
            let times: Vec<f64> = points.iter().map(|p| p.0).collect();
            let ints: Vec<f32> = points.iter().map(|p| p.1).collect();
            let times_ok = points.len() == t.n_points
                && t.first_points.iter().zip(&times).all(|(o, &ours)| {
                    (o[0] - ours).abs() <= 1e-9 || (o[0] - f64::from(ours as f32)).abs() <= 1e-9
                });
            let nonzero: Vec<&(f64, f32)> = points.iter().filter(|p| p.1 != 0.0).collect();
            let nz_times: Vec<f64> = nonzero.iter().map(|p| p.0).collect();
            let nz_ints: Vec<f32> = nonzero.iter().map(|p| p.1).collect();
            if times_ok && xxh3_f32(&ints) == t.xxh3_intensity {
                Some(true)
            } else if nonzero.len() == t.n_nonzero
                && (xxh3_f64(&nz_times) == t.xxh3_time_min_nonzero
                    // a 32-bit export rounded its times to f32
                    || xxh3_f64(&nz_times.iter().map(|&x| f64::from(x as f32)).collect::<Vec<_>>())
                        == t.xxh3_time_min_nonzero)
                && xxh3_f32(&nz_ints) == t.xxh3_intensity_nonzero
            {
                Some(false)
            } else {
                None
            }
        };
        let verdict = candidates.iter().find_map(|c| matches(c));
        let points = candidates.first().cloned().unwrap_or_default();
        let ints: Vec<f32> = points.iter().map(|p| p.1).collect();
        if verdict == Some(true) {
            exact += 1;
        } else if verdict == Some(false) {
            padded += 1;
        } else if problems.len() < 8 {
            let sum: f64 = ints.iter().map(|&v| f64::from(v)).sum();
            problems.push(format!(
                "{}: {} points (oracle {}), intensity sum {sum} (oracle {}), first {:?} vs {:?}",
                t.id,
                points.len(),
                t.n_points,
                t.sum_intensity,
                points.iter().take(2).collect::<Vec<_>>(),
                t.first_points.iter().take(2).collect::<Vec<_>>()
            ));
        }
    }
    let msg = format!(
        "{n} scans; {exact}/{} chromatograms rebuilt exactly from the spectra (TIC/BPC: or our traces), {padded} exact on their non-zero points (the export pads them with zeros from a neighbouring transition)",
        traces.len()
    );
    if problems.is_empty() {
        Ok(msg)
    } else {
        Err(format!("{msg}; {}", problems.join("; ")))
    }
}

/// Seconds since 1970 of the clock time `YYYY-MM-DDTHH:MM:SS` at the start of `s` (any
/// fraction and zone ignored).
pub(crate) fn naive_seconds(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let num = |a: usize, z: usize| text.get(a..z)?.parse::<i64>().ok();
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // days from civil (proleptic Gregorian)
    let y2 = if month <= 2 { year - 1 } else { year };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Mass-spectrometry exports write the run start (`startTimeStamp`) with conflicting zone
/// conventions (local clock marked `Z`, UTC, a zone offset): our acquisition start is taken to
/// agree when it is the same clock time up to a whole number of hours (at most 14) and 2 s.
/// Agreement covers `experiment.acquisition.started_at`; disagreement only leaves it uncovered.
pub(crate) fn start_time_agrees(reg: &Registry, path: &Path, export: &Path) {
    let Ok(head) = std::fs::read(export).map(|b| b[..b.len().min(65_536)].to_vec()) else {
        return;
    };
    let head = String::from_utf8_lossy(&head);
    let Some(theirs) = head
        .split_once("startTimeStamp=\"")
        .and_then(|(_, r)| r.split_once('"'))
        .map(|(t, _)| t.to_string())
    else {
        return;
    };
    let Ok((_, ds)) = reg.open(path) else {
        return;
    };
    let Ok(info) = ds.info() else {
        return;
    };
    let exp = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
    let Some(ours) = exp.acquisition.and_then(|a| a.started_at) else {
        return;
    };
    if let (Some(a), Some(b)) = (naive_seconds(&ours), naive_seconds(&theirs)) {
        let d = a - b;
        let hours = (d as f64 / 3600.0).round() as i64;
        if hours.abs() <= 14 && (d - hours * 3600).abs() <= 2 {
            covered("experiment.acquisition.started_at");
        }
    }
}

/// An mzML export read as an input: compare its chromatograms (our traces) with the oracle's
/// reading of the same file: point count, intensity hash (f32) and the first times (minutes).
pub(crate) fn check_chromatogram_traces(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    oracle: &[OracleChromatogram],
) -> Result<String, String> {
    let mut problems = Vec::new();
    let mut ok = 0usize;
    for o in oracle {
        let Some(t) = info
            .traces
            .iter()
            .find(|t| t.name.as_deref() == Some(o.id.as_str()))
        else {
            problems.push(format!("chromatogram {} missing", o.id));
            continue;
        };
        let tr = match ds.read_trace(t.index, 0, 0, u64::MAX) {
            Ok(tr) => tr,
            Err(e) => {
                problems.push(format!("{}: {e}", o.id));
                continue;
            }
        };
        let time = tr.channels.first().cloned().unwrap_or_default();
        let ints: Vec<f32> = tr
            .channels
            .get(1)
            .map_or_else(Vec::new, |c| c.iter().map(|&v| v as f32).collect());
        let times_ok = o
            .first_points
            .iter()
            .zip(&time)
            .all(|(p, &s)| (p[0] - s / 60.0).abs() <= 1e-9);
        if ints.len() == o.n_points && xxh3_f32(&ints) == o.xxh3_intensity && times_ok {
            ok += 1;
        } else if problems.len() < 8 {
            problems.push(format!(
                "{}: {} points (oracle {})",
                o.id,
                ints.len(),
                o.n_points
            ));
        }
    }
    let msg = format!(
        "{ok}/{} chromatograms match (points, intensity hash, times)",
        oracle.len()
    );
    if problems.is_empty() {
        Ok(msg)
    } else {
        Err(format!("{msg}; {}", problems.join("; ")))
    }
}
