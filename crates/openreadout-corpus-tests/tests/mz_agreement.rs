//! m/z and intensity agreement of the vendor mass-spectrometry readers with conversions made
//! by the vendor's own library (ProteoWizard msconvert with the vendor DLL, or the depositor's
//! vendor-software export): the measure of whether our calibration equals the vendor's.
//!
//! An input entry of `corpus/manifest.toml` names its references with `mz_references = [ids]`
//! (mzML `oracle-export` entries) and the bar it must meet: `mz_ppm_max` (largest |m/z error|
//! in ppm over every matched point, default 1), optionally `mz_intensity_rel_max` (largest
//! relative intensity difference of matched points) and `mz_unmatched_max` (fraction of
//! reference points without a point of ours within 50 ppm, default 0) and
//! `mz_unmatched_spectra_max` (fraction of reference spectra without a counterpart, default
//! 0). `mz_match` says how
//! spectra pair up: `native-id` (default; the reference's id equals ours), `scan-number`,
//! `index`, or `tdf-scan` (timsTOF per-scan references `frame=F scan=S`, compared with scan
//! S − 1 of our decoded frame F, including its 1/K0). `mz_drift_references = [ids]` names
//! per-drift-bin references of a Waters ion-mobility or SONAR acquisition, compared by native id
//! with our run 1 (one spectrum per drift bin). `mz_run = N` compares `mz_references` with run N
//! instead of run 0 (one sample of a multi-sample Sciex `.wiff`).
//!
//! Every reference point with non-zero intensity is matched to our nearest point in m/z. The
//! report gives, per pair, the spectra and points compared and the |ppm| median, 99th
//! percentile and maximum, plus the intensity agreement.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test mz_agreement -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]
#![allow(
    clippy::float_cmp,
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, Registry, Spectrum, SpectrumView};
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    #[serde(default)]
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    mz_references: Vec<String>,
    /// References compared with run 1 (Waters drift bins).
    #[serde(default)]
    mz_drift_references: Vec<String>,
    /// The run `mz_references` are compared with (a sample of a multi-sample Sciex `.wiff`;
    /// default 0).
    #[serde(default)]
    mz_run: Option<u32>,
    #[serde(default)]
    mz_ppm_max: Option<f64>,
    #[serde(default)]
    mz_intensity_rel_max: Option<f64>,
    #[serde(default)]
    mz_unmatched_max: Option<f64>,
    #[serde(default)]
    mz_match: Option<String>,
    /// Fraction of reference spectra allowed to have no counterpart (default 0).
    #[serde(default)]
    mz_unmatched_spectra_max: Option<f64>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
}

/// Accumulated agreement of one (input, reference) pair.
#[derive(Default)]
struct Stats {
    spectra: usize,
    spectra_unmatched: usize,
    points: usize,
    points_unmatched: usize,
    ppm: Vec<f64>,
    intensity_rel: Vec<f64>,
    mobility_abs_max: f64,
    mobility_n: usize,
    first_problem: Option<String>,
}

impl Stats {
    /// Match every reference point (non-zero intensity) to our nearest point in m/z.
    fn compare(&mut self, ours_mz: &[f64], ours_int: &[f32], theirs: &Spectrum) {
        for (&mz, &it) in theirs.mz.iter().zip(&theirs.intensity) {
            if it == 0.0 || !mz.is_finite() || mz <= 0.0 {
                continue;
            }
            self.points += 1;
            let k = ours_mz.partition_point(|&x| x < mz);
            let best = [k.checked_sub(1), Some(k)]
                .into_iter()
                .flatten()
                .filter(|&j| j < ours_mz.len())
                .min_by(|&a, &b| {
                    (ours_mz[a] - mz)
                        .abs()
                        .partial_cmp(&(ours_mz[b] - mz).abs())
                        .unwrap()
                });
            let Some(j) = best else {
                self.points_unmatched += 1;
                continue;
            };
            let ppm = (ours_mz[j] - mz) / mz * 1e6;
            if ppm.abs() > 50.0 {
                self.points_unmatched += 1;
                if self.first_problem.is_none() {
                    self.first_problem = Some(format!(
                        "{}: no point of ours within 50 ppm of m/z {mz}",
                        theirs.native_id.as_deref().unwrap_or("?")
                    ));
                }
                continue;
            }
            self.ppm.push(ppm);
            let ours = f64::from(ours_int[j]);
            let t = f64::from(it);
            self.intensity_rel
                .push((ours - t).abs() / t.abs().max(1e-9));
        }
    }

    fn summary(&mut self) -> (f64, f64, f64, f64, f64) {
        let mut a: Vec<f64> = self.ppm.iter().map(|v| v.abs()).collect();
        a.sort_by(f64::total_cmp);
        let q = |v: &[f64], p: f64| -> f64 {
            if v.is_empty() {
                return f64::NAN;
            }
            v[((v.len() - 1) as f64 * p).round() as usize]
        };
        let mut i = self.intensity_rel.clone();
        i.sort_by(f64::total_cmp);
        (
            q(&a, 0.5),
            q(&a, 0.99),
            a.last().copied().unwrap_or(f64::NAN),
            q(&i, 0.5),
            i.last().copied().unwrap_or(f64::NAN),
        )
    }
}

/// `frame=F scan=S` → (F, S).
fn tdf_scan_id(id: &str) -> Option<(i64, usize)> {
    let mut f = None;
    let mut s = None;
    for w in id.split_whitespace() {
        if let Some(v) = w.strip_prefix("frame=") {
            f = v.parse().ok();
        } else if let Some(v) = w.strip_prefix("scan=") {
            s = v.parse().ok();
        }
    }
    Some((f?, s?))
}

fn scan_number_of(id: &str) -> Option<u64> {
    id.split_whitespace()
        .find_map(|w| {
            w.strip_prefix("scan=")
                .or_else(|| w.strip_prefix("scanId="))
        })
        .and_then(|v| v.parse().ok())
}

/// Compare a timsTOF input scan by scan with a per-scan reference.
fn compare_tdf_scans(path: &Path, reference: &mut dyn Dataset, n: u64, st: &mut Stats) {
    let mut ds = openreadout_bruker_tims::TimsDataset::open(path).expect("open .d");
    let ids: HashMap<i64, usize> = ds
        .frame_records()
        .iter()
        .enumerate()
        .map(|(i, f)| (f.id, i))
        .collect();
    let mut cache: Option<(usize, openreadout_bruker_tims::TimsFrame)> = None;
    for k in 0..n {
        let theirs = reference.read_spectrum(0, k).expect("reference spectrum");
        let Some((f, s)) = theirs.native_id.as_deref().and_then(tdf_scan_id) else {
            continue;
        };
        let Some(&fi) = ids.get(&f) else {
            st.spectra_unmatched += 1;
            continue;
        };
        let scan = s - 1;
        if let Some(im) = theirs.inverse_reduced_mobility
            && let Some(m) = ds.mobility_model(fi)
        {
            let d = (m.inverse_mobility(scan as f64) - im).abs();
            st.mobility_abs_max = st.mobility_abs_max.max(d);
            st.mobility_n += 1;
        }
        if theirs.mz.is_empty() {
            st.spectra += 1;
            continue;
        }
        if cache.as_ref().is_none_or(|(c, _)| *c != fi) {
            match ds.read_frame(fi) {
                Ok(fr) => cache = Some((fi, fr)),
                Err(e) => {
                    st.spectra_unmatched += 1;
                    st.first_problem.get_or_insert(format!("frame {f}: {e}"));
                    continue;
                }
            }
        }
        let fr = &cache.as_ref().unwrap().1;
        let (tof, int) = fr.scan_range(scan, scan + 1);
        let Some(model) = ds.mz_model(fi) else {
            st.first_problem
                .get_or_insert(format!("frame {f}: no m/z calibration model"));
            continue;
        };
        let mut pts: Vec<(f64, f32)> = tof
            .iter()
            .zip(int)
            .map(|(&t, &i)| (model.mz(f64::from(t)), i as f32))
            .collect();
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mz, it): (Vec<f64>, Vec<f32>) = pts.into_iter().unzip();
        st.spectra += 1;
        st.compare(&mz, &it, &theirs);
    }
}

#[test]
fn vendor_readers_match_vendor_calibrated_conversions() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    // References are oracle exports; an export may share its input's id (depositor pairs).
    let mut by_id: HashMap<&str, &Entry> = HashMap::new();
    for e in &manifest.file {
        if e.role == "oracle-export" || !by_id.contains_key(e.id.as_str()) {
            by_id.insert(e.id.as_str(), e);
        }
    }
    let reg = registry();
    let mut failures = Vec::new();
    let mut rows = Vec::new();
    let mut compared_inputs: Vec<(String, String)> = Vec::new();
    for e in &manifest.file {
        if (e.mz_references.is_empty() && e.mz_drift_references.is_empty())
            || e.role != "input"
            || e.id.starts_with("ho-")
            || only.as_ref().is_some_and(|o| !e.id.contains(o.as_str()))
        {
            continue;
        }
        let path = dir.join(&e.filename);
        if !path.exists() {
            eprintln!("skip {}: not downloaded", e.id);
            continue;
        }
        if !compared_inputs.iter().any(|(i, _)| i == &e.id) {
            compared_inputs.push((e.id.clone(), e.format.clone()));
        }
        let refs = e
            .mz_references
            .iter()
            .map(|r| (r, e.mz_run.unwrap_or(0)))
            .chain(e.mz_drift_references.iter().map(|r| (r, 1u32)));
        for (rid, run) in refs {
            let Some(r) = by_id.get(rid.as_str()) else {
                failures.push(format!("{}: reference {rid} is not in the manifest", e.id));
                continue;
            };
            let rpath = dir.join(&r.filename);
            if !rpath.exists() {
                eprintln!("skip {} vs {rid}: reference not downloaded", e.id);
                continue;
            }
            let (_, mut reference) = reg.open(&rpath).expect("open reference");
            let rinfo = reference.info().expect("reference info");
            let n = rinfo.spectra.first().map_or(0, |s| s.scan_count);
            let mut st = Stats::default();
            let mode = e.mz_match.as_deref().unwrap_or("native-id");
            if mode == "tdf-scan" {
                compare_tdf_scans(&path, reference.as_mut(), n, &mut st);
            } else {
                let mut ours = match reg.open(&path).and_then(|(_, d)| d.info().map(|_| d)) {
                    Ok(d) => d,
                    Err(err) => {
                        failures.push(format!("{}: cannot be read: {err}", e.id));
                        continue;
                    }
                };
                let oinfo = ours.info().expect("input info");
                let on = oinfo
                    .spectra
                    .iter()
                    .find(|s| s.index == run)
                    .map_or(0, |s| s.scan_count);
                // our native id → index (header-only where the reader can)
                let mut ids: HashMap<String, u64> = HashMap::new();
                let mut numbers: HashMap<u64, u64> = HashMap::new();
                let _ = openreadout_core::scans::visit_scans(ours.as_mut(), run, &mut |h| {
                    if let Some(id) = &h.native_id {
                        ids.insert(id.clone(), h.index);
                    }
                    numbers.insert(h.scan_number, h.index);
                    true
                });
                for k in 0..n {
                    let theirs = reference.read_spectrum(0, k).expect("reference spectrum");
                    // not a mass spectrum (a UV/Vis spectrum of a detector function)
                    if theirs.ms_level == 0 {
                        continue;
                    }
                    let at = match mode {
                        "index" => Some(k).filter(|&k| k < on),
                        "scan-number" => theirs
                            .native_id
                            .as_deref()
                            .and_then(scan_number_of)
                            .or(Some(theirs.scan_number))
                            .and_then(|s| numbers.get(&s).copied()),
                        _ => theirs
                            .native_id
                            .as_ref()
                            .and_then(|id| ids.get(id).copied()),
                    };
                    let Some(at) = at else {
                        st.spectra_unmatched += 1;
                        continue;
                    };
                    let view = if theirs.centroided {
                        SpectrumView::Centroid
                    } else {
                        SpectrumView::Primary
                    };
                    let sp = match ours.read_spectrum_view(run, at, view) {
                        Ok(sp) => sp,
                        Err(err) => {
                            st.spectra_unmatched += 1;
                            st.first_problem
                                .get_or_insert(format!("our spectrum {at}: {err}"));
                            continue;
                        }
                    };
                    st.spectra += 1;
                    if let (Some(a), Some(b)) =
                        (sp.inverse_reduced_mobility, theirs.inverse_reduced_mobility)
                    {
                        st.mobility_abs_max = st.mobility_abs_max.max((a - b).abs());
                        st.mobility_n += 1;
                    }
                    let mut pts: Vec<(f64, f32)> = sp
                        .mz
                        .iter()
                        .copied()
                        .zip(sp.intensity.iter().copied())
                        .collect();
                    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let (mz, it): (Vec<f64>, Vec<f32>) = pts.into_iter().unzip();
                    st.compare(&mz, &it, &theirs);
                }
            }
            let (med, p99, max, imed, imax) = st.summary();
            let unmatched_frac = if st.points == 0 {
                0.0
            } else {
                st.points_unmatched as f64 / st.points as f64
            };
            let row = format!(
                "| {} | {} | {} ({}) | {} | {} | {:.4} | {:.4} | {:.4} | {:.2e} | {:.2e} | {} |",
                e.id,
                rid,
                st.spectra,
                st.spectra_unmatched,
                st.points,
                st.points_unmatched,
                med,
                p99,
                max,
                imed,
                imax,
                if st.mobility_n > 0 {
                    format!("{:.1e} ({})", st.mobility_abs_max, st.mobility_n)
                } else {
                    "-".into()
                }
            );
            println!("{row}");
            rows.push(row);
            let ppm_max = e.mz_ppm_max.unwrap_or(1.0);
            let mut bad = Vec::new();
            if st.spectra == 0 {
                bad.push("no spectra compared".to_string());
            }
            if st.points > st.points_unmatched && (max.is_nan() || max > ppm_max) {
                bad.push(format!("max |ppm| {max} > {ppm_max}"));
            }
            let total_spectra = st.spectra + st.spectra_unmatched;
            if total_spectra > 0
                && st.spectra_unmatched as f64 / total_spectra as f64
                    > e.mz_unmatched_spectra_max.unwrap_or(0.0)
            {
                bad.push(format!(
                    "{} of {total_spectra} reference spectra have no counterpart",
                    st.spectra_unmatched
                ));
            }
            if unmatched_frac > e.mz_unmatched_max.unwrap_or(0.0) {
                bad.push(format!(
                    "{} of {} reference points unmatched",
                    st.points_unmatched, st.points
                ));
            }
            if let Some(lim) = e.mz_intensity_rel_max
                && imax > lim
            {
                bad.push(format!("intensity |rel| {imax} > {lim}"));
            }
            if st.mobility_n > 0 && st.mobility_abs_max > 1e-6 {
                bad.push(format!("1/K0 differs by {}", st.mobility_abs_max));
            }
            if !bad.is_empty() {
                failures.push(format!(
                    "{} vs {rid}: {}{}",
                    e.id,
                    bad.join(", "),
                    st.first_problem
                        .as_ref()
                        .map(|p| format!(" (first: {p})"))
                        .unwrap_or_default()
                ));
            }
        }
    }
    // Per-input results for `cargo xtask assurance-audit refresh --results`: a vendor-library
    // conversion is an independent oracle of the spectra.
    if let Ok(p) = std::env::var("MZ_RESULTS") {
        let mut lines = String::new();
        for (id, format) in &compared_inputs {
            let failed = failures.iter().any(|f| {
                f.split_once(':').is_some_and(|(i, _)| i == id)
                    || f.starts_with(&format!("{id} vs "))
            });
            lines.push_str(
                &serde_json::json!({"id": id, "format": format, "status": if failed { "FAIL" } else { "pass" }, "independent": true, "compared": ["spectra"]})
                    .to_string(),
            );
            lines.push('\n');
        }
        std::fs::write(p, lines).unwrap();
    }
    println!(
        "\n| input | reference | spectra (not comparable) | points | unmatched | median |ppm| | p99 |ppm| | max |ppm| | median |ΔI|/I | max |ΔI|/I | max |Δ1/K0| (n) |"
    );
    for r in &rows {
        println!("{r}");
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
